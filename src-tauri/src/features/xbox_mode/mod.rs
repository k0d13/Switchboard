//! Xbox mode and PC mode: switching the monitor, turning the Xbox on and off, and following it
//! when it's turned on or off some other way. Its watcher also tells presence whether the Xbox is
//! on. Turning the Xbox on and off through Xbox Live uses the Xbox account (`xbox_live::account`).

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};

use super::xbox_live::{self, Power, Shared};
use crate::core::feature::{ActionDef, Feature, FeatureInfo, Field, FieldOption, Kind, Tone, option};
use crate::core::host::Ctx;
use crate::core::worker::{StopToken, Worker};
use crate::platform::ddc::{self, INPUT_SOURCE, Monitor};
use crate::platform::display_config::{self, Topology};
use crate::platform::smartglass;

const ID: &str = "xbox";

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Settings {
    monitor_model: String,
    xbox_input: u32,
    /// Updated with whichever input the PC was on each time Xbox mode starts.
    pc_input: u32,
    xbox_live_id: String,
    off_in_pc_mode: bool,
    switch_delay_ms: u64,
    wake_timeout_secs: u64,
    xbox_boot_secs: u64,
    /// Remembered whenever the Xbox answers, so finding it doesn't rely on broadcasts alone.
    xbox_address: String,
    xbox_name: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            monitor_model: String::new(),
            xbox_input: 0x11,
            pc_input: 0x0F,
            xbox_live_id: String::new(),
            off_in_pc_mode: false,
            switch_delay_ms: 500,
            wake_timeout_secs: 10,
            xbox_boot_secs: 30,
            xbox_address: String::new(),
            xbox_name: String::new(),
        }
    }
}

impl Settings {
    fn address(&self) -> Option<Ipv4Addr> {
        self.xbox_address.trim().parse().ok()
    }

    fn live_id(&self) -> Option<&str> {
        Some(self.xbox_live_id.trim()).filter(|id| !id.is_empty())
    }
}

/// What the workers and actions share.
struct State {
    shared: Arc<Shared>,
    mode: Mutex<Option<Mode>>,
    /// Set while switching modes, so the watcher doesn't start a switch of its own meanwhile.
    switching: AtomicBool,
    watch: Mutex<Option<Worker>>,
}

/// Clears a flag however the scope ends.
struct FlagGuard<'a>(&'a AtomicBool);

impl Drop for FlagGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Saves where the Xbox is and what it's called, whenever it answers.
fn remember(ctx: &Ctx, s: &Settings, console: &smartglass::Console) {
    let address = console.address.to_string();
    if s.xbox_address != address {
        ctx.set_setting("xboxAddress", json!(address));
    }
    if s.xbox_name != console.name {
        ctx.set_setting("xboxName", json!(console.name));
    }
}

/// Turns the Xbox on or off through Xbox Live, the way the Xbox app does.
fn cloud_power(ctx: &Ctx, shared: &Shared, live_id: &str, on: bool) -> anyhow::Result<()> {
    let client_id = shared.client_id();
    let Some(token) = shared.token(ctx, &client_id) else {
        bail!("Sign in to Xbox under Accounts first");
    };
    let mut xbox = xbox_live::Xbox::new(&client_id);
    xbox.set_refresh_token(Some(token));
    let result = xbox.power(live_id, on).map_err(|e| {
        let remote_off = xbox.consoles().ok().and_then(|list| {
            list.into_iter().find(|c| c.live_id.eq_ignore_ascii_case(live_id)).map(|c| !c.remote_management)
        });
        match remote_off {
            Some(true) => anyhow!("Remote features are off in the Xbox's settings"),
            _ => anyhow!("Xbox Live couldn't turn the Xbox {}: {e}", if on { "on" } else { "off" }),
        }
    });
    if let Some(token) = xbox.take_rotated() {
        shared.save_token(ctx, &client_id, Some(&token));
        shared.auth_changed();
    }
    result
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Xbox,
    Pc,
    Other(Topology),
}

impl Mode {
    fn from_topology(t: Topology) -> Self {
        match t {
            Topology::External => Mode::Xbox,
            Topology::Extend => Mode::Pc,
            other => Mode::Other(other),
        }
    }
}

