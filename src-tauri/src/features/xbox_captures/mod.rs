//! Copies new Xbox screenshots and recordings to the clipboard. Take one on the console, it uploads to
//! Xbox network, and this downloads it to a folder and puts it on the clipboard so it can be
//! pasted straight into Discord, like Win+PrtScn.
//!
//! One long-lived worker polls the media hub. When it polls follows the Xbox: see
//! `Shared::poll_every`. Copy latest runs on the host's action thread and uses its own Xbox
//! client, so it never waits for the worker. They coordinate through `Core`, which is also where
//! the list of captures already dealt with lives.
//!
//! The sign-in is shared with the other Xbox features (see `xbox_live`).

mod clipboard;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use parking_lot::{Mutex, MutexGuard};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::xbox_live::{self, Capture, CaptureKind, Power, Shared, Xbox, XboxError};
use crate::core::feature::{ActionDef, Feature, FeatureInfo, Field, Tone};
use crate::core::host::Ctx;
use crate::core::worker::{StopToken, Worker};
use crate::platform::shell;

const STOP_WAIT: Duration = Duration::from_secs(3);

/// While the Xbox is on.
const INTERVAL: Duration = Duration::from_secs(10);
/// Without Xbox mode there's no telling when it's on, so check all the time, less often (unless
/// the user turned that off).
const UNATTENDED_INTERVAL: Duration = Duration::from_secs(30);
/// Uploads lag behind the console, so keep checking for a while after it turns off.
const AFTER_OFF: Duration = Duration::from_secs(120);
const RETRY: Duration = Duration::from_secs(15);
/// Nothing to poll, so only wake for a nudge or stop.
const IDLE: Duration = Duration::from_secs(3600);
const SLICE: Duration = Duration::from_millis(500);
/// How many capture IDs to remember. A search only returns the newest few, so this is plenty.
const SEEN_LIMIT: usize = 200;
/// Discord's upload limit without Nitro.
const DISCORD_LIMIT: u64 = 10 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Settings {
    screenshot_folder: String,
    download_screenshots: bool,
    copy_screenshots: bool,
    recording_folder: String,
    download_recordings: bool,
    copy_recordings: bool,
    check_without_xbox_mode: bool,
    ms_client_id: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            screenshot_folder: String::new(),
            download_screenshots: true,
            copy_screenshots: true,
            recording_folder: String::new(),
            download_recordings: true,
            copy_recordings: true,
            check_without_xbox_mode: true,
            ms_client_id: String::new(),
        }
    }
}

impl Settings {
    fn downloads(&self, kind: CaptureKind) -> bool {
        match kind {
            CaptureKind::Screenshot => self.download_screenshots,
            CaptureKind::Recording => self.download_recordings,
        }
    }

    fn copies(&self, kind: CaptureKind) -> bool {
        self.downloads(kind)
            && match kind {
                CaptureKind::Screenshot => self.copy_screenshots,
                CaptureKind::Recording => self.copy_recordings,
            }
    }

    fn folder(&self, kind: CaptureKind) -> PathBuf {
        let chosen = match kind {
            CaptureKind::Screenshot => &self.screenshot_folder,
            CaptureKind::Recording => &self.recording_folder,
        };
        match chosen.trim() {
            "" => default_folder(kind),
            folder => PathBuf::from(folder),
        }
    }
}

/// Xbox, in Pictures for screenshots and in Videos for recordings.
fn default_folder(kind: CaptureKind) -> PathBuf {
    let (known, name) = match kind {
        CaptureKind::Screenshot => (shell::pictures_dir(), "Pictures"),
        CaptureKind::Recording => (shell::videos_dir(), "Videos"),
    };
    known
        .or_else(|| std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
        .join("Xbox")
}

// ---- what has been dealt with ------------------------------------------------------------------

/// The captures already downloaded (or deliberately skipped), for one account.
#[derive(Default, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Seen {
    xuid: String,
    /// Oldest first, so the oldest are the ones dropped.
    seen: Vec<String>,
}

