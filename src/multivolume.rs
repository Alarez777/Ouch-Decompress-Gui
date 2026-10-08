//! Multi-volume WORKAROUND.
//!
//! `ouch`/`sevenz-rust2` cannot read raw split archives (`name.7z.001`, ...):
//! they only accept a single `Read + Seek` file. This module materializes the
//! volumes into one temporary file so `ouch` can read it.
//!
//! REPLACEABLE: when `ouch` gains native multi-volume support (or the app reads
//! the volumes with a `Read + Seek` reader), delete this module and make
//! [`prepare`] simply return the original first part. Nothing else depends on
//! the concatenation.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::split::SplitKind;

/// Keep this much RAM free before putting the temporary in `/dev/shm`.
const RAM_MARGIN: u64 = 2 * 1024 * 1024 * 1024;

/// A source ready to be handed to `ouch`, plus the temporary it may own.
pub struct Prepared {
    path: PathBuf,
    /// Removes the temporary directory on drop; `None` when nothing was copied.
    _temp: Option<TempDir>,
}

impl Prepared {
    /// The file `ouch` must read.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// A temporary directory that deletes itself on drop.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Returns the file `ouch` should read for `archive`.
///
/// For anything but a raw numeric split this is `archive` itself. For a split
/// the volumes are concatenated into a temporary file (in RAM when it fits and
/// there is enough free memory, next to the archive otherwise). `progress` is
/// called with `(copied_bytes, total_bytes)` while copying, and `Ok(None)` is
/// returned when `cancel` is set mid-copy.
pub fn prepare(
    archive: &Path,
    parts: &[PathBuf],
    kind: SplitKind,
    cancel: &AtomicBool,
    progress: impl FnMut(u64, u64),
) -> Result<Option<Prepared>> {
    if kind != SplitKind::Concat || parts.is_empty() {
        return Ok(Some(Prepared {
            path: archive.to_path_buf(),
            _temp: None,
        }));
    }

    let total: u64 = parts.iter().map(|part| file_size(part)).sum();
    let temp_dir = choose_temp_dir(archive, total);
    std::fs::create_dir_all(&temp_dir)
        .with_context(|| format!("creating {}", temp_dir.display()))?;
    // The guard removes the temporary if we return early (cancelled).
    let temp = TempDir(temp_dir.clone());
    let temp_path = temp_dir.join(base_name(archive));

    let done = copy_parts(parts, &temp_path, total, cancel, progress)
        .with_context(|| format!("concatenating {} parts", parts.len()))?;
    if !done {
        return Ok(None);
    }

    Ok(Some(Prepared {
        path: temp_path,
        _temp: Some(temp),
    }))
}

/// The base name of the split without its volume number (`def.7z.001` ->
/// `def.7z`), so `ouch` names its output folder after the archive.
fn base_name(archive: &Path) -> String {
    let name = archive
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "archive".to_string());
    match name.rsplit_once('.') {
        Some((prefix, last)) if !last.is_empty() && last.bytes().all(|b| b.is_ascii_digit()) => {
            prefix.to_string()
        }
        _ => name,
    }
}

/// Picks the directory for the temporary: `/dev/shm` (RAM) when the whole set
/// fits with a healthy margin, otherwise a hidden folder next to the archive.
fn choose_temp_dir(archive: &Path, total: u64) -> PathBuf {
    let name = format!("ouch-{}-{}", std::process::id(), unique_nanos());

    let shm = Path::new("/dev/shm");
    if shm.is_dir() && is_writable(shm) {
        let fits_disk = available_space(shm).is_some_and(|free| free >= total);
        let fits_ram = mem_available().is_some_and(|free| free >= total.saturating_add(RAM_MARGIN));
        if fits_disk && fits_ram {
            return shm.join(name);
        }
    }

    let parent = archive
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    parent.join(format!(".{name}"))
}

/// Copies every part, in order, into `dest`, reporting progress. Returns
/// `false` when `cancel` was requested (the caller removes the temporary).
fn copy_parts(
    parts: &[PathBuf],
    dest: &Path,
    total: u64,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<bool> {
    const BUFFER: usize = 1 << 20;

    let file = File::create(dest).with_context(|| format!("creating {}", dest.display()))?;
    let mut writer = BufWriter::with_capacity(BUFFER, file);
    let mut buffer = vec![0u8; BUFFER];
    let mut copied = 0u64;

    for part in parts {
        let file = File::open(part).with_context(|| format!("opening {}", part.display()))?;
        let mut reader = BufReader::with_capacity(BUFFER, file);
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Ok(false);
            }
            let read = reader
                .read(&mut buffer)
                .with_context(|| format!("reading {}", part.display()))?;
            if read == 0 {
                break;
            }
            writer
                .write_all(&buffer[..read])
                .with_context(|| format!("writing {}", dest.display()))?;
            copied += read as u64;
            progress(copied, total);
        }
    }

    writer
        .flush()
        .with_context(|| format!("flushing {}", dest.display()))?;
    Ok(true)
}

/// Size of a file, or 0 when it cannot be read.
fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

/// Monotonic-ish unique suffix for temporary names.
fn unique_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0)
}

/// True when `path` exists and is writable by this user.
fn is_writable(path: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 }
}

/// Free bytes on the filesystem containing `path`.
fn available_space(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::statvfs(path.as_ptr(), &mut stat) } == 0;
    ok.then(|| stat.f_bavail as u64 * stat.f_frsize as u64)
}

/// `MemAvailable` from `/proc/meminfo`, in bytes.
fn mem_available() -> Option<u64> {
    let contents = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("MemAvailable:") {
            let kib: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kib * 1024);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_name_strips_volume_number() {
        assert_eq!(base_name(Path::new("/t/def.7z.001")), "def.7z");
        assert_eq!(base_name(Path::new("/t/x.01")), "x");
        assert_eq!(base_name(Path::new("/t/noext")), "noext");
    }

    #[test]
    fn non_split_returns_the_archive_untouched() {
        let archive = Path::new("/t/photo.jpg");
        let prepared = prepare(
            archive,
            &[archive.to_path_buf()],
            SplitKind::None,
            &AtomicBool::new(false),
            |_, _| {},
        )
        .unwrap()
        .unwrap();
        assert_eq!(prepared.path(), archive);
    }

    #[test]
    fn concatenates_parts_in_order() {
        let dir = std::env::temp_dir().join(format!(
            "ouch-multivolume-{}-{}",
            std::process::id(),
            unique_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let first = dir.join("a.001");
        let second = dir.join("a.002");
        std::fs::write(&first, b"hello ").unwrap();
        std::fs::write(&second, b"world").unwrap();

        let mut last = (0u64, 0u64);
        let prepared = prepare(
            &first,
            &[first.clone(), second.clone()],
            SplitKind::Concat,
            &AtomicBool::new(false),
            |c, t| {
                last = (c, t);
            },
        )
        .unwrap()
        .unwrap();

        assert_eq!(std::fs::read(prepared.path()).unwrap(), b"hello world");
        assert_eq!(last, (11, 11));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancelled_before_copying_returns_none() {
        let dir = std::env::temp_dir().join(format!(
            "ouch-multivolume-cancel-{}-{}",
            std::process::id(),
            unique_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let first = dir.join("a.001");
        let second = dir.join("a.002");
        std::fs::write(&first, b"hello ").unwrap();
        std::fs::write(&second, b"world").unwrap();

        let prepared = prepare(
            &first,
            &[first.clone(), second.clone()],
            SplitKind::Concat,
            &AtomicBool::new(true),
            |_, _| {},
        )
        .unwrap();

        assert!(prepared.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
