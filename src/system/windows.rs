//! Windows desktop-integration helpers.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

/// Hide the console window that `cmd.exe` and `taskkill` would otherwise flash.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Opens a directory in File Explorer.
pub fn open_folder(path: &Path) {
    let _ = Command::new("explorer").arg(path).spawn();
}

/// Opens a URL in the user's default browser.
pub fn open_url(url: &str) {
    let _ = Command::new("cmd")
        .args(["/C", "start", "", url])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

/// Moves a file to the Recycle Bin.
pub fn trash_file(path: &Path) -> bool {
    trash::delete(path).is_ok()
}

/// Windows toast notifications need an AppUserModelID and a Start Menu
/// shortcut; the installer will own that later.
pub fn notify(_summary: &str, _body: &str) {}

/// Opens the native multi-file picker.
pub fn pick_files(pid_slot: &AtomicU32) -> Vec<PathBuf> {
    pid_slot.store(0, Ordering::Relaxed);
    rfd::FileDialog::new()
        .add_filter(
            "Archives",
            &[
                "tar", "gz", "tgz", "tbz", "tbz2", "txz", "tzst", "zip", "7z", "rar", "bz2", "xz",
                "zst", "lz4",
            ],
        )
        .pick_files()
        .unwrap_or_default()
}

/// Opens the native directory picker.
pub fn pick_directory(pid_slot: &AtomicU32) -> Option<PathBuf> {
    pid_slot.store(0, Ordering::Relaxed);
    rfd::FileDialog::new().pick_folder()
}

/// Terminates a process and its children with `taskkill`.
///
/// A zero/invalid PID and an already-dead process are ignored.
pub fn terminate_process(pid: u32) {
    if pid == 0 {
        return;
    }
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminate_process_ignores_invalid_pid() {
        terminate_process(0);
        terminate_process(u32::MAX);
    }
}
