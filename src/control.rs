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

/// Hands a call to the app and waits for its answer.
pub async fn ask(to_app: &mpsc::Sender<Shared>, command: Command) -> Result<String, String> {
    let (reply, answer) = oneshot::channel();
    let request = Shared(Arc::new(Mutex::new(Some(Request { command, reply }))));
    to_app.clone().send(request).await.map_err(|_| "Lumen is closing".to_string())?;
    match tokio::time::timeout(PATIENCE, answer).await {
        Ok(Ok(result)) => result,
        _ => Err("Lumen did not answer".into()),
    }
}
