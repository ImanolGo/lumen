mod app;
mod document;
mod image;
mod widgets;

use std::path::PathBuf;

use relm4::RelmApp;
use tracing_subscriber::EnvFilter;

pub const APP_ID: &str = "io.github.imanolgo.Lumen";
pub const APP_NAME: &str = "Lumen";

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "lumen=info".into()))
        .init();

    // For now we parse the optional file argument ourselves and keep GTK from
    // seeing it. Step 1.4 in docs/PLAN.md replaces this with GApplication's
    // HANDLES_OPEN so "Open With…" from the file manager works too.
    let initial_file = std::env::args_os().nth(1).map(PathBuf::from);

    let app = RelmApp::new(APP_ID).with_args(Vec::new());
    app.run::<app::App>(initial_file);
}
