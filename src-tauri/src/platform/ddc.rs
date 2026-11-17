//! DDC/CI through Windows' built-in dxva2: read and set monitor VCP features (input, brightness…).

use windows::Win32::Devices::Display::{
    CapabilitiesRequestAndCapabilitiesReply, DestroyPhysicalMonitor, GetCapabilitiesStringLength,
    GetNumberOfPhysicalMonitorsFromHMONITOR, GetPhysicalMonitorsFromHMONITOR, GetVCPFeatureAndVCPFeatureReply,
    PHYSICAL_MONITOR, SetVCPFeature,
};
use windows::Win32::Foundation::{HANDLE, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW};
use windows::core::BOOL;

/// VCP 0x60: input source.
pub const INPUT_SOURCE: u8 = 0x60;

/// Standard MCCS input values.
pub const INPUTS: &[(u32, &str)] = &[
    (0x0F, "DisplayPort 1"),
    (0x10, "DisplayPort 2"),
    (0x11, "HDMI 1"),
    (0x12, "HDMI 2"),
    (0x1B, "USB-C"),
];

/// A physical monitor handle; released when dropped.
pub struct Monitor {
    /// e.g. \\.\DISPLAY1
    pub device: String,
    pub description: String,
    handle: HANDLE,
}

impl Drop for Monitor {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyPhysicalMonitor(self.handle);
        }
    }
}

impl Monitor {
    pub fn get(&self, code: u8) -> Option<u32> {
        let mut current = 0u32;
        let ok = unsafe { GetVCPFeatureAndVCPFeatureReply(self.handle, code, None, &mut current, None) };
        (ok != 0).then_some(current)
    }

    pub fn set(&self, code: u8, value: u32) -> bool {
        unsafe { SetVCPFeature(self.handle, code, value) != 0 }
    }

    /// The MCCS capabilities string. Slow (often around a second), so cache what you learn from it.
    pub fn capabilities(&self) -> Option<String> {
        let mut len = 0u32;
        unsafe {
            if GetCapabilitiesStringLength(self.handle, &mut len) == 0 || len == 0 {
                return None;
            }
            let mut buf = vec![0u8; len as usize];
            if CapabilitiesRequestAndCapabilitiesReply(self.handle, &mut buf) == 0 {
                return None;
            }
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            Some(String::from_utf8_lossy(&buf[..end]).into_owned())
        }
    }

    /// The `model(...)` part of the capabilities string, e.g. "Acme 27Q".
    pub fn model(&self) -> Option<String> {
        model_from_caps(&self.capabilities()?)
    }
}

pub fn model_from_caps(caps: &str) -> Option<String> {
    let start = caps.find("model(")? + "model(".len();
    let end = caps[start..].find(')')? + start;
    Some(caps[start..end].trim().to_string())
}

/// Every physical monitor Windows is currently driving. Ones it has switched off don't appear.
pub fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn each(hmon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let handles = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
        handles.push(hmon);
        true.into()
    }

    let mut handles: Vec<HMONITOR> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut handles as *mut _ as isize));
    }

    let mut out = Vec::new();
    for hmon in handles {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        unsafe {
            if !GetMonitorInfoW(hmon, &mut info as *mut _ as *mut MONITORINFO).as_bool() {
                continue;
            }
        }
        let device = wide_to_string(&info.szDevice);

        let mut count = 0u32;
        if unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(hmon, &mut count) }.is_err() || count == 0 {
            continue;
        }
        let mut physical = vec![PHYSICAL_MONITOR::default(); count as usize];
        if unsafe { GetPhysicalMonitorsFromHMONITOR(hmon, &mut physical) }.is_err() {
            continue;
        }
        for p in physical {
            // PHYSICAL_MONITOR is packed, so copy fields out before borrowing them
            let description = p.szPhysicalMonitorDescription;
            out.push(Monitor { device: device.clone(), description: wide_to_string(&description), handle: p.hPhysicalMonitor });
        }
    }
    out
}

fn wide_to_string(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

pub fn input_name(value: u32) -> String {
    INPUTS
        .iter()
        .find(|(v, _)| *v == value)
        .map(|(_, n)| n.to_string())
        .unwrap_or_else(|| format!("input 0x{value:02X}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_model_from_capabilities() {
        let caps = "(prot(monitor)type(LCD)model(Acme 27Q)cmds(01 02 03)vcp(02 04 60(0F 11 12)))";
        assert_eq!(model_from_caps(caps).as_deref(), Some("Acme 27Q"));
        assert_eq!(model_from_caps("(prot(monitor))"), None);
    }
}
