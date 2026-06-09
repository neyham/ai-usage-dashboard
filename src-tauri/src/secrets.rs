//! DeepSeek API key resolution. The key is read at call time and never logged,
//! persisted, or handed to the renderer.
//!
//! Resolution order (highest priority first):
//!   1. OS credential store
//!        - Windows: Credential Manager generic credential whose target name is
//!          `config.deep_seek_credential_target` (default
//!          `AiUsageDashboard/DeepSeekApiKey`). The WinForms prototype wrote the
//!          key as a UTF-16LE string, so the blob is decoded as UTF-16 first.
//!        - other OS: `keyring` (macOS Keychain / Linux Secret Service), using
//!          the target split into `service/user`.
//!   2. `DEEPSEEK_API_KEY` environment variable.
//!   3. `deepSeekApiKey` plaintext fallback in config.json (lowest priority).

use crate::config::Config;

/// Resolve the DeepSeek key, or `None` if nothing is configured.
pub fn deepseek_key(config: &Config) -> Option<String> {
    if let Some(key) = non_empty(from_credential_store(config)) {
        return Some(key);
    }
    if let Some(key) = non_empty(std::env::var("DEEPSEEK_API_KEY").ok()) {
        return Some(key);
    }
    non_empty(Some(config.deep_seek_api_key.clone()))
}

/// Trim and drop empty strings so a blank credential never wins.
fn non_empty(value: Option<String>) -> Option<String> {
    let v = value?;
    let t = v.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

// ---------------------------------------------------------------------------
// Windows: read the generic credential directly from Credential Manager.
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn from_credential_store(config: &Config) -> Option<String> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Security::Credentials::{
        CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC,
    };

    let target = config.deep_seek_credential_target.trim();
    if target.is_empty() {
        return None;
    }
    let wide = HSTRING::from(target);

    // SAFETY: `wide` outlives the call; on success Windows allocates `cred`,
    // which we free with `CredFree` before returning.
    unsafe {
        let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
        if CredReadW(PCWSTR(wide.as_ptr()), CRED_TYPE_GENERIC, 0, &mut cred).is_err()
            || cred.is_null()
        {
            return None;
        }

        let c = &*cred;
        let size = c.CredentialBlobSize as usize;
        let key = if size == 0 || c.CredentialBlob.is_null() {
            None
        } else {
            let bytes = std::slice::from_raw_parts(c.CredentialBlob, size);
            decode_blob(bytes)
        };

        CredFree(cred as *const core::ffi::c_void);
        key
    }
}

/// The credential blob may be a UTF-16LE string (how the WinForms prototype
/// stored it) or, defensively, UTF-8. A real DeepSeek key is ASCII, so a clean
/// ASCII UTF-16 decode is taken as authoritative; otherwise fall back to UTF-8.
#[cfg(windows)]
fn decode_blob(bytes: &[u8]) -> Option<String> {
    if bytes.len() % 2 == 0 {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        if let Ok(s) = String::from_utf16(&units) {
            let t = s.trim_matches('\0').trim();
            if !t.is_empty() && t.is_ascii() {
                return Some(t.to_string());
            }
        }
    }
    let s = String::from_utf8_lossy(bytes);
    let t = s.trim_matches('\0').trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

// ---------------------------------------------------------------------------
// Other platforms: read via the `keyring` crate (macOS Keychain / Secret
// Service). Falls through to env/config when no backend or entry is available.
// ---------------------------------------------------------------------------

#[cfg(not(windows))]
fn from_credential_store(config: &Config) -> Option<String> {
    let target = config.deep_seek_credential_target.trim();
    if target.is_empty() {
        return None;
    }
    // Map "Service/User" onto keyring's (service, user) pair.
    let (service, user) = match target.rsplit_once('/') {
        Some((s, u)) => (s, u),
        None => (target, "deepseek"),
    };
    let entry = keyring::Entry::new(service, user).ok()?;
    entry.get_password().ok()
}
