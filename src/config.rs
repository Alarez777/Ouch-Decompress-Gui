//! Persistent application settings stored as JSON under
//! `~/.config/ouch-decompress-gui/config.json`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::i18n::Language;
use crate::modes::DecompressMode;
use crate::theme::ThemeChoice;

/// Directory name used under the user's config dir.
pub const APP_DIR: &str = "ouch-decompress-gui";

/// What to do when the destination already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConflictPolicy {
    /// Ask the user every time.
    Ask,
    /// Overwrite existing files.
    Overwrite,
    /// Keep both by renaming the existing entries with a numeric suffix.
    Rename,
    /// Do not extract the archive at all.
    Skip,
}

impl ConflictPolicy {
    /// All variants in presentation order.
    pub const ALL: [ConflictPolicy; 4] = [
        ConflictPolicy::Ask,
        ConflictPolicy::Overwrite,
        ConflictPolicy::Rename,
        ConflictPolicy::Skip,
    ];

    /// Translation key for the label.
    pub fn label_key(&self) -> &'static str {
        match self {
            ConflictPolicy::Ask => "conflict.ask",
            ConflictPolicy::Overwrite => "conflict.overwrite",
            ConflictPolicy::Rename => "conflict.rename",
            ConflictPolicy::Skip => "conflict.skip",
        }
    }
}

/// What to do with the source archive after a successful extraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AfterExtract {
    /// Move the archive to the trash.
    Trash,
    /// Delete the archive permanently.
    Delete,
    /// Leave the archive untouched.
    Keep,
}

impl AfterExtract {
    /// All variants in presentation order.
    pub const ALL: [AfterExtract; 3] = [
        AfterExtract::Trash,
        AfterExtract::Delete,
        AfterExtract::Keep,
    ];

    /// Translation key for the label.
    pub fn label_key(&self) -> &'static str {
        match self {
            AfterExtract::Trash => "after.trash",
            AfterExtract::Delete => "after.delete",
            AfterExtract::Keep => "after.keep",
        }
    }
}

/// What to do when moving the archive to the trash is not possible (for
/// example on a filesystem without a trash directory).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrashFallback {
    /// Delete the archive permanently.
    Delete,
    /// Leave the archive untouched.
    Nothing,
}

impl TrashFallback {
    /// All variants in presentation order.
    pub const ALL: [TrashFallback; 2] = [TrashFallback::Delete, TrashFallback::Nothing];

    /// Translation key for the label.
    pub fn label_key(&self) -> &'static str {
        match self {
            TrashFallback::Delete => "trash_fallback.delete",
            TrashFallback::Nothing => "trash_fallback.nothing",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Schema version, lets us migrate the file in the future.
    pub version: u32,
    /// How archives should be unpacked.
    pub decompress_mode: DecompressMode,
    /// Passwords tried automatically, in order. Stored in plain text on
    /// purpose; the UI warns the user about this.
    pub passwords: Vec<String>,
    /// Format ids (see `formats::FORMATS`) the user does NOT want to
    /// auto-extract without confirmation.
    pub disabled_formats: Vec<String>,
    /// UI language.
    pub language: Language,
    /// UI theme.
    pub theme: ThemeChoice,
    /// UI scale in percent (100 = normal).
    pub ui_scale: u32,
    /// What to do when the destination already exists.
    pub conflict_policy: ConflictPolicy,
    /// What to do with the source archive after a successful extraction.
    pub after_extract: AfterExtract,
    /// What to do when [`Self::after_extract`] is `Trash` but the archive
    /// cannot be moved to the trash.
    pub trash_fallback: TrashFallback,
    /// Show a desktop notification when a batch finishes.
    pub notify_on_done: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            decompress_mode: DecompressMode::Smart,
            passwords: Vec::new(),
            disabled_formats: Vec::new(),
            language: Language::System,
            theme: ThemeChoice::System,
            ui_scale: 100,
            conflict_policy: ConflictPolicy::Ask,
            after_extract: AfterExtract::Trash,
            trash_fallback: TrashFallback::Delete,
            notify_on_done: true,
        }
    }
}

impl Config {
    /// Absolute path of the config file.
    pub fn path() -> PathBuf {
        let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        base.join(APP_DIR).join("config.json")
    }

    /// Loads the config, falling back to defaults when the file is missing or
    /// unreadable. Never fails: a corrupt config should not brick the app.
    pub fn load() -> Self {
        Self::load_from(&Self::path())
    }

    /// Loads the config from an explicit path.
    pub fn load_from(path: &std::path::Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|err| {
                eprintln!("warning: invalid config at {}: {err}", path.display());
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    }

    /// Writes the config to disk, creating the parent directory if needed.
    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::path())
    }

    /// Writes the config to an explicit path.
    pub fn save_to(&self, path: &std::path::Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating config dir {}", parent.display()))?;
        }
        let raw = serde_json::to_string_pretty(self)?;
        std::fs::write(path, raw)
            .with_context(|| format!("writing config to {}", path.display()))?;
        Ok(())
    }

    /// True when the given format id is disabled by the user.
    pub fn is_format_disabled(&self, id: &str) -> bool {
        self.disabled_formats.iter().any(|disabled| disabled == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn round_trips_to_disk() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("ouch-gui-config-{nanos}"));
        let path = dir.join("config.json");

        let config = Config {
            conflict_policy: ConflictPolicy::Rename,
            after_extract: AfterExtract::Keep,
            trash_fallback: TrashFallback::Nothing,
            ui_scale: 150,
            passwords: vec!["secret".into()],
            ..Config::default()
        };

        config.save_to(&path).unwrap();
        let loaded = Config::load_from(&path);

        assert_eq!(loaded.conflict_policy, ConflictPolicy::Rename);
        assert_eq!(loaded.after_extract, AfterExtract::Keep);
        assert_eq!(loaded.trash_fallback, TrashFallback::Nothing);
        assert_eq!(loaded.ui_scale, 150);
        assert_eq!(loaded.passwords, vec!["secret".to_string()]);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
