//! User-level file associations.
//!
//! Setting the app as the default handler only runs `xdg-mime default` for the
//! enabled MIME types. The desktop entry itself is normally provided by the
//! AppImage manager (e.g. Gear Lever); this module detects it and uses it, so
//! it does not create a second, duplicate entry.
//!
//! The legacy entry/mime/icons that older versions installed are only written
//! as a fallback when no other launcher exists, and are removed when a real one
//! is found. Everything lives in the user's home, so no root is required.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use crate::config::Config;
use crate::formats;

/// Base name of the generated desktop entry.
pub const DESKTOP_FILE: &str = "ouch-decompress-gui.desktop";
/// Base name of the generated MIME package.
pub const MIME_FILE: &str = "ouch-decompress-gui.xml";
/// Icon name referenced by the desktop entry.
const ICON_NAME: &str = "ouch-decompress-gui";

/// Icon assets embedded in the binary and installed into the user theme.
const ICON_256: &[u8] = include_bytes!("../assets/ouch-decompress-gui-256.png");
const ICON_512: &[u8] = include_bytes!("../assets/ouch-decompress-gui.png");

/// Path of the executable that the desktop entry should launch.
///
/// Inside an AppImage the real artifact is pointed to by `$APPIMAGE`; running
/// from a build tree falls back to the current executable.
pub fn app_exec_path() -> PathBuf {
    if let Ok(appimage) = std::env::var("APPIMAGE") {
        if !appimage.is_empty() {
            return PathBuf::from(appimage);
        }
    }
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("ouch-decompress-gui"))
}

fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// `~/.local/share/applications`
pub fn applications_dir() -> PathBuf {
    data_dir().join("applications")
}

/// `~/.local/share/mime`
pub fn mime_dir() -> PathBuf {
    data_dir().join("mime")
}

/// Full path of the generated desktop entry.
pub fn desktop_path() -> PathBuf {
    applications_dir().join(DESKTOP_FILE)
}

/// Full path of the generated MIME package.
pub fn mime_package_path() -> PathBuf {
    mime_dir().join("packages").join(MIME_FILE)
}

/// `~/.local/share/icons/hicolor`
pub fn icons_dir() -> PathBuf {
    data_dir().join("icons").join("hicolor")
}

/// Returns the MIME types that should be associated for the enabled formats.
pub fn enabled_mime_types(config: &Config) -> Vec<String> {
    let mut mimes: Vec<String> = Vec::new();
    for format in formats::FORMATS {
        if config.is_format_disabled(format.id) {
            continue;
        }
        for mime in format.mime_types {
            let mime = mime.to_string();
            if !mimes.contains(&mime) {
                mimes.push(mime);
            }
        }
    }
    mimes
}

/// Maps a single extension to the MIME type used in the generated package.
fn extension_mime(extension: &str) -> Option<&'static str> {
    match extension {
        "epub" => Some("application/epub+zip"),
        "cbz" => Some("application/vnd.comicbook+zip"),
        "cbr" => Some("application/vnd.comicbook-rar"),
        "cb7" => Some("application/x-7z-compressed"),
        "cbt" => Some("application/x-tar"),
        _ => formats::format_for_extension(extension)
            .and_then(|format| format.mime_types.first().copied()),
    }
}

/// Builds the MIME package XML from the enabled formats.
pub fn build_mime_xml(config: &Config) -> String {
    // mime type -> glob patterns, kept sorted for stable output.
    let mut globs: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();

    for format in formats::FORMATS {
        if config.is_format_disabled(format.id) {
            continue;
        }
        for extension in format.extensions {
            if let Some(mime) = extension_mime(extension) {
                globs
                    .entry(mime)
                    .or_default()
                    .push(format!("*.{extension}"));
            }
        }
    }

    // Composite tar+compression globs, only when both parts are enabled.
    let tar_enabled = !config.is_format_disabled("tar");
    if tar_enabled {
        for compression in [
            "gz", "bz2", "bz", "xz", "lzma", "lz", "lz4", "sz", "zst", "br",
        ] {
            if config.is_format_disabled(compression) {
                continue;
            }
            if let Some(mime) = extension_mime(compression) {
                globs
                    .entry(mime)
                    .or_default()
                    .push(format!("*.tar.{compression}"));
            }
        }
    }

    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str("<mime-info xmlns=\"http://www.freedesktop.org/standards/shared-mime-info\">\n");
    for (mime, patterns) in &globs {
        xml.push_str(&format!("  <mime-type type=\"{mime}\">\n"));
        for pattern in patterns {
            xml.push_str(&format!("    <glob pattern=\"{pattern}\"/>\n"));
        }
        xml.push_str("  </mime-type>\n");
    }
    xml.push_str("</mime-info>\n");
    xml
}

/// Builds the desktop entry text from the enabled formats.
pub fn build_desktop(config: &Config) -> String {
    let exec_path = app_exec_path();
    let exec = quote_exec(&exec_path.to_string_lossy());
    let mimes = enabled_mime_types(config);
    let mime_list = if mimes.is_empty() {
        String::new()
    } else {
        format!("{};", mimes.join(";"))
    };

    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.0\n\
         Name=Ouch Decompress\n\
         Comment=Extract archives with ouch\n\
         Icon=ouch-decompress-gui\n\
         Exec={exec} %F\n\
         TryExec={try_exec}\n\
         Terminal=false\n\
         Categories=Utility;Archiving;\n\
         MimeType={mime_list}\n\
         StartupNotify=false\n\
         StartupWMClass=ouch-decompress-gui\n\
         X-AppImage-Name=Ouch-Decompress-Gui\n",
        try_exec = exec_path.to_string_lossy(),
        mime_list = mime_list,
    )
}