impl Seen {
    /// Everything listed so far counts as seen: what's on the account before this feature was
    /// turned on isn't new. `newest_first` is how a listing comes.
    fn seeded(xuid: &str, newest_first: &[Capture]) -> Self {
        let mut s = Self { xuid: xuid.to_string(), seen: Vec::new() };
        s.remember(newest_first.iter().rev().map(|c| c.id.clone()));
        s
    }

    fn contains(&self, id: &str) -> bool {
        self.seen.iter().any(|s| s == id)
    }

    fn remember(&mut self, ids: impl IntoIterator<Item = String>) {
        for id in ids {
            if !self.contains(&id) {
                self.seen.push(id);
            }
        }
        if self.seen.len() > SEEN_LIMIT {
            self.seen.drain(..self.seen.len() - SEEN_LIMIT);
        }
    }
}

/// `None` until the file has been read, then `None` inside means there's no saved state yet.
#[derive(Default)]
struct Store {
    loaded: bool,
    seen: Option<Seen>,
}

impl Store {
    fn save(&self, path: &Path) -> std::io::Result<()> {
        let Some(seen) = &self.seen else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(seen)?)
    }
}

/// Shared by the worker and Copy latest. Neither lock is taken by `actions()`, but still keep
/// them short.
#[derive(Default)]
struct Core {
    /// Held while downloading and updating what's been seen, so the two never fetch the same
    /// capture at once.
    io: Mutex<()>,
    store: Mutex<Store>,
}

impl Core {
    fn path(ctx: &Ctx) -> PathBuf {
        ctx.data_dir().join("xbox-captures.json")
    }

    fn store(&self, path: &Path) -> MutexGuard<'_, Store> {
        let mut store = self.store.lock();
        if !store.loaded {
            store.seen = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok());
            store.loaded = true;
        }
        store
    }
}

// ---- files -------------------------------------------------------------------------------------

/// Makes a title safe to use in a Windows file name.
fn sanitise(name: &str) -> String {
    let cleaned: String =
        name.chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c }).take(80).collect();
    let cleaned = cleaned.trim().trim_end_matches('.').trim_end();
    if cleaned.is_empty() { "Xbox".into() } else { cleaned.into() }
}

/// `Game YYYY-MM-DD HH.MM.SS`, in local time. Without a readable time, part of the ID instead.
fn file_stem(title: &str, when: Option<[u16; 6]>, id: &str) -> String {
    let title = sanitise(title);
    match when {
        Some([y, mo, d, h, mi, s]) => {
            format!("{title} {y:04}-{mo:02}-{d:02} {h:02}.{mi:02}.{s:02}")
        }
        None => format!("{title} {}", sanitise(&id.chars().take(8).collect::<String>())),
    }
}

fn possible_extensions(kind: CaptureKind) -> &'static [&'static str] {
    match kind {
        CaptureKind::Screenshot => &["png", "jpg"],
        CaptureKind::Recording => &["mp4"],
    }
}

/// What the file really is, from its first bytes, then from the URL's path, then a guess by kind.
fn extension(kind: CaptureKind, url: &str, head: &[u8]) -> &'static str {
    if clipboard::is_png(head) {
        return "png";
    }
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return "jpg";
    }
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let named = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match named.as_deref() {
        Some("png") => "png",
        Some("jpg" | "jpeg") => "jpg",
        Some("mp4") => "mp4",
        _ => possible_extensions(kind)[0],
    }
}

