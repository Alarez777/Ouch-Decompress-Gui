//! Application state and egui rendering.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;

use crate::config::{AfterExtract, Config, ConflictPolicy, TrashFallback};
use crate::formats;
use crate::i18n::I18n;
use crate::job::{self, JobAnswer, JobController, JobEvent};
use crate::modes::DecompressMode;
use crate::ouch::OuchClient;
use crate::theme::{self, ThemeChoice};

/// Tabs inside the settings window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    General,
    Formats,
    Passwords,
}

/// Per-file processing status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Pending,
    Running,
    Done,
    Failed,
    Skipped,
}

impl Status {
    fn label_key(self) -> &'static str {
        match self {
            Status::Pending => "status.pending",
            Status::Running => "status.running",
            Status::Done => "status.done",
            Status::Failed => "status.failed",
            Status::Skipped => "status.skipped",
        }
    }

    fn color(self) -> egui::Color32 {
        match self {
            Status::Pending => egui::Color32::GRAY,
            Status::Running => egui::Color32::from_rgb(0x2f, 0x80, 0xed),
            Status::Done => egui::Color32::from_rgb(0x2e, 0xa0, 0x43),
            Status::Failed => egui::Color32::from_rgb(0xd9, 0x30, 0x25),
            Status::Skipped => egui::Color32::from_rgb(0xc8, 0x8a, 0x00),
        }
    }
}

/// One queued archive.
struct FileEntry {
    path: PathBuf,
    status: Status,
    message: String,
}

/// Pending password request from the worker.
struct PasswordPrompt {
    index: usize,
    archive: PathBuf,
    /// Error from a previous, failed password attempt (shown in red).
    error: Option<String>,
}

/// Pending overwrite confirmation from the worker.
struct OverwritePrompt {
    index: usize,
    conflicts: Vec<PathBuf>,
}

/// Root application.
pub struct App {
    config: Config,
    i18n: I18n,
    ouch: Option<OuchClient>,
    ouch_error: Option<String>,
    /// Decoded application logo, shown in the About window.
    logo: Option<egui::TextureHandle>,
    /// Whether the independent settings window is open.
    open_settings: bool,
    /// Which tab is selected in the settings window.
    settings_tab: SettingsTab,
    /// Whether the independent log window is open.
    open_log: bool,
    /// Whether the independent about window is open.
    open_about: bool,
    /// Last applied UI scale (percent), to avoid re-setting it every frame.
    applied_scale: u32,
    /// Background update-check result, when ready.
    update_status: Option<crate::update::UpdateStatus>,
    /// Receiver for the in-flight update check.
    update_rx: Option<Receiver<crate::update::UpdateStatus>>,
    /// Receiver for the in-flight native file picker.
    file_picker_rx: Option<Receiver<Vec<PathBuf>>>,
    /// PID of the running `zenity` file picker, or 0. Terminated on exit so it
    /// does not outlive the window.
    picker_pid: Arc<AtomicU32>,
    /// Set when a setting changed and the config still needs to be written.
    config_dirty: bool,
    /// Last time the config was written, used to debounce writes.
    last_config_save: Option<std::time::Instant>,
    /// True while the interface-size slider is dragged, to avoid re-applying
    /// the (expensive) zoom on every frame.
    scale_dragging: bool,
    files: Vec<FileEntry>,
    log: Vec<String>,
    status_line: String,
    running: bool,
    controller: Option<JobController>,
    event_rx: Option<Receiver<JobEvent>>,
    password_prompt: Option<PasswordPrompt>,
    overwrite_prompt: Option<OverwritePrompt>,
    password_input: String,
    new_password_input: String,
    applied_theme: Option<ThemeChoice>,
    /// Indices of queued files whose format is disabled; the user must confirm
    /// before the batch starts.
    confirm_disabled: Option<Vec<usize>>,
    /// Feedback line for the file-association buttons.
    association_status: String,
    /// Error text shown in a modal popup (accumulates multiple failures).
    error_popup: Option<String>,
    /// True when the app was launched with archives (double-click / "Open
    /// with"); in that case it closes itself after a fully successful batch.
    auto_close_on_success: bool,
    /// Set when the window should close at the end of the current frame.
    close_requested: bool,
    /// Set when the user asked to close while a batch is running, so a
    /// confirmation modal is shown instead of closing immediately.
    confirm_close: bool,
    /// Maps a worker batch index to the index in `files`.
    batch_indices: Vec<usize>,
    /// Number of archives whose source could not be deleted after extraction.
    cleanup_failures: usize,
    /// "source → result" summaries when extraction produced a renamed result.
    results: Vec<String>,
    /// Set when a batch finished but more files were added meanwhile.
    restart_pending: bool,
    /// Progress of the archive currently being extracted:
    /// `(fraction, total_bytes)`. `total == 0` means unknown, so the bar is
    /// shown as indeterminate.
    progress: (f32, u64),
    /// When the current batch started, for the elapsed-time label.
    batch_started: Option<std::time::Instant>,
}

