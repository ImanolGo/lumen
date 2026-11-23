//! Lumen (Rust/iced edition): a fast, safe image and PDF viewer.

#![forbid(unsafe_code)]

mod annotate;
mod app;
mod forms;
mod icons;
mod library;
mod pdf;
mod portal;
mod print;
mod raster;
mod signatures;
mod structure;
mod style;

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;

static STARTED: OnceLock<Instant> = OnceLock::new();

/// Milliseconds since the process started, for the timing logs.
pub fn since_start() -> u64 {
    STARTED.get().map_or(0, |t| t.elapsed().as_millis() as u64)
}

const HELP: &str = "\
Usage: lumen-rs [FILE|FOLDER]...

A fast, safe viewer for pictures and PDFs.

Opening one file also lets you flip through the other pictures and PDFs in
its folder with the arrow keys. Opening a folder shows everything inside it.

Options:
  -h, --help       Show this help
  -V, --version    Show the version";

fn main() -> iced::Result {
    STARTED.get_or_init(Instant::now);
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,lumen_rs=info,wgpu=error,naga=error".into()),
        )
        .init();

    let mut paths = Vec::new();
    for arg in std::env::args_os().skip(1) {
        match arg.to_str() {
            Some("-h" | "--help") => {
                println!("{HELP}");
                return Ok(());
            }
            Some("-V" | "--version") => {
                println!("lumen-rs {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            _ => paths.push(PathBuf::from(arg)),
        }
    }

    iced::application(move || app::App::new(paths.clone()), app::App::update, app::App::view)
        .title(app::App::title)
        .theme(app::App::theme)
        .subscription(app::App::subscription)
        .window(iced::window::Settings {
            size: iced::Size::new(1180.0, 800.0),
            min_size: Some(iced::Size::new(520.0, 360.0)),
            // Closing asks first while there is unsaved markup.
            exit_on_close_request: false,
            platform_specific: iced::window::settings::PlatformSpecific {
                application_id: "io.github.imanolgo.LumenRs".into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .antialiasing(true)
        .run()
}
