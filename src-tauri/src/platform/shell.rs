//! Small shell and time helpers: known folders, local time and opening Explorer.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows::Win32::UI::Shell::{
    FOLDERID_Pictures, FOLDERID_Videos, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{GUID, HSTRING, w};

/// The user's Pictures folder, wherever they've moved it (OneDrive often does).
pub fn pictures_dir() -> Option<PathBuf> {
    known_dir(&FOLDERID_Pictures)
}

/// The user's Videos folder, wherever they've moved it.
pub fn videos_dir() -> Option<PathBuf> {
    known_dir(&FOLDERID_Videos)
}

fn known_dir(id: &GUID) -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let dir = p.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(p.0 as _));
        dir
    }
}

/// A Unix timestamp in the local time zone as year, month, day, hour, minute, second.
pub fn local_time(unix: u64) -> Option<[u16; 6]> {
    let ticks = unix.checked_add(11_644_473_600)?.checked_mul(10_000_000)?;
    let file_time = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    unsafe {
        let mut utc = SYSTEMTIME::default();
        FileTimeToSystemTime(&file_time, &mut utc).ok()?;
        let mut local = SYSTEMTIME::default();
        SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).ok()?;
        Some([local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute, local.wSecond])
    }
}

/// Opens a folder in Explorer.
pub fn open_folder(path: &Path) -> anyhow::Result<()> {
    let result =
        unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(path.as_os_str()), None, None, SW_SHOWNORMAL) };
    // ShellExecute reports success as a value above 32
    if result.0 as isize <= 32 {
        anyhow::bail!("Explorer wouldn't open {}", path.display());
    }
    Ok(())
}