/// Downloads a capture into `folder`, unless it's already there. Written under a temporary name
/// and renamed, so a half-finished download never looks like a capture.
fn fetch(folder: &Path, c: &Capture) -> Result<PathBuf, String> {
    let url = c.download.as_deref().ok_or("it has no download link yet")?;
    std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let stem = file_stem(&c.title, shell::local_time(c.captured), &c.id);
    for ext in possible_extensions(c.kind) {
        let existing = folder.join(format!("{stem}.{ext}"));
        if existing.exists() {
            return Ok(existing);
        }
    }

    let part = folder.join(format!("{stem}.part"));
    let finish = || -> Result<PathBuf, String> {
        xbox_live::download(url, &part)?;
        let mut head = [0u8; 8];
        let read = std::fs::File::open(&part).and_then(|mut f| f.read(&mut head)).map_err(|e| e.to_string())?;
        let target = folder.join(format!("{stem}.{}", extension(c.kind, url, &head[..read])));
        std::fs::rename(&part, &target).map_err(|e| format!("{}: {e}", target.display()))?;
        Ok(target)
    };
    finish().inspect_err(|_| {
        let _ = std::fs::remove_file(&part);
    })
}

/// Puts a downloaded capture on the clipboard: an image as an image, a recording as a file.
fn copy_to_clipboard(kind: CaptureKind, path: &Path) -> anyhow::Result<()> {
    match kind {
        CaptureKind::Screenshot => {
            let bytes = std::fs::read(path).with_context(|| format!("couldn't read {}", path.display()))?;
            clipboard::copy_image(&bytes)
        }
        CaptureKind::Recording => clipboard::copy_files(&[path]),
    }
}

struct Saved {
    capture: Capture,
    path: PathBuf,
}

impl Saved {
    fn over_discord_limit(&self) -> bool {
        self.capture.kind == CaptureKind::Recording
            && std::fs::metadata(&self.path).is_ok_and(|m| m.len() > DISCORD_LIMIT)
    }
}

fn toast(ctx: &Ctx, saved: &[Saved], copied: bool) {
    let Some(latest) = saved.last() else { return };
    let title = match (saved.len(), copied) {
        (1, true) => format!("{} copied", latest.capture.kind.noun()),
        (1, false) => format!("{} saved", latest.capture.kind.noun()),
        (n, true) => format!("{n} new captures, latest copied"),
        (n, false) => format!("{n} new captures saved"),
    };
    let mut body = latest.capture.title.clone();
    if copied && latest.over_discord_limit() {
        body.push_str(". Over Discord's 10 MB upload limit");
    }
    ctx.notify(&title, &body);
}

// ---- searching ---------------------------------------------------------------------------------

struct Listing {
    /// Newest upload first.
    captures: Vec<Capture>,
    /// Searches that failed while the other worked.
    problems: Vec<String>,
}

/// Screenshots and recordings both, always: what counts as seen shouldn't depend on whether either is
/// being downloaded right now. Only fails if both searches do.
fn list(xbox: &mut Xbox) -> Result<Listing, XboxError> {
    let mut captures = Vec::new();
    let mut problems = Vec::new();
    for kind in [CaptureKind::Screenshot, CaptureKind::Recording] {
        match xbox.captures(kind) {
            Ok(mut found) => captures.append(&mut found),
            Err(XboxError::SignInExpired) => return Err(XboxError::SignInExpired),
            Err(XboxError::Failed(m)) => problems.push(m),
        }
    }
    if problems.len() == 2 {
        return Err(XboxError::Failed(problems.swap_remove(0)));
    }
    captures.sort_by(|a, b| b.uploaded.cmp(&a.uploaded));
    Ok(Listing { captures, problems })
}

// ---- the feature -------------------------------------------------------------------------------

struct MainLoop {
    worker: Worker,
    done: Arc<AtomicBool>,
}

pub struct XboxCaptures {
    shared: Arc<Shared>,
    core: Arc<Core>,
    main: Mutex<Option<MainLoop>>,
}

impl XboxCaptures {
    pub fn new(shared: Arc<Shared>) -> Self {
        Self { shared, core: Arc::default(), main: Mutex::new(None) }
    }