/// Checks in a row the Xbox has to miss before it counts as off.
const MISSES_BEFORE_OFF: u32 = 3;

/// Matches the saved monitor setting against a name, ignoring case. A substring is enough, so
/// "Acme 27Q" still finds "Acme 27Q Pro".
fn matches(saved: &str, name: &str) -> bool {
    let saved = saved.trim().to_lowercase();
    !saved.is_empty() && name.to_lowercase().contains(&saved)
}

/// The chosen monitor, if Windows is using it. Matched by the EDID name Windows already knows,
/// which is instant; only monitors without one are asked over DDC/CI, which takes a second or so.
fn find_monitor(wanted: &str) -> Option<Monitor> {
    let displays = display_config::displays();
    let mut unnamed = Vec::new();
    for m in ddc::monitors() {
        match displays.iter().find(|d| d.device.as_deref() == Some(m.device.as_str())) {
            Some(d) if matches(wanted, &d.name) => return Some(m),
            Some(_) => {}
            None => unnamed.push(m),
        }
    }
    unnamed.into_iter().find(|m| m.model().is_some_and(|model| matches(wanted, &model)))
}

fn monitor_settings(ctx: &Ctx) -> anyhow::Result<Settings> {
    let s: Settings = ctx.settings();
    if s.monitor_model.trim().is_empty() {
        bail!("Choose the monitor the Xbox is plugged into in Xbox mode settings first");
    }
    Ok(s)
}

/// Reads the topology and, if the mode changed, updates the status and tray.
fn sync(state: &State, ctx: &Ctx) {
    let Some(mode) = display_config::current().map(Mode::from_topology) else { return };
    let previous = state.mode.lock().replace(mode);
    if previous == Some(mode) {
        return;
    }
    let status = match mode {
        Mode::Xbox => ("Xbox mode".to_string(), Tone::Active),
        Mode::Pc => ("PC mode".to_string(), Tone::Ok),
        Mode::Other(t) => (t.label().to_string(), Tone::Idle),
    };
    // Outside the lock: set_status calls back into actions()
    ctx.set_status(status.0, status.1);
}

fn switch(state: &Arc<State>, ctx: &Ctx, to: Mode) -> anyhow::Result<()> {
    state.switching.store(true, Ordering::SeqCst);
    let _guard = FlagGuard(&state.switching);
    let result = match to {
        Mode::Xbox => to_xbox(state, ctx),
        _ => to_pc(state, ctx),
    };
    sync(state, ctx);
    result
}

/// Turns the Xbox on if it isn't, waits until it answers, then switches the monitor over.
fn to_xbox(state: &State, ctx: &Ctx) -> anyhow::Result<()> {
    let s = monitor_settings(ctx)?;
    if display_config::current() == Some(Topology::External) {
        bail!("Already in Xbox mode");
    }
    if let Some(live_id) = s.live_id() {
        wake(state, ctx, &s, live_id)?;
    }

    let mon = find_monitor(&s.monitor_model).ok_or_else(|| {
        anyhow!("Couldn't find the {} monitor. Check it's plugged in and DDC/CI is on in its menu.", s.monitor_model)
    })?;

    // Remember which input the PC is on, so PC mode switches back to the right one
    if let Some(current) = mon.get(INPUT_SOURCE)
        && current > 0
        && current != s.xbox_input
        && current != s.pc_input
    {
        ctx.set_setting("pcInput", json!(current));
        ctx.info(format!("PC is on {}, remembered for PC mode", ddc::input_name(current)));
    }

    if !mon.set(INPUT_SOURCE, s.xbox_input) {
        bail!("The monitor didn't accept the input change. Check DDC/CI is on in its menu.");
    }
    drop(mon);

    std::thread::sleep(Duration::from_millis(s.switch_delay_ms));
    display_config::set(Topology::External)?;
    ctx.info(format!("Xbox mode: {} on {}", s.monitor_model, ddc::input_name(s.xbox_input)));
    Ok(())
}

