//! The two webview windows. Both are created on demand and destroyed when closed, so the tray app
//! costs no WebView2 memory while nothing is open.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tauri::utils::config::WindowEffectsConfig;
use tauri::window::Effect;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, Position, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

const FLYOUT_WIDTH: f64 = 360.0;
const FLYOUT_MARGIN: f64 = 12.0;

/// When the flyout last closed because it lost focus. Clicking the tray icon while the flyout is
/// open steals focus first, so without this the click would close and immediately reopen it.
static FLYOUT_BLURRED: Mutex<Option<Instant>> = Mutex::new(None);
/// When the flyout was last opened, to swallow the second click of a double click.
static FLYOUT_OPENED: Mutex<Option<Instant>> = Mutex::new(None);
/// Where the tray was clicked, to pick the monitor the flyout opens on.
static FLYOUT_ANCHOR: Mutex<Option<PhysicalPosition<f64>>> = Mutex::new(None);
/// One window open/close at a time, so a double click can't race itself.
static UI: Mutex<()> = Mutex::new(());

/// Runs window work on its own thread. Building a webview from inside an event callback (a tray
/// click, a second launch, a non-async command) can deadlock: WebView2 needs the main thread's
/// message loop to finish, and another event can re-enter while it waits. From any other thread,
/// Tauri hands the work to the event loop and waits for it safely.
fn off_main(app: &AppHandle, f: impl FnOnce(&AppHandle) + Send + 'static) {
    let app = app.clone();
    std::thread::spawn(move || {
        let _one_at_a_time = UI.lock();
        f(&app);
    });
}

pub fn open_settings(app: &AppHandle, feature: Option<String>) {
    off_main(app, move |app| open_settings_now(app, feature));
}

/// Closes the flyout and opens Settings, in that order.
pub fn flyout_to_settings(app: &AppHandle, feature: Option<String>) {
    off_main(app, move |app| {
        if let Some(w) = app.get_webview_window("flyout") {
            let _ = w.close();
        }
        open_settings_now(app, feature);
    });
}

pub fn toggle_flyout(app: &AppHandle, click: PhysicalPosition<f64>) {
    off_main(app, move |app| toggle_flyout_now(app, click));
}

fn open_settings_now(app: &AppHandle, feature: Option<String>) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        if let Some(f) = feature {
            let _ = app.emit_to("settings", "navigate", f);
        }
        return;
    }

    let built = WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("index.html".into()))
        .title("Switchboard")
        .inner_size(880.0, 600.0)
        .min_inner_size(640.0, 420.0)
        .center()
        .decorations(false)
        .transparent(true)
        .shadow(true)
        .effects(WindowEffectsConfig { effects: vec![Effect::Mica], ..Default::default() })
        .build();

    match built {
        Ok(_) => {
            if let Some(f) = feature {
                // The page needs a moment to load and start listening
                let app = app.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(900));
                    let _ = app.emit_to("settings", "navigate", f);
                });
            }
        }
        Err(e) => eprintln!("couldn't open settings: {e}"),
    }
}

fn toggle_flyout_now(app: &AppHandle, click: PhysicalPosition<f64>) {
    if let Some(w) = app.get_webview_window("flyout") {
        // The second click of a double click shouldn't close what the first one opened
        if FLYOUT_OPENED.lock().is_some_and(|t| t.elapsed() < Duration::from_millis(500)) {
            return;
        }
        if w.is_visible().unwrap_or(false) {
            let _ = w.close();
        } else {
            let _ = w.show();
            let _ = w.set_focus();
        }
        return;
    }
    if FLYOUT_BLURRED.lock().is_some_and(|t| t.elapsed() < Duration::from_millis(300)) {
        return;
    }
    *FLYOUT_ANCHOR.lock() = Some(click);

    *FLYOUT_OPENED.lock() = Some(Instant::now());

    // Shown off-screen first: WebView2 doesn't render a hidden window, so the page could never
    // measure itself and send the `flyout_resize` that moves it into place. The builder's own
    // position isn't applied reliably before the first paint, so it's moved before being shown.
    let built = WebviewWindowBuilder::new(app, "flyout", WebviewUrl::App("index.html".into()))
        .title("Switchboard")
        .inner_size(FLYOUT_WIDTH, 320.0)
        .visible(false)
        .decorations(false)
        .transparent(true)
        .shadow(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .effects(WindowEffectsConfig { effects: vec![Effect::Acrylic], ..Default::default() })
        .build();

    match built {
        Ok(w) => {
            let handle = w.clone();
            // Windows can send a focus-lost before the window was ever focused (while it's being
            // created), which would close it before it's seen. Only close once it has had focus.
            let had_focus = AtomicBool::new(false);
            w.on_window_event(move |event| match event {
                WindowEvent::Focused(true) => had_focus.store(true, Ordering::SeqCst),
                // The second click of a double click lands on the taskbar and takes focus; take it back
                WindowEvent::Focused(false)
                    if FLYOUT_OPENED.lock().is_some_and(|t| t.elapsed() < Duration::from_millis(500)) =>
                {
                    let handle = handle.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(150));
                        let _ = handle.set_focus();
                    });
                }
                WindowEvent::Focused(false) if had_focus.load(Ordering::SeqCst) => {
                    *FLYOUT_BLURRED.lock() = Some(Instant::now());
                    let _ = handle.close();
                }
                _ => {}
            });
            let _ = w.set_position(Position::Physical(PhysicalPosition::new(-32000, -32000)));
            let _ = w.show();
            let _ = w.set_focus();
        }
        Err(e) => eprintln!("couldn't open flyout: {e}"),
    }
}

/// Sizes the flyout to its content and keeps it in the bottom-right corner of the work area of
/// the monitor the tray is on, like Windows' own flyouts.
pub fn flyout_resize(window: &WebviewWindow, height: f64) {
    let height = height.clamp(80.0, 900.0);
    let _ = window.set_size(LogicalSize::new(FLYOUT_WIDTH, height));

    let anchor = *FLYOUT_ANCHOR.lock();
    let monitor = anchor
        .and_then(|p| window.monitor_from_point(p.x, p.y).ok().flatten())
        .or_else(|| window.primary_monitor().ok().flatten());
    if let Some(m) = monitor {
        let scale = m.scale_factor();
        let area = m.work_area();
        let w = (FLYOUT_WIDTH * scale).round() as i32;
        let h = (height * scale).round() as i32;
        let margin = (FLYOUT_MARGIN * scale).round() as i32;
        let x = area.position.x + area.size.width as i32 - w - margin;
        let y = area.position.y + area.size.height as i32 - h - margin;
        let _ = window.set_position(Position::Physical(PhysicalPosition::new(x, y)));
    }

    if !window.is_visible().unwrap_or(true) {
        let _ = window.show();
    }
    if !window.is_focused().unwrap_or(true) {
        let _ = window.set_focus();
    }
}
