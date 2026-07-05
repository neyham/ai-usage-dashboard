//! DeepSeek API key resolution. Priority mirrors the WinForms prototype:
//!   1. OS credential store (Windows Credential Manager / macOS Keychain / etc.)
//!   2. DEEPSEEK_API_KEY environment variable
//!   3. deepSeekApiKey from config (plaintext fallback)
//!
//! The key is read into memory only when a request is about to be made and is
//! never logged, cached to disk, or sent to the renderer.

use crate::config::Config;

pub fn deepseek_key(config: &Config) -> Option<String> {
    if let Some(k) = from_credential_store(&config.deep_seek_credential_target) {
        let k = k.trim().to_string();
        if !k.is_empty() {
            return Some(k);
        }
    }
    if let Ok(k) = std::env::var("DEEPSEEK_API_KEY") {
        let k = k.trim().to_string();
        if !k.is_empty() {
            return Some(k);
        }
    }
    let k = config.deep_seek_api_key.trim().to_string();
    if !k.is_empty() {
        return Some(k);
    }
    None
}

// ---------- Windows: read the exact Credential Manager target ----------
//
// The WinForms prototype stored the key under the literal generic-credential
// target "AiUsageDashboard/DeepSeekApiKey", so we read that exact name rather
// than going through keyring's name-mangling.

#[cfg(windows)]
fn from_credential_store(target: &str) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Security::Credentials::{
        CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC,
    };

    if target.is_empty() {
        return None;
    }
    let wide: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();

    unsafe {
        let mut pcred: *mut CREDENTIALW = std::ptr::null_mut();
        let res = CredReadW(PCWSTR(wide.as_ptr()), CRED_TYPE_GENERIC, 0, &mut pcred);
        if res.is_err() || pcred.is_null() {
            return None;
        }
        let cred = &*pcred;
        let size = cred.CredentialBlobSize as usize;
        let result = if size == 0 || cred.CredentialBlob.is_null() {
            None
        } else {
            let bytes = std::slice::from_raw_parts(cred.CredentialBlob, size);
            Some(decode_blob(bytes))
        };
        CredFree(pcred as *const core::ffi::c_void);
        result.filter(|s| !s.is_empty())
    }
}

/// Most Windows tools (PowerShell, cmdkey) store the blob as UTF-16LE; some
/// store UTF-8. Try UTF-16 first, then fall back to UTF-8 — same order as the
/// WinForms prototype.
#[cfg(windows)]
fn decode_blob(bytes: &[u8]) -> String {
    if bytes.len() % 2 == 0 {
        let utf16: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        if let Ok(s) = String::from_utf16(&utf16) {
            let trimmed = s.trim_end_matches('\0');
            if !trimmed.is_empty() && !trimmed.contains('\0') {
                return trimmed.to_string();
            }
        }
    }
    String::from_utf8_lossy(bytes)
        .trim_end_matches('\0')
        .to_string()
}

// ---------- Non-Windows: use the keyring crate ----------
//
// Target form "service/account" maps to keyring's (service, account). This is
// where macOS Keychain support lands when the app ships on macOS.

#[cfg(not(windows))]
fn from_credential_store(target: &str) -> Option<String> {
    let (service, account) = target.split_once('/').unwrap_or((target, "DeepSeekApiKey"));
    let entry = keyring::Entry::new(service, account).ok()?;
    entry.get_password().ok()
}