    /// The newest capture, downloaded if it isn't already, and copied.
    fn copy_latest(&self, ctx: &Ctx) -> anyhow::Result<()> {
        let s: Settings = ctx.settings();
        let ms = s.ms_client_id.trim();
        let Some(token) = self.shared.token(ctx, ms) else { bail!("Sign in to Xbox first") };
        let mut xbox = Xbox::new(ms);
        xbox.set_refresh_token(Some(token));

        let listing = list(&mut xbox).map_err(|e| match e {
            XboxError::SignInExpired => anyhow!("Your Xbox sign-in expired, sign in again"),
            XboxError::Failed(m) => anyhow!("Can't reach Xbox Live: {m}"),
        })?;
        // Another feature may have used the old token meanwhile, so hand them the new one
        if let Some(rotated) = xbox.take_rotated() {
            self.shared.save_token(ctx, ms, Some(&rotated));
            self.shared.auth_changed();
        }
        for p in &listing.problems {
            ctx.warn(p.clone());
        }
        if listing.captures.is_empty() {
            bail!(
                "No captures on Xbox network yet. Check Automatically upload is on in the console's Capture & share settings"
            );
        }
        let latest = listing
            .captures
            .iter()
            .find(|c| s.downloads(c.kind))
            .context("Both screenshots and recordings are turned off")?;
        let xuid = xbox.xsts().map_err(|e| anyhow!("{e}"))?.xuid;

        let path = {
            let _io = self.core.io.lock();
            let path = fetch(&s.folder(latest.kind), latest).map_err(|e| anyhow!("Couldn't download it: {e}"))?;
            // Only once the first listing has been seeded: otherwise this one capture would make
            // the worker think everything else was new
            let state = Core::path(ctx);
            let mut store = self.core.store(&state);
            if let Some(seen) = store.seen.as_mut().filter(|s| s.xuid == xuid) {
                seen.remember([latest.id.clone()]);
                let _ = store.save(&state);
            }
            path
        };

        copy_to_clipboard(latest.kind, &path).context("Couldn't copy it")?;
        let saved = [Saved { capture: latest.clone(), path }];
        toast(ctx, &saved, true);
        ctx.info(format!("Copied the latest {} ({})", latest.kind.noun().to_lowercase(), latest.title));
        Ok(())
    }
}

impl Feature for XboxCaptures {
    fn info(&self) -> FeatureInfo {
        FeatureInfo {
            id: "xbox-captures",
            name: "Xbox captures",
            description: "Copy new Xbox screenshots and recordings to the clipboard",
            icon: "camera",
            enabled_by_default: false,
        }
    }

    fn defaults(&self) -> Value {
        let d = Settings::default();
        json!({
            "screenshotFolder": d.screenshot_folder,
            "downloadScreenshots": d.download_screenshots,
            "copyScreenshots": d.copy_screenshots,
            "recordingFolder": d.recording_folder,
            "downloadRecordings": d.download_recordings,
            "copyRecordings": d.copy_recordings,
            "checkWithoutXboxMode": d.check_without_xbox_mode,
            "msClientId": d.ms_client_id,
        })
    }

    fn schema(&self, _ctx: &Ctx) -> Vec<Field> {
        let mut fields = Vec::new();
        // Once Xbox mode knows whether the Xbox is on, this has no effect
        if self.shared.power() == Power::Unknown {
            fields.push(
                Field::toggle("checkWithoutXboxMode", "Check without Xbox mode")
                    .help("Keep checking Xbox Live now and then, since nothing says when the Xbox is on"),
            );
        }
        fields.extend([
            Field::toggle("downloadScreenshots", "Download screenshots").section("Screenshots"),
            Field::toggle("copyScreenshots", "Copy to the clipboard")
                .help("The newest one, ready to paste into Discord or Paint")
                .section("Screenshots"),
            Field::text("screenshotFolder", "Save to")
                .placeholder(default_folder(CaptureKind::Screenshot).display().to_string())
                .section("Screenshots"),
            Field::toggle("downloadRecordings", "Download recordings").section("Recordings"),
            Field::toggle("copyRecordings", "Copy to the clipboard")
                .help("Pasted as a file. Discord only takes 10 MB without Nitro")
                .section("Recordings"),
            Field::text("recordingFolder", "Save to")
                .placeholder(default_folder(CaptureKind::Recording).display().to_string())
                .section("Recordings"),
        ]);
        fields.push(
            Field::text("msClientId", "Microsoft client ID")
                .placeholder("Xbox app's public client")
                .help("Your own Azure app registration's client ID. Needs its own sign-in.")
                .section("Advanced"),
        );
        fields
    }

