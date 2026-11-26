//! The control interface: Lumen's state and a few operations, for scripts and tests.
//!
//! With `LUMEN_CONTROL=1` it is on the session bus (Linux). It is off otherwise, since anything on
//! the session bus could open files in your viewer. With `LUMEN_CONTROL=stdio` (every system) only
//! the process that started Lumen reaches it: one JSON call per line on standard input,
//! `{"method": "GotoPage", "args": [3]}`, and one JSON answer per line on standard output,
//! `{"ok": …}` or `{"error": "why"}`, in order. Methods and answers are the same.
//!
//! `io.github.imanolgo.Lumen.Control1` at `/io/github/imanolgo/Lumen/Control`. Every
//! method answers with the state afterwards as JSON (see `App::control_state`), except the two
//! that read something (`GetForm`, `GetAnnotations`). Work that finishes later (opening a file,
//! saving, exporting, an edit being drawn) is not waited for: poll `GetState` until `busy` is
//! false; `notice` in the state is the last message the window showed. Pages are counted from 1.
//!
//! - Browsing: `Open`, `Next`, `Previous`, `First`, `Last`, `GotoPage`, `Zoom`, `Search`, `Quit`,
//!   `OpenWindow(path)` (in a window of its own).
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
//! - `Show(what)` shows a part of the interface, for pictures of it (see `App::control_show`).
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

/// Whether it runs over standard input and output.
pub fn stdio() -> bool {
    std::env::var_os("LUMEN_CONTROL").is_some_and(|v| v == "stdio")
}

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
    OpenWindow(PathBuf),
    /// Shows a part of the interface (a menu, a dialog, a tool), for looking at it.
    Show(String),
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

/// The calls that arrive on standard input.
pub fn stdio_requests() -> impl futures::Stream<Item = Shared> {
    use futures::StreamExt;
    iced::stream::channel(16, async |to_app: mpsc::Sender<Shared>| {
        let (tx, mut lines) = mpsc::unbounded::<String>();
        std::thread::spawn(move || {
            for line in std::io::stdin().lines().map_while(Result::ok) {
                if tx.unbounded_send(line).is_err() {
                    return;
                }
            }
        });
        while let Some(line) = lines.next().await {
            let result = match parse(&line) {
                Ok(command) => ask(&to_app, command).await,
                Err(e) => Err(e),
            };
            let answer = match result {
                Ok(json) => format!("{{\"ok\":{json}}}"),
                Err(why) => serde_json::json!({ "error": why }).to_string(),
            };
            use std::io::Write;
            let mut out = std::io::stdout().lock();
            if writeln!(out, "{answer}").and_then(|()| out.flush()).is_err() {
                break;
            }
        }
        std::future::pending::<()>().await;
    })
}

/// A line of the stdio interface as the command it is.
fn parse(line: &str) -> Result<Command, String> {
    use serde_json::{Value, from_value};
    #[derive(serde::Deserialize)]
    struct Call {
        method: String,
        #[serde(default)]
        args: Value,
    }
    let Call { method, args } = serde_json::from_str(line).map_err(|e| format!("not a call: {e}"))?;
    fn take<T: serde::de::DeserializeOwned>(method: &str, args: Value) -> Result<T, String> {
        from_value(args).map_err(|e| format!("wrong arguments for {method}: {e}"))
    }
    let m = method.as_str();
    Ok(match m {
        "GetState" => Command::State,
        "Open" => {
            let (paths, password): (Vec<PathBuf>, String) = take(m, args)?;
            Command::Open { paths, password: Some(password).filter(|p| !p.is_empty()) }
        }
        "Next" => Command::Next,
        "Previous" => Command::Previous,
        "First" => Command::First,
        "Last" => Command::Last,
        "GotoPage" => Command::GotoPage(take::<(u32,)>(m, args)?.0),
        "Zoom" => Command::Zoom(take::<(String,)>(m, args)?.0),
        "Search" => Command::Search(take::<(String,)>(m, args)?.0),
        "Show" => Command::Show(take::<(String,)>(m, args)?.0),
        "Quit" => Command::Quit,
        "OpenWindow" => Command::OpenWindow(take::<(PathBuf,)>(m, args)?.0),
        "GetForm" => Command::GetForm,
        "SetField" => {
            let (name, value) = take(m, args)?;
            Command::SetField { name, value }
        }
        "SetMarkup" => Command::SetMarkup(take::<(bool,)>(m, args)?.0),
        "Annotate" => {
            let (kind, page, x0, y0, x1, y1, text, color) = take(m, args)?;
            Command::Annotate { kind, page, rect: [x0, y0, x1, y1], text, color }
        }
        "GetAnnotations" => Command::GetAnnotations,
        "Edit" => {
            let (op, arg) = take(m, args)?;
            Command::Edit { op, arg }
        }
        "Save" => Command::Save,
        "SaveCopy" => Command::SaveCopy(take::<(PathBuf,)>(m, args)?.0),
        "Export" => {
            let (kind, path, dpi) = take(m, args)?;
            Command::Export { kind, path, dpi }
        }
        _ => return Err(format!("unknown method {method}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_the_calls_of_the_bus() {
        assert_eq!(parse(r#"{"method":"GetState"}"#), Ok(Command::State));
        assert_eq!(parse(r#"{"method":"GetState","args":null}"#), Ok(Command::State));
        assert_eq!(parse(r#"{"method":"GotoPage","args":[3]}"#), Ok(Command::GotoPage(3)));
        assert_eq!(
            parse(r#"{"method":"Open","args":[["/a","/b"],""]}"#),
            Ok(Command::Open { paths: vec!["/a".into(), "/b".into()], password: None })
        );
        assert_eq!(
            parse(r##"{"method":"Annotate","args":["line",1,1.0,2.0,3.0,4.0,"","#fff"]}"##),
            Ok(Command::Annotate {
                kind: "line".into(),
                page: 1,
                rect: [1.0, 2.0, 3.0, 4.0],
                text: String::new(),
                color: "#fff".into()
            })
        );
        assert!(parse(r#"{"method":"GotoPage","args":["x"]}"#).unwrap_err().contains("wrong arguments"));
        assert!(parse(r#"{"method":"Fly"}"#).unwrap_err().contains("unknown method"));
        assert!(parse("nonsense").unwrap_err().contains("not a call"));
    }
}
