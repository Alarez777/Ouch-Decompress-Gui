# AGENTS.md

Guidance for AI coding agents working on this repository.

## Project

`ouch-decompress-gui` is an extract-only archive GUI for Linux,
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
- `src/app.rs` — UI state and rendering; Settings/Log/About are modal overlays
  inside the main window, which grows to fit them and restores its size when
  they close.
- `src/job.rs` — worker pool that extracts several archives at once; prompts
  are serialized, and it talks to the UI only through channels.
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
- Do not re-introduce independent OS windows for Settings/Log/About: they are
  modal overlays inside the main window, and the window size is fitted and
  restored around them (`request_window_fit`/`close_view` in `src/app.rs`).
- Drag & drop only works on X11; upstream `winit` does not implement it on
  Wayland. Do not reintroduce the patched `winit`/`egui` fork until it lands
  upstream, because it makes the interface unresponsive.
- The bundled `ouch` is a static-PIE binary, so it must be copied into the
  AppDir *after* `linuxdeploy` runs (see `packaging/build-appimage.sh`).
- Extraction progress is *estimated*: `ouch` exposes no progress, so each worker
  polls its child's `/proc/<pid>/io` `rchar` and divides it by the archive size
  (`src/job.rs`). Keep it best-effort and Linux-only.

## Rust quality bar

The Rust here follows the `rust-best-practices` skill from BeautifulRust
(https://github.com/morganlinton/BeautifulRust). Load it before writing,
refactoring or explaining Rust, `rust-review` before reviewing a file, module
or diff (it produces a prioritized `Must fix` / `Should fix` / `Consider`
report with `file:line` and the rule each finding breaks), and `rust-port` only
when translating code from another language.

- This is **application code**: concrete types, `anyhow` with `.context(...)`,
  no speculative generics or trait abstractions.
- No `unwrap()` outside `#[cfg(test)]`; use `?` or `expect("why it cannot
  fail")`.
- Suppressions must explain themselves and expire when the lint stops firing:
  `#[expect(clippy::lint, reason = "...")]`, never a bare `#[allow]`.
- Every `unsafe` block carries a `// SAFETY:` comment saying why the invariants
  hold (the `libc` calls in `src/multivolume.rs` and `src/system.rs`).
- No `#![deny(warnings)]` in source; warnings are enforced with
  `cargo clippy --all-targets -- -D warnings` instead.
- No `bool` parameters in signatures shared across modules: use a two-variant
  enum (`ArchiveKind`, `PromptScope`, `Flush`, `Startup`) or two functions.
- Public items get `///` docs with a one-line summary, plus `# Errors` /
  `# Panics` when they apply. Prefer `format!` over `+`.

### Lints

The BeautifulRust lint set lives in `Cargo.toml` (`[lints.rust]` and
`[lints.clippy]`) with `clippy.toml` next to it (`max-fn-params-bools = 0`, and
unwrap/expect/print allowed inside tests). It is adapted to a binary crate:
`unreachable_pub` is omitted because every `pub` item in a binary is
unreachable from outside, and the warning groups (`all`, `pedantic`) are
deliberately not enabled yet. Adding a `#[allow]` that the `[lints]` table
already covers, or a numeric `as` cast in UI code, needs a `#[expect]` with a
reason.

## Branching and releases

- `develop` is the integration branch; `main` holds released code.
- **Every change starts on its own branch.** Never commit directly to `develop`
  or `main`: branch off `develop` using a descriptive prefix (`feature/...`,
  `fix/...`, `docs/...`, `chore/...`) and merge it back into `develop`.
- Merge `develop` into `main` only when preparing a release, then push a tag
  `vX.Y.Z`. `.github/workflows/release.yml` builds the AppImages and creates the
  GitHub release.
- Before tagging, bump `Cargo.toml` to the release version and add a matching
  section to `CHANGELOG.md`; the release body is taken from that section.
- The AppImages embed `gh-releases-zsync` update information and the release
  ships the matching `.zsync` files, so AppImage managers (Gear Lever,
  AppImageUpdate) can update an installed AppImage from GitHub Releases.
