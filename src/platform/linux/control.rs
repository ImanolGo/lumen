//! The control interface on the session bus (see `crate::control`).

use std::path::PathBuf;

use futures::channel::mpsc;
use zbus::interface;

use crate::control::{Command, Shared};

const NAME: &str = "io.github.imanolgo.Lumen";
const PATH: &str = "/io/github/imanolgo/Lumen/Control";

struct Control {
    to_app: mpsc::Sender<Shared>,
}

impl Control {
    async fn ask(&self, command: Command) -> zbus::fdo::Result<String> {
        crate::control::ask(&self.to_app, command).await.map_err(zbus::fdo::Error::Failed)
    }
}

#[interface(name = "io.github.imanolgo.Lumen.Control1")]
impl Control {
    async fn get_state(&self) -> zbus::fdo::Result<String> {
        self.ask(Command::State).await
    }

    /// Opens files or folders; `password` is tried on protected PDFs ("" for none).
    async fn open(&self, paths: Vec<String>, password: String) -> zbus::fdo::Result<String> {
        let paths = paths.into_iter().map(PathBuf::from).collect();
        self.ask(Command::Open { paths, password: Some(password).filter(|p| !p.is_empty()) }).await
    }

    async fn next(&self) -> zbus::fdo::Result<String> {
        self.ask(Command::Next).await
    }

    async fn previous(&self) -> zbus::fdo::Result<String> {
        self.ask(Command::Previous).await
    }

    async fn first(&self) -> zbus::fdo::Result<String> {
        self.ask(Command::First).await
    }

    async fn last(&self) -> zbus::fdo::Result<String> {
        self.ask(Command::Last).await
    }

    async fn goto_page(&self, page: u32) -> zbus::fdo::Result<String> {
        self.ask(Command::GotoPage(page)).await
    }

    async fn zoom(&self, how: String) -> zbus::fdo::Result<String> {
        self.ask(Command::Zoom(how)).await
    }

    async fn search(&self, query: String) -> zbus::fdo::Result<String> {
        self.ask(Command::Search(query)).await
    }

    async fn show(&self, what: String) -> zbus::fdo::Result<String> {
        self.ask(Command::Show(what)).await
    }

    async fn quit(&self) -> zbus::fdo::Result<String> {
        self.ask(Command::Quit).await
    }

    async fn open_window(&self, path: String) -> zbus::fdo::Result<String> {
        self.ask(Command::OpenWindow(PathBuf::from(path))).await
    }

    /// The form's fields as JSON: name, kind, value, whether read-only, page, options.
    async fn get_form(&self) -> zbus::fdo::Result<String> {
        self.ask(Command::GetForm).await
    }

    async fn set_field(&self, name: String, value: String) -> zbus::fdo::Result<String> {
        self.ask(Command::SetField { name, value }).await
    }

    async fn set_markup(&self, on: bool) -> zbus::fdo::Result<String> {
        self.ask(Command::SetMarkup(on)).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn annotate(
        &self,
        kind: String,
        page: u32,
        x0: f64,
        y0: f64,
        x1: f64,
        y1: f64,
        text: String,
        color: String,
    ) -> zbus::fdo::Result<String> {
        self.ask(Command::Annotate { kind, page, rect: [x0, y0, x1, y1], text, color }).await
    }

    /// The annotations on the open file as JSON.
    async fn get_annotations(&self) -> zbus::fdo::Result<String> {
        self.ask(Command::GetAnnotations).await
    }

    async fn edit(&self, op: String, arg: String) -> zbus::fdo::Result<String> {
        self.ask(Command::Edit { op, arg }).await
    }

    /// Saves the open file in place (a picture that cannot be saved in place is refused).
    async fn save(&self) -> zbus::fdo::Result<String> {
        self.ask(Command::Save).await
    }

    async fn save_copy(&self, path: String) -> zbus::fdo::Result<String> {
        self.ask(Command::SaveCopy(PathBuf::from(path))).await
    }

    async fn export(&self, kind: String, path: String, dpi: u32) -> zbus::fdo::Result<String> {
        self.ask(Command::Export { kind, path: PathBuf::from(path), dpi }).await
    }
}

/// Serves the interface for as long as the stream is polled, handing each call to the app.
pub fn requests() -> impl futures::Stream<Item = Shared> {
    iced::stream::channel(16, async |output: mpsc::Sender<Shared>| {
        let served = zbus::connection::Builder::session().name(NAME).serve_at(PATH, Control { to_app: output }).build();
        match served.await {
            // The connection lives (and answers calls) while this future is parked.
            Ok(_connection) => std::future::pending::<()>().await,
            Err(e) => tracing::warn!("the control interface could not start: {e}"),
        }
    })
}