    fn actions(&self, ctx: &Ctx) -> Vec<ActionDef> {
        let s: Settings = ctx.settings();
        let mut actions = Vec::new();
        if self.shared.token(ctx, &s.ms_client_id).is_some() {
            actions.push(ActionDef::new("copy-latest", "Copy latest capture").icon("image"));
        }
        actions.push(ActionDef::new("open-screenshots", "Open screenshots folder").icon("folder-open"));
        if s.download_recordings {
            actions.push(ActionDef::new("open-recordings", "Open recordings folder").icon("folder-open").no_flyout());
        }
        actions.push(self.shared.account_action(ctx, &s.ms_client_id));
        actions
    }

    fn run(&self, ctx: &Ctx, action: &str) -> anyhow::Result<()> {
        match action {
            "copy-latest" => self.copy_latest(ctx),
            "open-screenshots" | "open-recordings" => {
                let s: Settings = ctx.settings();
                let kind = if action == "open-recordings" { CaptureKind::Recording } else { CaptureKind::Screenshot };
                let folder = s.folder(kind);
                std::fs::create_dir_all(&folder).with_context(|| format!("couldn't create {}", folder.display()))?;
                shell::open_folder(&folder)
            }
            _ => {
                let s: Settings = ctx.settings();
                if !self.shared.run_account_action(ctx, &s.ms_client_id, action)? {
                    bail!("Unknown action {action}");
                }
                Ok(())
            }
        }
    }

    fn start(&self, ctx: &Ctx) {
        let done = Arc::new(AtomicBool::new(false));
        let (c, shared, core, flag) = (ctx.clone(), self.shared.clone(), self.core.clone(), done.clone());
        let worker = Worker::spawn("xbox-captures", move |token| {
            Runner::new(c, shared, core, token).run();
            flag.store(true, Ordering::SeqCst);
        });
        *self.main.lock() = Some(MainLoop { worker, done });
        ctx.refresh();
    }

    fn stop(&self, ctx: &Ctx) {
        self.shared.cancel_own_sign_in(ctx);

        // Taken out first so the lock is released before anything calls back into actions()
        let main = self.main.lock().take();
        let Some(main) = main else { return };
        main.worker.signal();
        // It can be stuck in a download, so don't wait for it beyond a few seconds.
        let deadline = Instant::now() + STOP_WAIT;
        while !main.done.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        if main.done.load(Ordering::SeqCst) {
            main.worker.stop();
        }
    }

    fn settings_changed(&self, _ctx: &Ctx) {
        self.shared.nudge();
    }
}

struct Runner {
    ctx: Ctx,
    shared: Arc<Shared>,
    core: Arc<Core>,
    token: StopToken,
    xbox: Xbox,
    ms_client_id: String,
    auth_seen: u64,
    nudge_seen: u64,
    last_error: Option<String>,
    last_power: Power,
    /// Keep checking until then after the Xbox turns off.
    grace_until: Option<Instant>,
    /// The status last given to the host: `None` when unknown, `Some(None)` when cleared.
    shown_status: Option<Option<(String, Tone)>>,
}

