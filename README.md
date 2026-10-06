# Ouch Decompress GUI

A simple, Keka-like **archive extractor** for Linux, powered by
[ouch](https://github.com/ouch-org/ouch).

**This app only decompresses.** It is deliberately focused on extraction;
creating or editing archives is out of scope.

The intended workflow is the one you already use in your file manager:

1. **Associate the extensions** you care about, once, from the settings.
2. **Double-click** an archive to extract it with this app, or
   **select several archives and press Enter** (i.e. "Open with" this app) to
   extract them all in one batch.

If an archive is password-protected, the app asks for the password, remembers
it for the rest of the batch, and retries automatically on the following
archives. Everything is written in Rust and ships as a small self-contained
AppImage.

## Features

- Extract archives by double-clicking them, or by opening several at once
  (native file-manager integration).
- Multiple extraction strategies:
  - **Always create a folder** named after the archive.
  - **Always extract here** (into the archive's folder).
  - **Smart**: create a folder only when the archive has multiple root entries.
- Password handling: saved passwords are tried automatically, otherwise the
  app prompts and reuses the first working password for the whole batch. The
  prompt only shows an error after a wrong password.
- Overwrite handling: ask, overwrite, rename with numbering, or skip.
- After a successful extraction the source archive can be moved to the trash,
  deleted permanently, or kept (default: trash). If the filesystem has no
  trash, a configurable fallback (delete permanently or do nothing) is used.
- Choose which formats may be extracted without confirmation.
- Associate this app with the supported file extensions, and set it as the
  default handler in one click.
- Desktop notifications when a batch finishes, including the resulting name
  when it is not obvious from the archive (for example
  `photos.zip -> vacation`), plus an "Open folder" shortcut per file.
- Dedicated Log window with copy-to-clipboard, and errors shown in a popup.
- Add files with the **Add file** button (native file picker).
- Settings in their own window, split into **General**, **Formats** and
  **Passwords** tabs, with dropdowns for the mutually exclusive options.
  Changes are saved automatically, and **General** has a **Reset options**
  button.
- Adjustable interface size (percentage), language (System, English, Spanish),
  and theme (System, Light, Dark) with desktop detection.
- About window with the app and `ouch` versions, a link to the GitHub project,
  and a background check for new releases.
- Lightweight and fast: a single Rust binary plus the bundled `ouch` tool.

## Supported formats

`tar` (`tgz`, `tbz`, `tbz2`, `tbz3`, `tlz4`, `txz`, `tlzma`, `tsz`, `tzst`,
`tlz`, `cbt`), `zip` (`cbz`, `epub`), `7z` (`cb7`), `rar` (`cbr`), `gz`, `bz`,
`bz2`, `bz3`, `xz`, `lzma`, `lz`, `lz4`, `sz`, `zst` and `br`.

> RAR is decompression/listing only, due to the format's licensing.

## Known limitation: drag & drop on Wayland

Dragging and dropping files onto the window **does not work on Wayland yet**.
Wayland supports it, but the released `winit` that the UI toolkit uses does not
implement it on its Wayland backend (only on X11).

A patched `winit`/`egui` fork (the one used by
[ZapFast](https://github.com/crmne/zapfast)) does implement it, and it was
tested here. It restored drag & drop, but it made the secondary windows
(Settings/Log/About) feel unresponsive, so it was left out until the fix lands
upstream. On X11 sessions drag & drop works.

Until then, use the **Add file** button, or open the archives from your file
manager (double-click / select several and press Enter).

## Architecture

`ouch` is **not** usable as a library (it is a binary-only crate), so this app
runs the bundled `ouch` executable as a subprocess:

- `ouch list -A -q <archive>` is used to validate passwords and to drive the
  **Smart** extraction mode (it counts the archive's root entries).
- `ouch decompress` performs the actual extraction, using either the native
  stem-folder behavior or `--here`, always relative to the archive's parent
  directory.
- Passwords are passed with `-p`; `ouch` never prompts on its own.

## Building

```sh
cargo build --release
```

To also have a working extraction backend during development, install `ouch`
(make sure it is on `PATH`) or place an `ouch` binary next to the built
executable.

## Running

```sh
# Open the main window
ouch-decompress-gui

# Open the settings directly
ouch-decompress-gui --settings

# Extract specific archives immediately
ouch-decompress-gui file.zip another.tar.gz
```

## Configuration

Settings are stored in JSON at:

```
~/.config/ouch-decompress-gui/config.json
```

Passwords are stored **in plain text** in that file. The UI shows a warning
about this.

## File associations

The **Formats** tab can associate the app at user level (no root required):

- **Associate checked formats** writes
  `~/.local/share/applications/ouch-decompress-gui.desktop`,
  `~/.local/share/mime/packages/ouch-decompress-gui.xml` and the icons, then
  refreshes the XDG databases.
- **Set as default** makes the app the default handler for every checked format
  in one step.
- **Remove associations** undoes it.

## Releases (AppImage)

Releases are built automatically by GitHub Actions
(`.github/workflows/release.yml`) **only when a tag like `v0.1.0` is pushed**.
Each release contains a self-contained AppImage for `x86_64` and `aarch64`; the
bundled `ouch` binary lives under `usr/bin/ouch` and is resolved at runtime
through `$APPDIR`.

To cut a release:

```sh
git tag v0.1.0
git push origin v0.1.0
```

To build an AppImage locally:

```sh
# For the host architecture (x86_64 or aarch64)
packaging/build-appimage.sh

# Explicitly select an architecture
packaging/build-appimage.sh aarch64
```

The script downloads `linuxdeploy` and `appimagetool` into `.cache/`, and the
`ouch` release into the same cache, so subsequent runs are offline. Set
`SKIP_LINUXDEPLOY=1` to produce a smaller AppImage that relies on system
libraries instead of bundling them.

The bundled `ouch` is a statically linked musl binary. `linuxdeploy` rewrites
the `rpath` of every ELF it finds, which corrupts static-PIE binaries
(SIGSEGV), so the script copies `ouch` into the AppDir *after* `linuxdeploy`
has run. At runtime the app also validates `ouch --version` and falls back to a
system `ouch` if the bundled one cannot run.

The result is written to `dist/Ouch-Decompress-Gui-<arch>.AppImage`.

## License

MIT. See [LICENSE](LICENSE).

`ouch` is distributed under the MIT license; RAR support relies on the
non-free `unrar` library, so builds including RAR cannot be relicensed as
fully free software.
