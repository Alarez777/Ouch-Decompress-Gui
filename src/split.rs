//! Multi-volume RAR helper.
//!
//! Only RAR multi-volume sets are detected (`name.part1.rar`, `name.part2.rar`,
//! ...); `ouch`/`unrar` extracts the whole set when given the first volume.

use std::path::{Path, PathBuf};

/// Every part of the multi-volume RAR set `path` belongs to, first volume
/// first. A path that is not a multi-volume part yields just `[path]`.
pub fn archive_parts(path: &Path) -> Vec<PathBuf> {
    let single = || vec![path.to_path_buf()];

    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return single();
    };
    let Some(prefix) = rar_part_prefix(name) else {
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
        if let Some((child_prefix, number)) = rar_part_info(file_name) {
            if child_prefix.eq_ignore_ascii_case(prefix) {
                parts.push((number, entry.path()));
            }
        }
    }
    if parts.is_empty() {
        return single();
    }
    parts.sort_by_key(|(number, _)| *number);
    parts.into_iter().map(|(_, path)| path).collect()
}

/// The prefix (archive base name) when `name` looks like `base.partN.rar`.
fn rar_part_prefix(name: &str) -> Option<&str> {
    rar_part_info(name).map(|(prefix, _)| prefix)
}

/// Splits `base.partN.rar` into its prefix and the numeric part.
fn rar_part_info(name: &str) -> Option<(&str, u64)> {
    let (stem, extension) = name.rsplit_once('.')?;
    if !extension.eq_ignore_ascii_case("rar") {
        return None;
    }
    let (prefix, part) = stem.rsplit_once('.')?;
    let digits = part.to_ascii_lowercase();
    let digits = digits.strip_prefix("part")?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((prefix, digits.parse().ok()?))
}

/// A multi-volume set that is missing some of its volumes.
pub struct MissingParts {
    /// The missing file names, matching the present parts' naming.
    pub names: Vec<String>,
    /// Total number of volumes, when the last present one is provably the
    /// final volume.
    pub total: Option<u64>,
    /// True when further volumes are known to be missing after the named ones.
    pub more: bool,
}

/// Reports the missing volumes of a multi-volume set, if any.
///
/// Volumes are numbered from 1 and expected to be contiguous, so a set whose
/// first volumes are gone, with gaps, or whose last present volume is not the
/// final one is incomplete. Returns `None` when nothing is provably missing.
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

    // Every volume but the last shares a size, so a last part smaller than the
    // previous one is the final volume. A lone `part1` always has more volumes
    // (a single file is named `name.rar`, not `name.part1.rar`).
    let (more, total) = match numbered.as_slice() {
        [(1, _)] => (true, None),
        [_] => (false, None),
        many => {
            let last_size = size_of(many[many.len() - 1].1);
            let previous_size = size_of(many[many.len() - 2].1);
            match (last_size, previous_size) {
                (Some(last_size), Some(previous_size)) if last_size < previous_size => {
                    (false, Some(last))
                }
                (Some(_), Some(_)) => (true, None),
                _ => (false, None),
            }
        }
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

/// The volume number of `path` when it is a `base.partN.rar` part.
fn part_number(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_str()?;
    rar_part_info(name).map(|(_, number)| number)
}

/// Size of a file, or `None` when it cannot be read.
fn size_of(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|metadata| metadata.len())
}

/// Rebuilds the file name of a missing volume from an existing part, keeping
/// its prefix, label, casing and digit width (`a.PART02.RAR` -> `a.PART01.RAR`).
fn format_part_name(template: &Path, number: u64) -> Option<String> {
    let name = template.file_name()?.to_str()?;
    let (stem, extension) = name.rsplit_once('.')?;
    let (prefix, part) = stem.rsplit_once('.')?;
    let digit_count = part.chars().rev().take_while(char::is_ascii_digit).count();
    if digit_count == 0 {
        return None;
    }
    let label = &part[..part.len() - digit_count];
    let width = digit_count;
    Some(format!("{prefix}.{label}{number:0width$}.{extension}"))
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

    #[test]
    fn groups_rar_parts_in_order() {
        let dir = unique_dir("parts");
        for number in 1..=3 {
            std::fs::write(dir.join(format!("movie.part{number}.rar")), b"x").unwrap();
        }

        let parts = archive_parts(&dir.join("movie.part2.rar"));
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].file_name().unwrap(), "movie.part1.rar");
        assert_eq!(parts[1].file_name().unwrap(), "movie.part2.rar");
        assert_eq!(parts[2].file_name().unwrap(), "movie.part3.rar");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_multipart_is_a_single_path() {
        let dir = unique_dir("single");
        let path = dir.join("archive.zip");
        std::fs::write(&path, b"x").unwrap();
        assert_eq!(archive_parts(&path), vec![path.clone()]);
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

    fn write_part(dir: &Path, name: &str, size: usize) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, vec![0u8; size]).unwrap();
        path
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
