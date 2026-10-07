# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.2] - 2026-10-07

### Changed

- File associations: removed the "Associate" and "Remove associations" buttons.
  "Set as default" now targets the desktop entry provided by the AppImage
  manager (e.g. Gear Lever), so no duplicate menu entry is created. A duplicate
  left by older versions is removed automatically.

### Fixed

- The fallback desktop entry now includes its `Icon`, so it no longer shows up
  without an icon.

## [0.2.1] - 2026-10-07

### Added

- AppImages now embed `gh-releases-zsync` update information and ship a
  matching `.zsync` file, so AppImage managers such as Gear Lever can update an
  installed AppImage automatically from GitHub Releases.

## [0.2.0] - 2026-10-07

### Added

- Per-archive progress bar with elapsed time and a Cancel button.
- Confirmation when closing the window during extraction: it stops the batch and
  terminates the running `ouch` process.
- Desktop notification when closing aborts an extraction.
- Application logo and the egui/eframe versions in the About window.
- `AGENTS.md` with guidance for contributors and AI agents.

### Changed

- The native file picker runs on a background thread, so the window no longer
  reports as "not responding"; the button is disabled while it is open and the
  picker is closed when the app exits.
- README: independent-project notice, logo next to the title, and emphasis on
  being written in Rust and lightweight.

## [0.1.0] - 2026-10-05

### Added

- First release: a lightweight, extract-only archive GUI for Linux,
  driven by the bundled `ouch` binary.
