//! Multi-volume (split) archive helper.
//!
//! Two naming schemes are recognized:
//!
//! * **RAR volumes**: `name.part1.rar`, `name.part2.rar`, ... — `unrar` reads
//!   the whole set when given the first part.
//! * **Numeric splits**: `name.001`, `name.002`, ... (made by 7-Zip's `-v` or
//!   Unix `split`) — a raw byte split that has to be concatenated before `ouch`
//!   can read it (see [`crate::multivolume`]).
//!
//! Anything else is a single file (`SplitKind::None`).

use std::path::{Path, PathBuf};

/// How a set of files must be handed to `ouch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitKind {
    /// Not a split.
    None,
    /// RAR volumes (`name.partN.rar`); the first part can be read directly.
    RarParts,
    /// A raw split (`name.NNN`) that must be concatenated first.
    Concat,
}

/// A parsed split-part file name.
struct PartName<'a> {
    kind: SplitKind,
    /// Base name before the volume marker: `name` for RAR, `name.ext` for a
    /// numeric split.
    prefix: &'a str,
    number: u64,
    /// Zero-padding width of the number as written.
    width: usize,
    /// RAR only: the `part` label (e.g. `part`).
    label: &'a str,
    /// RAR only: the file extension (e.g. `rar`).
    extension: &'a str,
}

/// Parses a split-part file name, if it is one.
fn parse_part_name(name: &str) -> Option<PartName<'_>> {
    let (prefix, last) = name.rsplit_once('.')?;

    // Numeric split: `name.ext.001` (or `name.001`).
    if !last.is_empty() && last.bytes().all(|byte| byte.is_ascii_digit()) {
        return Some(PartName {
            kind: SplitKind::Concat,
            prefix,
            number: last.parse().ok()?,
            width: last.len(),
            label: "",
            extension: "",
        });
    }

    // RAR volume: `name.partN.rar`.
    if last.eq_ignore_ascii_case("rar") {
        let (rar_prefix, part) = prefix.rsplit_once('.')?;
        let digits = part.to_ascii_lowercase();
        let digits = digits.strip_prefix("part")?;
        if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Some(PartName {
                kind: SplitKind::RarParts,
                prefix: rar_prefix,
                number: digits.parse().ok()?,
                width: digits.len(),
                label: &part[..part.len() - digits.len()],
                extension: last,
            });
        }
    }

    None
}

/// The kind of split `path` is part of.
pub fn kind(path: &Path) -> SplitKind {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(parse_part_name)
        .map(|part| part.kind)
        .unwrap_or(SplitKind::None)
}

/// Every part of the split `path` belongs to, first volume first. A path that
/// is not a split part yields just `[path]`.
pub fn archive_parts(path: &Path) -> Vec<PathBuf> {
    let single = || vec![path.to_path_buf()];

    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return single();
    };
    let Some(part) = parse_part_name(name) else {
        return single();
    };
    let parent = path.parent().filter(|dir| !dir.as_os_str().is_empty());
    let dir = parent.unwrap_or_else(|| Path::new("."));

    let Ok(entries) = std::fs::read_dir(dir) else {
        return single();
    };
    let mut parts: Vec<(u64, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        if let Some(candidate) = parse_part_name(file_name) {
            if candidate.kind == part.kind && candidate.prefix.eq_ignore_ascii_case(part.prefix) {
                parts.push((candidate.number, entry.path()));
            }
        }
    }
    if parts.is_empty() {
        return single();
    }
    parts.sort_by_key(|(number, _)| *number);
    parts.into_iter().map(|(_, path)| path).collect()
}

/// A split set that is missing some of its volumes.
pub struct MissingParts {
    /// The missing file names, matching the present parts' naming.
    pub names: Vec<String>,
    /// Total number of volumes, when the last present one is provably final.
    pub total: Option<u64>,
    /// True when further volumes are known to be missing after the named ones.
    pub more: bool,
}

