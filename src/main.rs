//! Lumen (Rust/iced edition): a fast, safe image and PDF viewer.

#![forbid(unsafe_code)]
// No console window on Windows (a debug build keeps its log).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[macro_use]
mod i18n;
mod animation;
mod annotate;
mod app;
mod clipboard;
mod control;
mod cups;
mod edit;
mod ellipsis;
mod export;
mod firstsize;
mod forms;
mod frame;
mod fsio;
#[cfg(feature = "heif")]
mod heif;
mod icons;
mod library;
mod lossless;
mod merge;
mod metadata;
mod newwindow;
mod pages;
mod pdf;
mod platform;
mod print;
mod properties;
mod raster;
mod redact;
mod sandbox;
mod settings;
mod settle;
mod signatures;
mod state;
mod structure;
mod style;
mod textfont;
mod thumbcache;
mod ui;

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

Opening one picture also lets you flip through the other pictures in its
folder with the arrow keys. Opening several files, or a folder, shows them as
a list.

Options:
  -h, --help       Show this help
  -V, --version    Show the version";

/// The window's size the first time.
pub const WINDOW: (f32, f32) = (1180.0, 800.0);

/// The window's icon, where windows have one of their own (Windows, X11).
fn window_icon() -> Option<iced::window::Icon> {
    let png = include_bytes!("../data/icons/64x64/apps/io.github.imanolgo.LumenRs.png");
    let icon = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?.into_rgba8();
    let (w, h) = icon.dimensions();
    iced::window::icon::from_rgba(icon.into_raw(), w, h).ok()
}

fn main() -> iced::Result {
    STARTED.get_or_init(Instant::now);
    // The sandboxed decoder: a separate, tiny job that must not start the app.
    if std::env::args_os().nth(1).is_some_and(|a| a == "--decode-worker") {
        let args: Vec<String> = std::env::args_os().skip(2).map(|a| a.to_string_lossy().into_owned()).collect();
        sandbox::worker_main(&args);
    }
    platform::startup::before_window();
    let log = tracing_subscriber::fmt().with_env_filter(
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "warn,lumen_rs=info,wgpu=error,naga=error".into()),
    );
    if control::stdio() {
        // Standard output carries the answers.
        log.with_writer(std::io::stderr).init();
    } else {
        log.init();
    }

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

    let first = firstsize::picture(&paths).map(firstsize::guess);
    let result = iced::application(move || app::App::new(paths.clone()), app::App::update, app::App::view)
        .title(app::App::title)
        .theme(app::App::theme)
        .subscription(app::App::subscription)
        .window(iced::window::Settings {
            size: first.unwrap_or_else(|| {
                state::State::load().window_size().map_or(iced::Size::from(WINDOW), iced::Size::from)
            }),
            min_size: Some(firstsize::SMALLEST),
            icon: window_icon(),
            // Closing asks first while there is unsaved markup.
            exit_on_close_request: false,
            position: if platform::caps().frameless {
                iced::window::Position::Default
            } else {
                iced::window::Position::Centered
            },
            decorations: !platform::caps().frameless,
            platform_specific: platform::startup::window(),
            ..Default::default()
        })
        .font(include_bytes!("../assets/fonts/Inter-Regular.ttf").as_slice())
        .font(include_bytes!("../assets/fonts/Inter-Medium.ttf").as_slice())
        .font(include_bytes!("../assets/fonts/Inter-SemiBold.ttf").as_slice())
        .default_font(style::FONT)
        .antialiasing(true)
        .run();
    print::delete_copies();
    result
}
