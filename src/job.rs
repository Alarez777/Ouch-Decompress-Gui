//! Background batch extraction.
//!
//! A small pool of worker threads extracts several archives at once
//! (`Config::parallel_extractions`, clamped to a safe range). A password
//! entered for one archive is reused for the rest of the batch, and prompts to
//! the UI are serialized so only one modal is shown at a time. Communication
//! with the UI uses channels; workers never touch egui state directly.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use crate::config::{
    AfterExtract, Config, ConflictPolicy, TrashFallback, MAX_PARALLEL_EXTRACTIONS,
};
use crate::formats::{self, ArchiveKind};
use crate::i18n::I18n;
use crate::modes::{self, DecompressMode, DestinationMode, Target};
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
    /// The worker needs a destination directory (configured to ask per batch
    /// or per file). `batch` is true when the answer applies to the whole
    /// batch. `error` is set when a previously submitted path was invalid.
    NeedDestination {
        index: usize,
        archive: PathBuf,
        batch: bool,
        suggested: PathBuf,
        error: Option<String>,
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
    /// is the volume being read. `total == 0` means unknown. `speed_bps` is the
    /// smoothed write rate in bytes per second (`0` when not extracted yet).
    /// `eta_secs` is the estimated seconds left from the read rate (`0` when
    /// unknown).
    Progress {
        index: usize,
        fraction: f32,
        total: u64,
        part_index: usize,
        speed_bps: u64,
        eta_secs: u64,
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
    /// Directory chosen for a destination prompt.
    Destination(PathBuf),
}

/// Handle used by the UI to answer prompts and cancel the batch.
pub struct JobController {
    pub answer_tx: Sender<JobAnswer>,
    pub cancel_all: Arc<AtomicBool>,
    /// PID of each worker's `ouch` process, or 0 when that worker is idle. The
    /// UI terminates every non-zero PID when the user cancels or closes.
    pub current_pids: Vec<Arc<AtomicU32>>,
}

/// State the worker's steps share: where to report, how to ask the UI and how
/// to stop.
struct WorkerCtx<'a> {
    /// Events for the UI; every one is followed by a repaint request.
    events: &'a Sender<JobEvent>,
    /// egui context, used to wake the UI after each event.
    ui: &'a egui::Context,
    /// Answers from the UI. The lock is also the prompt gate: a worker holds it
    /// from emitting a prompt until the answer arrives, so the single prompt
    /// modal always belongs to the worker that receives the answer.
    answers: &'a Mutex<Receiver<JobAnswer>>,
    /// Set by the UI when the whole batch must stop.
    cancel: &'a Arc<AtomicBool>,
    /// Keeps the other workers behind the first archive until its password
    /// phase is resolved, so the first prompt is for the first queued file.
    startup_gate: Option<&'a StartupGate>,
    /// Batch configuration.
    config: &'a Config,
    /// Translations for the messages built here.
    i18n: &'a I18n,
}

/// Keeps the other workers behind the first archive until it has resolved its
/// password phase, so the first prompt belongs to the first queued archive.
struct StartupGate {
    open: Mutex<bool>,
    cv: Condvar,
}

impl StartupGate {
    fn new() -> Self {
        Self {
            open: Mutex::new(false),
            cv: Condvar::new(),
        }
    }

    /// Blocks until the gate opens or the batch is cancelled.
    fn wait(&self, cancel: &AtomicBool) {
        let mut open = self.open.lock().expect("startup gate poisoned");
        while !*open && !cancel.load(Ordering::Relaxed) {
            let (guard, _) = self
                .cv
                .wait_timeout(open, Duration::from_millis(100))
                .expect("startup gate poisoned");
            open = guard;
        }
    }

    fn open(&self) {
        *self.open.lock().expect("startup gate poisoned") = true;
        self.cv.notify_all();
    }

    fn guard(&self) -> StartupGateGuard<'_> {
        StartupGateGuard(self)
    }
}

/// Opens the gate when dropped, so an early return cannot leave workers stuck.
struct StartupGateGuard<'a>(&'a StartupGate);

impl Drop for StartupGateGuard<'_> {
    fn drop(&mut self) {
        self.0.open();
    }
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
    let workers = worker_count(&config, archives.len());
    let current_pids: Vec<Arc<AtomicU32>> =
        (0..workers).map(|_| Arc::new(AtomicU32::new(0))).collect();

    let controller = JobController {
        answer_tx,
        cancel_all: Arc::clone(&cancel_all),
        current_pids: current_pids.clone(),
    };

    thread::spawn(move || {
        let startup_gate = StartupGate::new();
        let answers = Mutex::new(answer_rx);
        let batch_password = Mutex::new(None);
        let last_destination = Mutex::new(None);
        let worker = WorkerCtx {
            events: &event_tx,
            ui: &ctx,
            answers: &answers,
            cancel: &cancel_all,
            startup_gate: Some(&startup_gate),
            config: &config,
            i18n: &i18n,
        };
        run(
            &ouch,
            &archives,
            &current_pids,
            &batch_password,
            &last_destination,
            &startup_gate,
            &worker,
        );
    });

    (controller, event_rx)
}

/// Number of worker threads for a batch: never more than the archives to
/// process, and never zero even for a hand-edited config.
fn worker_count(config: &Config, archives: usize) -> usize {
    let configured = usize::try_from(
        config
            .parallel_extractions
            .clamp(1, MAX_PARALLEL_EXTRACTIONS),
    )
    .unwrap_or(1);
    configured.min(archives.max(1))
}

/// Runs the whole batch with a pool of workers pulling archives from a shared
/// queue, so a slow archive does not hold up the rest.
fn run(
    ouch: &OuchClient,
    archives: &[PathBuf],
    pids: &[Arc<AtomicU32>],
    batch_password: &Mutex<Option<String>>,
    last_destination: &Mutex<Option<PathBuf>>,
    startup_gate: &StartupGate,
    worker: &WorkerCtx<'_>,
) {
    let event_tx = worker.events;
    let ui = worker.ui;
    let next = AtomicUsize::new(0);

    thread::scope(|scope| {
        for pid in pids {
            let next = &next;
            scope.spawn(move || loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(archive) = archives.get(index) else {
                    break;
                };
                // The first archive resolves its password alone, so the first
                // prompt is deterministic; the rest wait for its gate.
                if index != 0 {
                    startup_gate.wait(worker.cancel);
                }
                if worker.cancel.load(Ordering::Relaxed) {
                    break;
                }
                send(event_tx, ui, JobEvent::Started { index });

                let event = process_one(
                    ouch,
                    archive,
                    index,
                    batch_password,
                    last_destination,
                    pid,
                    worker,
                );
                send(event_tx, ui, event);
            });
        }
    });

    send(event_tx, ui, JobEvent::AllDone);
}

