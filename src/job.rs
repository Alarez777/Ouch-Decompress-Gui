//! Background batch extraction.
//!
//! A single worker thread processes archives sequentially so that a password
//! entered for the first archive can be reused for the rest of the batch.
//! Communication with the UI uses channels; the worker never touches egui
//! state directly.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use crate::config::{AfterExtract, Config, ConflictPolicy, TrashFallback};
use crate::formats;
use crate::modes::{self, DecompressMode, Target};
use crate::ouch::OuchClient;

/// Events sent from the worker to the UI.
#[derive(Debug, Clone)]
pub enum JobEvent {
    Started {
        index: usize,
    },
    Log {
        index: usize,
        line: String,
    },
    /// The worker is blocked waiting for a password. The UI must answer via
    /// the controller's channel. `error` is only set after a submitted password
    /// failed, so the first prompt stays clean.
    NeedPassword {
        index: usize,
        archive: PathBuf,
        error: Option<String>,
    },
    /// The worker found existing files and is waiting for an overwrite
    /// decision.
    NeedOverwrite {
        index: usize,
        conflicts: Vec<PathBuf>,
    },
    Done {
        index: usize,
        success: bool,
        message: String,
        /// Set when extraction succeeded but the source archive could not be
        /// removed (no trash available, missing permissions, ...).
        cleanup_error: Option<String>,
        /// "source -> result" summary when extracting "here" produced a name
        /// different from the archive (used in the notification).
        result: Option<String>,
    },
    AllDone,
}

/// Answer sent from the UI back to the worker.
#[derive(Debug, Clone)]
pub enum JobAnswer {
    /// A password to try.
    Password(String),
    /// Skip the current archive and keep going.
    Skip,
    /// Abort the whole batch.
    CancelBatch,
    /// Extract anyway, overwriting existing files.
    Overwrite,
}

/// Handle used by the UI to answer prompts and cancel the batch.
pub struct JobController {
    pub answer_tx: Sender<JobAnswer>,
    pub cancel_all: Arc<AtomicBool>,
}

/// Spawns the extraction worker.
///
/// Returns the controller used to answer prompts, plus the receiver of worker
/// events that the UI should poll every frame.
pub fn spawn(
    ouch: OuchClient,
    archives: Vec<PathBuf>,
    config: Config,
    ctx: egui::Context,
) -> (JobController, Receiver<JobEvent>) {
    let (event_tx, event_rx) = mpsc::channel::<JobEvent>();
    let (answer_tx, answer_rx) = mpsc::channel::<JobAnswer>();
    let cancel_all = Arc::new(AtomicBool::new(false));

    let controller = JobController {
        answer_tx,
        cancel_all: cancel_all.clone(),
    };

    thread::spawn(move || {
        run(ouch, archives, config, event_tx, answer_rx, cancel_all, ctx);
    });

    (controller, event_rx)
}

fn run(
    ouch: OuchClient,
    archives: Vec<PathBuf>,
    config: Config,
    event_tx: Sender<JobEvent>,
    answer_rx: Receiver<JobAnswer>,
    cancel_all: Arc<AtomicBool>,
    ctx: egui::Context,
) {
    // Password remembered from the first successful prompt, reused afterwards.
    let mut batch_password: Option<String> = None;

    for (index, archive) in archives.iter().enumerate() {
        if cancel_all.load(Ordering::Relaxed) {
            break;
        }
        send(&event_tx, &ctx, JobEvent::Started { index });

        let event = process_one(
            &ouch,
            archive,
            &config,
            &mut batch_password,
            &answer_rx,
            &event_tx,
            index,
            &ctx,
        );
        send(&event_tx, &ctx, event);
    }

    send(&event_tx, &ctx, JobEvent::AllDone);
}

