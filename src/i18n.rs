//! Tiny, dependency-light internationalization layer.
//!
//! Translations live in `locales/<code>.json` and are embedded in the binary
//! with `include_str!`, so adding a language only requires a new file plus an
//! arm in [`Language`].

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// User-facing language preference, including "follow the system".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    System,
    En,
    Es,
}

impl Language {
    /// All variants in presentation order.
    pub const ALL: [Language; 3] = [Language::System, Language::En, Language::Es];

    /// Translation key for the label.
    pub fn label_key(&self) -> &'static str {
        match self {
            Language::System => "language.system",
            Language::En => "language.en",
            Language::Es => "language.es",
        }
    }

    /// Resolves a concrete language code (`en`/`es`) for this preference.
    pub fn resolve(self) -> &'static str {
        match self {
            Language::En => "en",
            Language::Es => "es",
            Language::System => detect_system_language(),
        }
    }
}

/// Reads `LC_ALL`/`LC_MESSAGES`/`LANG` and returns `es` when the locale is
/// Spanish, otherwise `en`.
fn detect_system_language() -> &'static str {
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(value) = std::env::var(key) {
            let value = value.to_ascii_lowercase();
            if value.starts_with("es") {
                return "es";
            }
            if value.starts_with("en") {
                return "en";
            }
        }
    }
    "en"
}

const EN_JSON: &str = include_str!("../locales/en.json");
const ES_JSON: &str = include_str!("../locales/es.json");

/// Resolved translator instance. Both maps are parsed once, up front, so
/// lookups never touch the JSON again.
#[derive(Clone)]
pub struct I18n {
    map: HashMap<String, String>,
    fallback: HashMap<String, String>,
}

impl I18n {
    /// Builds a translator for the given preference.
    pub fn new(language: Language) -> Self {
        let english: HashMap<String, String> = serde_json::from_str(EN_JSON).unwrap_or_default();

        match language.resolve() {
            "es" => Self {
                map: serde_json::from_str(ES_JSON).unwrap_or_default(),
                fallback: english,
            },
            _ => Self {
                map: english,
                fallback: HashMap::new(),
            },
        }
    }

    /// Translates a key. Falls back to the English map, then to the key
    /// itself, so a missing translation is visible but never panics.
    pub fn t(&self, key: &str) -> String {
        if let Some(value) = self.map.get(key) {
            return value.clone();
        }
        if let Some(value) = self.fallback.get(key) {
            return value.clone();
        }
        key.to_string()
    }
}
