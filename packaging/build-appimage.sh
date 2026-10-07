#!/usr/bin/env bash
#
# Builds a self-contained AppImage for Ouch Decompress.
#
# The resulting bundle contains:
#   * the Rust GUI binary,
#   * a pinned `ouch` release binary under usr/bin/ouch,
#   * the desktop entry and icon.
#
# Usage:
#   packaging/build-appimage.sh [x86_64|aarch64]
#
# Environment:
#   OUCH_VERSION   overrides the pinned ouch version (default: 0.8.3)
#   SKIP_LINUXDEPLOY=1  skip bundling shared libraries (smaller, less portable)
#
set -euo pipefail

ROOT="$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)"
cd "$ROOT"

ARCH="${1:-$(uname -m)}"
OUCH_VERSION="${OUCH_VERSION:-0.8.3}"
TOOLS_DIR="$ROOT/.cache/appimage-tools"
DIST_DIR="$ROOT/dist"
APPDIR="$DIST_DIR/Ouch-Decompress-Gui.AppDir"

case "$ARCH" in
    x86_64|amd64)
        APPIMAGE_ARCH="x86_64"
        OUCH_TRIPLE="x86_64-unknown-linux-musl"
        RUST_TARGET="x86_64-unknown-linux-gnu"
        ;;
    aarch64|arm64)
        APPIMAGE_ARCH="aarch64"
        OUCH_TRIPLE="aarch64-unknown-linux-musl"
        RUST_TARGET="aarch64-unknown-linux-gnu"
        ;;
    *)
        echo "unsupported architecture: $ARCH" >&2
        exit 1
        ;;
esac

mkdir -p "$TOOLS_DIR" "$DIST_DIR"

download() {
    # download <url> <destination>
    local url="$1" dest="$2"
    if [ -f "$dest" ]; then
        return 0
    fi
    echo "downloading $(basename "$dest")"
    curl -fsSL "$url" -o "$dest"
    chmod +x "$dest"
}

echo "==> Building the GUI (release)"
if [ "$(uname -m)" = "$ARCH" ]; then
    cargo build --release
    GUI_BIN="$ROOT/target/release/ouch-decompress-gui"
else
    cargo build --release --target "$RUST_TARGET"
    GUI_BIN="$ROOT/target/$RUST_TARGET/release/ouch-decompress-gui"
fi
[ -x "$GUI_BIN" ] || { echo "missing GUI binary: $GUI_BIN" >&2; exit 1; }

echo "==> Fetching ouch $OUCH_VERSION ($OUCH_TRIPLE)"
OUCH_TARBALL="$TOOLS_DIR/ouch-$OUCH_VERSION-$OUCH_TRIPLE.tar.gz"
download \
    "https://github.com/ouch-org/ouch/releases/download/$OUCH_VERSION/ouch-$OUCH_TRIPLE.tar.gz" \
    "$OUCH_TARBALL"
OUCH_EXTRACT="$TOOLS_DIR/ouch-$OUCH_VERSION-$OUCH_TRIPLE"
mkdir -p "$OUCH_EXTRACT"
tar -xzf "$OUCH_TARBALL" -C "$OUCH_EXTRACT"
OUCH_BIN="$(find "$OUCH_EXTRACT" -type f -name ouch | head -n1)"
[ -n "$OUCH_BIN" ] || { echo "could not find ouch in tarball" >&2; exit 1; }

echo "==> Assembling AppDir"
rm -rf "$APPDIR"
mkdir -p \
    "$APPDIR/usr/bin" \
    "$APPDIR/usr/share/applications" \
    "$APPDIR/usr/share/icons/hicolor/256x256/apps" \
    "$APPDIR/usr/share/icons/hicolor/512x512/apps"

install -m 0755 "$GUI_BIN" "$APPDIR/usr/bin/ouch-decompress-gui"
install -m 0644 "$ROOT/packaging/ouch-decompress-gui.desktop" \
    "$APPDIR/ouch-decompress-gui.desktop"
