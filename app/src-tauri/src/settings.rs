//! User settings, stored as settings.json in the app's config folder. The Claude API key is
//! not a setting: it lives in the macOS Keychain (Windows Credential Manager later).

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
    /// Show a "Record this call?" pop-up when a Zoom, Meet or WhatsApp call starts.
    pub ask_on_call: bool,
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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            your_name: String::new(),
            shortcut: default_shortcut().into(),
            ask_on_call: true,
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

fn key_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).map_err(|e| e.to_string())
}

pub fn api_key() -> Option<String> {
    key_entry()
        .ok()?
        .get_password()
        .ok()
        .filter(|k| !k.trim().is_empty())
}

pub fn set_api_key(key: &str) -> Result<(), String> {
    let entry = key_entry()?;
    let key = key.trim();
    if key.is_empty() {
        return match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        };
    }
    entry.set_password(key).map_err(|e| e.to_string())
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
        assert!(s.ask_on_call);
        assert!(s.builtin_vocabulary);
        assert!(s.vocabulary.is_empty());
    }
}
