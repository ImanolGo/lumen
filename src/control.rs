//! The control interface: Lumen's state and a few operations on the session bus, for scripts
//! and tests. It is off unless `LUMEN_CONTROL=1` is in the environment, since anything on the
//! session bus could otherwise open files in your viewer.
//!
//! `io.github.imanolgo.LumenRs.Control1` at `/io/github/imanolgo/LumenRs/Control`. Every
//! method answers with the state afterwards as JSON (see `App::control_state`), except the two
//! that read something (`GetForm`, `GetAnnotations`). Work that finishes later (opening a file,
//! saving, exporting, an edit being drawn) is not waited for: poll `GetState` until `busy` is
//! false; `notice` in the state is the last message the window showed. Pages are counted from 1.
//!
//! - Browsing: `Open`, `Next`, `Previous`, `First`, `Last`, `GotoPage`, `Zoom`, `Search`, `Quit`.
//! - Forms: `GetForm` lists the fields; `SetField(name, value)` fills one (text as it is, a
//!   check box with on/off, a radio group with the name of the state to turn on, a list with the
//!   option's text).
//! - Markup: `SetMarkup(on)`, then `Annotate(kind, page, x0, y0, x1, y1, text, color)` draws a
//!   rectangle, ellipse, line, arrow, pen stroke, highlight box or text box between two points
//!   (document units from the page's top-left: points for a PDF, pixels for a picture);
//!   `GetAnnotations` lists what is there.
//! - Pictures: `Edit(op, arg)` with `rotate-right`, `rotate-left`, `flip-h`, `flip-v`, `undo`,
//!   `redo`, `reset`, `crop` ("x,y,w,h" in pixels), `resize` ("WxH" or "50%") and `adjust`
//!   ("exposure=0.5,contrast=-0.2,saturation=0.1").
//! - Saving and export: `Save`, `SaveCopy(path)`, `Export(kind, path, dpi)` with kind `png`
//!   (a file, or a folder for several pages), `pdf` (the page or the organiser's selection) or
//!   `picture-pdf`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::SinkExt;
use futures::channel::mpsc;
use tokio::sync::oneshot;
use zbus::interface;

pub const NAME: &str = "io.github.imanolgo.LumenRs";
pub const PATH: &str = "/io/github/imanolgo/LumenRs/Control";

/// How long a call waits for the app to answer.
const PATIENCE: Duration = Duration::from_secs(30);

pub fn enabled() -> bool {
    std::env::var_os("LUMEN_CONTROL").is_some_and(|v| !v.is_empty() && v != "0")
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    State,
    Open {
        paths: Vec<PathBuf>,
        password: Option<String>,
    },
    Next,
    Previous,
    First,
    Last,
    GotoPage(u32),
    /// "in", "out", "fit", "width" or "actual".
    Zoom(String),
    Search(String),
    Quit,
    GetForm,
    SetField {
        name: String,
        value: String,
    },
    SetMarkup(bool),
    Annotate {
        kind: String,
        page: u32,
        /// x0, y0, x1, y1.
        rect: [f64; 4],
        text: String,
        color: String,
    },
    GetAnnotations,
    Edit {
        op: String,
        arg: String,
    },
    Save,
    SaveCopy(PathBuf),
    Export {
        kind: String,
        path: PathBuf,
        dpi: u32,
    },
}

/// A call waiting for its answer.
pub struct Request {
    pub command: Command,
    reply: oneshot::Sender<Result<String, String>>,
}

impl Request {
    pub fn answer(self, result: Result<String, String>) {
        let _ = self.reply.send(result);
    }
}

/// A request on its way to the app. Messages are cloned and printed, so it is shared
/// and taken once.
#[derive(Clone)]
pub struct Shared(Arc<Mutex<Option<Request>>>);

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Shared(control request)")
    }
}

impl Shared {
    pub fn take(&self) -> Option<Request> {
        self.0.lock().ok()?.take()
    }
}

struct Control {
    to_app: mpsc::Sender<Shared>,
}

impl Control {
    async fn ask(&self, command: Command) -> zbus::fdo::Result<String> {
        let (reply, answer) = oneshot::channel();
        let request = Shared(Arc::new(Mutex::new(Some(Request { command, reply }))));
        self.to_app.clone().send(request).await.map_err(|_| zbus::fdo::Error::Failed("Lumen is closing".into()))?;
        match tokio::time::timeout(PATIENCE, answer).await {
            Ok(Ok(Ok(state))) => Ok(state),
            Ok(Ok(Err(why))) => Err(zbus::fdo::Error::Failed(why)),
            _ => Err(zbus::fdo::Error::Failed("Lumen did not answer".into())),
        }
    }
}

#[interface(name = "io.github.imanolgo.LumenRs.Control1")]
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

    async fn quit(&self) -> zbus::fdo::Result<String> {
        self.ask(Command::Quit).await
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
