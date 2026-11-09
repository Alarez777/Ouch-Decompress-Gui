# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- Changing the interface size no longer leaves the window smaller than the
  layout needs. The window is fitted in points at startup and after a zoom
  change, so the per-file **Cancel** button stays visible at every scale.
  Long file names are truncated to keep the row controls reachable, and the
  restart hint is gone because it is no longer necessary.

## [0.4.0] - 2026-11-07

### Added

- Archives are now extracted in parallel. Settings → General has a new
  **Parallel extractions** spinbox (default 4, maximum 16) that sets how many
  archives are unpacked at the same time. Passwords and destination prompts are
  shared and shown one at a time; each distinct password is remembered and tried
  on the remaining archives before asking again. Each archive is extracted into
  a hidden temporary directory inside the destination and moved into place only
  when the extraction succeeds, so a wrong password never leaves partial files.
  Each archive gets its own progress bar.

## [0.3.2] - 2026-11-01

### Added

- Changing the interface size now shows a hint after 500 ms offering to restart
  the app, since the new scale only lays the window out correctly on a fresh
  start. Restarting is disabled while an extraction is running, and the hint
  can be dismissed to keep using the current session.

### Fixed

- Failures shown in the status line, such as a config that cannot be written,
  now use a red **Error** prefix instead of looking like ordinary information.

## [0.3.1] - 2026-10-31

### Added

- Settings now include an extraction destination: next to each archive (default),
  a fixed folder, ask once when the batch starts, or ask for every file. The
  extraction mode is applied inside the chosen destination.

### Changed

- The destination prompt now focuses **Extract here**, so `Enter` confirms it.
  With **Ask for every file**, the first archive suggests its own directory and
  the following ones suggest the last chosen directory.

## [0.3.0] - 2026-10-25

### Added

- Settings, Log and About now open on top of the main window and block it until closed. The window grows to fit the view and returns to its previous size afterwards, so the layout stays readable without separate windows. `Esc` or the `×` button closes them.

### Changed

- Updated to egui/eframe 0.37.
- Removed the separate Settings/Log/About windows, which were unreliable on some compositors. The window title still shows the active view.
- Vsync is no longer disabled, since there is only one window now.

### Fixed

- The window now returns to its previous size after closing Settings, Log or About on Wayland, where the previous implementation could not read the window size and fell back to a fixed one.
- The AppImage is now built against glibc 2.35, so it starts on current distributions instead of requiring glibc 2.43.

## [0.2.7] - 2026-10-09

### Added

- When running from an AppImage, the Formats settings tab can create or remove a stand-alone `.desktop` entry (plus MIME package and icons), for installs that do not use an AppImage manager such as Gear Lever.

## [0.2.6] - 2026-10-09

### Added

- The progress bar now lives in the row of the file being extracted (just above its name), next to the Cancel button and the elapsed time, and scrolls to stay visible on long lists.
- A smoothed write-speed readout (e.g. `12.3 MB/s`) to the right of the bar.
- An estimated time left (ETA) next to the elapsed time, computed from the progress rate.

### Changed

- The "joining parts" phase is highlighted in green and the bar reads "JOINING", so it is clear the archive is not being extracted yet.

## [0.2.5] - 2026-10-08

### Added

- Support for numeric split archives (`name.7z.001`, `name.7z.002`, ...): the volumes are joined into a temporary file — with a "Joining parts" phase — and extracted as a single archive.
- Missing volumes of a numeric split set are detected and reported before extracting anything.

### Fixed

- The Cancel button now stops the running extraction immediately by terminating the `ouch` process, and also interrupts the concatenation of split volumes and the password prompts.

## [0.2.4] - 2026-10-08

### Added

- Incomplete multi-volume RAR sets are detected — missing first, middle or last volumes — and reported with the missing file names before extracting anything.
- The Passwords tab shows saved passwords in plain text and can add several at once (one per line; empty and duplicate entries are skipped).
- Saved passwords can be listed alphabetically (default) or in insertion order.

### Changed

- Saved passwords are shown in a framed, scrollable block so they stand out.
- Dialogs no longer grow wider than the window.
- Removed the RAR licensing note from the README, since the app only extracts.

## [0.2.3] - 2026-10-07

### Added

- Support for multi-volume RAR sets (`name.part1.rar`, ...): the whole set is queued as one job, all parts are removed after extraction, and the running entry shows the volume being read (`k/N`).
- Notification policy: Always, Only on failures (default) or Never.
- The Log window now shows every `ouch` command that runs (passwords masked).

### Changed

- The password field is focused automatically when its prompt appears.
- When `ouch list` fails — it can crash on some archives — the plain listing is retried, and if that fails too the password is requested before giving up, so Smart mode keeps working.
- Finish notifications omit the failure count when there are no failures, and stay quiet when nothing was processed.
- Spanish: the Log button and window now read "Log".

## [0.2.2] - 2026-10-07

### Changed

- File associations: removed the "Associate" and "Remove associations" buttons. "Set as default" now targets the desktop entry provided by the AppImage manager (e.g. Gear Lever), so no duplicate menu entry is created. A duplicate left by older versions is removed automatically.

### Fixed

- The fallback desktop entry now includes its `Icon`, so it no longer shows up without an icon.

## [0.2.1] - 2026-10-07

### Added

- AppImages now embed `gh-releases-zsync` update information and ship a matching `.zsync` file, so AppImage managers such as Gear Lever can update an installed AppImage automatically from GitHub Releases.

## [0.2.0] - 2026-10-07

### Added

- Per-archive progress bar with elapsed time and a Cancel button.
- Confirmation when closing the window during extraction: it stops the batch and terminates the running `ouch` process.
- Desktop notification when closing aborts an extraction.
- Application logo and the egui/eframe versions in the About window.
- `AGENTS.md` with guidance for contributors and AI agents.

### Changed

- The native file picker runs on a background thread, so the window no longer reports as "not responding"; the button is disabled while it is open and the picker is closed when the app exits.
- README: independent-project notice, logo next to the title, and emphasis on being written in Rust and lightweight.

## [0.1.0] - 2026-10-05

### Added

- First release: a lightweight, extract-only archive GUI for Linux, driven by the bundled `ouch` binary.