/// Turns the Xbox on and waits until it answers on the network.
fn wake(state: &State, ctx: &Ctx, s: &Settings, live_id: &str) -> anyhow::Result<()> {
    if let Some(c) = smartglass::find(live_id, s.address(), Duration::from_millis(600)) {
        remember(ctx, s, &c);
        state.shared.set_power(Power::On);
        return Ok(());
    }
    if state.shared.token(ctx, &state.shared.client_id()).is_none() {
        bail!("The Xbox is off. Sign in to Xbox under Accounts so Switchboard can turn it on, or turn it on yourself and the monitor switches over by itself.");
    }
    ctx.info("Turning the Xbox on");
    // Series X|S consoles only wake through Xbox Live. The local packet is kept for older
    // consoles, and costs nothing.
    if let Err(e) = cloud_power(ctx, &state.shared, live_id, true) {
        ctx.warn(e.to_string());
    }
    let deadline = Instant::now() + Duration::from_secs(s.xbox_boot_secs);
    while Instant::now() < deadline {
        smartglass::power_on(live_id, s.address())?;
        if let Some(c) = smartglass::find(live_id, s.address(), Duration::from_secs(2)) {
            remember(ctx, s, &c);
            state.shared.set_power(Power::On);
            ctx.info(format!("{} is on", c.name));
            return Ok(());
        }
    }
    bail!(
        "The Xbox didn't turn on. It needs Remote features turned on in its settings, and its power mode set to Sleep."
    )
}

fn to_pc(state: &Arc<State>, ctx: &Ctx) -> anyhow::Result<()> {
    let s = monitor_settings(ctx)?;

    // Only if it's on, so PC mode after the Xbox turned itself off doesn't ask again
    if s.off_in_pc_mode
        && let Some(live_id) = s.live_id()
        && state.shared.power() != Power::Off
    {
        // On a thread, so it can't hold up switching back
        let (ctx, state, live_id, s) = (ctx.clone(), state.clone(), live_id.to_string(), s.clone());
        std::thread::spawn(move || {
            if smartglass::find(&live_id, s.address(), Duration::from_millis(800)).is_none() {
                return;
            }
            match cloud_power(&ctx, &state.shared, &live_id, false) {
                Ok(()) => ctx.info("Turning the Xbox off"),
                Err(e) => ctx.warn(e.to_string()),
            }
        });
    }

    // Turn the monitor back on in Windows first; it can't be sent commands while it's off
    display_config::set(Topology::Extend)?;

    let deadline = Instant::now() + Duration::from_secs(s.wake_timeout_secs);
    let mon = loop {
        if let Some(m) = find_monitor(&s.monitor_model) {
            break m;
        }
        if Instant::now() > deadline {
            bail!("Windows turned the {} back on, but it isn't responding yet. Try PC mode again in a few seconds.", s.monitor_model);
        }
        std::thread::sleep(Duration::from_millis(250));
    };

    // It's showing the Xbox at this point, and some monitors ignore the first command or two
    for attempt in 0..5 {
        if mon.set(INPUT_SOURCE, s.pc_input) {
            ctx.info(format!("PC mode: {} back on {}", s.monitor_model, ddc::input_name(s.pc_input)));
            return Ok(());
        }
        if attempt < 4 {
            std::thread::sleep(Duration::from_millis(700));
        }
    }
    bail!("The monitor didn't accept the switch back to the PC. Switch its input by hand this time.")
}

