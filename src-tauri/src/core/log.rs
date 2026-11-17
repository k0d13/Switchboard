//! In-memory log (for the settings window) mirrored to %LOCALAPPDATA%\Switchboard\switchboard.log.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::Serialize;

const KEEP: usize = 1000;
const MAX_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, Serialize)]
pub struct LogLine {
    pub time: u64,
    pub level: Level,
    pub source: String,
    pub message: String,
}

pub struct Log {
    lines: Mutex<VecDeque<LogLine>>,
    file: Mutex<Option<File>>,
}

impl Log {
    pub fn new(path: PathBuf) -> Self {
        // Keep one previous file so a crash's log survives the restart
        if fs::metadata(&path).map(|m| m.len() > MAX_FILE_BYTES).unwrap_or(false) {
            let _ = fs::rename(&path, path.with_extension("old.log"));
        }
        let file = OpenOptions::new().create(true).append(true).open(&path).ok();
        Self { lines: Mutex::new(VecDeque::with_capacity(KEEP)), file: Mutex::new(file) }
    }

    pub fn push(&self, level: Level, source: &str, message: impl Into<String>) -> LogLine {
        let time = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        let line = LogLine { time, level, source: source.into(), message: message.into() };

        if let Some(f) = self.file.lock().as_mut() {
            let secs = time / 1000;
            let _ = writeln!(
                f,
                "{} [{:?}] {}: {}",
                format_utc(secs),
                line.level,
                line.source,
                line.message
            );
        }
        #[cfg(debug_assertions)]
        eprintln!("[{:?}] {}: {}", line.level, line.source, line.message);

        let mut lines = self.lines.lock();
        if lines.len() == KEEP {
            lines.pop_front();
        }
        lines.push_back(line.clone());
        line
    }

    pub fn all(&self) -> Vec<LogLine> {
        self.lines.lock().iter().cloned().collect()
    }
}

/// "2026-11-18 06:42:10Z" without pulling in a date crate.
fn format_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}