/// Reports the missing volumes of a split set, if any.
///
/// Volumes are numbered from 1 and expected to be contiguous. Returns `None`
/// when nothing is provably missing.
pub fn missing_parts(parts: &[PathBuf]) -> Option<MissingParts> {
    let mut numbered: Vec<(u64, &PathBuf)> = parts
        .iter()
        .filter_map(|path| part_number(path).map(|number| (number, path)))
        .collect();
    numbered.sort_by_key(|(number, _)| *number);
    numbered.dedup_by_key(|(number, _)| *number);

    let numbers: Vec<u64> = numbered.iter().map(|(number, _)| *number).collect();
    let first = *numbers.first()?;
    let last = *numbers.last()?;

    let mut missing: Vec<u64> = (1..first).collect();
    for pair in numbers.windows(2) {
        missing.extend((pair[0] + 1)..pair[1]);
    }

    // A lone RAR `part1` always has more volumes; a lone numeric `.001` may be
    // a one-part split, so it is not flagged. With two or more parts, the last
    // one is final only when it is smaller than the previous (all volumes but
    // the last share a size).
    let rar = numbered
        .first()
        .is_some_and(|(_, path)| kind_of(path.as_path()) == SplitKind::RarParts);
    let (more, total) = match numbered.as_slice() {
        [(1, _)] if rar => (true, None),
        [_] => (false, None),
        many => match (
            size_of(many[many.len() - 1].1),
            size_of(many[many.len() - 2].1),
        ) {
            (Some(last_size), Some(previous_size)) if last_size < previous_size => {
                (false, Some(last))
            }
            (Some(_), Some(_)) => (true, None),
            _ => (false, None),
        },
    };
    if more {
        missing.push(last + 1);
    }
    if missing.is_empty() {
        return None;
    }

    let template = numbered.first().map(|(_, path)| *path)?;
    let names = missing
        .iter()
        .filter_map(|number| format_part_name(template, *number))
        .collect();

    Some(MissingParts { names, total, more })
}

/// The kind of split `path` is part of, from its file name alone.
fn kind_of(path: &Path) -> SplitKind {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(parse_part_name)
        .map(|part| part.kind)
        .unwrap_or(SplitKind::None)
}

/// The volume number of `path` when it is a split part.
fn part_number(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_str()?;
    parse_part_name(name).map(|part| part.number)
}

/// Size of a file, or `None` when it cannot be read.
fn size_of(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|metadata| metadata.len())
}

