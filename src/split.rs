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
}