impl Runner {
    fn new(ctx: Ctx, shared: Arc<Shared>, core: Arc<Core>, token: StopToken) -> Self {
        let settings: Settings = ctx.settings();
        let ms_client_id = settings.ms_client_id.trim().to_string();
        let auth_seen = shared.auth_gen.load(Ordering::SeqCst);
        let mut xbox = Xbox::new(&ms_client_id);
        xbox.set_refresh_token(shared.token(&ctx, &ms_client_id));
        let last_power = shared.power();
        Self {
            ctx,
            shared,
            core,
            token,
            xbox,
            ms_client_id,
            auth_seen,
            nudge_seen: 0,
            last_error: None,
            last_power,
            grace_until: None,
            shown_status: None,
        }
    }

    fn run(mut self) {
        while !self.token.stopped() {
            self.nudge_seen = self.shared.nudge.load(Ordering::SeqCst);
            let wait = self.tick();
            if self.sleep(wait) {
                break;
            }
        }
    }

    /// True if the worker should stop.
    fn sleep(&self, dur: Duration) -> bool {
        let end = Instant::now() + dur;
        loop {
            if self.shared.nudge.load(Ordering::SeqCst) != self.nudge_seen {
                return false;
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            if self.token.sleep(left.min(SLICE)) {
                return true;
            }
        }
    }

    /// Only calls into the host when the status changes: clearing it rebuilds the whole UI.
    fn status(&mut self, part: Option<(&str, Tone)>) {
        let part = part.map(|(t, tone)| (t.to_string(), tone));
        if self.shown_status.as_ref() == Some(&part) {
            return;
        }
        match &part {
            Some((text, tone)) => self.ctx.set_status(text.clone(), *tone),
            None => self.ctx.clear_status(),
        }
        self.shown_status = Some(part);
    }

    /// One pass of the loop. Returns how long to wait before the next.
    fn tick(&mut self) -> Duration {
        let settings: Settings = self.ctx.settings();
        self.apply_settings(&settings);

        let generation = self.shared.auth_gen.load(Ordering::SeqCst);
        if generation != self.auth_seen {
            self.auth_seen = generation;
            self.xbox.set_refresh_token(self.shared.token(&self.ctx, &self.ms_client_id));
        }

        if !self.xbox.signed_in() {
            if self.shared.sign_in_pending() {
                self.status(Some(("Waiting for you to sign in", Tone::Idle)));
            } else if self.shared.expired.load(Ordering::SeqCst) {
                self.status(Some(("Sign-in expired", Tone::Warn)));
            } else {
                self.status(Some(("Not signed in", Tone::Warn)));
            }
            return IDLE;
        }

        let power = self.shared.power();
        if self.last_power == Power::On && power == Power::Off {
            self.grace_until = Some(Instant::now() + AFTER_OFF);
        }
        if power != Power::Off {
            self.grace_until = None;
        }
        self.last_power = power;

        let unattended = settings.check_without_xbox_mode.then_some(UNATTENDED_INTERVAL);
        let mut interval = self.shared.poll_every(INTERVAL, unattended);
        if let Some(until) = self.grace_until {
            if Instant::now() < until {
                interval = Some(INTERVAL);
            } else {
                self.grace_until = None;
            }
        }
        let Some(interval) = interval else {
            match power {
                Power::Off => self.status(None),
                _ => self.status(Some((xbox_live::NEEDS_XBOX_MODE, Tone::Idle))),
            }
            return IDLE;
        };

        match self.check(&settings) {
            Ok(()) => {
                self.status(None);
                interval
            }
            Err(e) => self.xbox_failed(e),
        }
    }

    fn apply_settings(&mut self, settings: &Settings) {
        let ms = settings.ms_client_id.trim();
        if ms != self.ms_client_id {
            self.ms_client_id = ms.to_string();
            self.xbox = Xbox::new(ms);
            self.xbox.set_refresh_token(self.shared.token(&self.ctx, ms));
        }
    }

    /// Looks for new captures and deals with them.
    fn check(&mut self, settings: &Settings) -> Result<(), XboxError> {
        let xuid = self.xbox.xsts()?.xuid;
        let listing = list(&mut self.xbox)?;
        self.save_rotated_token();
        for problem in &listing.problems {
            self.report(problem.clone());
        }
        if listing.problems.is_empty() {
            self.last_error = None;
        }

        let state = Core::path(&self.ctx);
        let core = self.core.clone();
        let _io = core.io.lock();

        // Work out what's new, or seed on the first run for this account
        let new = {
            let mut store = core.store(&state);
            match store.seen.as_ref().filter(|s| s.xuid == xuid) {
                None => {
                    // A missing half of the picture would make its captures look new later
                    if !listing.problems.is_empty() {
                        return Ok(());
                    }
                    store.seen = Some(Seen::seeded(&xuid, &listing.captures));
                    let saved = store.save(&state);
                    drop(store);
                    if let Err(e) = saved {
                        self.ctx.warn(format!("couldn't save {}: {e}", state.display()));
                    }
                    self.ctx.info(format!("First check: {} existing captures marked as seen", listing.captures.len()));
                    return Ok(());
                }
                Some(seen) => {
                    let mut new: Vec<Capture> =
                        listing.captures.iter().filter(|c| !seen.contains(&c.id)).cloned().collect();
                    new.reverse(); // oldest first
                    new
                }
            }
        };
        if new.is_empty() {
            return Ok(());
        }

        let mut saved: Vec<Saved> = Vec::new();
        let mut handled: Vec<String> = Vec::new();
        for capture in new {
            if !settings.downloads(capture.kind) {
                handled.push(capture.id);
                continue;
            }
            // A failure isn't marked as seen, so the next check tries again
            match fetch(&settings.folder(capture.kind), &capture) {
                Ok(path) => {
                    self.ctx.info(format!("Saved {}", path.display()));
                    handled.push(capture.id.clone());
                    saved.push(Saved { capture, path });
                }
                Err(e) => self.report(format!(
                    "Couldn't download {} of {}: {e}",
                    capture.kind.noun().to_lowercase(),
                    capture.title
                )),
            }
        }
        {
            let mut store = core.store(&state);
            if let Some(seen) = store.seen.as_mut() {
                seen.remember(handled);
                if let Err(e) = store.save(&state) {
                    drop(store);
                    self.ctx.warn(format!("couldn't save {}: {e}", state.display()));
                }
            }
        }

        if saved.is_empty() {
            return Ok(());
        }
        // The newest of the kinds that get copied
        let mut copied = false;
        if let Some(latest) = saved.iter().rev().find(|s| settings.copies(s.capture.kind)) {
            match copy_to_clipboard(latest.capture.kind, &latest.path) {
                Ok(()) => copied = true,
                Err(e) => self.ctx.warn(format!("couldn't copy to the clipboard: {e:#}")),
            }
        }
        toast(&self.ctx, &saved, copied);
        Ok(())
    }

    fn xbox_failed(&mut self, e: XboxError) -> Duration {
        match e {
            XboxError::SignInExpired => {
                self.shared.save_token(&self.ctx, &self.ms_client_id, None);
                self.xbox.set_refresh_token(None);
                self.shared.expired.store(true, Ordering::SeqCst);
                self.ctx.warn("Xbox sign-in expired, sign in again");
                self.ctx.notify("Xbox", "Your Xbox sign-in expired. Sign in again on the Xbox captures settings page.");
                self.ctx.refresh();
                Duration::ZERO
            }
            XboxError::Failed(m) => {
                self.report(m);
                self.status(Some(("Can't reach Xbox Live, retrying", Tone::Warn)));
                RETRY
            }
        }
    }

    /// Microsoft sometimes replaces the refresh token when it's used. Keep the new one, unless the
    /// user signed out or in while this pass was running.
    fn save_rotated_token(&mut self) {
        if let Some(token) = self.xbox.take_rotated()
            && self.shared.auth_gen.load(Ordering::SeqCst) == self.auth_seen
        {
            self.shared.save_token(&self.ctx, &self.ms_client_id, Some(&token));
        }
    }

    /// Logs a problem unless it's the same one as last time, so a long outage isn't a wall of text.
    fn report(&mut self, message: String) {
        if self.last_error.as_deref() != Some(&message) {
            self.ctx.warn(message.clone());
            self.last_error = Some(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(id: &str) -> Capture {
        Capture {
            id: id.into(),
            kind: CaptureKind::Screenshot,
            title: "Game".into(),
            uploaded: 0,
            captured: 0,
            download: None,
            size: None,
        }
    }

    #[test]
    fn titles_are_made_safe_for_windows() {
        assert_eq!(sanitise("Halo: Infinite"), "Halo_ Infinite");
        assert_eq!(sanitise("a/b\\c|d?e*f\"g<h>i"), "a_b_c_d_e_f_g_h_i");
        assert_eq!(sanitise("  Trailing dots...  "), "Trailing dots");
        assert_eq!(sanitise("..."), "Xbox");
        assert_eq!(sanitise(""), "Xbox");
        assert_eq!(sanitise(&"x".repeat(200)).len(), 80);
    }

    #[test]
    fn files_are_named_by_game_and_time() {
        assert_eq!(
            file_stem("Forza: Horizon 5", Some([2026, 3, 7, 9, 5, 1]), "id"),
            "Forza_ Horizon 5 2026-03-07 09.05.01"
        );
        assert_eq!(file_stem("Game", None, "abcdef123456"), "Game abcdef12");
    }

    #[test]
    fn extensions_come_from_the_file_then_the_url_then_the_kind() {
        let png = b"\x89PNG\r\n\x1a\n";
        assert_eq!(extension(CaptureKind::Screenshot, "https://x/a.jpg", png), "png");
        assert_eq!(extension(CaptureKind::Screenshot, "https://x/a", &[0xFF, 0xD8, 0xFF, 0xE0]), "jpg");
        assert_eq!(extension(CaptureKind::Screenshot, "https://x/a.JPEG?sig=1.png", b"??"), "jpg");
        assert_eq!(extension(CaptureKind::Recording, "https://x/clip.mp4#t", b"ftyp"), "mp4");
        assert_eq!(extension(CaptureKind::Recording, "https://x/clip", b""), "mp4");
        assert_eq!(extension(CaptureKind::Screenshot, "https://x/blob", b""), "png");
    }

    #[test]
    fn seeding_marks_everything_listed_and_remembering_is_capped() {
        let listing = [capture("new"), capture("old")];
        let mut seen = Seen::seeded("1", &listing);
        assert_eq!(seen.seen, vec!["old".to_string(), "new".to_string()]);
        assert!(seen.contains("new") && !seen.contains("other"));

        seen.remember(["new".to_string()]);
        assert_eq!(seen.seen.len(), 2);

        seen.remember((0..SEEN_LIMIT + 50).map(|i| format!("c{i}")));
        assert_eq!(seen.seen.len(), SEEN_LIMIT);
        // The oldest went, the newest stayed
        assert!(!seen.contains("old"));
        assert!(seen.contains(&format!("c{}", SEEN_LIMIT + 49)));
    }

    #[test]
    fn seen_state_round_trips_and_tolerates_a_bare_list() {
        let s = Seen { xuid: "7".into(), seen: vec!["a".into()] };
        let back: Seen = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        let bare: Seen = serde_json::from_str(r#"{"seen":["a"]}"#).unwrap();
        assert_eq!(bare.xuid, "");
    }

    #[test]
    fn defaults_match_the_settings_struct() {
        let from_json: Settings = serde_json::from_value(XboxCaptures::new(Arc::default()).defaults()).unwrap();
        assert_eq!(from_json, Settings::default());
    }
}
