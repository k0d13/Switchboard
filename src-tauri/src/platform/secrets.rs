//! Small secrets (tokens) encrypted with DPAPI, so only this Windows account can read them.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};
use windows::core::PCWSTR;

pub struct Secrets {
    path: PathBuf,
    values: BTreeMap<String, String>,
}

impl Secrets {
    pub fn load(path: PathBuf) -> Self {
        let values = fs::read(&path)
            .ok()
            .and_then(|enc| unprotect(&enc))
            .and_then(|plain| serde_json::from_slice(&plain).ok())
            .unwrap_or_default();
        Self { path, values }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.values.get(key).cloned()
    }

    pub fn set(&mut self, key: &str, value: Option<&str>) -> anyhow::Result<()> {
        match value {
            Some(v) => self.values.insert(key.to_string(), v.to_string()),
            None => self.values.remove(key),
        };
        let plain = serde_json::to_vec(&self.values)?;
        fs::write(&self.path, protect(&plain)?)?;
        Ok(())
    }
}

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
}

fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    // SAFETY: DPAPI allocated `out` with LocalAlloc; copy it then free it.
    unsafe {
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData as _)));
        v
    }
}

fn protect(plain: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe { CryptProtectData(&blob(plain), PCWSTR::null(), None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)? };
    Ok(take(out))
}

fn unprotect(enc: &[u8]) -> Option<Vec<u8>> {
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe { CryptUnprotectData(&blob(enc), None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).ok()? };
    Some(take(out))
}