#[allow(clippy::too_many_arguments)]
fn process_one(
    ouch: &OuchClient,
    archive: &Path,
    config: &Config,
    batch_password: &mut Option<String>,
    answer_rx: &Receiver<JobAnswer>,
    event_tx: &Sender<JobEvent>,
    index: usize,
    ctx: &egui::Context,
) -> JobEvent {
    if !archive.exists() {
        return JobEvent::Done {
            index,
            success: false,
            message: format!("file not found: {}", archive.display()),
            cleanup_error: None,
            result: None,
        };
    }

    let is_archive = formats::is_archive_path(archive);

    if let Some(format) = formats::outer_format(archive) {
        if config.is_format_disabled(format.id) {
            let _ = send(
                event_tx,
                ctx,
                JobEvent::Log {
                    index,
                    line: format!("note: format '{}' is disabled in settings", format.id),
                },
            );
        }
    }

    // Non-archive formats (gz, xz, zst, ...) hold a single file and never
    // need a password; unpack straight into the parent directory. The
    // configured conflict policy still applies to the produced file.
    if !is_archive {
        let conflicts = detect_single_file_conflict(archive);
        if let Err(message) =
            apply_conflict_policy(&conflicts, config, answer_rx, event_tx, ctx, index)
        {
            return fail(index, message);
        }
        let result = result_name(archive, false, None);
        return match ouch.decompress(archive, Target::Here, None) {
            Ok(outcome) if outcome.success => {
                finish_success(index, archive, "here", result, config, event_tx, ctx)
            }
            Ok(outcome) => fail(index, outcome_message(&outcome)),
            Err(err) => fail(index, err.to_string()),
        };
    }

    // Archives. Always try to list them: the listing validates the password,
    // drives smart target selection, and lets us detect overwrite conflicts
    // before touching the filesystem.
    let mut password: Option<String> = None;
    let mut listing: Option<String> = None;
    let mut target = match config.decompress_mode {
        DecompressMode::Folder => Target::Folder,
        DecompressMode::Here => Target::Here,
        DecompressMode::Smart => Target::Folder,
    };

    match obtain_listing(
        ouch,
        archive,
        config,
        batch_password,
        answer_rx,
        event_tx,
        index,
        ctx,
    ) {
        Ok((text, found)) => {
            password = found;
            if config.decompress_mode == DecompressMode::Smart {
                let roots = modes::roots_from_listing(&text);
                target = modes::choose_target(DecompressMode::Smart, true, &roots);
            }
            listing = Some(text);
        }
        Err(SkipReason::Cancelled) => return fail(index, "skipped".into()),
        Err(SkipReason::BatchCancelled) => return fail(index, "cancelled".into()),
        Err(SkipReason::HardError(message)) => {
            // Listing failed for a non-password reason. Keep the configured
            // target and let decompression surface the real error.
            let _ = send(
                event_tx,
                ctx,
                JobEvent::Log {
                    index,
                    line: message,
                },
            );
        }
    }

    // Handle existing destinations according to the configured policy.
    if let Some(text) = &listing {
        let conflicts = detect_conflicts(archive, target, text);
        if let Err(message) =
            apply_conflict_policy(&conflicts, config, answer_rx, event_tx, ctx, index)
        {
            return fail(index, message);
        }
    }

    // When extracting "here" and the result name differs from the archive
    // (e.g. `foo.zip` containing `bar.ipa`), report the resulting name.
    let result = if target == Target::Here {
        result_name(archive, true, listing.as_deref())
    } else {
        None
    };

    // Try passwords in a sensible order, then prompt only if the failure looks
    // password-related.
    let mut candidates: Vec<Option<String>> = Vec::new();
    if let Some(found) = password {
        candidates.push(Some(found));
    }
    if let Some(batch) = batch_password.clone() {
        if !candidates.contains(&Some(batch.clone())) {
            candidates.push(Some(batch));
        }
    }
    for saved in &config.passwords {
        let candidate = Some(saved.clone());
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    if !candidates.iter().any(Option::is_none) {
        candidates.push(None);
    }

    let mut last_error = String::new();
    let mut password_related = false;
    for candidate in candidates {
        match ouch.decompress(archive, target, candidate.as_deref()) {
            Ok(outcome) if outcome.success => {
                if candidate.is_some() {
                    *batch_password = candidate;
                }
                return finish_success(
                    index,
                    archive,
                    target.label(),
                    result.clone(),
                    config,
                    event_tx,
                    ctx,
                );
            }
            Ok(outcome) => {
                last_error = outcome_message(&outcome);
                password_related |= is_password_error(&last_error);
            }
            Err(err) => {
                last_error = err.to_string();
                password_related |= is_password_error(&last_error);
            }
        }
    }

    if !password_related {
        return fail(index, last_error);
    }

    // Ask the user, retrying until a password works or the user skips/cancels.
    let mut first_prompt = true;
    loop {
        let _ = send(
            event_tx,
            ctx,
            JobEvent::NeedPassword {
                index,
                archive: archive.to_path_buf(),
                error: (!first_prompt).then(|| last_error.clone()),
            },
        );
        first_prompt = false;
        match answer_rx.recv().unwrap_or(JobAnswer::CancelBatch) {
            JobAnswer::Password(password) => {
                match ouch.decompress(archive, target, Some(&password)) {
                    Ok(outcome) if outcome.success => {
                        *batch_password = Some(password);
                        return finish_success(
                            index,
                            archive,
                            target.label(),
                            result.clone(),
                            config,
                            event_tx,
                            ctx,
                        );
                    }
                    Ok(outcome) => last_error = outcome_message(&outcome),
                    Err(err) => last_error = err.to_string(),
                }
                // A failure that is no longer password-related (disk full,
                // permissions, corrupt data) must be reported, not re-prompted.
                if !is_password_error(&last_error) {
                    return fail(index, last_error);
                }
            }
            JobAnswer::Skip => return fail(index, "skipped".into()),
            JobAnswer::CancelBatch => return fail(index, "cancelled".into()),
            JobAnswer::Overwrite => continue,
        }
    }
}

/// Directory the extracted files are written into (the archive's parent).
fn parent_dir(archive: &Path) -> PathBuf {
    archive
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// True when `name` is a single, normal path component.
///
/// Guards against archive entries like `..`, `../x` or `/abs` turning a
/// conflict check into a rename of a directory outside the archive's folder.
fn is_safe_relative_component(name: &str) -> bool {
    matches!(
        Path::new(name).components().collect::<Vec<_>>().as_slice(),
        [std::path::Component::Normal(_)]
    )
}

/// Returns the destination paths that already exist and would be overwritten.
fn detect_conflicts(archive: &Path, target: Target, listing: &str) -> Vec<PathBuf> {
    let parent = parent_dir(archive);

    match target {
        Target::Folder => {
            let stem = formats::stem_without_known_extensions(archive);
            // A hidden archive (`.zip`) or a non-UTF-8 name yields an empty
            // stem; joining it would make the parent itself the destination and
            // could rename a whole directory, so bail out.
            if stem.is_empty() {
                return Vec::new();
            }
            let destination = parent.join(stem);
            let non_empty = destination
                .read_dir()
                .map(|mut entries| entries.next().is_some())
                .unwrap_or(false);
            if destination.is_dir() && non_empty {
                vec![destination]
            } else {
                Vec::new()
            }
        }
        Target::Here => modes::roots_from_listing(listing)
            .into_iter()
            .filter(|root| is_safe_relative_component(root))
            .map(|root| parent.join(root))
            .filter(|path| path.exists())
            .collect(),
    }
}

/// Returns the output file of a single-file format when it already exists.
fn detect_single_file_conflict(archive: &Path) -> Vec<PathBuf> {
    let stem = formats::stem_without_known_extensions(archive);
    if stem.is_empty() {
        return Vec::new();
    }
    let destination = parent_dir(archive).join(stem);
    if destination.exists() {
        vec![destination]
    } else {
        Vec::new()
    }
}

/// Applies the configured conflict policy to `conflicts`.
///
/// Returns `Ok(())` when extraction should proceed (possibly after renaming or
/// removing conflicting paths) and `Err(message)` when it should be skipped or
/// cancelled.
fn apply_conflict_policy(
    conflicts: &[PathBuf],
    config: &Config,
    answer_rx: &Receiver<JobAnswer>,
    event_tx: &Sender<JobEvent>,
    ctx: &egui::Context,
    index: usize,
) -> Result<(), String> {
    if conflicts.is_empty() {
        return Ok(());
    }
    match config.conflict_policy {
        ConflictPolicy::Overwrite => Ok(()),
        ConflictPolicy::Skip => {
            log_line(
                event_tx,
                ctx,
                index,
                "destination already exists; skipping".into(),
            );
            Err("skipped".into())
        }
        ConflictPolicy::Rename => rename_conflicts(conflicts),
        ConflictPolicy::Ask => {
            let _ = send(
                event_tx,
                ctx,
                JobEvent::NeedOverwrite {
                    index,
                    conflicts: conflicts.to_vec(),
                },
            );
            loop {
                match answer_rx.recv().unwrap_or(JobAnswer::CancelBatch) {
                    JobAnswer::Overwrite => return Ok(()),
                    JobAnswer::Skip => return Err("skipped".into()),
                    JobAnswer::CancelBatch => return Err("cancelled".into()),
                    JobAnswer::Password(_) => continue,
                }
            }
        }
    }
}

/// Renames each conflicting path to a numbered variant (`name_1`, `name_2`, ...)
/// so that extracting can create the original names without losing data.
fn rename_conflicts(conflicts: &[PathBuf]) -> Result<(), String> {
    for path in conflicts {
        let target = find_available_renamed(path);
        std::fs::rename(path, &target)
            .map_err(|err| format!("renaming {}: {err}", path.display()))?;
    }
    Ok(())
}

/// Finds a free `name_N(ext)` path, mirroring `ouch`'s numbering scheme.
fn find_available_renamed(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    for index in 1u64.. {
        let candidate = match file_name.split_once('.') {
            Some((stem, extension)) if !stem.is_empty() => {
                format!("{stem}_{index}.{extension}")
            }
            _ => format!("{file_name}_{index}"),
        };
        let candidate = parent.join(candidate);
        if !candidate.exists() {
            return candidate;
        }
    }
    // `1u64..` cannot realistically exhaust; avoid aborting the whole app.
    parent.join(format!("{file_name}_copy"))
}

/// Applies the configured post-extraction action to the source archive, then
/// builds the success event.
fn finish_success(
    index: usize,
    archive: &Path,
    target_label: &str,
    result: Option<String>,
    config: &Config,
    event_tx: &Sender<JobEvent>,
    ctx: &egui::Context,
) -> JobEvent {
    let cleanup_error = match config.after_extract {
        AfterExtract::Keep => None,
        AfterExtract::Delete => match std::fs::remove_file(archive) {
            Ok(()) => {
                log_line(
                    event_tx,
                    ctx,
                    index,
                    format!("removed {}", archive.display()),
                );
                None
            }
            Err(err) => {
                let message = format!("could not delete {}: {err}", archive.display());
                log_line(event_tx, ctx, index, message.clone());
                Some(message)
            }
        },
        AfterExtract::Trash => {
            if crate::system::trash_file(archive) {
                log_line(
                    event_tx,
                    ctx,
                    index,
                    format!("moved to trash: {}", archive.display()),
                );
                None
            } else {
                // No usable trash (e.g. a filesystem without a trash directory).
                match config.trash_fallback {
                    TrashFallback::Delete => match std::fs::remove_file(archive) {
                        Ok(()) => {
                            log_line(
                                event_tx,
                                ctx,
                                index,
                                format!("no trash: deleted {}", archive.display()),
                            );
                            None
                        }
                        Err(err) => {
                            let message = format!(
                                "could not delete {} (no trash and no permission): {err}",
                                archive.display()
                            );
                            log_line(event_tx, ctx, index, message.clone());
                            Some(message)
                        }
                    },
                    // The user asked not to be warned when set to do nothing.
                    TrashFallback::Nothing => {
                        log_line(
                            event_tx,
                            ctx,
                            index,
                            format!("no trash available; kept {}", archive.display()),
                        );
                        None
                    }
                }
            }
        }
    };

    let archive_name = archive
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let result_summary = result
        .as_ref()
        .map(|result| format!("{archive_name} → {result}"));
    let message_name = result_summary
        .clone()
        .unwrap_or_else(|| archive_name.clone());

    JobEvent::Done {
        index,
        success: true,
        message: format!("extracted {message_name} ({target_label})"),
        cleanup_error,
        result: result_summary,
    }
}

/// Computes the top-level name produced when extracting `archive` "here", when
/// it is *not* obvious from the archive's own name.
///
/// Obvious means the result equals the archive's stem (`foo.tar.gz` -> `foo`)
/// or just extends it with another extension (`foo.zip` -> `foo.ipa`). Only a
/// genuinely different name (`photos.zip` -> `vacation`) is reported.
fn result_name(archive: &Path, is_archive: bool, listing: Option<&str>) -> Option<String> {
    let archive_stem = formats::stem_without_known_extensions(archive);
    let name = if is_archive {
        let roots = modes::roots_from_listing(listing?);
        if roots.len() == 1 {
            roots.into_iter().next()?
        } else {
            return None;
        }
    } else {
        formats::stem_without_known_extensions(archive)
    };

    let extended = name.starts_with(&format!("{archive_stem}."));
    (name != archive_stem && !extended).then_some(name)
}

/// Sends a log event for the given batch index.
fn log_line(event_tx: &Sender<JobEvent>, ctx: &egui::Context, index: usize, line: String) {
    let _ = send(event_tx, ctx, JobEvent::Log { index, line });
}

/// Builds a failed `Done` event.
fn fail(index: usize, message: String) -> JobEvent {
    JobEvent::Done {
        index,
        success: false,
        message,
        cleanup_error: None,
        result: None,
    }
}

/// Extracts the most relevant message from an ouch outcome.
fn outcome_message(outcome: &crate::ouch::DecompressOutcome) -> String {
    if !outcome.stderr.trim().is_empty() {
        outcome.stderr.trim().to_string()
    } else {
        outcome.stdout.trim().to_string()
    }
}

/// Heuristic: does this error indicate the archive is encrypted or the
/// password is missing/wrong?
fn is_password_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("password") || message.contains("encrypt") || message.contains("decrypt")
}

enum SkipReason {
    Cancelled,
    BatchCancelled,
    /// A non-password failure that should be reported instead of prompting.
    HardError(String),
}

/// Tries to list an archive using, in order: no password, the batch password,
/// then every saved password. If a password is required, asks the user until a
/// password works or the user skips/cancels. Non-password failures are
/// returned as [`SkipReason::HardError`] so the caller never asks for a
/// password the archive does not have.
#[allow(clippy::too_many_arguments)]
fn obtain_listing(
    ouch: &OuchClient,
    archive: &Path,
    config: &Config,
    batch_password: &mut Option<String>,
    answer_rx: &Receiver<JobAnswer>,
    event_tx: &Sender<JobEvent>,
    index: usize,
    ctx: &egui::Context,
) -> Result<(String, Option<String>), SkipReason> {
    let mut candidates: Vec<Option<String>> = vec![None];
    if let Some(batch) = batch_password.clone() {
        candidates.push(Some(batch));
    }
    for saved in &config.passwords {
        if !candidates
            .iter()
            .any(|candidate| candidate.as_deref() == Some(saved.as_str()))
        {
            candidates.push(Some(saved.clone()));
        }
    }

    let mut last_error = String::new();
    let mut password_related = false;
    for candidate in candidates {
        match ouch.list(archive, candidate.as_deref()) {
            Ok(listing) => {
                if candidate.is_some() {
                    *batch_password = candidate.clone();
                }
                return Ok((listing, candidate));
            }
            Err(err) => {
                last_error = err.to_string();
                if is_password_error(&last_error) {
                    password_related = true;
                }
            }
        }
    }

    if !password_related {
        return Err(SkipReason::HardError(last_error));
    }

    let mut first_prompt = true;
    loop {
        let _ = send(
            event_tx,
            ctx,
            JobEvent::NeedPassword {
                index,
                archive: archive.to_path_buf(),
                error: (!first_prompt).then(|| last_error.clone()),
            },
        );
        first_prompt = false;

        let answer = answer_rx.recv().unwrap_or(JobAnswer::CancelBatch);
        match answer {
            JobAnswer::Password(password) => match ouch.list(archive, Some(&password)) {
                Ok(listing) => {
                    *batch_password = Some(password);
                    return Ok((listing, batch_password.clone()));
                }
                Err(err) => {
                    last_error = err.to_string();
                    if !is_password_error(&last_error) {
                        return Err(SkipReason::HardError(last_error));
                    }
                }
            },
            JobAnswer::Skip => return Err(SkipReason::Cancelled),
            JobAnswer::CancelBatch => return Err(SkipReason::BatchCancelled),
            JobAnswer::Overwrite => continue,
        }
    }
}

/// Sends an event and wakes the UI so it repaints.
fn send(tx: &Sender<JobEvent>, ctx: &egui::Context, event: JobEvent) -> bool {
    let sent = tx.send(event).is_ok();
    ctx.request_repaint();
    sent
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn detects_password_related_errors() {
        assert!(is_password_error(
            "ERROR: Unsupported zip archive\n - Password required to decrypt file"
        ));
        assert!(is_password_error("ERROR: 7z error\n - PasswordRequired"));
        assert!(is_password_error(
            "failed to extract \"secret.txt\"\n - Password for encrypted archive not specified"
        ));
    }

    #[test]
    fn ignores_unrelated_errors() {
        assert!(!is_password_error(
            "ERROR: Invalid zip archive\n - Could not find EOCD"
        ));
        assert!(!is_password_error("No such file or directory (os error 2)"));
        assert!(!is_password_error("It is not a valid archive"));
    }

    #[test]
    fn finds_numbered_rename_target() {
        let base = unique_dir("rename");
        let path = base.join("a.txt");
        assert_eq!(find_available_renamed(&path), base.join("a_1.txt"));
        std::fs::write(base.join("a_1.txt"), b"x").unwrap();
        assert_eq!(find_available_renamed(&path), base.join("a_2.txt"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_unsafe_relative_roots() {
        assert!(is_safe_relative_component("folder"));
        assert!(is_safe_relative_component("a b.txt"));
        assert!(!is_safe_relative_component(""));
        assert!(!is_safe_relative_component("."));
        assert!(!is_safe_relative_component(".."));
        assert!(!is_safe_relative_component("../evil"));
        assert!(!is_safe_relative_component("/abs"));
        assert!(!is_safe_relative_component("a/b"));
    }

    #[test]
    fn hidden_archive_does_not_conflict_with_parent() {
        let base = unique_dir("hidden");
        // A populated parent must never be reported as the destination of a
        // hidden archive like `.zip`, whose stem is empty.
        std::fs::write(base.join("other.txt"), b"x").unwrap();
        let archive = base.join(".zip");
        std::fs::write(&archive, b"x").unwrap();
        assert!(detect_conflicts(&archive, Target::Folder, "").is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn detects_single_file_output_conflict() {
        let base = unique_dir("single");
        let archive = base.join("note.txt.gz");
        std::fs::write(&archive, b"x").unwrap();
        assert!(detect_single_file_conflict(&archive).is_empty());
        std::fs::write(base.join("note.txt"), b"x").unwrap();
        assert_eq!(
            detect_single_file_conflict(&archive),
            vec![base.join("note.txt")]
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn detects_renamed_result_when_extracting_here() {
        // Obvious: result only changes the extension of the same stem.
        assert_eq!(
            result_name(
                Path::new("/tmp/PatreonTV-unsigned.zip"),
                true,
                Some("PatreonTV-unsigned.ipa\n")
            ),
            None
        );
        // Obvious: `.tar.gz` -> the stem.
        assert_eq!(
            result_name(
                Path::new("/tmp/crispasr-linux-x86_64-hip.tar.gz"),
                true,
                Some("crispasr-linux-x86_64-hip\n")
            ),
            None
        );
        // Non-archive formats simply drop the compression extension.
        assert_eq!(
            result_name(Path::new("/tmp/report.txt.gz"), false, None),
            None
        );
        assert_eq!(result_name(Path::new("/tmp/file.gz"), false, None), None);
        // Not obvious: the root has a genuinely different name.
        assert_eq!(
            result_name(Path::new("/tmp/photos.zip"), true, Some("vacation/\n")),
            Some("vacation".to_string())
        );
        // Multiple roots are extracted into a folder named after the archive.
        assert_eq!(
            result_name(Path::new("/tmp/a.zip"), true, Some("x\ny\n")),
            None
        );
    }

    fn unique_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("ouch-gui-job-{}-{tag}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Builds a plain zip fixture with a single root file. The archive lives
    /// in an `out/` subdirectory so that extracting "here" does not collide
    /// with the source file.
    fn build_zip(ouch: &OuchClient, dir: &Path) -> PathBuf {
        std::fs::write(dir.join("hello.txt"), b"hello").unwrap();
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let archive = out.join("good.zip");
        let status = Command::new(ouch.path())
            .current_dir(dir)
            .args(["compress", "-y", "hello.txt"])
            .arg(&archive)
            .stdin(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        archive
    }

    /// Runs `process_one` with `answers` pre-queued, so a mistaken password
    /// prompt cannot hang the test. Returns the final event and every event
    /// the worker emitted.
    fn run_process_one_with(
        ouch: &OuchClient,
        archive: &Path,
        config: &Config,
        answers: &[JobAnswer],
    ) -> (JobEvent, Vec<JobEvent>) {
        let (event_tx, event_rx) = mpsc::channel();
        let (answer_tx, answer_rx) = mpsc::channel();
        let mut batch_password = None;
        let ctx = egui::Context::default();

        for answer in answers {
            let _ = answer_tx.send(answer.clone());
        }

        let event = process_one(
            ouch,
            archive,
            config,
            &mut batch_password,
            &answer_rx,
            &event_tx,
            0,
            &ctx,
        );
        let events: Vec<JobEvent> = event_rx.try_iter().collect();
        (event, events)
    }

    /// Convenience wrapper that never supplies a password.
    fn run_process_one(
        ouch: &OuchClient,
        archive: &Path,
        config: &Config,
    ) -> (JobEvent, Vec<JobEvent>) {
        run_process_one_with(ouch, archive, config, &[JobAnswer::Skip, JobAnswer::Skip])
    }

    #[test]
    fn plain_zip_extracts_without_password_prompt() {
        let Ok(ouch) = OuchClient::discover() else {
            eprintln!("skipping: ouch binary not found");
            return;
        };
        let base = unique_dir("plain");
        let archive = build_zip(&ouch, &base);
        let config = Config::default();

        let (event, events) = run_process_one(&ouch, &archive, &config);

        let prompted = events
            .iter()
            .any(|event| matches!(event, JobEvent::NeedPassword { .. }));
        assert!(!prompted, "plain zip must not trigger a password prompt");
        match event {
            JobEvent::Done {
                success, message, ..
            } => {
                assert!(success, "plain zip should extract, got: {message}");
            }
            other => panic!("unexpected event: {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn corrupt_zip_fails_without_password_prompt() {
        let Ok(ouch) = OuchClient::discover() else {
            eprintln!("skipping: ouch binary not found");
            return;
        };
        let base = unique_dir("corrupt");
        let archive = build_zip(&ouch, &base);
        // Truncate so ouch reports a parse error rather than encryption.
        let bytes = std::fs::read(&archive).unwrap();
        std::fs::write(&archive, &bytes[..bytes.len().min(100)]).unwrap();

        let config = Config::default();
        let (event, events) = run_process_one(&ouch, &archive, &config);

        let prompted = events
            .iter()
            .any(|event| matches!(event, JobEvent::NeedPassword { .. }));
        assert!(!prompted, "corrupt zip must not trigger a password prompt");
        match event {
            JobEvent::Done {
                success, message, ..
            } => {
                assert!(!success, "corrupt zip should fail");
                assert_ne!(message, "skipped", "corrupt zip should not be skipped");
            }
            other => panic!("unexpected event: {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn existing_file_triggers_overwrite_prompt() {
        let Ok(ouch) = OuchClient::discover() else {
            eprintln!("skipping: ouch binary not found");
            return;
        };
        let base = unique_dir("overwrite");
        let archive = build_zip(&ouch, &base);
        let out = base.join("out");
        // Pre-existing file with the same name as the archive entry.
        std::fs::write(out.join("hello.txt"), b"OLD").unwrap();

        let config = Config::default();
        let (event, events) =
            run_process_one_with(&ouch, &archive, &config, &[JobAnswer::Overwrite]);

        let prompted = events
            .iter()
            .any(|event| matches!(event, JobEvent::NeedOverwrite { .. }));
        assert!(prompted, "existing file must trigger an overwrite prompt");
        match event {
            JobEvent::Done { success, .. } => assert!(success, "overwrite should extract"),
            other => panic!("unexpected event: {other:?}"),
        }
        assert_eq!(std::fs::read(out.join("hello.txt")).unwrap(), b"hello");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn encrypted_zip_prompts_and_accepts_password() {
        let Ok(ouch) = OuchClient::discover() else {
            eprintln!("skipping: ouch binary not found");
            return;
        };
        // Creating an encrypted zip needs the external `zip` tool.
        if Command::new("zip").arg("--version").output().is_err() {
            eprintln!("skipping: `zip` tool not found");
            return;
        }

        let base = unique_dir("encrypted");
        let out = base.join("out");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("secret.txt"), b"secret").unwrap();
        let archive = out.join("secret.zip");
        let status = Command::new("zip")
            .current_dir(&out)
            .args(["-q", "-P", "hunter2", "secret.zip", "secret.txt"])
            .status()
            .unwrap();
        assert!(status.success());

        let config = Config::default();
        let (event, events) = run_process_one_with(
            &ouch,
            &archive,
            &config,
            &[JobAnswer::Password("hunter2".into()), JobAnswer::Overwrite],
        );

        let prompted = events
            .iter()
            .any(|event| matches!(event, JobEvent::NeedPassword { .. }));
        assert!(prompted, "encrypted zip must trigger a password prompt");
        match event {
            JobEvent::Done {
                success, message, ..
            } => {
                assert!(success, "correct password should extract, got: {message}");
            }
            other => panic!("unexpected event: {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn wrong_password_retry_reports_the_error() {
        let Ok(ouch) = OuchClient::discover() else {
            eprintln!("skipping: ouch binary not found");
            return;
        };
        if Command::new("zip").arg("--version").output().is_err() {
            eprintln!("skipping: `zip` tool not found");
            return;
        }

        let base = unique_dir("wrongpw");
        let out = base.join("out");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("secret.txt"), b"secret").unwrap();
        let archive = out.join("secret.zip");
        assert!(Command::new("zip")
            .current_dir(&out)
            .args(["-q", "-P", "hunter2", "secret.zip", "secret.txt"])
            .status()
            .unwrap()
            .success());

        let config = Config::default();
        let (event, events) = run_process_one_with(
            &ouch,
            &archive,
            &config,
            &[
                JobAnswer::Password("wrong".into()),
                JobAnswer::Password("hunter2".into()),
                JobAnswer::Overwrite,
            ],
        );

        // The first prompt is clean; the error only appears after the wrong one.
        let prompts: Vec<Option<String>> = events
            .iter()
            .filter_map(|event| match event {
                JobEvent::NeedPassword { error, .. } => Some(error.clone()),
                _ => None,
            })
            .collect();
        assert!(prompts.len() >= 2, "expected at least two prompts");
        assert!(prompts[0].is_none(), "first prompt should have no error");
        assert!(prompts[1].is_some(), "retry prompt should carry the error");
        match event {
            JobEvent::Done { success, .. } => assert!(success, "correct password should extract"),
            other => panic!("unexpected event: {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&base);
    }
}
