//! The control interface: Lumen's state and a few operations on the session bus, for scripts
//! and tests. It is off unless `LUMEN_CONTROL=1` is in the environment, since anything on the
//! session bus could otherwise open files in your viewer.
//!
//! `io.github.imanolgo.LumenRs.Control1` at `/io/github/imanolgo/LumenRs/Control`. Every
//! method answers with the state afterwards as JSON (see `App::control_state`). Work that
//! finishes later (opening a file) is not waited for: poll `GetState` until `busy` is false.
//! Pages are counted from 1.

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
