//! Thin wrapper around the `ouch` command-line tool.
//!
//! `ouch` is a binary-only crate ("ouch is not a library"), so every
//! interaction happens through `std::process::Command`. All parsing of ouch's
//! textual output is confined to this module.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::modes::Target;

/// Result of running `ouch decompress`.
#[derive(Debug, Clone)]
pub struct DecompressOutcome {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Located `ouch` binary plus its parsed version.
#[derive(Debug, Clone)]
pub struct OuchClient {
    path: PathBuf,
    version: String,
}

impl OuchClient {
    /// Finds an `ouch` binary to use, preferring one bundled next to the app
    /// (as installed in an AppImage) over one found on `PATH`.
    ///
    /// A candidate is only accepted if it actually runs `--version`
    /// successfully, so a broken/corrupted bundled binary falls back to a
    /// working system one instead of crashing at runtime.
    pub fn discover() -> Result<Self> {
        for candidate in Self::candidates() {
            if !is_executable(&candidate) {
                continue;
            }
            if let Ok(version) = read_version(&candidate) {
                return Ok(Self {
                    path: candidate,
                    version,
                });
            }
        }
        bail!(
            "could not find a working `ouch` binary. It is normally bundled with \
             the AppImage; otherwise install it with your package manager or \
             `cargo install ouch`."
        )
    }

    /// Ordered list of locations to search for `ouch`.
    fn candidates() -> Vec<PathBuf> {
        let mut candidates = Vec::new();

        // 1. Bundled inside the AppImage: $APPDIR/usr/bin/ouch
        if let Ok(appdir) = std::env::var("APPDIR") {
            candidates.push(PathBuf::from(appdir).join("usr/bin/ouch"));
        }

        // 2. Next to the running executable / in the same tree.
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join("ouch"));
                // Typical `target/release` -> repo root layout during dev.
                if let Some(parent) = dir.parent() {
                    candidates.push(parent.join("ouch"));
                }
            }
        }

        // 3. Directories on PATH.
        if let Some(path_var) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path_var) {
                candidates.push(dir.join("ouch"));
            }
        }

        candidates
    }

    /// Path of the resolved binary.
    #[allow(dead_code)] // handy for diagnostics and future "open in terminal"
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Parsed `ouch` version (e.g. `0.8.3`), or `unknown`.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Runs `ouch list -A -q` and returns the raw listing.
    ///
    /// `-A` disables colors and `-q` suppresses the `Archive:` header, which
    /// makes the output easy to parse. Fails for non-archive formats and for
    /// encrypted archives when the password is wrong or missing.
    pub fn list(&self, archive: &Path, password: Option<&str>) -> Result<String> {
        let mut cmd = self.command();
        cmd.arg("list").arg("-A").arg("-q");
        if let Some(password) = password {
            cmd.arg("-p").arg(password);
        }
        cmd.arg(archive);

        let output = cmd
            .output()
            .with_context(|| format!("running `ouch list` for {}", archive.display()))?;
        if !output.status.success() {
            bail!(
                "ouch list failed: {}",
                stderr_or_stdout(&output.stderr, &output.stdout)
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Runs `ouch decompress` for a single archive.
    ///
    /// The working directory is set to the archive's parent so that ouch's
    /// native "stem folder" and `--here` behaviors land next to the archive
    /// regardless of where the app was launched from.
    pub fn decompress(
        &self,
        archive: &Path,
        target: Target,
        password: Option<&str>,
    ) -> Result<DecompressOutcome> {
        let workdir = archive
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        let mut cmd = self.command();
        cmd.current_dir(&workdir);
        cmd.arg("decompress").arg("-y");
        if target == Target::Here {
            cmd.arg("--here");
        }
        if let Some(password) = password {
            cmd.arg("-p").arg(password);
        }
        cmd.arg(archive);

        let output = cmd
            .output()
            .with_context(|| format!("running `ouch decompress` for {}", archive.display()))?;

        Ok(DecompressOutcome {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    /// Base command with stdin detached so ouch never blocks on a prompt.
    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.path);
        cmd.stdin(Stdio::null());
        cmd
    }
}

/// True when the path exists and is an executable file.
fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Runs `ouch --version` and returns the version token (e.g. `0.8.3`).
fn read_version(binary: &Path) -> Result<String> {
    let output = Command::new(binary)
        .arg("--version")
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success() {
        bail!("ouch --version failed");
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let version = text.split_whitespace().last().unwrap_or("unknown").trim();
    Ok(version.to_string())
}

/// Picks the stderr text, falling back to stdout when stderr is empty.
fn stderr_or_stdout(stderr: &[u8], stdout: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    if !stderr.trim().is_empty() {
        return stderr.trim().to_string();
    }
    String::from_utf8_lossy(stdout).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modes::{choose_target, roots_from_listing, DecompressMode, Target};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Returns a unique temporary directory for a test.
    fn unique_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "ouch-gui-test-{}-{tag}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Runs `ouch compress` to build a fixture archive. Panics on failure.
    fn build_archive(ouch: &OuchClient, cwd: &Path, inputs: &[&str], output: &str) {
        let status = Command::new(ouch.path())
            .current_dir(cwd)
            .arg("compress")
            .arg("-y")
            .args(inputs)
            .arg(output)
            .stdin(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "ouch compress failed for {output}");
    }

    #[test]
    fn end_to_end_list_and_extract() {
        let Ok(ouch) = OuchClient::discover() else {
            eprintln!("skipping: ouch binary not found");
            return;
        };

        let base = unique_dir("e2e");

        // --- Multi-root archive: smart mode should create a folder. ---
        let multi_dir = base.join("multi");
        std::fs::create_dir_all(multi_dir.join("pack/sub")).unwrap();
        std::fs::write(multi_dir.join("pack/a.txt"), b"a").unwrap();
        std::fs::write(multi_dir.join("pack/sub/b.txt"), b"b").unwrap();
        build_archive(&ouch, &multi_dir, &["pack/a.txt", "pack/sub"], "multi.zip");

        let multi = multi_dir.join("multi.zip");
        let listing = ouch.list(&multi, None).unwrap();
        let roots = roots_from_listing(&listing);
        assert!(roots.len() > 1, "expected multiple roots, got {roots:?}");
        assert_eq!(
            choose_target(DecompressMode::Smart, true, &roots),
            Target::Folder
        );

        let outcome = ouch.decompress(&multi, Target::Folder, None).unwrap();
        assert!(outcome.success, "decompress failed: {}", outcome.stderr);
        assert!(multi_dir.join("multi/a.txt").exists());
        assert!(multi_dir.join("multi/sub/b.txt").exists());

        // --- Single-root archive: smart mode should extract here. ---
        let single_dir = base.join("single");
        std::fs::create_dir_all(single_dir.join("source/one")).unwrap();
        std::fs::write(single_dir.join("source/one/file.txt"), b"data").unwrap();
        build_archive(&ouch, &single_dir, &["source"], "single.zip");

        let single = single_dir.join("single.zip");
        let listing = ouch.list(&single, None).unwrap();
        let roots = roots_from_listing(&listing);
        assert_eq!(roots.len(), 1, "expected a single root, got {roots:?}");
        assert_eq!(
            choose_target(DecompressMode::Smart, true, &roots),
            Target::Here
        );

        let outcome = ouch.decompress(&single, Target::Here, None).unwrap();
        assert!(outcome.success, "decompress failed: {}", outcome.stderr);
        assert!(single_dir.join("source/one/file.txt").exists());

        let _ = std::fs::remove_dir_all(&base);
    }
}
