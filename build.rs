//! Injects the resolved versions of the GUI crates into the binary, for the
//! About window. Cargo does not expose dependency versions to `env!`, so the
//! versions are read from `Cargo.lock` at build time.

use std::fs;

fn main() {
    println!("cargo:rerun-if-changed=Cargo.lock");

    let lock = fs::read_to_string("Cargo.lock").unwrap_or_default();
    for name in ["egui", "eframe"] {
        let version = find_version(&lock, name).unwrap_or_else(|| "?".to_string());
        println!("cargo:rustc-env={}_VERSION={version}", name.to_uppercase());
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
