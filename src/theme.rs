//! Theme preference handling with desktop detection.

use serde::{Deserialize, Serialize};

/// User-configurable theme choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    /// All variants in presentation order.
    pub const ALL: [ThemeChoice; 3] = [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark];

    /// Translation key for the label.
    pub fn label_key(&self) -> &'static str {
        match self {
            ThemeChoice::System => "theme.system",
            ThemeChoice::Light => "theme.light",
            ThemeChoice::Dark => "theme.dark",
        }
    }
}

/// Best-effort detection of whether the desktop currently uses a dark theme.
///
/// The authoritative source is the XDG desktop portal's `color-scheme`
/// (`org.freedesktop.appearance color-scheme`), which GNOME/KDE update when the
/// user switches between light and dark. `gsettings` can disagree (GNOME leaves
/// `color-scheme` at `default` even in dark mode), so it is only a fallback.
pub fn system_prefers_dark() -> bool {
    if let Some(dark) = portal_prefers_dark() {
        return dark;
    }

    if let Ok(gtk_theme) = std::env::var("GTK_THEME") {
        if gtk_theme.to_ascii_lowercase().contains("dark") {
            return true;
        }
    }

    if let Some(value) = gsettings_get("org.gnome.desktop.interface", "color-scheme") {
        if value.contains("dark") {
            return true;
        }
    }

    if let Some(value) = gsettings_get("org.gnome.desktop.interface", "gtk-theme") {
        if value.to_ascii_lowercase().contains("dark") {
            return true;
        }
    }

    if let Ok(color_scheme) = std::env::var("KDE_COLOR_SCHEME") {
        if color_scheme.to_ascii_lowercase().contains("dark") {
            return true;
        }
    }

    false
}

/// Asks the XDG desktop portal for the preferred color scheme.
///
/// `color-scheme` is `0` (no preference), `1` (prefer dark) or `2` (prefer
/// light). Returns `None` when the portal is unavailable or has no preference.
fn portal_prefers_dark() -> Option<bool> {
    let output = std::process::Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.freedesktop.portal.Desktop",
            "--object-path",
            "/org/freedesktop/portal/desktop",
            "--method",
            "org.freedesktop.portal.Settings.Read",
            "org.freedesktop.appearance",
            "color-scheme",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    // Output looks like "(<<uint32 1>>,)".
    if text.contains("uint32 1") {
        Some(true)
    } else if text.contains("uint32 2") {
        Some(false)
    } else {
        None
    }
}

/// Runs `gsettings get <schema> <key>` and returns the trimmed plain value,
/// stripping the surrounding single quotes gsettings uses for strings.
fn gsettings_get(schema: &str, key: &str) -> Option<String> {
    let output = std::process::Command::new("gsettings")
        .args(["get", schema, key])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Some(value.trim_matches('\'').to_ascii_lowercase())
}

/// Resolves a choice to a concrete dark/light boolean.
pub fn resolve(choice: ThemeChoice) -> bool {
    match choice {
        ThemeChoice::Light => false,
        ThemeChoice::Dark => true,
        ThemeChoice::System => system_prefers_dark(),
    }
}

/// Applies the theme to the egui context.
///
/// Besides selecting dark/light, it gives every text the same readable color:
/// egui's defaults draw labels with a dim gray and buttons with a different
/// gray, which makes controls look disabled. Setting `override_text_color` and
/// all widget strokes to one color keeps the whole UI consistent.
pub fn apply(ctx: &egui::Context, choice: ThemeChoice) {
    let dark = resolve(choice);
    ctx.set_theme(if dark {
        egui::ThemePreference::Dark
    } else {
        egui::ThemePreference::Light
    });

    let theme = if dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    };
    ctx.style_mut_of(theme, |style| {
        let text = if dark {
            egui::Color32::from_gray(235)
        } else {
            egui::Color32::from_gray(25)
        };
        let visuals = &mut style.visuals;
        visuals.override_text_color = Some(text);
        visuals.widgets.noninteractive.fg_stroke.color = text;
        visuals.widgets.inactive.fg_stroke.color = text;
        visuals.widgets.hovered.fg_stroke.color = text;
        visuals.widgets.active.fg_stroke.color = text;
        visuals.widgets.open.fg_stroke.color = text;
    });
}