/// The watcher's thread: follows Win+P and unplugged monitors, and the Xbox turning on and off.
fn watch(state: &State, ctx: &Ctx, stop: &StopToken) {
    // `None` until the first sure reading, so starting up never counts as the Xbox changing
    let mut on: Option<bool> = None;
    let mut misses = 0;
    let mut wait = Duration::ZERO;
    while !stop.sleep(wait) {
        sync(state, ctx);
        let s: Settings = ctx.settings();
        let mode = *state.mode.lock();
        // Often in Xbox mode, so turning the Xbox off gets you back to the PC quickly
        wait = if mode == Some(Mode::Xbox) { Duration::from_secs(1) } else { Duration::from_secs(3) };
        let Some(live_id) = s.live_id() else {
            on = None;
            state.shared.set_power(Power::Unknown);
            continue;
        };

        let was = on;
        match smartglass::find(live_id, s.address(), Duration::from_secs(1)) {
            Some(c) => {
                if was != Some(true) {
                    remember(ctx, &s, &c);
                }
                on = Some(true);
                misses = 0;
            }
            None => {
                misses += 1;
                if misses >= MISSES_BEFORE_OFF {
                    on = Some(false);
                }
            }
        }
        match on {
            Some(true) => state.shared.set_power(Power::On),
            Some(false) => state.shared.set_power(Power::Off),
            None => {}
        }

        // A switch in progress is already doing the right thing
        if state.switching.load(Ordering::SeqCst) || s.monitor_model.trim().is_empty() {
            continue;
        }
        match (was, on, mode) {
            (Some(true), Some(false), Some(Mode::Xbox)) => {
                ctx.info("The Xbox turned off, switching to PC mode");
                ctx.run_action("pc-mode");
            }
            (Some(false), Some(true), Some(Mode::Pc)) => {
                ctx.info("The Xbox turned on, switching to Xbox mode");
                ctx.run_action("xbox-mode");
            }
            _ => {}
        }
    }
}

pub struct XboxMode {
    state: Arc<State>,
}

impl XboxMode {
    pub fn new(shared: Arc<Shared>) -> Self {
        Self {
            state: Arc::new(State {
                shared,
                mode: Mutex::new(None),
                switching: AtomicBool::new(false),
                watch: Mutex::new(None),
            }),
        }
    }
}

impl Feature for XboxMode {
    fn info(&self) -> FeatureInfo {
        FeatureInfo {
            id: ID,
            name: "Xbox mode",
            description: "Put your Xbox on a monitor while it's on, and give the monitor back when it's off",
            icon: "gamepad",
            enabled_by_default: true,
            kind: Kind::Feature,
            uses: &[xbox_live::ACCOUNT_ID],
        }
    }

    fn defaults(&self) -> Value {
        let d = Settings::default();
        json!({
            "monitorModel": d.monitor_model,
            "xboxInput": d.xbox_input,
            "pcInput": d.pc_input,
            "xboxLiveId": d.xbox_live_id,
            "offInPcMode": d.off_in_pc_mode,
            "switchDelayMs": d.switch_delay_ms,
            "wakeTimeoutSecs": d.wake_timeout_secs,
            "xboxBootSecs": d.xbox_boot_secs,
            "xboxAddress": d.xbox_address,
            "xboxName": d.xbox_name,
        })
    }

