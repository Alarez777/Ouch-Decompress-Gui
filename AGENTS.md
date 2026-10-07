# AGENTS.md

Guidance for AI coding agents working on this repository.

## Project

`ouch-decompress-gui` is an extract-only, Keka-like archive GUI for Linux,
written in Rust with `egui`/`eframe`. `ouch` is a binary-only crate, so the app
runs the bundled `ouch` executable as a subprocess; every interaction with it
(and all parsing of its output) lives in `src/ouch.rs`.

This is an **independent, unofficial** project, not affiliated with the `ouch`
authors.

## Commands

- Format: `cargo fmt`
- Lint (must pass): `cargo clippy --all-targets -- -D warnings`
- Test (must pass): `cargo test --release --locked`
- Build AppImage: `bash packaging/build-appimage.sh x86_64` (or `aarch64`)
- Run: `cargo run -- [ARCHIVE...]` or `cargo run -- --settings`

Always run `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and
`cargo test --release --locked` before considering a change done.

## Architecture

- `src/main.rs` — CLI arguments, window creation, help/version.
- `src/app.rs` — UI state and rendering (main window plus separate
  Settings/Log/About viewports).
- `src/job.rs` — background batch extraction worker; talks to the UI only
  through channels.
- `src/ouch.rs` — the single place that builds `ouch` commands and parses them.
- `src/modes.rs` — folder/here/smart target selection.
- `src/formats.rs` — format table and extension/MIME mapping.
- `src/association.rs` — XDG desktop entry, MIME package and icons.
- `src/theme.rs` — light/dark theme and desktop theme detection.
- `src/i18n.rs` + `locales/{en,es}.json` — translations.
- `src/config.rs` — JSON config in `~/.config/ouch-decompress-gui/`.
- `src/update.rs` — GitHub release check.
- `src/system.rs` — `xdg-open`, `notify-send`, `zenity` and trash helpers.

## Conventions

- Do not add comments unless they explain a non-obvious *why*.
- Use official crates.io crates only; never add `[patch]` or git dependencies.
- Keep `locales/en.json` and `locales/es.json` key-aligned.
- Do not re-enable `vsync` in `NativeOptions`: it makes the secondary windows
  (Settings/Log/About) laggy.
- Drag & drop only works on X11; upstream `winit` does not implement it on
  Wayland. Do not reintroduce the patched `winit`/`egui` fork until it lands
  upstream, because it makes the secondary windows unresponsive.
- The bundled `ouch` is a static-PIE binary, so it must be copied into the
  AppDir *after* `linuxdeploy` runs (see `packaging/build-appimage.sh`).
- Extraction progress is *estimated*: `ouch` exposes no progress, so the worker
  polls the child's `/proc/<pid>/io` `rchar` and divides it by the archive size
  (`src/job.rs`). Keep it best-effort and Linux-only.

## Branching and releases

- `develop` is the integration branch; `main` holds released code.
- Branch off `develop` using a descriptive prefix (`feature/...`, `fix/...`,
  `docs/...`) and merge back into `develop`.
- Merge `develop` into `main` only when preparing a release, then push a tag
  `vX.Y.Z`. `.github/workflows/release.yml` builds the AppImages and creates the
  GitHub release.
- Before tagging, bump `Cargo.toml` to the release version and add a matching
  section to `CHANGELOG.md`; the release body is taken from that section.
- The AppImages embed `gh-releases-zsync` update information and the release
  ships the matching `.zsync` files, so AppImage managers (Gear Lever,
  AppImageUpdate) can update an installed AppImage from GitHub Releases.
