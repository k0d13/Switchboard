# Switchboard

Windows tray app for PC utilities. Tauri 2 (Rust, `src-tauri/`) + Svelte 5 UI (`src/`). PLAN.md has the architecture; README.md is the public overview. Intended to go on GitHub, so **nothing specific to the owner's setup is hardcoded** (monitor names, inputs, paths, IDs). Personal values live in `%APPDATA%\Switchboard\settings.json`, never in defaults.

## Every change: build, restart, verify

Don't stop at "it compiles". After any Rust or UI change:

1. Stop the running copy (it locks the exe): `Get-Process switchboard -ErrorAction SilentlyContinue | Stop-Process -Force`
2. Build: `mise exec -- npm run tauri build -- --no-bundle` (about a minute). An `ERR_PNPM_PACKAGE_MANAGER_SYMLINK_FAILED` line is noise; look for `Finished`.
3. Start it again the way login does: `Start-Process src-tauri\target\release\switchboard.exe -ArgumentList '--background'`. Start at login points at this exe, so it must be running from there when you finish.
4. Check `%LOCALAPPDATA%\Switchboard\switchboard.log` and that `(Get-Process switchboard).Responding` is True.
5. Test the actual behaviour (below), then suggest a commit message.

Tests: `mise exec -- cargo test --lib` in `src-tauri`. UI type check: `mise exec -- npm run check`.

Be economical: test what changed once, report, and stop. Don't loop on edge cases without checking in. If something would switch the owner's monitor or touch their accounts, ask first.

## Testing the real app

- **Run an action:** `switchboard.exe run <feature>.<action>` (e.g. `xbox.pc-mode`) goes to the running copy.
- **Open the flyout:** `switchboard.exe flyout` does the same as a tray click.
- **Drive the webviews:** start the app with `$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9333'`, then `node scripts/cdp.mjs list` or `node scripts/cdp.mjs eval <flyout|settings> @file.js`. Use `@file` for anything with quotes, because PowerShell 5 strips them from native arguments. Call node via `mise which node` to skip mise's startup delay.
- **Window positions:** `scripts/windows.ps1` (default `watch` mode) prints every visible Switchboard window and its rect each time they change. It catches flashes and windows that open or close by themselves. `-Mode shot -Out x.png` screenshots all monitors, which you can then crop and look at; `-Mode list` lists the windows once.
- `& switchboard.exe ...` returns immediately (it's a GUI exe). The second instance forwards its arguments and exits.

## Pitfalls that already bit us

- **Deadlocks from re-entrancy.** `Ctx::set_status`, `set_prompt`, `refresh` and `notify` call `Host::changed()`, which calls every feature's `actions()`. So a feature must never call into `Ctx` while holding one of its own locks that `actions()` also takes. Take values out, drop the guard, then call. Watch `if let Some(x) = mutex.lock().take() { ctx... }`: the guard lives for the whole block.
- **Never build a window on the main thread inside an event.** Non-async Tauri commands, tray events and single-instance callbacks all run on the main thread. Building a webview there deadlocks WebView2, and re-entrant events make it worse. `ui.rs` routes every open and close through `off_main`, which runs them on a thread, one at a time. Keep it that way.
- **Commands that touch the host are async** and use `spawn_blocking` (see `commands.rs`'s `blocking` helper). A slow feature otherwise freezes every window.
- **Hidden WebView2 windows don't render**, so ResizeObserver never fires. The flyout is shown off-screen at -32000,-32000 until `flyout_resize` moves it into place. A double click sends two toggles; the second is ignored for 500 ms.
- **Finding monitors:** use `display_config::displays()`, which gives EDID names instantly. DDC/CI capability strings take about a second per monitor, so only use them as a fallback.
- **PowerShell 5 writes UTF-8 with a BOM** (`Set-Content -Encoding utf8`), and serde_json rejects it. Write JSON or config files with the Write tool or bash.
- Hotkeys fail to register if another app holds them. The log says so on startup.

## Conventions

- New feature: one file or folder in `src-tauri/src/features/`, implementing `Feature` (`core/feature.rs`), plus one line in `features/mod.rs`. Settings are a `#[serde(default, rename_all = "camelCase")]` struct whose `Default` matches `defaults()` (add a test like the xbox feature's). Actions become tray items, hotkeys, flyout tiles and CLI commands automatically.
- Features talk to each other through `ctx.broadcast` and `on_event`. Use `ctx.run_action` to trigger one of a feature's own actions from an event.
- `src/lib/types.ts` is the contract between UI and Rust; change both together. `src/lib/api.ts` has mock data for `npm run dev` in a browser.
- Background work uses `core::worker::Worker` (stop signal plus an interruptible sleep).
- Keep it small: no new UI libraries, and think twice about new crates.
- Update PLAN.md and README.md when features change.
