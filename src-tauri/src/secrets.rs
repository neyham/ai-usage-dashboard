//! DeepSeek API key resolution:
//!   1. deepSeekApiKey saved directly from the settings UI
//!   2. DEEPSEEK_API_KEY environment variable
//!
//! The key is read into memory only when a request is about to be made and is
//! never logged, cached to disk, or sent to the renderer.

use crate::config::Config;

pub fn deepseek_key(config: &Config) -> Option<String> {
    let k = config.deep_seek_api_key.trim().to_string();
    if !k.is_empty() {
        return Some(k);
    }
    if let Ok(k) = std::env::var("DEEPSEEK_API_KEY") {
        let k = k.trim().to_string();
        if !k.is_empty() {
            return Some(k);
        }
    }
    None
}
