//! Decompression target strategies: always a folder, always "here", or a
//! smart decision based on the archive's root entries.

use serde::{Deserialize, Serialize};

/// User-configurable extraction strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecompressMode {
    /// Always unpack into a new folder named after the archive stem.
    Folder,
    /// Always unpack directly into the archive's parent directory.
    Here,
    /// Inspect the archive: multiple root entries -> folder, otherwise here.
    Smart,
}

impl DecompressMode {
    /// All variants in presentation order.
    pub const ALL: [DecompressMode; 3] = [
        DecompressMode::Folder,
        DecompressMode::Here,
        DecompressMode::Smart,
    ];

    /// Translation key for the label.
    pub fn label_key(&self) -> &'static str {
        match self {
            DecompressMode::Folder => "mode.folder",
            DecompressMode::Here => "mode.here",
            DecompressMode::Smart => "mode.smart",
        }
    }

    /// Translation key for the explanation shown under the option.
    pub fn hint_key(&self) -> &'static str {
        match self {
            DecompressMode::Folder => "mode.folder.hint",
            DecompressMode::Here => "mode.here.hint",
            DecompressMode::Smart => "mode.smart.hint",
        }
    }
}

/// Concrete decision for a single archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Pass no `--here`/`--dir`: `ouch` creates the stem-named folder.
    Folder,
    /// Pass `--here`: unpack into the archive's parent directory.
    Here,
}

impl Target {
    /// Short label used in status messages.
    pub fn label(self) -> &'static str {
        match self {
            Target::Folder => "folder",
            Target::Here => "here",
        }
    }
}

/// Extracts the unique root entries from the output of
/// `ouch list -A -q <archive>`.
///
/// `ouch list` prints one entry per line, directories with a trailing slash.
/// The header line (`Archive: ...`) is suppressed by `-q` but we skip it
/// defensively. ANSI escape sequences are stripped as well.
pub fn roots_from_listing(output: &str) -> Vec<String> {
    let mut roots: Vec<String> = Vec::new();
    for raw in output.lines() {
        let stripped = strip_ansi(raw);
        let line = stripped.trim();
        if line.is_empty() || line.starts_with("Archive:") {
            continue;
        }
        let line = line.trim_end_matches('/');
        let root = line.split('/').next().unwrap_or(line);
        if root.is_empty() {
            continue;
        }
        let root = root.to_string();
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
}

/// Decides the concrete target for one archive.
///
/// * Non-archive single-file formats (`gz`, `xz`, ...) always extract here.
/// * In smart mode, more than one root entry means a folder is warranted.
pub fn choose_target(mode: DecompressMode, is_archive: bool, roots: &[String]) -> Target {
    if !is_archive {
        return Target::Here;
    }
    match mode {
        DecompressMode::Folder => Target::Folder,
        DecompressMode::Here => Target::Here,
        DecompressMode::Smart => {
            if roots.len() > 1 {
                Target::Folder
            } else {
                Target::Here
            }
        }
    }
}

/// Removes ANSI CSI escape sequences (e.g. color codes) from a string.
fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                // Consume until a letter terminates the sequence.
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_collapse_common_prefix() {
        let listing = "libpng/1.6.58/\nlibpng/1.6.58/README\nlibpng/1.6.58/bin/\n";
        assert_eq!(roots_from_listing(listing), vec!["libpng"]);
    }

    #[test]
    fn roots_detect_multiple_entries() {
        let listing = "a.bin\nfolder/b\nfolder/c\n";
        let roots = roots_from_listing(listing);
        assert_eq!(roots, vec!["a.bin", "folder"]);
    }

    #[test]
    fn roots_skip_header_and_colors() {
        let listing = "Archive: x.zip\n\u{1b}[34mfoo\u{1b}[0m/\nbar\n";
        assert_eq!(roots_from_listing(listing), vec!["foo", "bar"]);
    }

    #[test]
    fn smart_mode_picks_target() {
        assert_eq!(
            choose_target(DecompressMode::Smart, true, &["only".into()]),
            Target::Here
        );
        assert_eq!(
            choose_target(DecompressMode::Smart, true, &["a".into(), "b".into()]),
            Target::Folder
        );
        assert_eq!(
            choose_target(DecompressMode::Smart, false, &["a".into(), "b".into()]),
            Target::Here
        );
        assert_eq!(
            choose_target(DecompressMode::Folder, false, &[]),
            Target::Here
        );
    }
}
