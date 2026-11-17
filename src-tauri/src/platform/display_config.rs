//! Display topology (what Win+P switches), through SetDisplayConfig / QueryDisplayConfig.

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, DISPLAYCONFIG_TOPOLOGY_CLONE,
    DISPLAYCONFIG_TOPOLOGY_EXTEND, DISPLAYCONFIG_TOPOLOGY_EXTERNAL, DISPLAYCONFIG_TOPOLOGY_ID,
    DISPLAYCONFIG_TOPOLOGY_INTERNAL, DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QDC_ALL_PATHS,
    QDC_DATABASE_CURRENT, QUERY_DISPLAY_CONFIG_FLAGS, QueryDisplayConfig, SDC_APPLY, SDC_TOPOLOGY_CLONE,
    SDC_TOPOLOGY_EXTEND, SDC_TOPOLOGY_EXTERNAL, SDC_TOPOLOGY_INTERNAL, SetDisplayConfig,
};
use windows::Win32::Foundation::ERROR_SUCCESS;

/// From wingdi.h; the windows crate files it under Graphics::Gdi.
const DISPLAYCONFIG_PATH_ACTIVE: u32 = 0x1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Topology {
    /// "PC screen only"
    Internal,
    /// "Duplicate"
    Clone,
    /// "Extend"
    Extend,
    /// "Second screen only"
    External,
}

impl Topology {
    pub fn label(self) -> &'static str {
        match self {
            Topology::Internal => "PC screen only",
            Topology::Clone => "Duplicate",
            Topology::Extend => "Extend",
            Topology::External => "Second screen only",
        }
    }
}

/// Same as `DisplaySwitch.exe /internal|/clone|/extend|/external`.
pub fn set(t: Topology) -> anyhow::Result<()> {
    let flag = match t {
        Topology::Internal => SDC_TOPOLOGY_INTERNAL,
        Topology::Clone => SDC_TOPOLOGY_CLONE,
        Topology::Extend => SDC_TOPOLOGY_EXTEND,
        Topology::External => SDC_TOPOLOGY_EXTERNAL,
    };
    let code = unsafe { SetDisplayConfig(None, None, SDC_APPLY | flag) };
    if code != 0 {
        anyhow::bail!("Windows refused to switch to {} (error {code})", t.label());
    }
    Ok(())
}

pub fn current() -> Option<Topology> {
    let (_, id) = query(QDC_DATABASE_CURRENT)?;
    match id? {
        DISPLAYCONFIG_TOPOLOGY_INTERNAL => Some(Topology::Internal),
        DISPLAYCONFIG_TOPOLOGY_CLONE => Some(Topology::Clone),
        DISPLAYCONFIG_TOPOLOGY_EXTEND => Some(Topology::Extend),
        DISPLAYCONFIG_TOPOLOGY_EXTERNAL => Some(Topology::External),
        _ => None,
    }
}

/// A monitor plugged into the PC, whether or not Windows is using it right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Display {
    /// The monitor's own name from its EDID, as Windows Settings shows it, e.g. "Acme 32U".
    pub name: String,
    /// GDI device name, e.g. `\\.\DISPLAY1`, while Windows is using it.
    pub device: Option<String>,
}

/// Every connected monitor, with its EDID name. Instant, unlike asking the monitor over DDC/CI.
pub fn displays() -> Vec<Display> {
    let Some((paths, _)) = query(QDC_ALL_PATHS) else { return Vec::new() };
    let mut out: Vec<(u32, i32, u32, Display)> = Vec::new();
    for p in paths {
        let t = p.targetInfo;
        if !t.targetAvailable.as_bool() {
            continue;
        }
        let key = (t.adapterId.LowPart, t.adapterId.HighPart, t.id);
        let active = p.flags & DISPLAYCONFIG_PATH_ACTIVE != 0;
        let device = if active { source_name(&p) } else { None };
        if let Some(existing) = out.iter_mut().find(|(a, b, c, _)| (*a, *b, *c) == key) {
            if existing.3.device.is_none() {
                existing.3.device = device;
            }
            continue;
        }
        let Some(name) = target_name(&p) else { continue };
        out.push((key.0, key.1, key.2, Display { name, device }));
    }
    out.into_iter().map(|(_, _, _, d)| d).collect()
}

fn target_name(p: &DISPLAYCONFIG_PATH_INFO) -> Option<String> {
    let mut req = DISPLAYCONFIG_TARGET_DEVICE_NAME {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
            size: std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
            adapterId: p.targetInfo.adapterId,
            id: p.targetInfo.id,
        },
        ..Default::default()
    };
    if unsafe { DisplayConfigGetDeviceInfo(&mut req.header) } != 0 {
        return None;
    }
    let name = wide(&req.monitorFriendlyDeviceName);
    (!name.is_empty()).then_some(name)
}

fn source_name(p: &DISPLAYCONFIG_PATH_INFO) -> Option<String> {
    let mut req = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
            size: std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
            adapterId: p.sourceInfo.adapterId,
            id: p.sourceInfo.id,
        },
        ..Default::default()
    };
    if unsafe { DisplayConfigGetDeviceInfo(&mut req.header) } != 0 {
        return None;
    }
    let name = wide(&req.viewGdiDeviceName);
    (!name.is_empty()).then_some(name)
}

fn wide(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end]).trim().to_string()
}

/// The topology id is only filled in for QDC_DATABASE_CURRENT.
fn query(flags: QUERY_DISPLAY_CONFIG_FLAGS) -> Option<(Vec<DISPLAYCONFIG_PATH_INFO>, Option<DISPLAYCONFIG_TOPOLOGY_ID>)> {
    let (mut paths, mut modes) = (0u32, 0u32);
    unsafe {
        if GetDisplayConfigBufferSizes(flags, &mut paths, &mut modes) != ERROR_SUCCESS {
            return None;
        }
        let mut path_buf = vec![DISPLAYCONFIG_PATH_INFO::default(); paths as usize];
        let mut mode_buf = vec![DISPLAYCONFIG_MODE_INFO::default(); modes as usize];
        let mut id = DISPLAYCONFIG_TOPOLOGY_ID::default();
        let want_id = flags == QDC_DATABASE_CURRENT;
        let rc = QueryDisplayConfig(
            flags,
            &mut paths,
            path_buf.as_mut_ptr(),
            &mut modes,
            mode_buf.as_mut_ptr(),
            if want_id { Some(&mut id) } else { None },
        );
        if rc != ERROR_SUCCESS {
            return None;
        }
        path_buf.truncate(paths as usize);
        Some((path_buf, want_id.then_some(id)))
    }
}

#[cfg(test)]
mod tests {
    /// Prints what's plugged in: `cargo test displays -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn displays() {
        for d in super::displays() {
            println!("{d:?}");
        }
    }
}
