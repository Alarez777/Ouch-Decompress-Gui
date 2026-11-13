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
#   OUCH_VERSION   overrides the pinned ouch version (default: from versions.env)
#   URUNTIME_VERSION  overrides the pinned uruntime version (default: from versions.env)
#   URUNTIME_SHA256   expected SHA-256 for a custom runtime version
#   SKIP_LINUXDEPLOY=1  skip bundling shared libraries (smaller, less portable)
#
# Pinned build inputs and their SHA-256 values live in packaging/versions.env.
#
set -euo pipefail

ROOT="$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)"
cd "$ROOT"

# shellcheck source=packaging/versions.env
source "$ROOT/packaging/versions.env"

ARCH="${1:-$(uname -m)}"
# The release URL and the checksum pin use the `v` prefix; accept both forms.
URUNTIME_VERSION="v${URUNTIME_VERSION#v}"
# appimagetool builds a SquashFS payload, so the runtime must be a SquashFS
# variant. Keep it fixed to avoid a runtime/payload mismatch.
URUNTIME_VARIANT="appimage-squashfs-lite"
URUNTIME_SHA256="${URUNTIME_SHA256:-}"
TOOLS_DIR="$ROOT/.cache/appimage-tools"
DIST_DIR="$ROOT/dist"
APPDIR="$DIST_DIR/Ouch-Decompress-Gui.AppDir"

case "$ARCH" in
    x86_64|amd64)
        APPIMAGE_ARCH="x86_64"
        OUCH_TRIPLE="x86_64-unknown-linux-musl"
        RUST_TARGET="x86_64-unknown-linux-gnu"
        URUNTIME_SHA256_PINNED="$URUNTIME_SHA256_X86_64"
        APPIMAGETOOL_SHA256_PINNED="$APPIMAGETOOL_SHA256_X86_64"
        LINUXDEPLOY_SHA256_PINNED="$LINUXDEPLOY_SHA256_X86_64"
        ;;
    aarch64|arm64)
        APPIMAGE_ARCH="aarch64"
        OUCH_TRIPLE="aarch64-unknown-linux-musl"
        RUST_TARGET="aarch64-unknown-linux-gnu"
        URUNTIME_SHA256_PINNED="$URUNTIME_SHA256_AARCH64"
        APPIMAGETOOL_SHA256_PINNED="$APPIMAGETOOL_SHA256_AARCH64"
        LINUXDEPLOY_SHA256_PINNED="$LINUXDEPLOY_SHA256_AARCH64"
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

verify_sha256() {
    # verify_sha256 <file> <expected>
    local file="$1" expected="$2"
    if ! printf '%s  %s\n' "$expected" "$file" | sha256sum -c -; then
        echo "SHA-256 mismatch for $file; remove it and retry" >&2
        exit 1
    fi
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

echo "==> Fetching uruntime $URUNTIME_VERSION ($URUNTIME_VARIANT, $APPIMAGE_ARCH)"
URUNTIME_BIN="$TOOLS_DIR/uruntime-$URUNTIME_VERSION-$URUNTIME_VARIANT-$APPIMAGE_ARCH"
download \
    "https://github.com/VHSgunzo/uruntime/releases/download/$URUNTIME_VERSION/uruntime-$URUNTIME_VARIANT-$APPIMAGE_ARCH" \
    "$URUNTIME_BIN"

if [ -n "$URUNTIME_SHA256" ]; then
    EXPECTED_URUNTIME_SHA256="$URUNTIME_SHA256"
elif [ "$URUNTIME_VERSION" = "$URUNTIME_PINNED_VERSION" ]; then
    EXPECTED_URUNTIME_SHA256="$URUNTIME_SHA256_PINNED"
else
    echo "set URUNTIME_SHA256 to verify uruntime $URUNTIME_VERSION" >&2
    exit 1
fi

verify_sha256 "$URUNTIME_BIN" "$EXPECTED_URUNTIME_SHA256"

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
    echo "==> Bundling shared libraries with linuxdeploy $LINUXDEPLOY_VERSION"
    LINUXDEPLOY="$TOOLS_DIR/linuxdeploy-$LINUXDEPLOY_VERSION-$APPIMAGE_ARCH.AppImage"
    download \
        "https://github.com/linuxdeploy/linuxdeploy/releases/download/$LINUXDEPLOY_VERSION/linuxdeploy-$APPIMAGE_ARCH.AppImage" \
        "$LINUXDEPLOY"
    verify_sha256 "$LINUXDEPLOY" "$LINUXDEPLOY_SHA256_PINNED"
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

echo "==> Creating AppImage with appimagetool $APPIMAGETOOL_VERSION"
APPIMAGETOOL="$TOOLS_DIR/appimagetool-$APPIMAGETOOL_VERSION-$APPIMAGE_ARCH.AppImage"
download \
    "https://github.com/AppImage/appimagetool/releases/download/$APPIMAGETOOL_VERSION/appimagetool-$APPIMAGE_ARCH.AppImage" \
    "$APPIMAGETOOL"
verify_sha256 "$APPIMAGETOOL" "$APPIMAGETOOL_SHA256_PINNED"

# uruntime replaces the type2 runtime. The squashfs-lite variant carries only
# the SquashFS mount/extract helpers; appimagetool still builds the SquashFS
# payload and writes update information into uruntime's `.upd_info` section.
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
    "$APPIMAGETOOL" --runtime-file "$URUNTIME_BIN" \
    --updateinformation "$UPDATE_INFO" "$APPDIR" "$OUTPUT"

# zsyncmake may drop the `.zsync` in the current directory instead of next to
# the AppImage; move it so the release upload picks it up. Always replace a
# stale file from a previous build.
if [ -f "$(basename "$OUTPUT").zsync" ]; then
    mv -f "$(basename "$OUTPUT").zsync" "$OUTPUT.zsync"
fi

echo "==> Done: $OUTPUT"
