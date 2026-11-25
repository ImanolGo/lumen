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
mod fsio;
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
mod redact;
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

/// iced asks wgpu for the "high performance" GPU unless `WGPU_POWER_PREF` says otherwise, which on
/// a laptop means waking the discrete one: measured on the author's machine, that costs about
/// 140 ms of start-up and 65 to 150 MB more memory for no visible gain in a viewer. The variable
/// can only be set before the process starts (setting it later needs `unsafe`, which is forbidden),
/// so Lumen starts itself again once with it set. Set `WGPU_POWER_PREF` (to `high`, say) or
/// `LUMEN_NO_REEXEC=1` to keep the default.
fn prefer_integrated_gpu() {
    use std::os::unix::process::CommandExt;
    if std::env::var_os("WGPU_POWER_PREF").is_some() || std::env::var_os("LUMEN_NO_REEXEC").is_some() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    // Only returns if the new process could not be started: carry on as we are.
    let error = std::process::Command::new(exe).args(std::env::args_os().skip(1)).env("WGPU_POWER_PREF", "low").exec();
    eprintln!("could not restart with the integrated GPU: {error}");
}

fn main() -> iced::Result {
    STARTED.get_or_init(Instant::now);
    // The sandboxed decoder: a separate, tiny job that must not start the app.
    if std::env::args_os().nth(1).is_some_and(|a| a == "--decode-worker") {
        let args: Vec<String> = std::env::args_os().skip(2).map(|a| a.to_string_lossy().into_owned()).collect();
        sandbox::worker_main(&args);
    }
    prefer_integrated_gpu();
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

    let result = iced::application(move || app::App::new(paths.clone()), app::App::update, app::App::view)
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
        .run();
    print::delete_copies();
    result
}
