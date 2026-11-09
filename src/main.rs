//! Ouch Decompress GUI: a lightweight GUI to extract archives on Linux.
//!
//! The binary doubles as the file-manager handler: when archive paths are
//! passed on the command line (double-click / "Open with"), it extracts them
//! immediately. With no arguments it opens the main window.

mod app;
mod association;
mod config;
mod formats;
mod i18n;
mod job;
mod modes;
mod multivolume;
mod ouch;
mod split;
mod system;
mod theme;
mod update;

use std::path::PathBuf;

/// Title of the single OS window. It briefly gets the name of the active
/// overlay appended to it, to keep the old "separate window" cue.
pub const WINDOW_TITLE: &str = "Ouch Decompress";

/// Default and minimum inner size of the main window, in points. It is also
/// restored when an overlay closes if the size before opening it is unknown.
pub const DEFAULT_WINDOW_SIZE: [f32; 2] = [580.0, 300.0];

#[expect(
    clippy::print_stdout,
    reason = "a CLI's stdout is its output; the GUI never uses it"
)]
fn main() -> eframe::Result<()> {
    let options = parse_args();

    if options.help {
        print_help();
        return Ok(());
    }
    if options.version {
        println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let mut viewport = egui::ViewportBuilder::default()
        .with_app_id("ouch-decompress-gui")
        .with_inner_size(DEFAULT_WINDOW_SIZE)
        .with_min_inner_size(DEFAULT_WINDOW_SIZE)
        .with_title(WINDOW_TITLE);
    if let Ok(icon) =
        eframe::icon_data::from_png_bytes(include_bytes!("../assets/ouch-decompress-gui-256.png"))
    {
        viewport = viewport.with_icon(icon);
    }

    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        WINDOW_TITLE,
        native_options,
        Box::new(move |cc| {
            let startup = if options.settings {
                app::Startup::Settings
            } else {
                app::Startup::Extract
            };
            Ok(Box::new(app::App::new(cc, options.files, startup)))
        }),
    )
}

/// Parsed command-line options.
#[derive(Default)]
struct CliOptions {
    files: Vec<PathBuf>,
    settings: bool,
    help: bool,
    version: bool,
}

#[expect(
    clippy::print_stderr,
    reason = "command-line diagnostics before the window exists"
)]
fn parse_args() -> CliOptions {
    let mut options = CliOptions::default();
    let mut only_files = false;

    for arg in std::env::args().skip(1) {
        if only_files {
            options.files.push(PathBuf::from(arg));
            continue;
        }
        match arg.as_str() {
            "--" => only_files = true,
            "-s" | "--settings" => options.settings = true,
            "-h" | "--help" => options.help = true,
            "-V" | "--version" => options.version = true,
            other if other.starts_with('-') => {
                eprintln!("unknown option: {other}");
            }
            other => options.files.push(PathBuf::from(other)),
        }
    }

    options
}

#[expect(
    clippy::print_stdout,
    reason = "`--help` writes to stdout, as every CLI does"
)]
fn print_help() {
    println!(
        "{name} {version}\n\
         A lightweight archive extractor powered by ouch.\n\n\
         USAGE:\n\
         \x20   {name} [OPTIONS] [ARCHIVE...]\n\n\
         ARGUMENTS:\n\
         \x20   <ARCHIVE>...   Archives to extract immediately\n\n\
         OPTIONS:\n\
         \x20   -s, --settings   Open the settings view\n\
         \x20   -h, --help       Print help\n\
         \x20   -V, --version    Print version",
        name = env!("CARGO_PKG_NAME"),
        version = env!("CARGO_PKG_VERSION"),
    );
}