/// Rebuilds the file name of a missing volume from an existing part, keeping
/// its prefix, label, casing and digit width (`a.PART02.RAR` -> `a.PART01.RAR`,
/// `b.7z.002` -> `b.7z.001`).
fn format_part_name(template: &Path, number: u64) -> Option<String> {
    let name = template.file_name()?.to_str()?;
    let part = parse_part_name(name)?;
    let width = part.width;
    Some(match part.kind {
        SplitKind::RarParts => format!(
            "{}.{}{:0width$}.{}",
            part.prefix, part.label, number, part.extension
        ),
        SplitKind::Concat => format!("{}.{:0width$}", part.prefix, number),
        SplitKind::None => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("ouch-split-{}-{tag}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_part(dir: &Path, name: &str, size: usize) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, vec![0u8; size]).unwrap();
        path
    }

    #[test]
    fn groups_rar_parts_in_order() {
        let dir = unique_dir("parts");
        for number in 1..=3 {
            std::fs::write(dir.join(format!("movie.part{number}.rar")), b"x").unwrap();
        }

        let parts = archive_parts(&dir.join("movie.part2.rar"));
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].file_name().unwrap(), "movie.part1.rar");
        assert_eq!(parts[2].file_name().unwrap(), "movie.part3.rar");
        assert_eq!(kind(&dir.join("movie.part2.rar")), SplitKind::RarParts);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn groups_numeric_parts_in_order() {
        let dir = unique_dir("numeric");
        for number in 1..=3 {
            std::fs::write(dir.join(format!("def.7z.{number:03}")), b"x").unwrap();
        }

        let parts = archive_parts(&dir.join("def.7z.002"));
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].file_name().unwrap(), "def.7z.001");
        assert_eq!(parts[2].file_name().unwrap(), "def.7z.003");
        assert_eq!(kind(&dir.join("def.7z.002")), SplitKind::Concat);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_multipart_is_a_single_path() {
        let dir = unique_dir("single");
        let path = dir.join("archive.zip");
        std::fs::write(&path, b"x").unwrap();
        assert_eq!(archive_parts(&path), vec![path.clone()]);
        assert_eq!(kind(&path), SplitKind::None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn matches_case_insensitively_and_ignores_other_groups() {
        let dir = unique_dir("case");
        std::fs::write(dir.join("A.PART01.RAR"), b"x").unwrap();
        std::fs::write(dir.join("A.PART02.RAR"), b"x").unwrap();
        std::fs::write(dir.join("B.part1.rar"), b"x").unwrap();

        let parts = archive_parts(&dir.join("a.part1.rar"));
        assert_eq!(parts.len(), 2);
        assert!(parts.iter().all(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .to_ascii_lowercase()
                .starts_with("a.part")
        }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reports_leading_missing_parts() {
        let dir = unique_dir("missing-leading");
        let parts = vec![
            write_part(&dir, "s.part2.rar", 100),
            write_part(&dir, "s.part3.rar", 40),
        ];
        let missing = missing_parts(&parts).unwrap();
        assert_eq!(missing.names, vec!["s.part1.rar".to_string()]);
        assert_eq!(missing.total, Some(3));
        assert!(!missing.more);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reports_gaps_between_parts() {
        let dir = unique_dir("missing-gap");
        let parts = vec![
            write_part(&dir, "s.part1.rar", 100),
            write_part(&dir, "s.part3.rar", 40),
        ];
        let missing = missing_parts(&parts).unwrap();
        assert_eq!(missing.names, vec!["s.part2.rar".to_string()]);
        assert_eq!(missing.total, Some(3));
        assert!(!missing.more);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reports_missing_after_a_lone_first_part() {
        let dir = unique_dir("missing-trailing-lone");
        let parts = vec![write_part(&dir, "s.part1.rar", 100)];
        let missing = missing_parts(&parts).unwrap();
        assert_eq!(missing.names, vec!["s.part2.rar".to_string()]);
        assert_eq!(missing.total, None);
        assert!(missing.more);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reports_missing_after_a_full_last_part() {
        let dir = unique_dir("missing-trailing");
        let parts = vec![
            write_part(&dir, "s.part1.rar", 100),
            write_part(&dir, "s.part2.rar", 100),
        ];
        let missing = missing_parts(&parts).unwrap();
        assert_eq!(missing.names, vec!["s.part3.rar".to_string()]);
        assert_eq!(missing.total, None);
        assert!(missing.more);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lone_numeric_first_part_is_not_flagged() {
        let dir = unique_dir("numeric-lone");
        let parts = vec![write_part(&dir, "def.7z.001", 100)];
        assert!(missing_parts(&parts).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reports_leading_missing_numeric_parts() {
        let dir = unique_dir("numeric-leading");
        let parts = vec![write_part(&dir, "def.7z.003", 40)];
        let missing = missing_parts(&parts).unwrap();
        assert_eq!(
            missing.names,
            vec!["def.7z.001".to_string(), "def.7z.002".to_string()]
        );
        assert!(!missing.more);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn complete_set_reports_nothing() {
        let dir = unique_dir("missing-none");
        let parts = vec![
            write_part(&dir, "s.part1.rar", 100),
            write_part(&dir, "s.part2.rar", 40),
        ];
        assert!(missing_parts(&parts).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keeps_digit_width_and_casing() {
        let dir = unique_dir("missing-width");
        let parts = vec![write_part(&dir, "s.PART02.RAR", 100)];
        let missing = missing_parts(&parts).unwrap();
        assert_eq!(missing.names, vec!["s.PART01.RAR".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
