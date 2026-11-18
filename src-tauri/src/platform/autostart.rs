//! Start at login, as a shortcut in the user's Startup folder.
//!
//! Not the `Run` registry key: recent Windows builds skip `Run` entries that apps add
//! themselves, so a shortcut is the only no-admin route that actually launches.

use std::path::PathBuf;

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
    IPersistFile,
};
use windows::Win32::UI::Shell::{FOLDERID_Startup, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellLink};
use windows::core::{HSTRING, Interface};

const ARGS: &str = "--background";

fn shortcut() -> anyhow::Result<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Startup, KF_FLAG_DEFAULT, None)?;
        let dir = PathBuf::from(p.to_string()?);
        CoTaskMemFree(Some(p.0 as _));
        Ok(dir.join("Switchboard.lnk"))
    }
}

pub fn is_enabled() -> bool {
    shortcut().is_ok_and(|p| p.exists())
}

pub fn set(enabled: bool) -> anyhow::Result<()> {
    let path = shortcut()?;
    if !enabled {
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    unsafe {
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let result = (|| -> windows::core::Result<()> {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.SetPath(&HSTRING::from(exe.as_os_str()))?;
            link.SetArguments(&HSTRING::from(ARGS))?;
            if let Some(dir) = exe.parent() {
                link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
            }
            link.cast::<IPersistFile>()?.Save(&HSTRING::from(path.as_os_str()), true)
        })();
        if init.is_ok() {
            CoUninitialize();
        }
        result?;
    }
    Ok(())
}
