//! Lumen (Rust/iced edition): a fast, safe image and PDF viewer.

#![forbid(unsafe_code)]

#[macro_use]
mod i18n;
mod animation;
mod annotate;
mod app;
mod clipboard;
mod control;
mod cups;
mod edit;
mod export;
mod forms;
#[cfg(feature = "heif")]
mod heif;
mod icons;
mod library;
mod lossless;
mod metadata;
mod pages;
mod pdf;
mod portal;
mod print;
mod properties;
mod raster;
mod sandbox;
mod settings;
mod signatures;
mod state;
mod structure;
mod style;
mod thumbcache;
mod trash;
mod watch;

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
    // The sandboxed decoder: a separate, tiny job that must not start the app.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--decode-worker") {
        sandbox::worker_main(&args[1..]);
    }
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
            size: state::State::load()
                .window_size()
                .map_or(iced::Size::new(1180.0, 800.0), |(w, h)| iced::Size::new(w, h)),
            min_size: Some(iced::Size::new(520.0, 360.0)),
            // Closing asks first while there is unsaved markup.
            exit_on_close_request: false,
            platform_specific: iced::window::settings::PlatformSpecific {
                application_id: "io.github.imanolgo.LumenRs".into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .font(include_bytes!("../assets/fonts/Inter-Regular.ttf").as_slice())
        .font(include_bytes!("../assets/fonts/Inter-Medium.ttf").as_slice())
        .font(include_bytes!("../assets/fonts/Inter-SemiBold.ttf").as_slice())
        .default_font(style::FONT)
        .antialiasing(true)
        .run()
}
