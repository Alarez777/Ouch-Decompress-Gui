//! Injects the resolved versions of the GUI crates into the binary, for the
//! About window, and embeds the Windows icon/version resource.
//!
//! Cargo does not expose dependency versions to `env!`, so the versions are
//! read from `Cargo.lock` at build time.

use std::fs;

fn main() {
    println!("cargo:rerun-if-changed=Cargo.lock");

    let lock = fs::read_to_string("Cargo.lock").unwrap_or_default();
    for name in ["egui", "eframe"] {
        let version = find_version(&lock, name).unwrap_or_else(|| "?".to_string());
        println!("cargo:rustc-env={}_VERSION={version}", name.to_uppercase());
    }

    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/ouch-decompress-gui.ico");
        let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
        let file_version = format!("{version}.0");
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/ouch-decompress-gui.ico");
        res.set("ProductName", "Ouch Decompress");
        res.set("FileDescription", "Archive extractor powered by ouch");
        res.set("ProductVersion", &file_version);
        res.set("FileVersion", &file_version);
        res.set("LegalCopyright", "MIT");
        res.set("OriginalFilename", "ouch-decompress-gui.exe");
        res.compile().expect("compiling Windows resources");
    }
}

/// Returns the `version` of the package named `name` in a `Cargo.lock`.
fn find_version(lock: &str, name: &str) -> Option<String> {
    let header = format!("name = \"{name}\"");
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line.trim() != header {
            continue;
        }
        let version = lines.next()?.trim();
        return Some(
            version
                .strip_prefix("version = \"")?
                .trim_end_matches('"')
                .to_string(),
        );
    }
    None
}
