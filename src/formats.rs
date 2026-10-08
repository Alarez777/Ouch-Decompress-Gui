//! Supported archive/compression formats and their extension/MIME mappings.
//!
//! The table mirrors what the bundled `ouch` binary supports. It is used for
//! three things:
//!   * detecting the format of an incoming file,
//!   * knowing whether a file is a real archive (tar/zip/7z/rar) or a single
//!     compressed file (gz/xz/zst/...),
//!   * building the file-association MIME list.

use std::path::Path;

/// Static description of a single format.
pub struct FormatInfo {
    /// Stable identifier, also used as the translation key suffix (`format.tar`).
    pub id: &'static str,
    /// Every extension that maps to this format, including aliases.
    pub extensions: &'static [&'static str],
    /// MIME types to associate for this format.
    pub mime_types: &'static [&'static str],
    /// True for container formats (tar/zip/7z/rar); false for single-stream
    /// compressions (gz/xz/zst/...).
    pub is_archive: bool,
}

/// Every format `ouch` can decompress, in presentation order.
pub const FORMATS: &[FormatInfo] = &[
    FormatInfo {
        id: "tar",
        extensions: &[
            "tar", "tgz", "tbz", "tbz2", "tbz3", "tlz4", "txz", "tlzma", "tsz", "tzst", "tlz",
            "cbt",
        ],
        mime_types: &["application/x-tar", "application/x-compressed-tar"],
        is_archive: true,
    },
    FormatInfo {
        id: "zip",
        extensions: &["zip", "cbz", "epub"],
        mime_types: &[
            "application/zip",
            "application/vnd.comicbook+zip",
            "application/epub+zip",
        ],
        is_archive: true,
    },
    FormatInfo {
        id: "7z",
        extensions: &["7z", "cb7"],
        mime_types: &["application/x-7z-compressed"],
        is_archive: true,
    },
    FormatInfo {
        id: "rar",
        extensions: &["rar", "cbr"],
        mime_types: &["application/vnd.rar", "application/x-rar"],
        is_archive: true,
    },
    FormatInfo {
        id: "gz",
        extensions: &["gz"],
        mime_types: &["application/gzip", "application/x-gzip"],
        is_archive: false,
    },
    FormatInfo {
        id: "bz",
        extensions: &["bz", "bz2"],
        mime_types: &["application/x-bzip2"],
        is_archive: false,
    },
    FormatInfo {
        id: "bz3",
        extensions: &["bz3"],
        mime_types: &["application/x-bzip3"],
        is_archive: false,
    },
    FormatInfo {
        id: "xz",
        extensions: &["xz"],
        mime_types: &["application/x-xz"],
        is_archive: false,
    },
    FormatInfo {
        id: "lzma",
        extensions: &["lzma"],
        mime_types: &["application/x-lzma"],
        is_archive: false,
    },
    FormatInfo {
        id: "lz",
        extensions: &["lz"],
        mime_types: &["application/x-lzip"],
        is_archive: false,
    },
    FormatInfo {
        id: "lz4",
        extensions: &["lz4"],
        mime_types: &["application/x-lz4"],
        is_archive: false,
    },
    FormatInfo {
        id: "sz",
        extensions: &["sz"],
        mime_types: &["application/x-snappy-framed"],
        is_archive: false,
    },
    FormatInfo {
        id: "zst",
        extensions: &["zst"],
        mime_types: &["application/zstd"],
        is_archive: false,
    },
    FormatInfo {
        id: "br",
        extensions: &["br"],
        mime_types: &["application/x-brotli"],
        is_archive: false,
    },
];

/// Returns the format a single, lowercased extension maps to.
pub fn format_for_extension(extension: &str) -> Option<&'static FormatInfo> {
    let extension = extension.trim_start_matches('.').to_ascii_lowercase();
    FORMATS
        .iter()
        .find(|format| format.extensions.contains(&extension.as_str()))
}

/// Removes a trailing numeric split extension (`a.7z.001` -> `a.7z`), which is
/// not a format but a volume number. Returns `None` when there is none.
fn strip_numeric_split(name: &str) -> Option<&str> {
    let (prefix, last) = name.rsplit_once('.')?;
    (!last.is_empty() && last.bytes().all(|byte| byte.is_ascii_digit())).then_some(prefix)
}

