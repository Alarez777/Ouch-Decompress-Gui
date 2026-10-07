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
mod ouch;
mod split;
mod system;
mod theme;
mod update;

use std::path::PathBuf;

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
        .with_inner_size([580.0, 300.0])
        .with_min_inner_size([580.0, 300.0])
        .with_title("Ouch Decompress");
    if let Ok(icon) =
        eframe::icon_data::from_png_bytes(include_bytes!("../assets/ouch-decompress-gui-256.png"))
    {
        viewport = viewport.with_icon(icon);
    }

    let native_options = eframe::NativeOptions {
        viewport,
        // Disable vsync: with separate (immediate-viewport) windows, waiting for
        // the display refresh on two surfaces caused the settings/log windows to
        // stall and feel unresponsive. Renders are composited by the compositor.
        glow_options: eframe::egui_glow::GlowConfiguration {
            vsync: false,
            ..Default::default()
        },
        ..Default::default()
    };

    eframe::run_native(
        "Ouch Decompress",
        native_options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, options.files, options.settings)))),
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

fn print_help() {
    println!(
        "{name} {version}\n\
         A lightweight archive extractor powered by ouch.\n\n\
         USAGE:\n\
         \x20   {name} [OPTIONS] [ARCHIVE...]\n\n\
         ARGUMENTS:\n\
         \x20   <ARCHIVE>...   Archives to extract immediately\n\n\
         OPTIONS:\n\
         \x20   -s, --settings   Open the settings tab\n\
         \x20   -h, --help       Print help\n\
         \x20   -V, --version    Print version",
        name = env!("CARGO_PKG_NAME"),
        version = env!("CARGO_PKG_VERSION"),
    );
}