/// Quotes a path for the `Exec=` line of a desktop entry (paths may contain
/// spaces). Not used for `TryExec=`, which must be a bare path for GIO to load
/// the entry.
fn quote_exec(path: &str) -> String {
    let escaped = path.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Installs a fallback desktop entry, MIME package and icons for the enabled
/// formats. Only used when no AppImage manager provides a launcher.
fn apply(config: &Config) -> Result<()> {
    let desktop = desktop_path();
    let mime = mime_package_path();

    if let Some(parent) = desktop.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    if let Some(parent) = mime.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    std::fs::write(&desktop, build_desktop(config))
        .with_context(|| format!("writing {}", desktop.display()))?;
    std::fs::write(&mime, build_mime_xml(config))
        .with_context(|| format!("writing {}", mime.display()))?;

    install_icons()?;
    refresh_databases();
    Ok(())
}

/// Basename of a desktop entry (other than ours) that launches this app, such
/// as the one an AppImage manager creates for the file.
fn find_external_launcher() -> Option<String> {
    let target = app_exec_path();
    let target = target.to_string_lossy();
    for entry in std::fs::read_dir(applications_dir()).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("desktop") {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name == DESKTOP_FILE {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        // Either the entry runs this file, or it is this app's window class
        // (the AppImage manager may point at a copy in another location).
        let runs_this_file = text.lines().any(|line| {
            line.strip_prefix("Exec=")
                .is_some_and(|exec| exec.contains(target.as_ref()))
        });
        let same_window_class = text
            .lines()
            .any(|line| line.trim() == "StartupWMClass=ouch-decompress-gui");
        if runs_this_file || same_window_class {
            return Some(name.to_string());
        }
    }
    None
}

/// Removes the legacy entry/mime/icons this app used to install, when another
/// manager already provides a launcher for this AppImage.
pub fn remove_legacy_if_redundant() {
    if desktop_path().exists() && find_external_launcher().is_some() {
        let _ = remove();
    }
}

/// Makes this app the default handler for every enabled MIME type, using the
/// desktop entry provided by the AppImage manager when there is one.
pub fn set_default(config: &Config) -> Result<()> {
    let handler = match find_external_launcher() {
        Some(name) => {
            // Another manager owns the entry; drop our duplicate if present.
            let _ = remove();
            name
        }
        None => {
            apply(config)?;
            DESKTOP_FILE.to_string()
        }
    };
    for mime_type in enabled_mime_types(config) {
        let _ = Command::new("xdg-mime")
            .args(["default", &handler, &mime_type])
            .status();
    }
    Ok(())
}

/// Removes the legacy desktop entry, MIME package and icons, then refreshes the
/// XDG databases.
fn remove() -> Result<()> {
    let _ = std::fs::remove_file(desktop_path());
    let _ = std::fs::remove_file(mime_package_path());
    remove_icons();
    refresh_databases();
    Ok(())
}

/// Installs the app icons into the user's icon theme so the application menu
/// and the GNOME dock can resolve `Icon=ouch-decompress-gui`.
fn install_icons() -> Result<()> {
    for (size, bytes) in [("256x256", ICON_256), ("512x512", ICON_512)] {
        let dir = icons_dir().join(size).join("apps");
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join(format!("{ICON_NAME}.png"));
        std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
    }
    refresh_icon_cache();
    Ok(())
}

/// Removes the installed icons from the user's icon theme.
fn remove_icons() {
    for size in ["256x256", "512x512"] {
        let path = icons_dir()
            .join(size)
            .join("apps")
            .join(format!("{ICON_NAME}.png"));
        let _ = std::fs::remove_file(path);
    }
    refresh_icon_cache();
}

/// Runs `gtk-update-icon-cache`, ignoring a missing tool.
fn refresh_icon_cache() {
    let _ = Command::new("gtk-update-icon-cache")
        .args(["-f", "-t"])
        .arg(icons_dir())
        .status();
}

/// Runs the standard database refreshers, ignoring missing tools.
fn refresh_databases() {
    let _ = Command::new("update-mime-database")
        .arg(mime_dir())
        .status();
    let _ = Command::new("update-desktop-database")
        .arg(applications_dir())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_xml_contains_enabled_glob() {
        let config = Config {
            disabled_formats: vec!["rar".into()],
            ..Config::default()
        };
        let xml = build_mime_xml(&config);
        assert!(xml.contains("<glob pattern=\"*.zip\"/>"));
        assert!(xml.contains("<glob pattern=\"*.tgz\"/>"));
        assert!(!xml.contains("<glob pattern=\"*.rar\"/>"));
        assert!(xml.contains("<glob pattern=\"*.tar.gz\"/>"));
    }

    #[test]
    fn desktop_lists_enabled_mimes() {
        let config = Config {
            disabled_formats: vec!["rar".into()],
            ..Config::default()
        };
        let desktop = build_desktop(&config);
        assert!(desktop.contains("MimeType="));
        assert!(desktop.contains("application/zip;"));
        assert!(!desktop.contains("application/vnd.rar"));
        assert!(desktop.contains("Exec=\""));
        assert!(desktop.contains("%F"));
        assert!(desktop.contains("Icon=ouch-decompress-gui"));
        assert!(desktop.contains("StartupWMClass=ouch-decompress-gui"));
        // TryExec must be a bare path, or GIO refuses to load the entry.
        assert!(!desktop.contains("TryExec=\""));
    }

    #[test]
    fn disabled_format_removes_mime() {
        let config = Config {
            disabled_formats: vec!["zip".into()],
            ..Config::default()
        };
        let mimes = enabled_mime_types(&config);
        assert!(!mimes.iter().any(|m| m == "application/zip"));
    }
}