impl App {
    /// Builds the app. When `initial_files` is non-empty the batch starts
    /// immediately (the double-click / "open with" flow).
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        initial_files: Vec<PathBuf>,
        open_settings: bool,
    ) -> Self {
        let config = Config::load();
        let i18n = I18n::new(config.language);
        theme::apply(&cc.egui_ctx, config.theme);
        cc.egui_ctx.set_zoom_factor(config.ui_scale as f32 / 100.0);

        let (ouch, ouch_error) = match OuchClient::discover() {
            Ok(client) => (Some(client), None),
            Err(err) => (None, Some(err.to_string())),
        };

        let logo = load_logo(&cc.egui_ctx);

        // Drop the duplicate desktop entry older versions installed, when the
        // AppImage manager already provides one.
        crate::association::remove_legacy_if_redundant();

        let auto_close_on_success = !initial_files.is_empty() && !open_settings;
        let initial_scale = config.ui_scale;

        let files: Vec<FileEntry> = initial_files
            .into_iter()
            .map(|path| FileEntry {
                path,
                status: Status::Pending,
                message: String::new(),
            })
            .collect();

        let mut app = Self {
            config,
            i18n,
            ouch,
            ouch_error,
            logo,
            open_settings,
            settings_tab: SettingsTab::General,
            open_log: false,
            open_about: false,
            applied_scale: initial_scale,
            update_status: None,
            update_rx: None,
            file_picker_rx: None,
            picker_pid: Arc::new(AtomicU32::new(0)),
            config_dirty: false,
            last_config_save: None,
            scale_dragging: false,
            files,
            log: Vec::new(),
            status_line: String::new(),
            running: false,
            controller: None,
            event_rx: None,
            password_prompt: None,
            overwrite_prompt: None,
            password_input: String::new(),
            new_password_input: String::new(),
            applied_theme: None,
            confirm_disabled: None,
            association_status: String::new(),
            error_popup: None,
            auto_close_on_success,
            close_requested: false,
            confirm_close: false,
            batch_indices: Vec::new(),
            cleanup_failures: 0,
            results: Vec::new(),
            restart_pending: false,
            progress: (0.0, 0),
            batch_started: None,
        };

        if !app.files.is_empty() {
            app.request_start_batch(&cc.egui_ctx);
        }
        app
    }

    /// Starts the batch unless some pending file uses a disabled format, in
    /// which case the user is asked to confirm first.
    fn request_start_batch(&mut self, ctx: &egui::Context) {
        if self.running {
            return;
        }
        let disabled: Vec<usize> = self
            .files
            .iter()
            .enumerate()
            .filter(|(_, file)| file.status == Status::Pending)
            .filter(|(_, file)| {
                formats::outer_format(&file.path)
                    .map(|format| self.config.is_format_disabled(format.id))
                    .unwrap_or(false)
            })
            .map(|(index, _)| index)
            .collect();

        if disabled.is_empty() {
            self.start_batch(ctx);
        } else {
            self.confirm_disabled = Some(disabled);
        }
    }

    /// Starts processing every pending file on a worker thread.
    fn start_batch(&mut self, ctx: &egui::Context) {
        if self.running {
            return;
        }
        let pending: Vec<usize> = self
            .files
            .iter()
            .enumerate()
            .filter(|(_, file)| file.status == Status::Pending)
            .map(|(index, _)| index)
            .collect();
        if pending.is_empty() {
            return;
        }
        let Some(ouch) = self.ouch.clone() else {
            self.status_line = self.i18n.t("ouch.not_found");
            return;
        };

        for &index in &pending {
            self.files[index].status = Status::Pending;
            self.files[index].message.clear();
        }
        self.log.clear();
        self.status_line.clear();
        self.error_popup = None;
        self.cleanup_failures = 0;
        self.results.clear();
        self.progress = (0.0, 0);
        self.batch_started = Some(std::time::Instant::now());

        let archives: Vec<PathBuf> = pending
            .iter()
            .map(|&index| self.files[index].path.clone())
            .collect();
        let config = self.config.clone();
        let (controller, rx) = job::spawn(ouch, archives, config, ctx.clone());
        self.controller = Some(controller);
        self.event_rx = Some(rx);
        self.batch_indices = pending;
        self.running = true;
    }

    /// Maps a worker batch index to the index in `files`.
    fn file_index(&self, batch_index: usize) -> Option<usize> {
        self.batch_indices.get(batch_index).copied()
    }

    /// Polls worker events and applies them to the UI state.
    fn pump_events(&mut self) {
        let events: Vec<JobEvent> = match &self.event_rx {
            Some(rx) => {
                let mut collected = Vec::new();
                while let Ok(event) = rx.try_recv() {
                    collected.push(event);
                }
                collected
            }
            None => Vec::new(),
        };
        for event in events {
            self.handle_event(event);
        }
    }

    /// Starts an update check once, if the About window is used.
    fn maybe_check_update(&mut self, ctx: &egui::Context) {
        if self.update_rx.is_some() || self.update_status.is_some() {
            return;
        }
        self.update_rx = Some(crate::update::check(env!("CARGO_PKG_VERSION"), ctx.clone()));
    }

    /// Receives the background update-check result, if ready.
    fn pump_update(&mut self) {
        let result = match &self.update_rx {
            Some(rx) => rx.try_recv().ok(),
            None => None,
        };
        if let Some(status) = result {
            self.update_status = Some(status);
            self.update_rx = None;
        }
    }

    /// Opens the native file picker on a background thread, so the main window
    /// keeps processing events (and the compositor does not flag it as "not
    /// responding") while the dialog is open.
    fn start_file_picker(&mut self, ctx: &egui::Context) {
        if self.file_picker_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let pid_slot = self.picker_pid.clone();
        thread::spawn(move || {
            let files = crate::system::pick_files(&pid_slot);
            if tx.send(files).is_ok() {
                ctx.request_repaint();
            }
        });
        self.file_picker_rx = Some(rx);
    }

    /// Receives the file-picker result, if it is ready.
    fn pump_file_picker(&mut self, ctx: &egui::Context) {
        let result = match &self.file_picker_rx {
            Some(rx) => rx.try_recv(),
            None => return,
        };
        match result {
            Ok(files) => {
                self.file_picker_rx = None;
                self.add_files_and_extract(ctx, files);
            }
            Err(mpsc::TryRecvError::Disconnected) => self.file_picker_rx = None,
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    fn handle_event(&mut self, event: JobEvent) {
        match event {
            JobEvent::Started { index } => {
                if let Some(file_index) = self.file_index(index) {
                    self.files[file_index].status = Status::Running;
                    self.push_log(format!("-> {}", self.file_name(file_index)));
                }
                self.progress = (0.0, 0);
                self.status_line = self.i18n.t("extract.working");
            }
            JobEvent::Log { index, line } => {
                let _ = index;
                self.push_log(line);
            }
            JobEvent::Progress {
                index,
                fraction,
                total,
            } => {
                if let Some(file_index) = self.file_index(index) {
                    if self.files[file_index].status == Status::Running {
                        self.progress = (fraction, total);
                    }
                }
            }
            JobEvent::NeedPassword {
                index,
                archive,
                error,
            } => {
                self.password_input.clear();
                if let Some(file_index) = self.file_index(index) {
                    self.password_prompt = Some(PasswordPrompt {
                        index: file_index,
                        archive,
                        error,
                    });
                    self.files[file_index].message = "waiting for password".into();
                }
            }
            JobEvent::NeedOverwrite { index, conflicts } => {
                if let Some(file_index) = self.file_index(index) {
                    self.overwrite_prompt = Some(OverwritePrompt {
                        index: file_index,
                        conflicts,
                    });
                    self.files[file_index].message = "waiting for overwrite confirmation".into();
                }
            }
            JobEvent::Done {
                index,
                success,
                message,
                cleanup_error,
                result,
            } => {
                let skipped = message == "skipped" || message == "cancelled";
                if let Some(file_index) = self.file_index(index) {
                    let file = &mut self.files[file_index];
                    file.status = if success {
                        Status::Done
                    } else if skipped {
                        Status::Skipped
                    } else {
                        Status::Failed
                    };
                    file.message = message.clone();

                    if !success && !skipped {
                        let name = self.file_name(file_index);
                        let entry = format!("{name}: {message}");
                        match &mut self.error_popup {
                            Some(existing) => {
                                existing.push('\n');
                                existing.push_str(&entry);
                            }
                            None => self.error_popup = Some(entry),
                        }
                    }
                }
                if success && self.progress.1 > 0 {
                    self.progress.0 = 1.0;
                }
                self.push_log(format!("   {message}"));
                if let Some(result) = result {
                    self.results.push(result);
                }
                if let Some(err) = cleanup_error {
                    self.cleanup_failures += 1;
                    self.push_log(format!("   WARNING: {err}"));
                }
            }
            JobEvent::AllDone => {
                self.running = false;
                self.controller = None;
                self.event_rx = None;
                self.progress = (0.0, 0);
                self.batch_started = None;
                self.confirm_close = false;
                self.status_line = self.i18n.t("extract.all_done");

                if self.config.notify_on_done {
                    let done = self
                        .files
                        .iter()
                        .filter(|file| file.status == Status::Done)
                        .count();
                    let failed = self
                        .files
                        .iter()
                        .filter(|file| file.status == Status::Failed)
                        .count();
                    let mut body = self
                        .i18n
                        .t("notify.done.body")
                        .replace("{done}", &done.to_string())
                        .replace("{failed}", &failed.to_string());
                    if !self.results.is_empty() {
                        body.push('\n');
                        body.push_str(&self.results.join("\n"));
                    }
                    if self.cleanup_failures > 0 {
                        body.push(' ');
                        body.push_str(
                            &self
                                .i18n
                                .t("notify.done.cleanup")
                                .replace("{count}", &self.cleanup_failures.to_string()),
                        );
                    }
                    crate::system::notify(&self.i18n.t("notify.done.title"), &body);
                }

                // Files added while the batch was running are processed next.
                let has_pending = self.files.iter().any(|file| file.status == Status::Pending);
                if has_pending {
                    self.restart_pending = true;
                    return;
                }

                // Double-click flow: close automatically when everything
                // extracted without errors, so the window does not linger.
                let all_ok = !self.files.is_empty()
                    && self.files.iter().all(|file| file.status == Status::Done);
                if self.auto_close_on_success && all_ok {
                    self.close_requested = true;
                }
            }
        }
    }

    fn push_log(&mut self, line: String) {
        self.log.push(line);
        if self.log.len() > 500 {
            self.log.drain(0..100);
        }
    }

    fn file_name(&self, index: usize) -> String {
        self.files
            .get(index)
            .and_then(|file| file.path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Adds a path to the queue if it is not already present. Returns whether
    /// it was actually added.
    fn add_file(&mut self, path: PathBuf) -> bool {
        if self.files.iter().any(|file| file.path == path) {
            return false;
        }
        self.files.push(FileEntry {
            path,
            status: Status::Pending,
            message: String::new(),
        });
        true
    }

    /// Adds several paths, logs the ones that were new, and starts extracting
    /// them automatically.
    fn add_files_and_extract(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>) {
        let mut added = false;
        for path in paths {
            if self.add_file(path.clone()) {
                self.push_log(format!("+ {}", path.display()));
                added = true;
            }
        }
        if !added {
            return;
        }
        if !self.running {
            self.request_start_batch(ctx);
        }
    }

    /// Marks the config as changed; the write itself is debounced in
    /// [`Self::flush_config`] so dragging a slider does not hit the disk every
    /// frame.
    fn persist(&mut self) {
        self.config_dirty = true;
    }

    /// Writes the config if there are pending changes and enough time passed
    /// since the last write (or `force` is set).
    fn flush_config(&mut self, force: bool) {
        if !self.config_dirty {
            return;
        }
        let now = std::time::Instant::now();
        let due = self
            .last_config_save
            .is_none_or(|last| now.duration_since(last) >= std::time::Duration::from_millis(400));
        if !force && !due {
            return;
        }
        self.last_config_save = Some(now);
        self.config_dirty = false;
        if let Err(err) = self.config.save() {
            // Keep the change pending so a later flush can retry the write.
            self.config_dirty = true;
            self.status_line = format!("{}: {err}", self.i18n.t("common.error"));
        }
    }

    fn answer_password(&mut self, answer: JobAnswer) {
        if let Some(controller) = &self.controller {
            let _ = controller.answer_tx.send(answer);
        }
        self.password_prompt = None;
        self.password_input.clear();
    }

    fn cancel_batch(&mut self) {
        if let Some(controller) = &self.controller {
            controller
                .cancel_all
                .store(true, std::sync::atomic::Ordering::Relaxed);
            let _ = controller.answer_tx.send(JobAnswer::CancelBatch);
        }
        self.password_prompt = None;
        self.password_input.clear();
    }

    /// Cancels the batch, terminates the running `ouch` process and closes the
    /// window. Used when the user confirms closing mid-extraction.
    fn abort_and_close(&mut self) {
        self.cancel_batch();
        if let Some(controller) = &self.controller {
            let pid = controller
                .current_pid
                .load(std::sync::atomic::Ordering::Relaxed);
            crate::system::terminate_process(pid);
        }

        if self.config.notify_on_done {
            let running = self
                .files
                .iter()
                .find(|file| file.status == Status::Running)
                .and_then(|file| file.path.file_name())
                .map(|name| name.to_string_lossy().into_owned());
            let body = match running {
                Some(name) => self
                    .i18n
                    .t("notify.incomplete.body")
                    .replace("{file}", &name),
                None => self.i18n.t("notify.incomplete.body.generic"),
            };
            crate::system::notify(&self.i18n.t("notify.incomplete.title"), &body);
        }

        self.close_requested = true;
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Keep the theme in sync with the user's choice.
        if self.applied_theme != Some(self.config.theme) {
            theme::apply(&ctx, self.config.theme);
            self.applied_theme = Some(self.config.theme);
        }

        // Keep the UI scale in sync with the user's choice. Applying it is
        // expensive (font atlas + relayout), so while the slider is dragged we
        // wait until it is released.
        if !self.scale_dragging && self.applied_scale != self.config.ui_scale {
            ctx.set_zoom_factor(self.config.ui_scale as f32 / 100.0);
            self.applied_scale = self.config.ui_scale;
        }

        self.pump_events();
        self.pump_update();
        self.pump_file_picker(&ctx);
        self.handle_dropped_files(&ctx);

        if self.restart_pending {
            self.restart_pending = false;
            if !self.running {
                self.request_start_batch(&ctx);
            }
        }

        // A confirmed close always wins, so it never gets intercepted again.
        if self.close_requested {
            self.flush_config(true);
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        // If the user closes the window while extracting, cancel the close and
        // ask for confirmation instead (see `ui_close_confirm`).
        if ctx.input(|input| input.viewport().close_requested()) {
            if self.running {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.confirm_close = true;
            } else {
                self.flush_config(true);
            }
        }

        egui::Panel::top("header").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let picking = self.file_picker_rx.is_some();
                if ui
                    .add_enabled(!picking, egui::Button::new(self.i18n.t("extract.add_file")))
                    .clicked()
                {
                    self.start_file_picker(ui.ctx());
                }
                if ui.button(self.i18n.t("tab.settings")).clicked() {
                    self.open_settings = true;
                }
                if ui.button(self.i18n.t("tab.log")).clicked() {
                    self.open_log = true;
                }
                if ui.button(self.i18n.t("tab.about")).clicked() {
                    self.open_about = true;
                    self.maybe_check_update(ui.ctx());
                }
            });
            ui.add_space(6.0);
        });

        egui::CentralPanel::default().show(ui, |ui| self.ui_extract(ui));

        self.show_settings_viewport(&ctx);
        self.show_log_viewport(&ctx);
        self.show_about_viewport(&ctx);

        // Persist any pending settings change (debounced).
        self.flush_config(false);
        if self.config_dirty {
            ctx.request_repaint_after(std::time::Duration::from_millis(400));
        }

        self.ui_password_modal(&ctx);
        self.ui_confirm_disabled(&ctx);
        self.ui_overwrite_modal(&ctx);
        self.ui_close_confirm(&ctx);
        self.ui_error_modal(&ctx);
        self.draw_drop_overlay(&ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Do not let the file picker outlive the window.
        crate::system::terminate_process(self.picker_pid.load(Ordering::Relaxed));
    }
}

impl App {
    fn ouch_version_label(&self) -> String {
        if let Some(ouch) = &self.ouch {
            self.i18n
                .t("app.ouch_version")
                .replace("{version}", ouch.version())
        } else {
            self.i18n.t("ouch.not_found")
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .collect()
        });
        if dropped.is_empty() {
            return;
        }
        self.add_files_and_extract(ctx, dropped);
    }

    fn ui_extract(&mut self, ui: &mut egui::Ui) {
        if self.ouch.is_none() {
            let error = self.ouch_error.clone().unwrap_or_default();
            ui.colored_label(egui::Color32::from_rgb(0xd9, 0x30, 0x25), error);
            ui.label(self.i18n.t("ouch.not_found_hint"));
        }

        if self.running {
            let name = self
                .files
                .iter()
                .find(|file| file.status == Status::Running)
                .and_then(|file| file.path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.status_line_line());
            let elapsed = self
                .batch_started
                .map(|started| started.elapsed().as_secs());
            let mut cancel_clicked = false;
            ui.horizontal(|ui| {
                ui.add(egui::Label::new(name).truncate());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(self.i18n.t("common.cancel")).clicked() {
                        cancel_clicked = true;
                    }
                    if let Some(secs) = elapsed {
                        ui.label(
                            egui::RichText::new(
                                self.i18n
                                    .t("progress.elapsed")
                                    .replace("{time}", &format_duration(secs)),
                            )
                            .color(egui::Color32::GRAY),
                        );
                    }
                });
            });
            if cancel_clicked {
                self.cancel_batch();
            }

            let (fraction, total) = self.progress;
            if total > 0 {
                ui.add(egui::ProgressBar::new(fraction).text(format!("{:.0}%", fraction * 100.0)));
            } else {
                ui.add(egui::ProgressBar::new(0.0).animate(true));
            }
        } else if !self.status_line.is_empty() {
            ui.label(self.status_line.clone());
        }

        if self.files.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(self.i18n.t("extract.drop_hint"))
                        .size(20.0)
                        .color(egui::Color32::GRAY),
                );
            });
        } else {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let mut remove: Option<usize> = None;
                    let mut open: Option<PathBuf> = None;
                    for (index, file) in self.files.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(self.i18n.t(file.status.label_key()))
                                    .color(file.status.color())
                                    .small(),
                            );
                            let name = file
                                .path
                                .file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_else(|| file.path.display().to_string());
                            ui.label(name)
                                .on_hover_text(file.path.display().to_string());
                            if file.status == Status::Failed && !file.message.is_empty() {
                                ui.label(
                                    egui::RichText::new(&file.message)
                                        .small()
                                        .color(egui::Color32::from_rgb(0xd9, 0x30, 0x25)),
                                );
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if !self.running && ui.small_button("x").clicked() {
                                        remove = Some(index);
                                    }
                                    if file.status == Status::Done
                                        && ui
                                            .small_button(self.i18n.t("extract.open_folder"))
                                            .clicked()
                                    {
                                        open = Some(
                                            file.path
                                                .parent()
                                                .filter(|parent| !parent.as_os_str().is_empty())
                                                .map(PathBuf::from)
                                                .unwrap_or_else(|| PathBuf::from(".")),
                                        );
                                    }
                                },
                            );
                        });
                    }
                    if let Some(path) = open {
                        crate::system::open_folder(&path);
                    }
                    if let Some(index) = remove {
                        self.files.remove(index);
                    }
                });
        }
    }

    fn show_log_viewport(&mut self, ctx: &egui::Context) {
        if !self.open_log {
            return;
        }
        let title = format!(
            "{} — {}",
            self.i18n.t("app.title"),
            self.i18n.t("log.title")
        );
        let builder = egui::ViewportBuilder::default()
            .with_app_id("ouch-decompress-gui")
            .with_title(title)
            .with_inner_size([480.0, 320.0])
            .with_min_inner_size([480.0, 320.0]);
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("log"),
            builder,
            |ui, _class| {
                if ui.ctx().input(|input| input.viewport().close_requested()) {
                    self.open_log = false;
                    self.flush_config(true);
                    ui.ctx().request_repaint();
                    return;
                }
                egui::CentralPanel::default().show(ui, |ui| self.ui_log(ui));
            },
        );
    }

    fn ui_log(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button(self.i18n.t("log.copy")).clicked() {
                ui.ctx().copy_text(self.log.join("\n"));
            }
            if ui.button(self.i18n.t("log.clear")).clicked() {
                self.log.clear();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(self.i18n.t("common.close")).clicked() {
                    self.open_log = false;
                    self.flush_config(true);
                    ui.ctx().request_repaint();
                }
            });
        });
        ui.add_space(4.0);
        ui.separator();

        if self.log.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(self.i18n.t("log.empty"));
            });
        } else {
            egui::ScrollArea::vertical()
                .id_salt("log")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for line in &self.log {
                        ui.label(egui::RichText::new(line).monospace().small());
                    }
                });
        }
    }

    fn show_about_viewport(&mut self, ctx: &egui::Context) {
        if !self.open_about {
            return;
        }
        let title = format!(
            "{} — {}",
            self.i18n.t("app.title"),
            self.i18n.t("about.title")
        );
        let builder = egui::ViewportBuilder::default()
            .with_app_id("ouch-decompress-gui")
            .with_title(title)
            .with_inner_size([420.0, 470.0])
            .with_min_inner_size([400.0, 430.0]);
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("about"),
            builder,
            |ui, _class| {
                if ui.ctx().input(|input| input.viewport().close_requested()) {
                    self.open_about = false;
                    self.flush_config(true);
                    ui.ctx().request_repaint();
                    return;
                }
                egui::CentralPanel::default().show(ui, |ui| self.ui_about(ui));
            },
        );
    }

    fn ui_about(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        if let Some(logo) = self.logo.clone() {
            ui.vertical_centered(|ui| {
                ui.add(egui::Image::from_texture(&logo).fit_to_exact_size(egui::vec2(72.0, 72.0)));
            });
            ui.add_space(6.0);
        }
        ui.heading(self.i18n.t("app.title"));
        ui.label(
            egui::RichText::new(format!(
                "{} {}",
                self.i18n.t("about.version"),
                env!("CARGO_PKG_VERSION")
            ))
            .color(egui::Color32::GRAY),
        );

        // Update status for *this* application.
        ui.add_space(6.0);
        match &self.update_status {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(self.i18n.t("about.checking"));
                });
            }
            Some(crate::update::UpdateStatus::Available { version, url }) => {
                let url = url.clone();
                ui.colored_label(
                    egui::Color32::from_rgb(0x2e, 0xa0, 0x43),
                    self.i18n
                        .t("about.update_available")
                        .replace("{version}", version),
                );
                if ui.button(self.i18n.t("about.open_release")).clicked() {
                    crate::system::open_url(&url);
                }
            }
            Some(crate::update::UpdateStatus::UpToDate) => {
                ui.label(self.i18n.t("about.up_to_date"));
            }
            Some(crate::update::UpdateStatus::Unknown) => {
                ui.label(
                    egui::RichText::new(self.i18n.t("about.update_unknown"))
                        .color(egui::Color32::GRAY),
                );
            }
        }

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(4.0);

        // Bundled tools (informational only).
        ui.label(self.i18n.t("about.components"));
        ui.label(self.ouch_version_label());

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(4.0);

        // GUI toolkit used to build this app.
        ui.label(self.i18n.t("about.interface"));
        ui.label(format!(
            "egui {} · eframe {}",
            env!("EGUI_VERSION"),
            env!("EFRAME_VERSION")
        ));

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(4.0);

        if ui.button(self.i18n.t("about.project")).clicked() {
            crate::system::open_url(crate::update::REPO_URL);
        }

        ui.add_space(10.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(self.i18n.t("common.close")).clicked() {
                self.open_about = false;
                self.flush_config(true);
                ui.ctx().request_repaint();
            }
        });
    }

    fn status_line_line(&self) -> String {
        if self.status_line.is_empty() {
            self.i18n.t("extract.working")
        } else {
            self.status_line.clone()
        }
    }

    fn show_settings_viewport(&mut self, ctx: &egui::Context) {
        if !self.open_settings {
            return;
        }
        let title = format!(
            "{} — {}",
            self.i18n.t("app.title"),
            self.i18n.t("settings.title")
        );
        let builder = egui::ViewportBuilder::default()
            .with_app_id("ouch-decompress-gui")
            .with_title(title)
            .with_inner_size([480.0, 420.0])
            .with_min_inner_size([480.0, 420.0]);
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("settings"),
            builder,
            |ui, _class| {
                if ui.ctx().input(|input| input.viewport().close_requested()) {
                    self.open_settings = false;
                    self.flush_config(true);
                    ui.ctx().request_repaint();
                    return;
                }
                egui::CentralPanel::default().show(ui, |ui| self.ui_settings(ui));
            },
        );
    }

    fn ui_settings(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for tab in [
                SettingsTab::General,
                SettingsTab::Formats,
                SettingsTab::Passwords,
            ] {
                let label = match tab {
                    SettingsTab::General => self.i18n.t("settings.general"),
                    SettingsTab::Formats => self.i18n.t("settings.formats"),
                    SettingsTab::Passwords => self.i18n.t("settings.passwords"),
                };
                if ui
                    .selectable_label(self.settings_tab == tab, label)
                    .clicked()
                {
                    self.settings_tab = tab;
                }
            }
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match self.settings_tab {
                SettingsTab::General => self.settings_general(ui),
                SettingsTab::Formats => self.settings_formats(ui),
                SettingsTab::Passwords => self.settings_passwords(ui),
            });
    }

    fn settings_general(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new(self.i18n.t("mode.section")).strong());
        let mut changed = false;
        egui::ComboBox::from_id_salt("mode-combo")
            .selected_text(self.i18n.t(self.config.decompress_mode.label_key()))
            .width(260.0)
            .show_ui(ui, |ui| {
                for mode in DecompressMode::ALL {
                    changed |= ui
                        .selectable_value(
                            &mut self.config.decompress_mode,
                            mode,
                            self.i18n.t(mode.label_key()),
                        )
                        .changed();
                }
            });
        if changed {
            self.persist();
        }
        ui.label(
            egui::RichText::new(self.i18n.t(self.config.decompress_mode.hint_key()))
                .small()
                .color(egui::Color32::GRAY),
        );

        ui.add_space(12.0);
        ui.label(egui::RichText::new(self.i18n.t("settings.conflict")).strong());
        let mut changed = false;
        egui::ComboBox::from_id_salt("conflict-combo")
            .selected_text(self.i18n.t(self.config.conflict_policy.label_key()))
            .width(260.0)
            .show_ui(ui, |ui| {
                for policy in ConflictPolicy::ALL {
                    changed |= ui
                        .selectable_value(
                            &mut self.config.conflict_policy,
                            policy,
                            self.i18n.t(policy.label_key()),
                        )
                        .changed();
                }
            });
        if changed {
            self.persist();
        }

        ui.add_space(12.0);
        ui.label(egui::RichText::new(self.i18n.t("settings.after_extract")).strong());
        let mut changed = false;
        egui::ComboBox::from_id_salt("after-combo")
            .selected_text(self.i18n.t(self.config.after_extract.label_key()))
            .width(260.0)
            .show_ui(ui, |ui| {
                for action in AfterExtract::ALL {
                    changed |= ui
                        .selectable_value(
                            &mut self.config.after_extract,
                            action,
                            self.i18n.t(action.label_key()),
                        )
                        .changed();
                }
            });
        if changed {
            self.persist();
        }

        if self.config.after_extract == AfterExtract::Trash {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(self.i18n.t("settings.trash_fallback")).strong());
            let mut changed = false;
            egui::ComboBox::from_id_salt("trash-fallback-combo")
                .selected_text(self.i18n.t(self.config.trash_fallback.label_key()))
                .width(260.0)
                .show_ui(ui, |ui| {
                    for fallback in TrashFallback::ALL {
                        changed |= ui
                            .selectable_value(
                                &mut self.config.trash_fallback,
                                fallback,
                                self.i18n.t(fallback.label_key()),
                            )
                            .changed();
                    }
                });
            if changed {
                self.persist();
            }
        }

        ui.add_space(12.0);
        ui.label(egui::RichText::new(self.i18n.t("settings.language")).strong());
        let mut changed = false;
        egui::ComboBox::from_id_salt("language-combo")
            .selected_text(self.i18n.t(self.config.language.label_key()))
            .width(260.0)
            .show_ui(ui, |ui| {
                for language in crate::i18n::Language::ALL {
                    changed |= ui
                        .selectable_value(
                            &mut self.config.language,
                            language,
                            self.i18n.t(language.label_key()),
                        )
                        .changed();
                }
            });
        if changed {
            self.i18n = I18n::new(self.config.language);
            self.persist();
        }

        ui.add_space(12.0);
        ui.label(egui::RichText::new(self.i18n.t("theme.title")).strong());
        let mut changed = false;
        egui::ComboBox::from_id_salt("theme-combo")
            .selected_text(self.i18n.t(self.config.theme.label_key()))
            .width(260.0)
            .show_ui(ui, |ui| {
                for choice in ThemeChoice::ALL {
                    changed |= ui
                        .selectable_value(
                            &mut self.config.theme,
                            choice,
                            self.i18n.t(choice.label_key()),
                        )
                        .changed();
                }
            });
        if changed {
            theme::apply(ui.ctx(), self.config.theme);
            self.applied_theme = Some(self.config.theme);
            self.persist();
        }

        ui.add_space(12.0);
        ui.label(egui::RichText::new(self.i18n.t("settings.ui_scale")).strong());
        let mut scale = self.config.ui_scale as f32;
        let response = ui.add(
            egui::Slider::new(&mut scale, 75.0..=200.0)
                .suffix("%")
                .step_by(5.0),
        );
        self.scale_dragging = response.dragged();
        if response.changed() {
            self.config.ui_scale = scale.round() as u32;
            self.persist();
        }

        ui.add_space(12.0);
        ui.separator();
        if ui
            .checkbox(
                &mut self.config.notify_on_done,
                self.i18n.t("settings.notify"),
            )
            .changed()
        {
            self.persist();
        }

        ui.add_space(12.0);
        ui.separator();
        if ui.button(self.i18n.t("settings.reset")).clicked() {
            // Keep saved passwords; reset everything else.
            let passwords = std::mem::take(&mut self.config.passwords);
            self.config = Config::default();
            self.config.passwords = passwords;
            self.i18n = I18n::new(self.config.language);
            theme::apply(ui.ctx(), self.config.theme);
            self.applied_theme = Some(self.config.theme);
            self.persist();
        }
    }

    fn settings_formats(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new(self.i18n.t("settings.formats_hint"))
                .small()
                .color(egui::Color32::GRAY),
        );
        for format in formats::FORMATS {
            let mut enabled = !self.config.is_format_disabled(format.id);
            let label = format!("{}  ({})", format.id, format.extensions.join(", "));
            if ui.checkbox(&mut enabled, label).changed() {
                if enabled {
                    self.config.disabled_formats.retain(|id| id != format.id);
                } else if !self
                    .config
                    .disabled_formats
                    .iter()
                    .any(|id| id == format.id)
                {
                    self.config.disabled_formats.push(format.id.to_string());
                }
                self.persist();
            }
        }

        ui.add_space(12.0);
        ui.separator();
        ui.label(egui::RichText::new(self.i18n.t("settings.associations")).strong());
        ui.label(
            egui::RichText::new(self.i18n.t("settings.associations_hint"))
                .small()
                .color(egui::Color32::GRAY),
        );
        if ui.button(self.i18n.t("settings.set_default")).clicked() {
            match crate::association::set_default(&self.config) {
                Ok(()) => {
                    self.association_status = self.i18n.t("settings.set_default.done");
                }
                Err(err) => {
                    self.association_status = format!("{}: {err}", self.i18n.t("common.error"));
                }
            }
        }
        if !self.association_status.is_empty() {
            ui.label(
                egui::RichText::new(&self.association_status)
                    .small()
                    .color(egui::Color32::GRAY),
            );
        }
    }

    fn settings_passwords(&mut self, ui: &mut egui::Ui) {
        ui.colored_label(
            egui::Color32::from_rgb(0xc8, 0x8a, 0x00),
            self.i18n.t("settings.passwords_warning"),
        );
        ui.label(
            egui::RichText::new(self.i18n.t("settings.passwords_hint"))
                .small()
                .color(egui::Color32::GRAY),
        );
        ui.add_space(6.0);

        let mut remove: Option<usize> = None;
        for (index, password) in self.config.passwords.iter().enumerate() {
            ui.horizontal(|ui| {
                let masked = "*".repeat(password.chars().count().max(1));
                ui.label(egui::RichText::new(masked).monospace());
                if ui.small_button("x").clicked() {
                    remove = Some(index);
                }
            });
        }
        if let Some(index) = remove {
            self.config.passwords.remove(index);
            self.persist();
        }

        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.new_password_input)
                    .password(true)
                    .hint_text(self.i18n.t("settings.password_placeholder"))
                    .desired_width(240.0),
            );
            let submitted = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button(self.i18n.t("extract.add")).clicked() || submitted {
                let password = self.new_password_input.trim().to_string();
                if !password.is_empty() && !self.config.passwords.contains(&password) {
                    self.config.passwords.push(password);
                    self.new_password_input.clear();
                    self.persist();
                }
            }
        });
    }

    fn ui_password_modal(&mut self, ctx: &egui::Context) {
        let Some(prompt) = self.password_prompt.as_ref() else {
            return;
        };
        let name = prompt
            .archive
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| prompt.archive.display().to_string());
        let error = prompt.error.clone();
        let index = prompt.index;

        let mut answer: Option<JobAnswer> = None;
        let mut cancel = false;
        egui::Modal::new(egui::Id::new("password-modal")).show(ctx, |ui| {
            ui.set_min_width(340.0);
            ui.heading(self.i18n.t("password.title"));
            ui.label(
                egui::RichText::new(self.i18n.t("password.for").replace("{name}", &name))
                    .small()
                    .color(egui::Color32::GRAY),
            );
            // Only shown after a submitted password failed.
            if let Some(error) = &error {
                ui.label(
                    egui::RichText::new(error)
                        .small()
                        .color(egui::Color32::from_rgb(0xd9, 0x30, 0x25)),
                );
            }
            ui.add_space(4.0);
            ui.label(self.i18n.t("password.prompt"));
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.password_input)
                    .password(true)
                    .desired_width(300.0),
            );
            let submitted = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button(self.i18n.t("password.submit")).clicked() || submitted {
                    let password = std::mem::take(&mut self.password_input);
                    answer = Some(JobAnswer::Password(password));
                }
                if ui.button(self.i18n.t("password.skip")).clicked() {
                    answer = Some(JobAnswer::Skip);
                }
                if ui.button(self.i18n.t("password.cancel_all")).clicked() {
                    cancel = true;
                }
            });
            if let Some(file) = self.files.get_mut(index) {
                file.status = Status::Running;
            }
        });

        if cancel {
            self.cancel_batch();
        } else if let Some(answer) = answer {
            self.answer_password(answer);
        }
    }

    fn ui_confirm_disabled(&mut self, ctx: &egui::Context) {
        let Some(indices) = self.confirm_disabled.clone() else {
            return;
        };

        let mut extract = false;
        let mut cancel = false;
        egui::Modal::new(egui::Id::new("disabled-modal")).show(ctx, |ui| {
            ui.set_min_width(380.0);
            ui.heading(self.i18n.t("confirm_disabled.title"));
            ui.label(self.i18n.t("confirm_disabled.body"));
            for index in &indices {
                if let Some(file) = self.files.get(*index) {
                    ui.label(
                        egui::RichText::new(format!("  {}", file.path.display()))
                            .small()
                            .monospace(),
                    );
                }
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button(self.i18n.t("confirm_disabled.extract")).clicked() {
                    extract = true;
                }
                if ui.button(self.i18n.t("common.cancel")).clicked() {
                    cancel = true;
                }
            });
        });

        if extract {
            self.confirm_disabled = None;
            let ctx = ctx.clone();
            self.start_batch(&ctx);
        } else if cancel {
            self.confirm_disabled = None;
            self.status_line = self.i18n.t("confirm_disabled.cancelled");
        }
    }

    fn ui_close_confirm(&mut self, ctx: &egui::Context) {
        if !self.confirm_close {
            return;
        }

        let mut stay = false;
        let mut close = false;
        egui::Modal::new(egui::Id::new("close-confirm-modal")).show(ctx, |ui| {
            ui.set_min_width(360.0);
            ui.heading(self.i18n.t("close_confirm.title"));
            ui.label(self.i18n.t("close_confirm.body"));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button(self.i18n.t("close_confirm.close")).clicked() {
                    close = true;
                }
                if ui.button(self.i18n.t("common.cancel")).clicked() {
                    stay = true;
                }
            });
        });

        if stay {
            self.confirm_close = false;
        } else if close {
            self.confirm_close = false;
            self.abort_and_close();
        }
    }

    fn ui_overwrite_modal(&mut self, ctx: &egui::Context) {
        let Some(prompt) = self.overwrite_prompt.as_ref() else {
            return;
        };
        let index = prompt.index;
        let conflicts = prompt.conflicts.clone();

        let mut answer: Option<JobAnswer> = None;
        egui::Modal::new(egui::Id::new("overwrite-modal")).show(ctx, |ui| {
            ui.set_min_width(420.0);
            ui.heading(self.i18n.t("overwrite.title"));
            ui.label(self.i18n.t("overwrite.body"));
            egui::ScrollArea::vertical()
                .id_salt("overwrite-scroll")
                .max_height(160.0)
                .show(ui, |ui| {
                    for path in &conflicts {
                        ui.label(
                            egui::RichText::new(path.display().to_string())
                                .small()
                                .monospace(),
                        );
                    }
                });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button(self.i18n.t("overwrite.confirm")).clicked() {
                    answer = Some(JobAnswer::Overwrite);
                }
                if ui.button(self.i18n.t("overwrite.skip")).clicked() {
                    answer = Some(JobAnswer::Skip);
                }
                if ui.button(self.i18n.t("password.cancel_all")).clicked() {
                    answer = Some(JobAnswer::CancelBatch);
                }
            });
            if let Some(file) = self.files.get_mut(index) {
                file.status = Status::Running;
            }
        });

        if let Some(answer) = answer {
            self.overwrite_prompt = None;
            if let Some(controller) = &self.controller {
                let _ = controller.answer_tx.send(answer);
            }
        }
    }

    fn ui_error_modal(&mut self, ctx: &egui::Context) {
        let Some(message) = self.error_popup.clone() else {
            return;
        };

        let mut close = false;
        egui::Modal::new(egui::Id::new("error-modal")).show(ctx, |ui| {
            ui.set_min_width(420.0);
            ui.set_max_width(680.0);
            ui.heading(
                egui::RichText::new(self.i18n.t("common.error"))
                    .color(egui::Color32::from_rgb(0xd9, 0x30, 0x25)),
            );
            egui::ScrollArea::vertical()
                .id_salt("error-scroll")
                .max_height(260.0)
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(&message).monospace().small());
                });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button(self.i18n.t("log.copy")).clicked() {
                    ui.ctx().copy_text(message.clone());
                }
                if ui.button(self.i18n.t("common.close")).clicked() {
                    close = true;
                }
            });
        });

        if close {
            self.error_popup = None;
        }
    }

    /// Shows a centered hint while files are being dragged over the window.
    fn draw_drop_overlay(&self, ctx: &egui::Context) {
        let hovering = ctx.input(|input| !input.raw.hovered_files.is_empty());
        if !hovering {
            return;
        }
        egui::Area::new(egui::Id::new("drop-overlay"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(egui::RichText::new(self.i18n.t("extract.drop_active")).heading());
                });
            });
    }
}