install -m 0644 "$ROOT/packaging/ouch-decompress-gui.desktop" \
    "$APPDIR/usr/share/applications/ouch-decompress-gui.desktop"
install -m 0644 "$ROOT/assets/ouch-decompress-gui.png" \
    "$APPDIR/ouch-decompress-gui.png"
install -m 0644 "$ROOT/assets/ouch-decompress-gui.png" \
    "$APPDIR/usr/share/icons/hicolor/512x512/apps/ouch-decompress-gui.png"
install -m 0644 "$ROOT/assets/ouch-decompress-gui-256.png" \
    "$APPDIR/usr/share/icons/hicolor/256x256/apps/ouch-decompress-gui.png"
cp "$ROOT/assets/ouch-decompress-gui-256.png" "$APPDIR/.DirIcon"
install -m 0755 "$ROOT/packaging/AppRun" "$APPDIR/AppRun"

if [ "${SKIP_LINUXDEPLOY:-0}" != "1" ]; then
    echo "==> Bundling shared libraries with linuxdeploy"
    LINUXDEPLOY="$TOOLS_DIR/linuxdeploy-$APPIMAGE_ARCH.AppImage"
    download \
        "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-$APPIMAGE_ARCH.AppImage" \
        "$LINUXDEPLOY"
    if APPIMAGE_EXTRACT_AND_RUN=1 "$LINUXDEPLOY" \
        --appdir "$APPDIR" \
        --desktop-file "$APPDIR/ouch-decompress-gui.desktop" \
        --icon-file "$APPDIR/ouch-decompress-gui.png"; then
        echo "linuxdeploy: libraries bundled"
    else
        echo "warning: linuxdeploy failed; the AppImage will rely on system libraries" >&2
    fi
fi

# ouch is a statically linked musl binary. linuxdeploy's rpath patching breaks
# static-pie binaries (SIGSEGV), so it must be copied in AFTER linuxdeploy has
# finished scanning usr/bin.
echo "==> Bundling ouch (post-linuxdeploy)"
install -m 0755 "$OUCH_BIN" "$APPDIR/usr/bin/ouch"

echo "==> Creating AppImage"
APPIMAGETOOL="$TOOLS_DIR/appimagetool-$APPIMAGE_ARCH.AppImage"
download \
    "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$APPIMAGE_ARCH.AppImage" \
    "$APPIMAGETOOL"

# Embed update information so AppImage managers (e.g. Gear Lever, which reads
# the ELF `.upd_info` section) can update the installed file from GitHub
# Releases. appimagetool bundles its own zsyncmake and also generates the
# matching `Ouch-Decompress-Gui-<arch>.AppImage.zsync`.
GH_REPO="${GITHUB_REPOSITORY:-Alarez777/Ouch-Decompress-Gui}"
GH_OWNER="${GH_REPO%%/*}"
GH_NAME="${GH_REPO##*/}"
UPDATE_INFO="gh-releases-zsync|${GH_OWNER}|${GH_NAME}|latest|Ouch-Decompress-Gui-*${APPIMAGE_ARCH}.AppImage.zsync"

OUTPUT="$DIST_DIR/Ouch-Decompress-Gui-$APPIMAGE_ARCH.AppImage"
ARCH="$APPIMAGE_ARCH" APPIMAGE_EXTRACT_AND_RUN=1 \
    "$APPIMAGETOOL" --updateinformation "$UPDATE_INFO" "$APPDIR" "$OUTPUT"

# zsyncmake may drop the `.zsync` in the current directory instead of next to
# the AppImage; move it so the release upload picks it up.
if [ ! -f "$OUTPUT.zsync" ] && [ -f "$(basename "$OUTPUT").zsync" ]; then
    mv "$(basename "$OUTPUT").zsync" "$OUTPUT.zsync"
fi

echo "==> Done: $OUTPUT"
