mod app;
mod document;
mod folder;
mod image;
mod thumbnails;
mod widgets;

use std::time::Duration;

use relm4::{RelmApp, adw, gtk::{gio, glib, prelude::*}};
use tracing_subscriber::EnvFilter;

pub const APP_ID: &str = "io.github.imanolgo.Lumen";
pub const APP_NAME: &str = "Lumen";

fn main() -> glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "lumen=info".into()))
        .init();

    // HANDLES_OPEN makes GApplication parse file arguments itself, so the CLI,
    // "Open With…" and a second `lumen file` launch all end up in `open`.
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
