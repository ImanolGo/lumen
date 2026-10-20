mod annotate;
mod app;
mod document;
mod edit;
mod folder;
mod fsutil;
mod geometry;
mod i18n;
mod image;
mod pdf;
mod print;
mod save;
mod settings;
mod signatures;
mod thumbnails;
mod widgets;

use std::time::Duration;

use relm4::{
    RelmApp, adw,
    gtk::{gio, glib, prelude::*},
};
use tracing_subscriber::EnvFilter;

/// When the program started, for the timings in the log.
pub static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

pub const APP_ID: &str = "io.github.imanolgo.Lumen";
pub const APP_NAME: &str = "Lumen";

fn main() -> glib::ExitCode {
    STARTED.get_or_init(std::time::Instant::now);
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "lumen=info".into()))
        .init();
    i18n::init();
    // Icons and the stylesheet. GtkApplication finds them under the app's resource base path.
    gio::resources_register_include!("lumen.gresource").expect("the bundled resources are valid");

    // HANDLES_OPEN makes GApplication parse file arguments itself, so the CLI,
    // "Open With…" and a second `lumen-viewer file` launch all end up in `open`.
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    app.connect_open(|app, files, _| {
        app::open_files(files);
        if let Some(window) = app.active_window() {
            window.present();
        }
    });

    // A second launch only forwards its files to the running instance and
    // then reaches Relm4's shutdown, which blocks in `iteration(true)` until
    // some event arrives. A no-op timeout makes sure one does.
    glib::timeout_add_once(Duration::from_millis(50), || {});

    RelmApp::from_app(app).run::<app::App>(());
    glib::ExitCode::SUCCESS
}
