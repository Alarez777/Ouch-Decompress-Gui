//! Windows file associations are registered by the installer.
//!
//! The application only exposes the Linux/XDG association controls, so these
//! functions exist to keep the shared UI code compiling and to report a clear
//! error if they are ever called.

use anyhow::{bail, Result};
use std::path::PathBuf;

use crate::config::Config;

/// Windows has no AppImage runtime.
pub fn running_as_appimage() -> bool {
    false
}

/// Associations are managed by the Windows installer.
pub fn set_default(_config: &Config) -> Result<()> {
    bail!("file associations are managed by the Windows installer")
}

/// Associations are managed by the Windows installer.
pub fn create_desktop(_config: &Config) -> Result<PathBuf> {
    bail!("file associations are managed by the Windows installer")
}

/// No XDG desktop entry exists on Windows.
pub fn remove_desktop() {}

/// No legacy XDG entry exists on Windows.
pub fn remove_legacy_if_redundant() {}
