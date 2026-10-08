//! Background batch extraction.
//!
//! A single worker thread processes archives sequentially so that a password
//! entered for the first archive can be reused for the rest of the batch.
//! Communication with the UI uses channels; the worker never touches egui
//! state directly.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::config::{AfterExtract, Config, ConflictPolicy, TrashFallback};
use crate::formats;
use crate::i18n::I18n;
use crate::modes::{self, DecompressMode, Target};
use crate::multivolume;
use crate::ouch::OuchClient;
use crate::split;

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
    /// The worker is preparing an archive before extracting it (joining the
    /// parts of a split) or has finished preparing. The UI shows this phase
    /// instead of "extracting".
    Joining {
        index: usize,
        active: bool,
    },
    /// Approximate progress of the archive currently being extracted.
    /// `fraction` is `0.0..=1.0`, computed from the bytes `ouch` has read from
    /// the archive versus its total size. For a multi-volume set `part_index`
    /// is the volume being read. `total == 0` means unknown.
    Progress {
        index: usize,
        fraction: f32,
        total: u64,
        part_index: usize,
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
    /// PID of the `ouch` process currently running, or 0 when none is. The UI
    /// uses it to terminate the child if the user closes mid-extraction.
    pub current_pid: Arc<AtomicU32>,
}

/// Spawns the extraction worker.
///
/// Returns the controller used to answer prompts, plus the receiver of worker
/// events that the UI should poll every frame.
pub fn spawn(
    ouch: OuchClient,
    archives: Vec<PathBuf>,
    config: Config,
    i18n: I18n,
    ctx: egui::Context,
) -> (JobController, Receiver<JobEvent>) {
    let (event_tx, event_rx) = mpsc::channel::<JobEvent>();
    let (answer_tx, answer_rx) = mpsc::channel::<JobAnswer>();
    let cancel_all = Arc::new(AtomicBool::new(false));
    let current_pid = Arc::new(AtomicU32::new(0));

    let controller = JobController {
        answer_tx,
        cancel_all: cancel_all.clone(),
        current_pid: current_pid.clone(),
    };

    thread::spawn(move || {
        run(
            ouch,
            archives,
            config,
            i18n,
            event_tx,
            answer_rx,
            cancel_all,
            current_pid,
            ctx,
        );
    });

    (controller, event_rx)
}