/// Formats a number of seconds as `M:SS`.
fn format_duration(total_secs: u64) -> String {
    let minutes = total_secs / 60;
    let seconds = total_secs % 60;
    format!("{minutes}:{seconds:02}")
}

/// Application logo, embedded in the binary for the About window.
const LOGO_PNG: &[u8] = include_bytes!("../assets/ouch-decompress-gui-256.png");

/// Decodes the embedded logo into an egui texture, if the PNG is valid.
fn load_logo(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let image = image::load_from_memory_with_format(LOGO_PNG, image::ImageFormat::Png).ok()?;
    let rgba = image.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let pixels = egui::ColorImage::from_rgba_unmultiplied(size, &rgba);
    Some(ctx.load_texture("app-logo", pixels, egui::TextureOptions::LINEAR))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    #[derive(Debug)]
    struct FakeDroppedFile(PathBuf);

    impl egui::DroppedFile for FakeDroppedFile {
        fn path(&self) -> &Path {
            &self.0
        }

        #[cfg(not(target_arch = "wasm32"))]
        fn bytes(&self) -> Result<Vec<u8>, String> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn reads_dropped_files_from_raw_input() {
        let ctx = egui::Context::default();
        let handle: egui::DroppedFileHandle =
            Arc::new(FakeDroppedFile(PathBuf::from("/tmp/example.zip")));
        let raw = egui::RawInput {
            dropped_files: vec![handle],
            ..Default::default()
        };

        let mut seen = 0;
        let mut output = ctx.run_ui(raw, |_ui| {
            seen = ctx.input(|input| input.raw.dropped_files.len());
        });
        output.textures_delta.clear();
        assert_eq!(seen, 1, "dropped file should be visible in input.raw");
    }

    #[test]
    fn embedded_logo_decodes() {
        let image = image::load_from_memory_with_format(LOGO_PNG, image::ImageFormat::Png).unwrap();
        assert!(image.width() >= 64 && image.height() >= 64);
    }

    #[test]
    fn loads_logo_texture() {
        let ctx = egui::Context::default();
        assert!(load_logo(&ctx).is_some());
    }

    #[test]
    fn gui_versions_are_injected() {
        assert_ne!(env!("EGUI_VERSION"), "?");
        assert_ne!(env!("EFRAME_VERSION"), "?");
        assert!(!env!("EGUI_VERSION").is_empty());
        assert!(!env!("EFRAME_VERSION").is_empty());
    }

    #[test]
    fn formats_duration_as_minutes_and_seconds() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(65), "1:05");
        assert_eq!(format_duration(3600), "60:00");
    }
}
