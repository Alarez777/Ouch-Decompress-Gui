//! Small desktop-integration helpers implemented through standard tools, to
//! avoid pulling in heavy D-Bus/GTK dependencies.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Opens a directory in the user's file manager via `xdg-open`.
pub fn open_folder(path: &Path) {
    let _ = Command::new("xdg-open").arg(path).spawn();
}

/// Opens a URL in the user's default browser via `xdg-open`.
pub fn open_url(url: &str) {
    let _ = Command::new("xdg-open").arg(url).spawn();
}

/// Moves a file to the trash using `gio` or `trash-put`.
///
/// Returns false if neither tool is available, so callers can warn the user
/// instead of silently doing nothing.
pub fn trash_file(path: &Path) -> bool {
    let gio = Command::new("gio")
        .arg("trash")
        .arg("--")
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if gio {
        return true;
    }
    Command::new("trash-put")
        .arg("--")
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Shows a desktop notification via `notify-send`, if available.
pub fn notify(summary: &str, body: &str) {
    let _ = Command::new("notify-send")
        .arg("--app-name=Ouch Decompress")
        .arg(summary)
        .arg(body)
        .spawn();
}

/// Opens a native multi-file picker through `zenity`.
///
/// Returns an empty list when the user cancels or when `zenity` is not
/// installed, in which case no files are added.
pub fn pick_files() -> Vec<PathBuf> {
    let output = Command::new("zenity")
        .args([
            "--file-selection",
            "--multiple",
            "--separator=\n",
            "--title=Select archives",
            "--file-filter=Archives | *.tar *.tar.gz *.tgz *.tbz *.tbz2 *.txz *.tzst *.zip *.7z *.rar *.gz *.bz2 *.xz *.zst *.lz4",
        ])
        .output();

    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect()
}