#[allow(clippy::too_many_arguments)]
fn run(
    ouch: OuchClient,
    archives: Vec<PathBuf>,
    config: Config,
    i18n: I18n,
    event_tx: Sender<JobEvent>,
    answer_rx: Receiver<JobAnswer>,
    cancel_all: Arc<AtomicBool>,
    current_pid: Arc<AtomicU32>,
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
            &i18n,
            &mut batch_password,
            &answer_rx,
            &event_tx,
            index,
            &current_pid,
            &cancel_all,
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
    i18n: &I18n,
    batch_password: &mut Option<String>,
    answer_rx: &Receiver<JobAnswer>,
    event_tx: &Sender<JobEvent>,
    index: usize,
    current_pid: &Arc<AtomicU32>,
    cancel_all: &Arc<AtomicBool>,
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

    // Every file of a multi-volume set, for the post-extraction cleanup.
    let cleanup_parts = split::archive_parts(archive);

    // Fail before touching the disk when volumes are missing; extracting the
    // remaining parts would just produce garbage and waste space.
    if let Some(missing) = split::missing_parts(&cleanup_parts) {
        return fail(index, missing_parts_message(i18n, &missing));
    }

    // Logs each `ouch` command as it runs.
    let log = |line: &str| log_line(event_tx, ctx, index, line.to_string());

    // Progress reporting is based on the bytes `ouch` reads from the archive,
    // so it works even for a single huge file (where counting entries does
    // not). The guard stops the poller when this function returns. `current_pid`
    // is shared with the UI so it can kill the child if the user closes.
    let _progress =
        ProgressPoller::spawn(index, &cleanup_parts, current_pid.clone(), event_tx, ctx);

    // Prepare the file `ouch` should read. For a raw split (`name.7z.001`) this
    // concatenates the volumes into a temporary (see `multivolume`); other
    // archives are returned untouched. `workdir` keeps the output next to the
    // original even when the temporary lives elsewhere (e.g. /dev/shm).
    let split_kind = split::kind(archive);
    let joining = split_kind == split::SplitKind::Concat;
    if joining {
        let _ = send(
            event_tx,
            ctx,
            JobEvent::Joining {
                index,
                active: true,
            },
        );
    }
    let prepared = multivolume::prepare(
        archive,
        &cleanup_parts,
        split_kind,
        cancel_all,
        |copied, total| {
            let fraction = if total == 0 {
                0.0
            } else {
                copied as f32 / total as f32
            };
            let _ = send(
                event_tx,
                ctx,
                JobEvent::Progress {
                    index,
                    fraction,
                    total,
                    part_index: 0,
                },
            );
            ctx.request_repaint();
        },
    );
    if joining {
        let _ = send(
            event_tx,
            ctx,
            JobEvent::Joining {
                index,
                active: false,
            },
        );
    }
    let prepared = match prepared {
        Ok(Some(prepared)) => prepared,
        Ok(None) => return fail(index, "cancelled".into()),
        Err(err) => return fail(index, err.to_string()),
    };
    let source = prepared.path();
    let workdir = parent_dir(archive);

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
        return match ouch.decompress(source, &workdir, Target::Here, None, current_pid, &log) {
            Ok(outcome) if outcome.success => finish_success(
                index,
                archive,
                &cleanup_parts,
                "here",
                result,
                config,
                event_tx,
                ctx,
            ),
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
        source,
        config,
        batch_password,
        answer_rx,
        event_tx,
        index,
        cancel_all,
        ctx,
    ) {
        Ok((text, found)) => {
            password = found;
            if config.decompress_mode == DecompressMode::Smart {
                let roots = modes::roots_from_listing(&text);
                target = modes::choose_target(DecompressMode::Smart, true, &roots);
                log_line(
                    event_tx,
                    ctx,
                    index,
                    format!("smart: {} root(s) -> {}", roots.len(), target.label()),
                );
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
        if cancel_all.load(Ordering::Relaxed) {
            return fail(index, "cancelled".into());
        }
        match ouch.decompress(
            source,
            &workdir,
            target,
            candidate.as_deref(),
            current_pid,
            &log,
        ) {
            Ok(outcome) if outcome.success => {
                if candidate.is_some() {
                    *batch_password = candidate;
                }
                return finish_success(
                    index,
                    archive,
                    &cleanup_parts,
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
        if cancel_all.load(Ordering::Relaxed) {
            return fail(index, "cancelled".into());
        }
    }

    if !password_related {
        return fail(index, last_error);
    }

    // Ask the user, retrying until a password works or the user skips/cancels.
    let mut first_prompt = true;
    loop {
        if cancel_all.load(Ordering::Relaxed) {
            return fail(index, "cancelled".into());
        }
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
                match ouch.decompress(source, &workdir, target, Some(&password), current_pid, &log)
                {
                    Ok(outcome) if outcome.success => {
                        *batch_password = Some(password);
                        return finish_success(
                            index,
                            archive,
                            &cleanup_parts,
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

/// Applies the configured post-extraction action to every source part and
/// returns the last error, if any.
fn cleanup_sources(
    parts: &[PathBuf],
    config: &Config,
    event_tx: &Sender<JobEvent>,
    ctx: &egui::Context,
    index: usize,
) -> Option<String> {
    let mut error = None;
    for path in parts {
        let result = match config.after_extract {
            AfterExtract::Keep => continue,
            AfterExtract::Delete => match std::fs::remove_file(path) {
                Ok(()) => {
                    log_line(event_tx, ctx, index, format!("removed {}", path.display()));
                    Ok(())
                }
                Err(err) => Err(format!("could not delete {}: {err}", path.display())),
            },
            AfterExtract::Trash => {
                if crate::system::trash_file(path) {
                    log_line(
                        event_tx,
                        ctx,
                        index,
                        format!("moved to trash: {}", path.display()),
                    );
                    Ok(())
                } else {
                    // No usable trash (e.g. a filesystem without a trash dir).
                    match config.trash_fallback {
                        TrashFallback::Delete => match std::fs::remove_file(path) {
                            Ok(()) => {
                                log_line(
                                    event_tx,
                                    ctx,
                                    index,
                                    format!("no trash: deleted {}", path.display()),
                                );
                                Ok(())
                            }
                            Err(err) => Err(format!(
                                "could not delete {} (no trash and no permission): {err}",
                                path.display()
                            )),
                        },
                        // The user asked not to be warned when set to do nothing.
                        TrashFallback::Nothing => {
                            log_line(
                                event_tx,
                                ctx,
                                index,
                                format!("no trash available; kept {}", path.display()),
                            );
                            Ok(())
                        }
                    }
                }
            }
        };
        if let Err(message) = result {
            log_line(event_tx, ctx, index, message.clone());
            error = Some(message);
        }
    }
    error
}

/// Applies the configured post-extraction action to every source part, then
/// builds the success event.
#[allow(clippy::too_many_arguments)]
fn finish_success(
    index: usize,
    archive: &Path,
    parts: &[PathBuf],
    target_label: &str,
    result: Option<String>,
    config: &Config,
    event_tx: &Sender<JobEvent>,
    ctx: &egui::Context,
) -> JobEvent {
    let cleanup_error = cleanup_sources(parts, config, event_tx, ctx, index);

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

/// Heuristic: a crash (or an unexplained non-zero exit) may be an encrypted
/// archive that `ouch` cannot describe without the password.
fn looks_like_crash(message: &str) -> bool {
    message.contains("signal:") || message.contains("exit status")
}

/// Builds the localized message for a multi-volume set with missing volumes.
fn missing_parts_message(i18n: &I18n, missing: &split::MissingParts) -> String {
    let names = missing.names.join(", ");
    if missing.more {
        return i18n
            .t("error.missing_parts_at_least")
            .replace("{names}", &names);
    }
    match missing.total {
        Some(total) => i18n
            .t("error.missing_parts_of")
            .replace("{names}", &names)
            .replace("{total}", &total.to_string()),
        None => i18n.t("error.missing_parts").replace("{names}", &names),
    }
}

/// Parses the `rchar` field (bytes read via syscalls) from `/proc/<pid>/io`.
fn parse_rchar(contents: &str) -> Option<u64> {
    contents
        .lines()
        .find_map(|line| line.strip_prefix("rchar:")?.trim().parse::<u64>().ok())
}

/// Bytes read by the given process so far, from `/proc/<pid>/io`.
fn process_read_bytes(pid: u32) -> Option<u64> {
    let contents = std::fs::read_to_string(format!("/proc/{pid}/io")).ok()?;
    parse_rchar(&contents)
}

/// Stops the progress poller thread when dropped.
struct ProgressPoller {
    stop_tx: Sender<()>,
    handle: Option<thread::JoinHandle<()>>,
}

impl ProgressPoller {
    /// Starts reporting progress for the given parts. Returns `None` when the
    /// total size is unknown, in which case the UI stays indeterminate.
    fn spawn(
        index: usize,
        parts: &[PathBuf],
        pid_slot: Arc<AtomicU32>,
        event_tx: &Sender<JobEvent>,
        ctx: &egui::Context,
    ) -> Option<Self> {
        let mut cumulative = Vec::with_capacity(parts.len());
        let mut total = 0u64;
        for part in parts {
            total += std::fs::metadata(part)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            cumulative.push(total);
        }
        if total == 0 {
            return None;
        }
        // Report the total right away so the UI shows a determinate bar even
        // if the extraction finishes before the first poll.
        let _ = event_tx.send(JobEvent::Progress {
            index,
            fraction: 0.0,
            total,
            part_index: 0,
        });
        ctx.request_repaint();
        let (stop_tx, stop_rx) = mpsc::channel();
        let event_tx = event_tx.clone();
        let ctx = ctx.clone();
        let handle = thread::spawn(move || {
            run_progress_poller(index, total, cumulative, pid_slot, event_tx, ctx, stop_rx);
        });
        Some(Self {
            stop_tx,
            handle: Some(handle),
        })
    }
}

impl Drop for ProgressPoller {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Poller thread body: while `ouch` runs, reads the child's `/proc/<pid>/io`
/// and reports `bytes_read / total_size` as the progress fraction.
fn run_progress_poller(
    index: usize,
    total: u64,
    cumulative: Vec<u64>,
    pid_slot: Arc<AtomicU32>,
    event_tx: Sender<JobEvent>,
    ctx: egui::Context,
    stop_rx: Receiver<()>,
) {
    loop {
        let pid = pid_slot.load(Ordering::Relaxed);
        if pid != 0 {
            if let Some(read) = process_read_bytes(pid) {
                let fraction = (read as f64 / total as f64).min(1.0) as f32;
                let part_index = part_index_for(read, &cumulative);
                let _ = event_tx.send(JobEvent::Progress {
                    index,
                    fraction,
                    total,
                    part_index,
                });
                ctx.request_repaint();
            }
        }
        match stop_rx.recv_timeout(Duration::from_millis(80)) {
            Ok(()) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Index of the part that contains byte `read`, given the cumulative byte
/// ends of every part.
fn part_index_for(read: u64, cumulative: &[u64]) -> usize {
    cumulative
        .iter()
        .position(|end| read < *end)
        .unwrap_or_else(|| cumulative.len().saturating_sub(1))
}

enum SkipReason {
    Cancelled,
    BatchCancelled,
    /// A non-password failure that should be reported instead of prompting.
    HardError(String),
}

/// Tries to list an archive using, in order: no password, the batch password,
/// then every saved password. If a password is required — or `ouch` crashes,
/// which may hide an encrypted archive — it asks the user until a password
/// works or the user skips/cancels. Other failures are returned as
/// [`SkipReason::HardError`] so the caller never asks for a password the
/// archive does not have.
#[allow(clippy::too_many_arguments)]
fn obtain_listing(
    ouch: &OuchClient,
    archive: &Path,
    config: &Config,
    batch_password: &mut Option<String>,
    answer_rx: &Receiver<JobAnswer>,
    event_tx: &Sender<JobEvent>,
    index: usize,
    cancel_all: &Arc<AtomicBool>,
    ctx: &egui::Context,
) -> Result<(String, Option<String>), SkipReason> {
    let log = |line: &str| log_line(event_tx, ctx, index, line.to_string());
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
        if cancel_all.load(Ordering::Relaxed) {
            return Err(SkipReason::BatchCancelled);
        }
        match ouch.list(archive, candidate.as_deref(), &log) {
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

    // A crash may hide an encrypted archive that `ouch list` cannot describe
    // without the password, so it is worth offering the password prompt.
    if !password_related && !looks_like_crash(&last_error) {
        return Err(SkipReason::HardError(last_error));
    }

    let mut first_prompt = true;
    loop {
        if cancel_all.load(Ordering::Relaxed) {
            return Err(SkipReason::BatchCancelled);
        }
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
            JobAnswer::Password(password) => match ouch.list(archive, Some(&password), &log) {
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
    fn treats_crashes_as_ambiguous() {
        assert!(looks_like_crash("ouch list failed: signal: 11 (SIGSEGV)"));
        assert!(looks_like_crash("ouch list failed: exit status: 1"));
        assert!(!looks_like_crash("ouch list failed: not a valid archive"));
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
    fn parses_rchar_from_proc_io() {
        assert_eq!(parse_rchar("rchar: 12345\nwchar: 678\n"), Some(12345));
        assert_eq!(parse_rchar("wchar: 1\nsyscr: 2\n"), None);
        assert_eq!(parse_rchar(""), None);
    }

    #[test]
    fn maps_read_bytes_to_part_index() {
        let cumulative = [100, 250, 300];
        assert_eq!(part_index_for(0, &cumulative), 0);
        assert_eq!(part_index_for(99, &cumulative), 0);
        assert_eq!(part_index_for(100, &cumulative), 1);
        assert_eq!(part_index_for(249, &cumulative), 1);
        assert_eq!(part_index_for(250, &cumulative), 2);
        assert_eq!(part_index_for(999, &cumulative), 2);
        assert_eq!(part_index_for(5, &[]), 0);
    }

    #[test]
    fn deletes_every_part_after_extraction() {
        let base = unique_dir("cleanup");
        let parts: Vec<PathBuf> = (1..=3)
            .map(|number| base.join(format!("movie.part{number}.rar")))
            .collect();
        for part in &parts {
            std::fs::write(part, b"x").unwrap();
        }

        let config = Config {
            after_extract: AfterExtract::Delete,
            ..Config::default()
        };
        let (tx, _rx) = mpsc::channel();
        let ctx = egui::Context::default();

        let error = cleanup_sources(&parts, &config, &tx, &ctx, 0);
        assert!(error.is_none());
        assert!(parts.iter().all(|part| !part.exists()));
        let _ = std::fs::remove_dir_all(&base);
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

        let current_pid = Arc::new(AtomicU32::new(0));
        let cancel_all = Arc::new(AtomicBool::new(false));
        let i18n = I18n::new(crate::i18n::Language::En);
        let event = process_one(
            ouch,
            archive,
            config,
            &i18n,
            &mut batch_password,
            &answer_rx,
            &event_tx,
            0,
            &current_pid,
            &cancel_all,
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

    /// Builds a (possibly multi-volume) RAR with the `rar` tool and returns its
    /// first part, or `None` when `rar` is not available.
    fn build_multipart_rar(
        dir: &Path,
        inputs: &[&str],
        base: &str,
        volume: &str,
    ) -> Option<PathBuf> {
        let status = Command::new("rar")
            .current_dir(dir)
            .args(["a", "-idq", "-v", volume, "-m0", base])
            .args(inputs)
            .stdin(Stdio::null())
            .status()
            .ok()?;
        if !status.success() {
            return None;
        }
        let stem = base.strip_suffix(".rar")?;
        let first = dir.join(format!("{stem}.part1.rar"));
        Some(if first.exists() {
            first
        } else {
            dir.join(base)
        })
    }

    /// Builds a numeric split (`base.7z.001`, ...) with the `7z` tool and
    /// returns its first part, or `None` when `7z` is not available.
    fn build_split_7z(dir: &Path, inputs: &[&str], base: &str, volume: &str) -> Option<PathBuf> {
        let status = Command::new("7z")
            .current_dir(dir)
            .args(["a", "-y", "-bso0", "-bsp0", "-mx0"])
            .arg(format!("-v{volume}"))
            .arg(base)
            .args(inputs)
            .stdin(Stdio::null())
            .status()
            .ok()?;
        if !status.success() {
            return None;
        }
        let first = dir.join(format!("{base}.001"));
        Some(if first.exists() {
            first
        } else {
            dir.join(base)
        })
    }

    #[test]
    fn extracts_split_7z_by_concatenating() {
        let Ok(ouch) = OuchClient::discover() else {
            eprintln!("skipping: ouch binary not found");
            return;
        };
        let base = unique_dir("split-7z");
        std::fs::write(base.join("Payload.iso"), vec![5u8; 300_000]).unwrap();
        let Some(first) = build_split_7z(&base, &["Payload.iso"], "multi.7z", "100k") else {
            eprintln!("skipping: 7z tool not found");
            let _ = std::fs::remove_dir_all(&base);
            return;
        };
        std::fs::remove_file(base.join("Payload.iso")).unwrap();
        assert!(
            base.join("multi.7z.002").exists(),
            "the archive should have been split"
        );

        let config = Config {
            decompress_mode: DecompressMode::Smart,
            conflict_policy: ConflictPolicy::Overwrite,
            after_extract: AfterExtract::Keep,
            ..Config::default()
        };
        let (event, _) = run_process_one(&ouch, &first, &config);

        assert!(
            matches!(event, JobEvent::Done { success: true, .. }),
            "event: {event:?}"
        );
        assert!(base.join("Payload.iso").exists(), "should extract the file");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn smart_single_root_multipart_extracts_here() {
        let Ok(ouch) = OuchClient::discover() else {
            eprintln!("skipping: ouch binary not found");
            return;
        };
        let base = unique_dir("smart-multi-here");
        std::fs::write(base.join("Payload.iso"), vec![7u8; 200_000]).unwrap();
        let Some(archive) = build_multipart_rar(&base, &["Payload.iso"], "multi.rar", "100k")
        else {
            eprintln!("skipping: rar tool not found");
            let _ = std::fs::remove_dir_all(&base);
            return;
        };
        std::fs::remove_file(base.join("Payload.iso")).unwrap();

        let config = Config {
            decompress_mode: DecompressMode::Smart,
            conflict_policy: ConflictPolicy::Overwrite,
            after_extract: AfterExtract::Keep,
            ..Config::default()
        };
        let (event, _) = run_process_one(&ouch, &archive, &config);

        assert!(
            matches!(event, JobEvent::Done { success: true, .. }),
            "event: {event:?}"
        );
        assert!(base.join("Payload.iso").exists(), "should extract here");
        assert!(
            !base.join("multi.part1").exists(),
            "a single root must not create a wrapper folder"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn smart_multiple_roots_multipart_makes_a_folder() {
        let Ok(ouch) = OuchClient::discover() else {
            eprintln!("skipping: ouch binary not found");
            return;
        };
        let base = unique_dir("smart-multi-folder");
        std::fs::write(base.join("one.txt"), vec![1u8; 80_000]).unwrap();
        std::fs::write(base.join("two.txt"), vec![2u8; 80_000]).unwrap();
        let Some(archive) =
            build_multipart_rar(&base, &["one.txt", "two.txt"], "multi.rar", "100k")
        else {
            eprintln!("skipping: rar tool not found");
            let _ = std::fs::remove_dir_all(&base);
            return;
        };
        std::fs::remove_file(base.join("one.txt")).unwrap();
        std::fs::remove_file(base.join("two.txt")).unwrap();

        let config = Config {
            decompress_mode: DecompressMode::Smart,
            conflict_policy: ConflictPolicy::Overwrite,
            after_extract: AfterExtract::Keep,
            ..Config::default()
        };
        let (event, _) = run_process_one(&ouch, &archive, &config);

        assert!(
            matches!(event, JobEvent::Done { success: true, .. }),
            "event: {event:?}"
        );
        assert!(
            base.join("multi.part1").join("one.txt").exists(),
            "multiple roots should create a folder"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn missing_first_volume_fails_clearly() {
        let Ok(ouch) = OuchClient::discover() else {
            eprintln!("skipping: ouch binary not found");
            return;
        };
        let base = unique_dir("missing-first");
        std::fs::write(base.join("Payload.iso"), vec![9u8; 200_000]).unwrap();
        let Some(_first) = build_multipart_rar(&base, &["Payload.iso"], "multi.rar", "100k") else {
            eprintln!("skipping: rar tool not found");
            let _ = std::fs::remove_dir_all(&base);
            return;
        };
        std::fs::remove_file(base.join("Payload.iso")).unwrap();
        // Remove the first volume and open the second one.
        std::fs::remove_file(base.join("multi.part1.rar")).unwrap();
        let second = base.join("multi.part2.rar");
        assert!(second.exists());

        let config = Config {
            decompress_mode: DecompressMode::Smart,
            conflict_policy: ConflictPolicy::Overwrite,
            after_extract: AfterExtract::Keep,
            ..Config::default()
        };
        let (event, _) = run_process_one(&ouch, &second, &config);

        match event {
            JobEvent::Done {
                success, message, ..
            } => {
                assert!(!success, "should fail");
                assert!(message.contains("multi.part1.rar"), "message: {message}");
            }
            other => panic!("unexpected event: {other:?}"),
        }
        assert!(!base.join("Payload.iso").exists(), "must not extract");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn missing_last_volumes_fails_clearly() {
        let Ok(ouch) = OuchClient::discover() else {
            eprintln!("skipping: ouch binary not found");
            return;
        };
        let base = unique_dir("missing-last");
        std::fs::write(base.join("Payload.iso"), vec![4u8; 250_000]).unwrap();
        let Some(_first) = build_multipart_rar(&base, &["Payload.iso"], "multi.rar", "100k") else {
            eprintln!("skipping: rar tool not found");
            let _ = std::fs::remove_dir_all(&base);
            return;
        };
        std::fs::remove_file(base.join("Payload.iso")).unwrap();
        // Keep only the first volume; the rest are the missing trailing parts.
        for number in 2..=12 {
            let _ = std::fs::remove_file(base.join(format!("multi.part{number}.rar")));
        }
        let first = base.join("multi.part1.rar");
        assert!(first.exists());

        let config = Config {
            decompress_mode: DecompressMode::Smart,
            conflict_policy: ConflictPolicy::Overwrite,
            after_extract: AfterExtract::Keep,
            ..Config::default()
        };
        let (event, _) = run_process_one(&ouch, &first, &config);

        match event {
            JobEvent::Done {
                success, message, ..
            } => {
                assert!(!success, "should fail");
                assert!(message.contains("multi.part2.rar"), "message: {message}");
            }
            other => panic!("unexpected event: {other:?}"),
        }
        assert!(!base.join("Payload.iso").exists(), "must not extract");
        let _ = std::fs::remove_dir_all(&base);
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
