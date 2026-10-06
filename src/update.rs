//! Best-effort update check against the project's GitHub releases.
//!
//! Uses `curl` (already present on most Linux desktops) so the app does not
//! need a full HTTP client dependency. The check runs on a background thread
//! and never blocks the UI.

use std::sync::mpsc::{self, Receiver};
use std::thread;

/// GitHub repository (`owner/name`).
pub const REPO: &str = "Alarez777/Ouch-Decompress-Gui";
/// Canonical project URL.
pub const REPO_URL: &str = "https://github.com/Alarez777/Ouch-Decompress-Gui";

/// Result of an update check.
#[derive(Debug, Clone)]
pub enum UpdateStatus {
    /// A newer release exists.
    Available { version: String, url: String },
    /// The running version is the latest.
    UpToDate,
    /// The check could not be completed (offline, curl missing, ...).
    Unknown,
}

/// Spawns a background update check.
pub fn check(current: &str, ctx: egui::Context) -> Receiver<UpdateStatus> {
    let (tx, rx) = mpsc::channel();
    let current = current.to_string();
    thread::spawn(move || {
        let status = query_latest(&current);
        let _ = tx.send(status);
        ctx.request_repaint();
    });
    rx
}

/// Queries the latest release and compares it with `current`.
fn query_latest(current: &str) -> UpdateStatus {
    let output = std::process::Command::new("curl")
        .args([
            "-sSL",
            "--max-time",
            "10",
            &format!("https://api.github.com/repos/{REPO}/releases/latest"),
        ])
        .output();

    let Ok(output) = output else {
        return UpdateStatus::Unknown;
    };
    if !output.status.success() {
        return UpdateStatus::Unknown;
    }

    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return UpdateStatus::Unknown;
    };

    // GitHub returns `{"message": "Not Found"}` when there are no releases.
    let Some(tag) = value.get("tag_name").and_then(|tag| tag.as_str()) else {
        return if value.get("message").is_some() {
            UpdateStatus::UpToDate
        } else {
            UpdateStatus::Unknown
        };
    };
    let url = value
        .get("html_url")
        .and_then(|url| url.as_str())
        .unwrap_or(REPO_URL)
        .to_string();

    if is_newer(tag, current) {
        UpdateStatus::Available {
            version: tag.to_string(),
            url,
        }
    } else {
        UpdateStatus::UpToDate
    }
}

/// Returns true when `latest` is a newer version than `current`.
fn is_newer(latest: &str, current: &str) -> bool {
    fn parse(text: &str) -> Vec<u64> {
        text.split(|c: char| !c.is_ascii_digit())
            .filter(|part| !part.is_empty())
            .filter_map(|part| part.parse().ok())
            .collect()
    }

    let latest = parse(latest);
    let current = parse(current);
    for index in 0..latest.len().max(current.len()) {
        let a = latest.get(index).copied().unwrap_or(0);
        let b = current.get(index).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("v0.0.9", "0.1.0"));
    }
}
