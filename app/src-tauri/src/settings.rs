//! User settings, stored as settings.json in the app's config folder. Secrets (the Claude API
//! key, the Google Drive sign-in) are not settings: they live in the macOS Keychain (Windows
//! Credential Manager later).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::vocabulary::{self, VocabTerm};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The user's own name, shown instead of "Me" in transcripts and told to Claude.
    pub your_name: String,
    /// Global start/stop shortcut, in Tauri accelerator syntax.
    pub shortcut: String,
    /// Language for new meetings: "he", "en" or "auto".
    pub default_language: String,
    /// "compressed" (quantized, faster, the default on Intel Macs) or "full".
    pub hebrew_model: String,
    /// "now" (right after recording), "on_power" (wait until plugged in) or "manual".
    pub transcribe_when: String,
    pub auto_summarize: bool,
    pub claude_model: String,
    /// Use the built-in list of English terms Israelis say in Hebrew.
    pub builtin_vocabulary: bool,
    /// The user's own terms, on top of the built-in list.
    pub vocabulary: Vec<VocabTerm>,
    /// Upload every call to Google Drive once the account is connected in Settings.
    pub drive_upload: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            your_name: String::new(),
            shortcut: default_shortcut().into(),
            default_language: "he".into(),
            hebrew_model: if cfg!(target_arch = "x86_64") {
                "compressed"
            } else {
                "full"
            }
            .into(),
            transcribe_when: "now".into(),
            auto_summarize: true,
            claude_model: crate::summary::DEFAULT_MODEL.into(),
            builtin_vocabulary: true,
            vocabulary: Vec::new(),
            drive_upload: true,
        }
    }
}

pub fn default_shortcut() -> &'static str {
    if cfg!(target_os = "macos") {
        "Control+Alt+Super+R"
    } else {
        "Control+Alt+Shift+R"
    }
}

pub fn load(path: &Path) -> Settings {
    let mut settings: Settings = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    // v1 saved its four default terms as the user's list; they are built in now.
    if vocabulary::is_v1_default(&settings.vocabulary) {
        settings.vocabulary.clear();
    }
    settings
}

pub fn save(path: &Path, settings: &Settings) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    crate::library::write_atomic(path, &json)
}

const KEYCHAIN_SERVICE: &str = "dev.slikk.gatit";
const KEYCHAIN_ACCOUNT: &str = "anthropic-api-key";

fn key_entry(account: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, account).map_err(|e| e.to_string())
}

/// Reads one of Gatit's keychain items, or None when it isn't there (or is empty).
pub fn secret(account: &str) -> Option<String> {
    key_entry(account)
        .ok()?
        .get_password()
        .ok()
        .filter(|k| !k.trim().is_empty())
}

/// Stores a keychain item; an empty value deletes it.
pub fn set_secret(account: &str, value: &str) -> Result<(), String> {
    let entry = key_entry(account)?;
    let value = value.trim();
    if value.is_empty() {
        return match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        };
    }
    entry.set_password(value).map_err(|e| e.to_string())
}

pub fn api_key() -> Option<String> {
    secret(KEYCHAIN_ACCOUNT)
}

pub fn set_api_key(key: &str) -> Result<(), String> {
    set_secret(KEYCHAIN_ACCOUNT, key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_take_defaults() {
        let s: Settings = serde_json::from_str(r#"{"default_language":"en"}"#).unwrap();
        assert_eq!(s.default_language, "en");
        assert_eq!(s.your_name, "");
        assert_eq!(s.transcribe_when, "now");
        assert!(s.builtin_vocabulary);
        assert!(s.vocabulary.is_empty());
    }
}
