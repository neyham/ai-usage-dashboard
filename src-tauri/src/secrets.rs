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
            decode_blob(bytes)
        };
        CredFree(pcred as *const core::ffi::c_void);
        result
    }
}

/// Most Windows tools (PowerShell, cmdkey) store the blob as UTF-16LE; some
/// store UTF-8. Prefer strict UTF-8 when it contains no embedded NULs, then try
/// UTF-16LE. This avoids treating an even-length ASCII key as arbitrary UTF-16.
#[cfg(any(windows, test))]
fn decode_blob(bytes: &[u8]) -> Option<String> {
    if let Some(decoded) = std::str::from_utf8(bytes)
        .ok()
        .and_then(valid_decoded_secret)
    {
        return Some(decoded);
    }

    if bytes.len().is_multiple_of(2) {
        let utf16: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        if let Some(decoded) = String::from_utf16(&utf16)
            .ok()
            .and_then(|value| valid_decoded_secret(&value))
        {
            return Some(decoded);
        }
    }
    None
}

#[cfg(any(windows, test))]
fn valid_decoded_secret(value: &str) -> Option<String> {
    let trimmed = value.trim_end_matches('\0');
    if trimmed.is_empty() || trimmed.contains('\0') {
        None
    } else {
        Some(trimmed.to_string())
    }
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

#[cfg(test)]
mod tests {
    use super::decode_blob;

    #[test]
    fn credential_blob_decodes_even_length_utf8_without_utf16_corruption() {
        assert_eq!(decode_blob(b"sk-abcde").as_deref(), Some("sk-abcde"));
    }

    #[test]
    fn credential_blob_decodes_utf16le_with_terminator() {
        let mut bytes = Vec::new();
        for unit in "sk-secret".encode_utf16().chain(std::iter::once(0)) {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }

        assert_eq!(decode_blob(&bytes).as_deref(), Some("sk-secret"));
    }

    #[test]
    fn credential_blob_rejects_empty_or_embedded_nul() {
        assert_eq!(decode_blob(b""), None);
        assert_eq!(decode_blob(b"sk\0secret"), None);
    }
}