    fn schema(&self, ctx: &Ctx) -> Vec<Field> {
        let s: Settings = ctx.settings();

        let mut monitors: Vec<FieldOption> = Vec::new();
        let mut saved_found = false;
        for d in display_config::displays() {
            // Keep the saved value as the option's value when it matches, so it shows as selected
            let value = if !saved_found && matches(&s.monitor_model, &d.name) {
                saved_found = true;
                s.monitor_model.clone()
            } else {
                d.name.clone()
            };
            if !monitors.iter().any(|o| o.value == json!(value)) {
                monitors.push(option(value, d.name));
            }
        }
        if s.monitor_model.trim().is_empty() {
            monitors.insert(0, option("", "Choose a monitor"));
        } else if !saved_found {
            monitors.insert(0, option(s.monitor_model.clone(), format!("{} (not plugged in)", s.monitor_model)));
        }

        let inputs = |current: u32| {
            let mut opts: Vec<FieldOption> = ddc::INPUTS.iter().map(|(v, n)| option(*v, *n)).collect();
            if !ddc::INPUTS.iter().any(|(v, _)| *v == current) {
                opts.push(option(current, ddc::input_name(current)));
            }
            opts
        };

        // Only consoles that are on answer, so a saved one that's off is kept as its own option
        let found = smartglass::discover(s.address(), Duration::from_millis(800)).unwrap_or_default();
        let saved = s.xbox_live_id.trim();
        let mut consoles = vec![option("", "None")];
        let mut saved_found = false;
        for c in &found {
            let is_saved = c.live_id.eq_ignore_ascii_case(saved);
            if is_saved {
                saved_found = true;
                remember(ctx, &s, c);
            }
            let value = if is_saved { saved.to_string() } else { c.live_id.clone() };
            consoles.push(option(value, format!("{} ({})", c.name, c.address)));
        }
        if !saved.is_empty() && !saved_found {
            let name = if s.xbox_name.is_empty() { saved } else { s.xbox_name.as_str() };
            consoles.push(option(saved, format!("{name} (off)")));
        }

        vec![
            Field::select("xboxLiveId", "Console", consoles)
                .help("Only an Xbox that's on shows up here. To turn it on and off, sign in to Xbox under Accounts, turn on Remote features in the Xbox's settings, and set its power mode to Sleep.")
                .section("Xbox"),
            Field::toggle("offInPcMode", "Turn the Xbox off when switching to PC mode").section("Xbox"),
            Field::select("monitorModel", "Monitor", monitors)
                .help("The one the Xbox is plugged into. It needs DDC/CI turned on in its own menu.")
                .section("Monitor"),
            Field::select("xboxInput", "Xbox input", inputs(s.xbox_input)).section("Monitor"),
            Field::select("pcInput", "PC input", inputs(s.pc_input))
                .help("Updated to whichever input the PC was on each time Xbox mode starts")
                .section("Monitor"),
            Field::number("switchDelayMs", "Wait before moving windows (ms)", 0.0, 5000.0).section("Advanced"),
            Field::number("wakeTimeoutSecs", "Wait for the monitor to wake (seconds)", 1.0, 60.0).section("Advanced"),
            Field::number("xboxBootSecs", "Wait for the Xbox to turn on (seconds)", 5.0, 120.0).section("Advanced"),
            Field::text("xboxAddress", "Xbox IP address")
                .placeholder("Found automatically")
                .help("Filled in when the Xbox is found. Change it if the Xbox isn't being noticed.")
                .section("Advanced"),
        ]
    }

    fn actions(&self, _ctx: &Ctx) -> Vec<ActionDef> {
        let mode = *self.state.mode.lock();
        vec![
            ActionDef::new("xbox-mode", "Xbox mode").icon("gamepad").hotkey("Ctrl+Alt+X").active(mode == Some(Mode::Xbox)),
            ActionDef::new("pc-mode", "PC mode").icon("monitor").hotkey("Ctrl+Alt+P").active(mode == Some(Mode::Pc)),
        ]
    }

    fn run(&self, ctx: &Ctx, action: &str) -> anyhow::Result<()> {
        match action {
            "xbox-mode" => switch(&self.state, ctx, Mode::Xbox),
            "pc-mode" => switch(&self.state, ctx, Mode::Pc),
            other => bail!("Unknown action {other}"),
        }
    }

    fn start(&self, ctx: &Ctx) {
        *self.state.mode.lock() = None;
        self.state.shared.set_power(Power::Unknown);
        sync(&self.state, ctx);

        let (state, c) = (self.state.clone(), ctx.clone());
        let worker = Worker::spawn("xbox-watch", move |stop| watch(&state, &c, &stop));
        *self.state.watch.lock() = Some(worker);
        ctx.refresh();
    }

    fn stop(&self, _ctx: &Ctx) {
        let worker = self.state.watch.lock().take();
        if let Some(w) = worker {
            w.stop();
        }
        *self.state.mode.lock() = None;
        // Presence goes back to checking on its own
        self.state.shared.set_power(Power::Unknown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_settings_struct() {
        let from_json: Settings = serde_json::from_value(XboxMode::new(Arc::default()).defaults()).unwrap();
        assert_eq!(from_json, Settings::default());
    }

    #[test]
    fn xbox_address_is_optional() {
        assert_eq!(Settings::default().address(), None);
        let s = Settings { xbox_address: " 192.168.1.20 ".into(), ..Settings::default() };
        assert_eq!(s.address(), Some([192, 168, 1, 20].into()));
    }

    #[test]
    fn monitor_names_match_loosely() {
        assert!(matches("Acme 27Q", "Acme 27Q Pro"));
        assert!(matches(" acme 27q ", "Acme 27Q"));
        assert!(!matches("", "Acme 27Q"));
        assert!(!matches("LG", "Acme 27Q"));
    }
}