/// Extracts one archive and reports the result.
fn process_one(
    ouch: &OuchClient,
    archive: &Path,
    index: usize,
    batch_password: &Mutex<Option<String>>,
    last_destination: &Mutex<Option<PathBuf>>,
    current_pid: &Arc<AtomicU32>,
    worker: &WorkerCtx<'_>,
) -> JobEvent {
    let event_tx = worker.events;
    let ctx = worker.ui;
    let answer_rx = worker.answers;
    let cancel_all = worker.cancel;
    let config = worker.config;
    let i18n = worker.i18n;
    // The first archive owns the startup gate; the guard opens it on every
    // early return (missing file, missing parts, cancellation).
    let _startup_gate = if index == 0 {
        worker.startup_gate.map(StartupGate::guard)
    } else {
        None
    };
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
    let _progress = ProgressPoller::spawn(
        index,
        &cleanup_parts,
        Arc::clone(current_pid),
        event_tx,
        ctx,
    );

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
            let fraction = progress_math::fraction_of(copied, total);
            let _ = send(
                event_tx,
                ctx,
                JobEvent::Progress {
                    index,
                    fraction,
                    total,
                    part_index: 0,
                    speed_bps: 0,
                    eta_secs: 0,
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
    let source = absolute(prepared.path());
    let workdir = match resolve_workdir(archive, index, last_destination, worker) {
        Ok(workdir) => workdir,
        Err(message) => return fail(index, message),
    };
    log_line(
        event_tx,
        ctx,
        index,
        format!("destination: {}", workdir.display()),
    );

    let kind = formats::ArchiveKind::of_path(archive);

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

    // Single-file formats (gz, xz, zst, ...) hold one file and never need a
    // password; unpack straight into the destination directory. The
    // configured conflict policy still applies to the produced file.
    if !kind.is_archive() {
        if index == 0 {
            if let Some(gate) = worker.startup_gate {
                gate.open();
            }
        }
        let conflicts = detect_single_file_conflict(archive, &workdir);
        if let Err(message) = apply_conflict_policy(
            &conflicts, config, answer_rx, cancel_all, event_tx, ctx, index,
        ) {
            return fail(index, message);
        }
        let result = result_name(archive, kind, None);
        return match ouch.decompress(&source, &workdir, Target::Here, None, current_pid, &log) {
            Ok(outcome) if outcome.success => finish_success(
                index,
                archive,
                &cleanup_parts,
                "here",
                result.as_deref(),
                worker,
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
        DecompressMode::Folder | DecompressMode::Smart => Target::Folder,
        DecompressMode::Here => Target::Here,
    };

    match obtain_listing(
        ouch,
        &source,
        &workdir,
        index,
        batch_password,
        current_pid,
        worker,
    ) {
        Ok((text, found)) => {
            password = found;
            if config.decompress_mode == DecompressMode::Smart {
                let roots = modes::roots_from_listing(&text);
                target = modes::choose_target(DecompressMode::Smart, kind, &roots);
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
    if index == 0 {
        if let Some(gate) = worker.startup_gate {
            gate.open();
        }
    }

    // Handle existing destinations according to the configured policy.
    if let Some(text) = &listing {
        let conflicts = detect_conflicts(archive, target, text, &workdir);
        if let Err(message) = apply_conflict_policy(
            &conflicts, config, answer_rx, cancel_all, event_tx, ctx, index,
        ) {
            return fail(index, message);
        }
    }

    // When extracting "here" and the result name differs from the archive
    // (e.g. `foo.zip` containing `bar.ipa`), report the resulting name.
    let result = if target == Target::Here {
        result_name(archive, kind, listing.as_deref())
    } else {
        None
    };

    // Try passwords in a sensible order, then prompt only if the failure looks
    // password-related.
    let mut candidates: Vec<Option<String>> = Vec::new();
    if let Some(found) = password {
        candidates.push(Some(found));
    }
    let shared_password = batch_password
        .lock()
        .expect("password lock poisoned")
        .clone();
    let mut tried_shared = shared_password.clone();
    if let Some(batch) = shared_password {
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
            &source,
            &workdir,
            target,
            candidate.as_deref(),
            current_pid,
            &log,
        ) {
            Ok(outcome) if outcome.success => {
                if candidate.is_some() {
                    *batch_password.lock().expect("password lock poisoned") = candidate;
                }
                return finish_success(
                    index,
                    archive,
                    &cleanup_parts,
                    target.label(),
                    result.as_deref(),
                    worker,
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

    // Try a shared password again before showing another dialog: the old
    // candidate list may predate the password another worker just found.
    let attempt = |candidate: &str| match ouch.decompress(
        &source,
        &workdir,
        target,
        Some(candidate),
        current_pid,
        &log,
    ) {
        Ok(outcome) if outcome.success => Ok(()),
        Ok(outcome) => Err(outcome_message(&outcome)),
        Err(err) => Err(err.to_string()),
    };
    let succeed = || {
        finish_success(
            index,
            archive,
            &cleanup_parts,
            target.label(),
            result.as_deref(),
            worker,
        )
    };

    if !password_related {
        // A missing password can produce an error that never mentions
        // passwords, so retry a password shared after the candidates were
        // built before reporting a hard failure.
        if let Some(shared) = take_new_shared_password(batch_password, &mut tried_shared) {
            match attempt(&shared) {
                Ok(()) => return succeed(),
                Err(err) => {
                    last_error = err;
                    password_related = is_password_error(&last_error);
                }
            }
        }
        if !password_related {
            return fail(index, last_error);
        }
    }

    // Ask the user, retrying until a password works or the user skips/cancels.
    // The prompt turn stays locked while the password is tested, so an invalid
    // one never reaches the rest of the batch.
    let mut first_prompt = true;
    loop {
        if cancel_all.load(Ordering::Relaxed) {
            return fail(index, "cancelled".into());
        }

        // Another worker may have shared a validated password while this one
        // was extracting; try it before showing another dialog.
        if let Some(shared) = take_new_shared_password(batch_password, &mut tried_shared) {
            match attempt(&shared) {
                Ok(()) => return succeed(),
                Err(err) => {
                    // A shared password is only one more guess; if it fails,
                    // ask the user instead of giving up. A wrong password can
                    // produce an error that never mentions passwords.
                    last_error = err;
                    continue;
                }
            }
        }

        let answers = answer_rx.lock().expect("prompt channel poisoned");
        if cancel_all.load(Ordering::Relaxed) {
            return fail(index, "cancelled".into());
        }
        // The shared password may have changed while this worker waited for
        // its prompt turn; re-check it before asking.
        if has_new_shared_password(batch_password, &tried_shared) {
            drop(answers);
            continue;
        }

        loop {
            // A validated password may have been shared while this worker was
            // re-prompting; use it instead of asking again.
            if let Some(shared) = take_new_shared_password(batch_password, &mut tried_shared) {
                match attempt(&shared) {
                    Ok(()) => return succeed(),
                    Err(err) => last_error = err,
                }
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
            match answers.recv().unwrap_or(JobAnswer::CancelBatch) {
                JobAnswer::Password(password) => {
                    // Validate while holding the turn: a wrong password must
                    // not be shared with the rest of the batch.
                    log_line(
                        event_tx,
                        ctx,
                        index,
                        "checking password with a trial extraction".into(),
                    );
                    match validate_password(ouch, &source, &workdir, &password, index, current_pid)
                    {
                        Ok(()) => {
                            if cancel_all.load(Ordering::Relaxed) {
                                return fail(index, "cancelled".into());
                            }
                            *batch_password.lock().expect("password lock poisoned") =
                                Some(password.clone());
                            match attempt(&password) {
                                Ok(()) => {
                                    if cancel_all.load(Ordering::Relaxed) {
                                        return fail(index, "cancelled".into());
                                    }
                                    drop(answers);
                                    return succeed();
                                }
                                Err(err) => {
                                    last_error = err;
                                    if !is_password_error(&last_error) {
                                        return fail(index, last_error);
                                    }
                                    if cancel_all.load(Ordering::Relaxed) {
                                        return fail(index, "cancelled".into());
                                    }
                                    // A validated password that still fails to
                                    // extract is reported through the prompt.
                                }
                            }
                        }
                        Err(err) => {
                            if cancel_all.load(Ordering::Relaxed) {
                                return fail(index, "cancelled".into());
                            }
                            last_error = err;
                            if !is_password_error(&last_error) {
                                return fail(index, last_error);
                            }
                            // Keep the turn and ask again.
                        }
                    }
                }
                JobAnswer::Skip => return fail(index, "skipped".into()),
                JobAnswer::CancelBatch => return fail(index, "cancelled".into()),
                JobAnswer::Overwrite | JobAnswer::Destination(_) => {}
            }
        }
    }
}

/// Resolves the base directory used for this archive and creates it when needed.
fn resolve_workdir(
    archive: &Path,
    index: usize,
    last_destination: &Mutex<Option<PathBuf>>,
    worker: &WorkerCtx<'_>,
) -> Result<PathBuf, String> {
    let config = worker.config;
    let previous = last_destination
        .lock()
        .expect("destination lock poisoned")
        .clone();
    let suggested = previous.unwrap_or_else(|| absolute(&parent_dir(archive)));
    let dir = match config.destination_mode {
        DestinationMode::ArchiveFolder => return Ok(suggested),
        DestinationMode::Fixed => match config.destination_dir.as_deref() {
            Some(dir) if !dir.as_os_str().is_empty() => absolute(dir),
            _ => return Err("no destination folder configured in settings".into()),
        },
        DestinationMode::AskBatch => {
            return prompt_destination(
                archive,
                PromptScope::Batch,
                suggested,
                index,
                last_destination,
                worker,
            );
        }
        DestinationMode::AskEach => {
            return prompt_destination(
                archive,
                PromptScope::Single,
                suggested,
                index,
                last_destination,
                worker,
            );
        }
    };
    ensure_directory(dir)
}

/// Whether a destination prompt applies to the whole batch or one archive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PromptScope {
    /// The answer is reused for the remaining archives.
    Batch,
    /// The answer applies to this archive only.
    Single,
}

/// Blocks the worker until the UI supplies a destination directory.
fn prompt_destination(
    archive: &Path,
    scope: PromptScope,
    suggested: PathBuf,
    index: usize,
    last_destination: &Mutex<Option<PathBuf>>,
    worker: &WorkerCtx<'_>,
) -> Result<PathBuf, String> {
    let event_tx = worker.events;
    let ctx = worker.ui;
    let cancel_all = worker.cancel;
    let mut suggested = suggested;
    let mut last_error: Option<String> = None;

    // Serialize prompts, then re-check the batch destination: another worker
    // may have answered while this one was waiting for its turn.
    let answers = worker.answers.lock().expect("prompt channel poisoned");
    if cancel_all.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    if scope == PromptScope::Batch {
        let chosen = last_destination
            .lock()
            .expect("destination lock poisoned")
            .clone();
        if let Some(dir) = chosen {
            return Ok(dir);
        }
    }

    loop {
        if cancel_all.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let _ = send(
            event_tx,
            ctx,
            JobEvent::NeedDestination {
                index,
                archive: archive.to_path_buf(),
                batch: scope == PromptScope::Batch,
                suggested: suggested.clone(),
                error: last_error.take(),
            },
        );
        match answers.recv().unwrap_or(JobAnswer::CancelBatch) {
            JobAnswer::Destination(path) => {
                if path.as_os_str().is_empty() {
                    last_error = Some("the destination folder is empty".into());
                    continue;
                }
                let dir = absolute(&path);
                match ensure_directory(dir.clone()) {
                    Ok(dir) => {
                        *last_destination.lock().expect("destination lock poisoned") =
                            Some(dir.clone());
                        return Ok(dir);
                    }
                    Err(err) => {
                        suggested = dir;
                        last_error = Some(err);
                    }
                }
            }
            JobAnswer::Skip => return Err("skipped".into()),
            JobAnswer::CancelBatch => return Err("cancelled".into()),
            JobAnswer::Password(_) | JobAnswer::Overwrite => {}
        }
    }
}

/// Creates `dir` if needed, then confirms it is a directory.
fn ensure_directory(dir: PathBuf) -> Result<PathBuf, String> {
    std::fs::create_dir_all(&dir).map_err(|err| format!("creating {}: {err}", dir.display()))?;
    if dir.is_dir() {
        Ok(dir)
    } else {
        Err(format!("not a directory: {}", dir.display()))
    }
}

/// Makes a path absolute without resolving symlinks, so it keeps working after
/// `ouch` changes its working directory.
fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Directory that contains the archive (the default extraction destination).
fn parent_dir(archive: &Path) -> PathBuf {
    archive
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
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
fn detect_conflicts(archive: &Path, target: Target, listing: &str, workdir: &Path) -> Vec<PathBuf> {
    match target {
        Target::Folder => {
            let stem = formats::stem_without_known_extensions(archive);
            // A hidden archive (`.zip`) or a non-UTF-8 name yields an empty
            // stem; joining it would make the workdir itself the destination and
            // could rename a whole directory, so bail out.
            if stem.is_empty() {
                return Vec::new();
            }
            let destination = workdir.join(stem);
            let non_empty = destination
                .read_dir()
                .is_ok_and(|mut entries| entries.next().is_some());
            if destination.is_dir() && non_empty {
                vec![destination]
            } else {
                Vec::new()
            }
        }
        Target::Here => modes::roots_from_listing(listing)
            .into_iter()
            .filter(|root| is_safe_relative_component(root))
            .map(|root| workdir.join(root))
            .filter(|path| path.exists())
            .collect(),
    }
}

/// Returns the output file of a single-file format when it already exists.
fn detect_single_file_conflict(archive: &Path, workdir: &Path) -> Vec<PathBuf> {
    let stem = formats::stem_without_known_extensions(archive);
    if stem.is_empty() {
        return Vec::new();
    }
    let destination = workdir.join(stem);
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
    answers: &Mutex<Receiver<JobAnswer>>,
    cancel: &Arc<AtomicBool>,
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
            let answers = answers.lock().expect("prompt channel poisoned");
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let _ = send(
                event_tx,
                ctx,
                JobEvent::NeedOverwrite {
                    index,
                    conflicts: conflicts.to_vec(),
                },
            );
            loop {
                match answers.recv().unwrap_or(JobAnswer::CancelBatch) {
                    JobAnswer::Overwrite => return Ok(()),
                    JobAnswer::Skip => return Err("skipped".into()),
                    JobAnswer::CancelBatch => return Err("cancelled".into()),
                    JobAnswer::Password(_) | JobAnswer::Destination(_) => {}
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
fn finish_success(
    index: usize,
    archive: &Path,
    parts: &[PathBuf],
    target_label: &str,
    result: Option<&str>,
    worker: &WorkerCtx<'_>,
) -> JobEvent {
    let cleanup_error = cleanup_sources(parts, worker.config, worker.events, worker.ui, index);

    let archive_name = archive
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let result_summary = result.map(|result| format!("{archive_name} → {result}"));
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
fn result_name(archive: &Path, kind: ArchiveKind, listing: Option<&str>) -> Option<String> {
    let archive_stem = formats::stem_without_known_extensions(archive);
    let name = if kind.is_archive() {
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
    if outcome.stderr.trim().is_empty() {
        outcome.stdout.trim().to_string()
    } else {
        outcome.stderr.trim().to_string()
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

/// Parses the `wchar` field (bytes written via syscalls) from `/proc/<pid>/io`.
fn parse_wchar(contents: &str) -> Option<u64> {
    contents
        .lines()
        .find_map(|line| line.strip_prefix("wchar:")?.trim().parse::<u64>().ok())
}

/// Bytes read and written by the given process so far, from `/proc/<pid>/io`.
fn process_io(pid: u32) -> Option<(u64, u64)> {
    let contents = std::fs::read_to_string(format!("/proc/{pid}/io")).ok()?;
    let read = parse_rchar(&contents)?;
    let write = parse_wchar(&contents).unwrap_or(0);
    Some((read, write))
}

/// Time constant (in seconds) of the write-speed average. Bigger values make
/// the displayed speed steadier and less twitchy.
const SPEED_TAU: f64 = 3.0;

/// Lossy numeric conversions for the progress display.
///
/// `ouch` exposes raw byte counters and the UI only renders them, so the
/// rounding and saturation rules of `as` are acceptable here. Keeping them in
/// one module keeps the casts out of the poller logic.
#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "progress figures are display-only estimates"
)]
mod progress_math {
    /// `part / whole`, clamped to `0.0..=1.0` (`0.0` when `whole` is zero).
    pub fn fraction_of(part: u64, whole: u64) -> f32 {
        if whole == 0 {
            0.0
        } else {
            (part as f64 / whole as f64).min(1.0) as f32
        }
    }

    /// Bytes per second for a sample of `bytes` taken over `secs` seconds.
    pub fn per_second(bytes: u64, secs: f64) -> f64 {
        bytes as f64 / secs
    }

    /// Full seconds needed for `remaining` bytes at `rate` bytes per second.
    pub fn eta_secs(remaining: u64, rate: f64) -> Option<u64> {
        (rate > 0.0).then(|| (remaining as f64 / rate) as u64)
    }

    /// Truncates a fractional rate to the whole number shown in the UI.
    pub fn trunc_rate(rate: f64) -> u64 {
        rate.max(0.0) as u64
    }
}

/// Exponential moving average of a transfer rate, weighted by the elapsed time
/// so it does not depend on how often it is sampled. `previous` is `None` on
/// the first sample, so the rate starts at the instantaneous value.
fn smooth_speed(previous: Option<f64>, delta_bytes: u64, secs: f64) -> f64 {
    if secs <= 0.0 {
        return previous.unwrap_or(0.0);
    }
    let instant = progress_math::per_second(delta_bytes, secs);
    match previous {
        Some(prev) => {
            let alpha = 1.0 - (-secs / SPEED_TAU).exp();
            alpha * instant + (1.0 - alpha) * prev
        }
        None => instant,
    }
}

/// Estimated seconds left for `remaining` bytes at `rate` bytes per second.
/// Returns `None` when the rate is not positive.
fn estimate_eta(remaining: u64, rate: f64) -> Option<u64> {
    progress_math::eta_secs(remaining, rate)
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
            total += std::fs::metadata(part).map_or(0, |metadata| metadata.len());
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
            speed_bps: 0,
            eta_secs: 0,
        });
        ctx.request_repaint();
        let (stop_tx, stop_rx) = mpsc::channel();
        let event_tx = event_tx.clone();
        let ctx = ctx.clone();
        let handle = thread::spawn(move || {
            run_progress_poller(
                index,
                total,
                &cumulative,
                &pid_slot,
                &event_tx,
                &ctx,
                &stop_rx,
            );
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
    cumulative: &[u64],
    pid_slot: &Arc<AtomicU32>,
    event_tx: &Sender<JobEvent>,
    ctx: &egui::Context,
    stop_rx: &Receiver<()>,
) {
    // Speed/ETA tracking. Both rates are averaged over a ~1 s window and only
    // refreshed that often, so the numbers stay readable. Reset whenever the
    // child changes (a new attempt after a wrong password starts a fresh
    // `/proc/<pid>/io` counter).
    let mut last_pid = 0u32;
    let mut last_speed_read = 0u64;
    let mut last_speed_write = 0u64;
    let mut last_speed_time = std::time::Instant::now();
    let mut read_speed: Option<f64> = None;
    let mut write_speed: Option<f64> = None;
    loop {
        let pid = pid_slot.load(Ordering::Relaxed);
        if pid != 0 {
            if let Some((read, write)) = process_io(pid) {
                let now = std::time::Instant::now();
                if pid == last_pid {
                    let window = now.duration_since(last_speed_time).as_secs_f64();
                    if window >= 1.0 {
                        read_speed = Some(smooth_speed(
                            read_speed,
                            read.saturating_sub(last_speed_read),
                            window,
                        ));
                        write_speed = Some(smooth_speed(
                            write_speed,
                            write.saturating_sub(last_speed_write),
                            window,
                        ));
                        last_speed_read = read;
                        last_speed_write = write;
                        last_speed_time = now;
                    }
                } else {
                    last_pid = pid;
                    last_speed_read = read;
                    last_speed_write = write;
                    last_speed_time = now;
                    read_speed = None;
                    write_speed = None;
                }
                let fraction = progress_math::fraction_of(read, total);
                let part_index = part_index_for(read, cumulative);
                let eta_secs = estimate_eta(total.saturating_sub(read), read_speed.unwrap_or(0.0))
                    .unwrap_or(0);
                let _ = event_tx.send(JobEvent::Progress {
                    index,
                    fraction,
                    total,
                    part_index,
                    speed_bps: progress_math::trunc_rate(write_speed.unwrap_or(0.0)),
                    eta_secs,
                });
                ctx.request_repaint();
            }
        }
        match stop_rx.recv_timeout(Duration::from_millis(80)) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
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

/// True when the batch password is set and this archive has not tried it yet.
fn has_new_shared_password(batch_password: &Mutex<Option<String>>, tried: &Option<String>) -> bool {
    batch_password
        .lock()
        .expect("password lock poisoned")
        .as_deref()
        .is_some_and(|shared| tried.as_deref() != Some(shared))
}

/// Returns the batch password when this archive has not tried it yet, marking
/// it as tried so a failed attempt does not repeat on every retry.
fn take_new_shared_password(
    batch_password: &Mutex<Option<String>>,
    tried: &mut Option<String>,
) -> Option<String> {
    let shared = batch_password
        .lock()
        .expect("password lock poisoned")
        .clone()?;
    if tried.as_deref() == Some(shared.as_str()) {
        return None;
    }
    *tried = Some(shared.clone());
    Some(shared)
}

/// Extracts `archive` into a throwaway directory to check that `password`
/// decrypts its data, then removes the directory.
///
/// `ouch list` cannot validate formats whose file names are not encrypted, so
/// a real extraction is the only reliable check before the destination is
/// touched.
fn validate_password(
    ouch: &OuchClient,
    archive: &Path,
    workdir: &Path,
    password: &str,
    index: usize,
    pid: &AtomicU32,
) -> Result<(), String> {
    let temp = workdir.join(format!(".ouch-decompress-gui-validate-{index}"));
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(&temp).map_err(|err| format!("creating {}: {err}", temp.display()))?;
    let _cleanup = TempDir(temp.clone());

    let log = |_line: &str| {};
    match ouch.decompress(archive, &temp, Target::Here, Some(password), pid, &log) {
        Ok(outcome) if outcome.success => Ok(()),
        Ok(outcome) => Err(outcome_message(&outcome)),
        Err(err) => Err(err.to_string()),
    }
}

/// Removes a throwaway directory when it goes out of scope.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Tries to list an archive using, in order: no password, the batch password,
/// then every saved password. If a password is required — or `ouch` crashes,
/// which may hide an encrypted archive — it asks the user until a password
/// works or the user skips/cancels. Other failures are returned as
/// [`SkipReason::HardError`] so the caller never asks for a password the
/// archive does not have.
fn obtain_listing(
    ouch: &OuchClient,
    archive: &Path,
    workdir: &Path,
    index: usize,
    batch_password: &Mutex<Option<String>>,
    pid: &AtomicU32,
    worker: &WorkerCtx<'_>,
) -> Result<(String, Option<String>), SkipReason> {
    let event_tx = worker.events;
    let ctx = worker.ui;
    let answer_rx = worker.answers;
    let cancel_all = worker.cancel;
    let config = worker.config;
    let log = |line: &str| log_line(event_tx, ctx, index, line.to_string());
    let mut candidates: Vec<Option<String>> = vec![None];
    let shared_password = batch_password
        .lock()
        .expect("password lock poisoned")
        .clone();
    let mut tried_shared = shared_password.clone();
    if let Some(batch) = shared_password {
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
            Ok(listing) => return Ok((listing, candidate)),
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
        // The candidate list may predate a password shared by another worker,
        // and a missing password can produce an error that never mentions
        // passwords. Try the shared one before reporting a hard failure.
        if let Some(shared) = take_new_shared_password(batch_password, &mut tried_shared) {
            match ouch.list(archive, Some(&shared), &log) {
                Ok(listing) => return Ok((listing, Some(shared))),
                Err(err) => {
                    last_error = err.to_string();
                    password_related = is_password_error(&last_error);
                }
            }
        }
        if !password_related && !looks_like_crash(&last_error) {
            return Err(SkipReason::HardError(last_error));
        }
    }

    let mut first_prompt = true;
    loop {
        if cancel_all.load(Ordering::Relaxed) {
            return Err(SkipReason::BatchCancelled);
        }
        // Another worker may have shared a validated password while this one
        // was trying its own candidates; test it before asking the user again.
        if let Some(shared) = take_new_shared_password(batch_password, &mut tried_shared) {
            match ouch.list(archive, Some(&shared), &log) {
                Ok(listing) => return Ok((listing, Some(shared))),
                Err(err) => {
                    // A shared password is only one more guess; if it fails,
                    // ask the user instead of giving up. A wrong password can
                    // produce an error that never mentions passwords.
                    last_error = err.to_string();
                    continue;
                }
            }
        }

        let answers = answer_rx.lock().expect("prompt channel poisoned");
        if cancel_all.load(Ordering::Relaxed) {
            return Err(SkipReason::BatchCancelled);
        }
        // The shared password may have changed while this worker waited for
        // its prompt turn; re-check it before asking.
        if has_new_shared_password(batch_password, &tried_shared) {
            drop(answers);
            continue;
        }

        loop {
            // A validated password may have been shared while this worker was
            // re-prompting; use it instead of asking again.
            if let Some(shared) = take_new_shared_password(batch_password, &mut tried_shared) {
                match ouch.list(archive, Some(&shared), &log) {
                    Ok(listing) => return Ok((listing, Some(shared))),
                    Err(err) => last_error = err.to_string(),
                }
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
            match answers.recv().unwrap_or(JobAnswer::CancelBatch) {
                JobAnswer::Password(password) => {
                    // Validate while holding the turn: a wrong password must
                    // not be shared with the rest of the batch.
                    match ouch.list(archive, Some(&password), &log) {
                        Ok(listing) => {
                            if cancel_all.load(Ordering::Relaxed) {
                                return Err(SkipReason::BatchCancelled);
                            }
                            log_line(
                                event_tx,
                                ctx,
                                index,
                                "checking password with a trial extraction".into(),
                            );
                            match validate_password(ouch, archive, workdir, &password, index, pid) {
                                Ok(()) => {
                                    if cancel_all.load(Ordering::Relaxed) {
                                        return Err(SkipReason::BatchCancelled);
                                    }
                                    *batch_password.lock().expect("password lock poisoned") =
                                        Some(password.clone());
                                    drop(answers);
                                    return Ok((listing, Some(password)));
                                }
                                Err(err) => {
                                    if cancel_all.load(Ordering::Relaxed) {
                                        return Err(SkipReason::BatchCancelled);
                                    }
                                    last_error = err;
                                    if !is_password_error(&last_error) {
                                        return Err(SkipReason::HardError(last_error));
                                    }
                                    // Keep the turn and ask again.
                                }
                            }
                        }
                        Err(err) => {
                            if cancel_all.load(Ordering::Relaxed) {
                                return Err(SkipReason::BatchCancelled);
                            }
                            last_error = err.to_string();
                            if !is_password_error(&last_error) {
                                return Err(SkipReason::HardError(last_error));
                            }
                            // Keep the turn and ask again.
                        }
                    }
                }
                JobAnswer::Skip => return Err(SkipReason::Cancelled),
                JobAnswer::CancelBatch => return Err(SkipReason::BatchCancelled),
                JobAnswer::Overwrite | JobAnswer::Destination(_) => {}
            }
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
    fn clamps_the_parallel_worker_count() {
        let config = Config {
            parallel_extractions: 0,
            ..Config::default()
        };
        assert_eq!(worker_count(&config, 5), 1, "zero workers is invalid");

        let config = Config {
            parallel_extractions: 3,
            ..Config::default()
        };
        assert_eq!(worker_count(&config, 10), 3);
        assert_eq!(
            worker_count(&config, 2),
            2,
            "one worker per archive at most"
        );

        let config = Config {
            parallel_extractions: 999,
            ..Config::default()
        };
        assert_eq!(worker_count(&config, 100), 16);
    }

    #[test]
    fn parallel_batch_extracts_every_archive() {
        let Ok(ouch) = OuchClient::discover() else {
            eprintln!("skipping: ouch binary not found");
            return;
        };
        let base = unique_dir("parallel-batch");
        let first_dir = base.join("one");
        let second_dir = base.join("two");
        std::fs::create_dir_all(&first_dir).unwrap();
        std::fs::create_dir_all(&second_dir).unwrap();
        let first = build_zip(&ouch, &first_dir);
        let second = build_zip(&ouch, &second_dir);

        let config = Config {
            parallel_extractions: 2,
            after_extract: AfterExtract::Keep,
            ..Config::default()
        };
        let (_controller, events) = spawn(
            ouch,
            vec![first, second],
            config,
            I18n::new(crate::i18n::Language::En),
            egui::Context::default(),
        );

        let mut done = 0;
        loop {
            match events.recv_timeout(Duration::from_secs(30)) {
                Ok(JobEvent::Done { success, .. }) => done += usize::from(success),
                Ok(JobEvent::AllDone) | Err(_) => break,
                Ok(_) => {}
            }
        }
        assert_eq!(done, 2, "both archives should be extracted");
        let _ = std::fs::remove_dir_all(&base);
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
    fn parses_wchar_from_proc_io() {
        assert_eq!(parse_wchar("rchar: 12345\nwchar: 678\n"), Some(678));
        assert_eq!(parse_wchar("rchar: 1\nsyscw: 2\n"), None);
        assert_eq!(parse_wchar(""), None);
    }

    #[test]
    fn smooths_write_speed() {
        // First sample is instantaneous.
        let instant = smooth_speed(None, 1_000_000, 1.0);
        assert!((instant - 1_000_000.0).abs() < f64::EPSILON);
        // Later samples move only partway toward the new rate.
        let smoothed = smooth_speed(Some(1_000_000.0), 2_000_000, 1.0);
        let alpha = 1.0 - (-1.0_f64 / SPEED_TAU).exp();
        let expected = alpha * 2_000_000.0 + (1.0 - alpha) * 1_000_000.0;
        assert!((smoothed - expected).abs() < 1.0);
        assert!(smoothed > 1_000_000.0 && smoothed < 2_000_000.0);
        // A zero interval keeps the previous value.
        let kept = smooth_speed(Some(42.0), 999, 0.0);
        assert!((kept - 42.0).abs() < f64::EPSILON);
    }

    #[test]
    fn estimates_eta_from_remaining_bytes_and_rate() {
        assert_eq!(estimate_eta(0, 1_000_000.0), Some(0));
        assert_eq!(estimate_eta(1_000_000, 1_000_000.0), Some(1));
        assert_eq!(estimate_eta(3_000_000, 1_000_000.0), Some(3));
        // A non-positive rate means the ETA is unknown.
        assert_eq!(estimate_eta(1_000_000, 0.0), None);
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
        assert!(detect_conflicts(&archive, Target::Folder, "", &base).is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn detects_single_file_output_conflict() {
        let base = unique_dir("single");
        let archive = base.join("note.txt.gz");
        std::fs::write(&archive, b"x").unwrap();
        assert!(detect_single_file_conflict(&archive, &base).is_empty());
        std::fs::write(base.join("note.txt"), b"x").unwrap();
        assert_eq!(
            detect_single_file_conflict(&archive, &base),
            vec![base.join("note.txt")]
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn fixed_destination_is_created() {
        let base = unique_dir("destination");
        let archive = base.join("archive.zip");
        std::fs::write(&archive, b"x").unwrap();
        let target = base.join("out/nested");
        let config = Config {
            destination_mode: DestinationMode::Fixed,
            destination_dir: Some(target.clone()),
            ..Config::default()
        };
        let (_answer_tx, answer_rx) = mpsc::channel();
        let answers = Mutex::new(answer_rx);
        let (event_tx, _event_rx) = mpsc::channel();
        let ctx = egui::Context::default();
        let cancel = Arc::new(AtomicBool::new(false));
        let last_destination = Mutex::new(None);

        let i18n = I18n::new(config.language);
        let worker = test_worker(&config, &i18n, &event_tx, &answers, &cancel, &ctx);
        let dir = resolve_workdir(&archive, 0, &last_destination, &worker).unwrap();

        assert_eq!(dir, target);
        assert!(target.is_dir());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn destination_prompt_uses_submitted_folder() {
        let base = unique_dir("destination-prompt");
        let archive = base.join("archive.zip");
        std::fs::write(&archive, b"x").unwrap();
        let target = base.join("chosen");
        let (answer_tx, answer_rx) = mpsc::channel();
        answer_tx
            .send(JobAnswer::Destination(target.clone()))
            .unwrap();
        let answers = Mutex::new(answer_rx);
        let (event_tx, event_rx) = mpsc::channel();
        let ctx = egui::Context::default();
        let cancel = Arc::new(AtomicBool::new(false));
        let last_destination = Mutex::new(None);

        let config = Config::default();
        let i18n = I18n::new(config.language);
        let worker = test_worker(&config, &i18n, &event_tx, &answers, &cancel, &ctx);
        let dir = prompt_destination(
            &archive,
            PromptScope::Single,
            base.clone(),
            0,
            &last_destination,
            &worker,
        )
        .unwrap();

        assert_eq!(dir, target);
        assert!(target.is_dir());
        assert!(matches!(
            event_rx.try_recv(),
            Ok(JobEvent::NeedDestination { batch: false, .. })
        ));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn archive_folder_destination_uses_archive_parent() {
        let base = unique_dir("destination-default");
        let archive = base.join("archive.zip");
        std::fs::write(&archive, b"x").unwrap();
        let config = Config::default();
        let (_answer_tx, answer_rx) = mpsc::channel();
        let answers = Mutex::new(answer_rx);
        let (event_tx, _event_rx) = mpsc::channel();
        let ctx = egui::Context::default();
        let cancel = Arc::new(AtomicBool::new(false));
        let last_destination = Mutex::new(None);

        let i18n = I18n::new(config.language);
        let worker = test_worker(&config, &i18n, &event_tx, &answers, &cancel, &ctx);
        let dir = resolve_workdir(&archive, 0, &last_destination, &worker).unwrap();

        assert_eq!(dir, base);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn ask_each_reuses_the_previous_destination() {
        let base = unique_dir("ask-each-previous");
        let first = base.join("one/archive.zip");
        let second = base.join("two/archive.zip");
        std::fs::create_dir_all(first.parent().unwrap()).unwrap();
        std::fs::create_dir_all(second.parent().unwrap()).unwrap();
        std::fs::write(&first, b"x").unwrap();
        std::fs::write(&second, b"x").unwrap();
        let chosen = base.join("chosen");
        let config = Config {
            destination_mode: DestinationMode::AskEach,
            ..Config::default()
        };
        let (answer_tx, answer_rx) = mpsc::channel();
        let answers = Mutex::new(answer_rx);
        let (event_tx, event_rx) = mpsc::channel();
        let ctx = egui::Context::default();
        let cancel = Arc::new(AtomicBool::new(false));
        let last_destination = Mutex::new(None);

        answer_tx
            .send(JobAnswer::Destination(chosen.clone()))
            .unwrap();
        let i18n = I18n::new(config.language);
        let worker = test_worker(&config, &i18n, &event_tx, &answers, &cancel, &ctx);
        let dir = resolve_workdir(&first, 0, &last_destination, &worker).unwrap();
        assert_eq!(dir, chosen);
        let JobEvent::NeedDestination { suggested, .. } = event_rx.try_recv().unwrap() else {
            panic!("expected a destination request");
        };
        assert_eq!(suggested, first.parent().unwrap());

        answer_tx
            .send(JobAnswer::Destination(chosen.clone()))
            .unwrap();
        let dir = resolve_workdir(&second, 1, &last_destination, &worker).unwrap();
        assert_eq!(dir, chosen);
        let JobEvent::NeedDestination { suggested, .. } = event_rx.try_recv().unwrap() else {
            panic!("expected a destination request");
        };
        assert_eq!(suggested, chosen);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn detects_renamed_result_when_extracting_here() {
        // Obvious: result only changes the extension of the same stem.
        assert_eq!(
            result_name(
                Path::new("/tmp/PatreonTV-unsigned.zip"),
                ArchiveKind::Archive,
                Some("PatreonTV-unsigned.ipa\n")
            ),
            None
        );
        // Obvious: `.tar.gz` -> the stem.
        assert_eq!(
            result_name(
                Path::new("/tmp/crispasr-linux-x86_64-hip.tar.gz"),
                ArchiveKind::Archive,
                Some("crispasr-linux-x86_64-hip\n")
            ),
            None
        );
        // Single-file streams simply drop the compression extension.
        assert_eq!(
            result_name(Path::new("/tmp/report.txt.gz"), ArchiveKind::Stream, None),
            None
        );
        assert_eq!(
            result_name(Path::new("/tmp/file.gz"), ArchiveKind::Stream, None),
            None
        );
        // Not obvious: the root has a genuinely different name.
        assert_eq!(
            result_name(
                Path::new("/tmp/photos.zip"),
                ArchiveKind::Archive,
                Some("vacation/\n")
            ),
            Some("vacation".to_string())
        );
        // Multiple roots are extracted into a folder named after the archive.
        assert_eq!(
            result_name(
                Path::new("/tmp/a.zip"),
                ArchiveKind::Archive,
                Some("x\ny\n")
            ),
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

    /// Builds an encrypted 7z fixture with data encryption but plain file names,
    /// the case where `ouch list` accepts a wrong password.
    fn build_encrypted_7z(base: &Path, folder: &str) -> PathBuf {
        let out = base.join(folder).join("out");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("secret.txt"), b"secret").unwrap();
        let archive = out.join("secret.7z");
        assert!(
            Command::new("7z")
                .current_dir(&out)
                .args([
                    "a",
                    "-y",
                    "-phunter2",
                    "-mhe=off",
                    "secret.7z",
                    "secret.txt"
                ])
                .status()
                .unwrap()
                .success(),
            "could not create the encrypted 7z"
        );
        archive
    }

    /// Wires a worker context around the locals a test already owns.
    fn test_worker<'a>(
        config: &'a Config,
        i18n: &'a I18n,
        events: &'a Sender<JobEvent>,
        answers: &'a Mutex<Receiver<JobAnswer>>,
        cancel: &'a Arc<AtomicBool>,
        ui: &'a egui::Context,
    ) -> WorkerCtx<'a> {
        WorkerCtx {
            events,
            ui,
            answers,
            cancel,
            startup_gate: None,
            config,
            i18n,
        }
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

        for answer in answers {
            let _ = answer_tx.send(answer.clone());
        }

        let answers = Mutex::new(answer_rx);
        let batch_password = Mutex::new(None);
        let last_destination = Mutex::new(None);
        let ctx = egui::Context::default();

        let current_pid = Arc::new(AtomicU32::new(0));
        let cancel_all = Arc::new(AtomicBool::new(false));
        let i18n = I18n::new(crate::i18n::Language::En);
        let worker = WorkerCtx {
            events: &event_tx,
            ui: &ctx,
            answers: &answers,
            cancel: &cancel_all,
            startup_gate: None,
            config,
            i18n: &i18n,
        };
        let event = process_one(
            ouch,
            archive,
            0,
            &batch_password,
            &last_destination,
            &current_pid,
            &worker,
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
    fn tracks_new_shared_passwords() {
        let shared = Mutex::new(None);
        let mut tried = None;
        assert!(!has_new_shared_password(&shared, &tried));
        assert_eq!(take_new_shared_password(&shared, &mut tried), None);

        *shared.lock().unwrap() = Some("one".into());
        assert!(has_new_shared_password(&shared, &tried));
        assert_eq!(
            take_new_shared_password(&shared, &mut tried).as_deref(),
            Some("one")
        );
        assert!(!has_new_shared_password(&shared, &tried));
        assert_eq!(take_new_shared_password(&shared, &mut tried), None);

        *shared.lock().unwrap() = Some("two".into());
        assert!(has_new_shared_password(&shared, &tried));
        assert_eq!(
            take_new_shared_password(&shared, &mut tried).as_deref(),
            Some("two")
        );
    }

    #[test]
    fn validation_rejects_a_wrong_password_before_extracting() {
        let Ok(ouch) = OuchClient::discover() else {
            eprintln!("skipping: ouch binary not found");
            return;
        };
        if Command::new("zip").arg("--version").output().is_err() {
            eprintln!("skipping: `zip` tool not found");
            return;
        }

        let base = unique_dir("validate-password");
        let out = base.join("out");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("secret.txt"), b"secret").unwrap();
        let archive = out.join("secret.zip");
        assert!(
            Command::new("zip")
                .current_dir(&out)
                .args(["-q", "-P", "hunter2", "secret.zip", "secret.txt"])
                .status()
                .unwrap()
                .success(),
            "could not create the encrypted zip"
        );
        let workdir = base.join("extract");
        std::fs::create_dir_all(&workdir).unwrap();
        let pid = AtomicU32::new(0);

        let err = validate_password(&ouch, &archive, &workdir, "wrong", 0, &pid)
            .expect_err("a wrong password must fail validation");
        assert!(is_password_error(&err), "unexpected error: {err}");
        assert!(
            std::fs::read_dir(&workdir).unwrap().next().is_none(),
            "a failed validation must not leave files in the destination"
        );

        validate_password(&ouch, &archive, &workdir, "hunter2", 0, &pid)
            .expect("the correct password should pass validation");
        assert!(
            std::fs::read_dir(&workdir).unwrap().next().is_none(),
            "validation must remove its temporary directory"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn validation_catches_a_wrong_password_for_seven_zip() {
        let Ok(ouch) = OuchClient::discover() else {
            eprintln!("skipping: ouch binary not found");
            return;
        };
        if Command::new("7z").arg("--help").output().is_err() {
            eprintln!("skipping: `7z` tool not found");
            return;
        }

        let base = unique_dir("validate-7z");
        let out = base.join("out");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("secret.txt"), b"secret").unwrap();
        let archive = out.join("secret.7z");
        assert!(
            Command::new("7z")
                .current_dir(&out)
                .args([
                    "a",
                    "-y",
                    "-phunter2",
                    "-mhe=off",
                    "secret.7z",
                    "secret.txt"
                ])
                .status()
                .unwrap()
                .success(),
            "could not create the encrypted 7z"
        );
        let workdir = base.join("extract");
        std::fs::create_dir_all(&workdir).unwrap();
        let pid = AtomicU32::new(0);

        // `ouch list` accepts a wrong password when the file names are not
        // encrypted, which is why validation needs a real trial extraction.
        ouch.list(&archive, Some("wrong"), &|_line: &str| {})
            .expect("7z without header encryption lists with a wrong password");

        let err = validate_password(&ouch, &archive, &workdir, "wrong", 0, &pid)
            .expect_err("a wrong password must fail the trial extraction");
        assert!(is_password_error(&err), "unexpected error: {err}");
        assert!(
            std::fs::read_dir(&workdir).unwrap().next().is_none(),
            "a failed validation must not leave files in the destination"
        );

        validate_password(&ouch, &archive, &workdir, "hunter2", 0, &pid)
            .expect("the correct password should pass validation");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn password_is_shared_between_parallel_workers() {
        let Ok(ouch) = OuchClient::discover() else {
            eprintln!("skipping: ouch binary not found");
            return;
        };
        if Command::new("zip").arg("--version").output().is_err() {
            eprintln!("skipping: `zip` tool not found");
            return;
        }

        let base = unique_dir("parallel-password");
        let mut archives = Vec::new();
        for folder in ["one", "two"] {
            let out = base.join(folder).join("out");
            std::fs::create_dir_all(&out).unwrap();
            std::fs::write(out.join("secret.txt"), b"secret").unwrap();
            let archive = out.join("secret.zip");
            assert!(
                Command::new("zip")
                    .current_dir(&out)
                    .args(["-q", "-P", "hunter2", "secret.zip", "secret.txt"])
                    .status()
                    .unwrap()
                    .success(),
                "could not create the encrypted zip"
            );
            archives.push(archive);
        }

        let config = Config {
            parallel_extractions: 2,
            after_extract: AfterExtract::Keep,
            conflict_policy: ConflictPolicy::Overwrite,
            ..Config::default()
        };
        let (controller, events) = spawn(
            ouch,
            archives,
            config,
            I18n::new(crate::i18n::Language::En),
            egui::Context::default(),
        );

        let mut prompts = 0;
        let mut done = 0;
        loop {
            match events.recv_timeout(Duration::from_secs(30)) {
                Ok(JobEvent::NeedPassword { .. }) => {
                    prompts += 1;
                    let _ = controller
                        .answer_tx
                        .send(JobAnswer::Password("hunter2".into()));
                }
                Ok(JobEvent::Done { success, .. }) => done += usize::from(success),
                Ok(JobEvent::AllDone) | Err(_) => break,
                Ok(_) => {}
            }
        }

        assert_eq!(prompts, 1, "the password should be asked only once");
        assert_eq!(done, 2, "both archives should use the shared password");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn cancel_stops_workers_waiting_for_a_prompt() {
        let Ok(ouch) = OuchClient::discover() else {
            eprintln!("skipping: ouch binary not found");
            return;
        };
        if Command::new("zip").arg("--version").output().is_err() {
            eprintln!("skipping: `zip` tool not found");
            return;
        }

        let base = unique_dir("parallel-cancel");
        let mut archives = Vec::new();
        for folder in ["one", "two"] {
            let out = base.join(folder).join("out");
            std::fs::create_dir_all(&out).unwrap();
            std::fs::write(out.join("secret.txt"), b"secret").unwrap();
            let archive = out.join("secret.zip");
            assert!(
                Command::new("zip")
                    .current_dir(&out)
                    .args(["-q", "-P", "hunter2", "secret.zip", "secret.txt"])
                    .status()
                    .unwrap()
                    .success(),
                "could not create the encrypted zip"
            );
            archives.push(archive);
        }

        let config = Config {
            parallel_extractions: 2,
            after_extract: AfterExtract::Keep,
            conflict_policy: ConflictPolicy::Overwrite,
            ..Config::default()
        };
        let (controller, events) = spawn(
            ouch,
            archives,
            config,
            I18n::new(crate::i18n::Language::En),
            egui::Context::default(),
        );

        let mut prompts = 0;
        let mut all_done = false;
        loop {
            match events.recv_timeout(Duration::from_secs(10)) {
                Ok(JobEvent::NeedPassword { .. }) => {
                    prompts += 1;
                    if prompts == 1 {
                        // Give the other worker time to queue behind the
                        // active prompt before cancelling.
                        std::thread::sleep(Duration::from_millis(100));
                        controller.cancel_all.store(true, Ordering::Relaxed);
                    }
                    let _ = controller.answer_tx.send(JobAnswer::CancelBatch);
                }
                Ok(JobEvent::AllDone) => {
                    all_done = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }

        assert!(all_done, "the batch should finish after cancelling");
        assert_eq!(prompts, 1, "waiting workers must not reopen the prompt");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn first_prompt_is_for_the_first_archive() {
        let Ok(ouch) = OuchClient::discover() else {
            eprintln!("skipping: ouch binary not found");
            return;
        };
        if Command::new("7z").arg("--help").output().is_err() {
            eprintln!("skipping: `7z` tool not found");
            return;
        }

        let base = unique_dir("prompt-order");
        let archives: Vec<PathBuf> = ["one", "two", "three"]
            .iter()
            .map(|folder| build_encrypted_7z(&base, folder))
            .collect();
        let config = Config {
            parallel_extractions: 3,
            after_extract: AfterExtract::Keep,
            conflict_policy: ConflictPolicy::Overwrite,
            ..Config::default()
        };
        let (controller, events) = spawn(
            ouch,
            archives.clone(),
            config,
            I18n::new(crate::i18n::Language::En),
            egui::Context::default(),
        );

        let mut first_prompt = None;
        let mut prompts = 0;
        let mut done = 0;
        loop {
            match events.recv_timeout(Duration::from_secs(30)) {
                Ok(JobEvent::NeedPassword { archive, .. }) => {
                    prompts += 1;
                    if first_prompt.is_none() {
                        first_prompt = Some(archive);
                    }
                    let _ = controller
                        .answer_tx
                        .send(JobAnswer::Password("hunter2".into()));
                }
                Ok(JobEvent::Done { success, .. }) => done += usize::from(success),
                Ok(JobEvent::AllDone) | Err(_) => break,
                Ok(_) => {}
            }
        }

        assert_eq!(
            first_prompt.as_deref(),
            Some(archives[0].as_path()),
            "the first password prompt must belong to the first queued archive"
        );
        assert_eq!(prompts, 1, "the shared password should avoid more prompts");
        assert_eq!(done, 3, "all archives should be extracted");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn cancel_stops_a_seven_zip_batch() {
        let Ok(ouch) = OuchClient::discover() else {
            eprintln!("skipping: ouch binary not found");
            return;
        };
        if Command::new("7z").arg("--help").output().is_err() {
            eprintln!("skipping: `7z` tool not found");
            return;
        }

        let base = unique_dir("cancel-7z");
        let archives: Vec<PathBuf> = ["one", "two", "three", "four", "five", "six"]
            .iter()
            .map(|folder| build_encrypted_7z(&base, folder))
            .collect();
        let config = Config {
            parallel_extractions: 2,
            after_extract: AfterExtract::Keep,
            conflict_policy: ConflictPolicy::Overwrite,
            ..Config::default()
        };
        let (controller, events) = spawn(
            ouch,
            archives,
            config,
            I18n::new(crate::i18n::Language::En),
            egui::Context::default(),
        );

        let mut prompts = 0;
        let mut started = 0;
        let mut all_done = false;
        loop {
            match events.recv_timeout(Duration::from_secs(10)) {
                Ok(JobEvent::NeedPassword { .. }) => {
                    prompts += 1;
                    if prompts == 1 {
                        controller.cancel_all.store(true, Ordering::Relaxed);
                        for _ in 0..controller.current_pids.len() {
                            let _ = controller.answer_tx.send(JobAnswer::CancelBatch);
                        }
                    } else {
                        let _ = controller.answer_tx.send(JobAnswer::CancelBatch);
                    }
                }
                Ok(JobEvent::Started { .. }) => started += 1,
                Ok(JobEvent::AllDone) => {
                    all_done = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }

        assert!(all_done, "the batch must finish after cancelling");
        assert_eq!(prompts, 1, "waiting workers must not reopen the prompt");
        assert_eq!(started, 1, "only the first archive should have started");
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