/// Walks the trailing extensions of a path and yields every recognized format,
/// from the innermost (leftmost) to the outermost (rightmost).
///
/// `archive.tar.gz` yields `[tar, gz]`; `photo.jpg` yields `[]`;
/// `def.7z.001` yields `[7z]` (the volume number is ignored).
pub fn formats_in_path(path: &Path) -> Vec<&'static FormatInfo> {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    let name = strip_numeric_split(name).unwrap_or(name);

    // Walk the extensions from right to left, collecting the contiguous known
    // tail. `a.tar.gz` -> [tar, gz]; `my.file.tar.gz` -> [tar, gz].
    let parts: Vec<&str> = name.split('.').skip(1).collect();
    let mut found = Vec::new();
    for part in parts.iter().rev() {
        match format_for_extension(part) {
            Some(format) => found.push(format),
            None => break,
        }
    }
    found.reverse();
    found
}

/// Returns the outermost recognized format of a path (e.g. `gz` for `a.tar.gz`).
pub fn outer_format(path: &Path) -> Option<&'static FormatInfo> {
    formats_in_path(path).last().copied()
}

/// Strips all trailing known extensions from a file name.
///
/// `archive.tar.gz` -> `archive`, `photo.tgz` -> `photo`, `data.zip` -> `data`.
/// This mirrors the folder name `ouch` derives in its default mode.
pub fn stem_without_known_extensions(path: &Path) -> String {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return String::new();
    };
    let strip = formats_in_path(path).len() + usize::from(strip_numeric_split(name).is_some());
    let parts: Vec<&str> = name.split('.').collect();
    if strip == 0 || parts.len() <= strip {
        return name.to_string();
    }
    parts[..parts.len() - strip].join(".")
}

/// True when the path contains an archive container anywhere in its extension
/// chain (`a.tar.gz` is an archive, `a.gz` is not).
pub fn is_archive_path(path: &Path) -> bool {
    formats_in_path(path).iter().any(|format| format.is_archive)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn detects_outer_extension() {
        assert_eq!(outer_format(&PathBuf::from("a.tar.gz")).unwrap().id, "gz");
        assert_eq!(outer_format(&PathBuf::from("a.zip")).unwrap().id, "zip");
        assert_eq!(outer_format(&PathBuf::from("noext")).map(|f| f.id), None);
    }

    #[test]
    fn detects_archives_through_compression() {
        assert!(is_archive_path(&PathBuf::from("a.tar.gz")));
        assert!(is_archive_path(&PathBuf::from("a.tgz")));
        assert!(is_archive_path(&PathBuf::from("a.zip")));
        assert!(!is_archive_path(&PathBuf::from("a.gz")));
        assert!(!is_archive_path(&PathBuf::from("a.txt")));
    }

    #[test]
    fn aliases_resolve_to_parent_format() {
        assert_eq!(format_for_extension("tgz").unwrap().id, "tar");
        assert_eq!(format_for_extension("CBZ").unwrap().id, "zip");
        assert_eq!(format_for_extension(".cb7").unwrap().id, "7z");
    }

    #[test]
    fn strips_known_extensions_for_folder_name() {
        assert_eq!(
            stem_without_known_extensions(&PathBuf::from("/t/a.tar.gz")),
            "a"
        );
        assert_eq!(stem_without_known_extensions(&PathBuf::from("a.tgz")), "a");
        assert_eq!(
            stem_without_known_extensions(&PathBuf::from("my.file.zip")),
            "my.file"
        );
        assert_eq!(
            stem_without_known_extensions(&PathBuf::from("plain.txt")),
            "plain.txt"
        );
    }

    #[test]
    fn ignores_numeric_split_extension() {
        assert_eq!(outer_format(&PathBuf::from("def.7z.001")).unwrap().id, "7z");
        assert!(is_archive_path(&PathBuf::from("def.7z.001")));
        assert_eq!(
            outer_format(&PathBuf::from("a.tar.gz.001")).unwrap().id,
            "gz"
        );
        assert_eq!(
            stem_without_known_extensions(&PathBuf::from("def.7z.001")),
            "def"
        );
        assert_eq!(
            stem_without_known_extensions(&PathBuf::from("a.tar.gz.001")),
            "a"
        );
    }
}
