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

/// Terminates a process, first politely and then forcefully.
///
/// Sends `SIGTERM` and waits up to 500 ms for the process to exit; if it is
/// still running, sends `SIGKILL`. A zero/invalid PID and an already-dead
/// process are ignored.
pub fn terminate_process(pid: u32) {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return;
    };
    if pid <= 0 {
        return;
    }

    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    while std::time::Instant::now() < deadline {
        if process_finished(pid as u32) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }

    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
}

/// True when the process does not exist or has already terminated.
///
/// A child that has exited can linger as a zombie until the worker reaps it, so
/// the `/proc/<pid>/stat` state is checked as well as its existence.
fn process_finished(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return true;
    };
    // The state is the first field after the closing parenthesis of `comm`.
    match stat.rsplit_once(')') {
        Some((_, rest)) => rest.trim_start().starts_with('Z'),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminate_process_kills_a_running_child() {
        let mut child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        terminate_process(child.id());
        let status = child.wait().expect("wait for sleep");
        assert!(!status.success(), "sleep should have been killed");
    }

    #[test]
    fn terminate_process_ignores_invalid_pid() {
        terminate_process(0);
        terminate_process(u32::MAX);
    }
}
