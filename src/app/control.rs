//! Running the control interface's commands (see `crate::control`).

use iced::Task;
use serde_json::{Value, json};

use super::{App, Doc, Message};
use crate::control::{Command, Shared};

impl App {
    pub(super) fn control_update(&mut self, shared: Shared) -> Task<Message> {
        let Some(request) = shared.take() else { return Task::none() };
        let (answer, task) = self.control_run(&request.command);
        request.answer(answer);
        task
    }

    pub(super) fn control_run(&mut self, command: &Command) -> (Result<String, String>, Task<Message>) {
        let page_count = match &self.doc {
            Doc::Pdf(v) => v.doc.page_count(),
            _ => 0,
        };
        let press = |app: &mut App, message: Message| app.update(message);
        let task = match command {
            Command::State => Task::none(),
            Command::Open { paths, password } => {
                if paths.is_empty() {
                    return (Err("nothing to open".into()), Task::none());
                }
                if let Some(missing) = paths.iter().find(|p| !p.exists()) {
                    return (Err(format!("{} does not exist", missing.display())), Task::none());
                }
                if let Some(password) = password {
                    for path in paths {
                        self.passwords.insert(path.clone(), password.clone());
                    }
                }
                self.control_opening = true;
                super::expand(paths.clone(), false)
            }
            Command::Next => press(self, Message::Next),
            Command::Previous => press(self, Message::Prev),
            Command::First => press(self, Message::First),
            Command::Last => press(self, Message::Last),
            Command::GotoPage(page) => {
                if page_count == 0 {
                    return (Err("no PDF is open".into()), Task::none());
                }
                if *page == 0 || *page as usize > page_count {
                    return (Err(format!("the PDF has pages 1 to {page_count}")), Task::none());
                }
                press(self, Message::GoToPage(*page as usize - 1))
            }
            Command::Zoom(how) => {
                let message = match how.as_str() {
                    "in" => Message::ZoomIn,
                    "out" => Message::ZoomOut,
                    "fit" => Message::ZoomFit,
                    "width" => Message::ZoomFitWidth,
                    "actual" => Message::ZoomActual,
                    other => {
                        return (Err(format!("unknown zoom “{other}” (in, out, fit, width, actual)")), Task::none());
                    }
                };
                press(self, message)
            }
            Command::Search(query) => {
                if page_count == 0 {
                    return (Err("no PDF is open".into()), Task::none());
                }
                let open = press(self, Message::SearchOpen);
                let input = press(self, Message::SearchInput(query.clone()));
                let submit = press(self, Message::SearchSubmit);
                Task::batch([open, input, submit])
            }
            Command::Quit => iced::exit(),
        };
        (Ok(self.control_state().to_string()), task)
    }

    /// What a script can see: what is open, where, and whether anything is still going on.
    pub(super) fn control_state(&self) -> Value {
        let (kind, path) = match &self.doc {
            Doc::Empty => ("none", None),
            Doc::Loading(p) => ("loading", Some(p.clone())),
            Doc::Failed { path, .. } => ("failed", Some(path.clone())),
            Doc::Locked { path, .. } => ("locked", Some(path.clone())),
            Doc::Image(v) => ("picture", Some(v.path.clone())),
            Doc::Pdf(v) => ("pdf", Some(v.doc.path().to_path_buf())),
        };
        let (page, pages) = match &self.doc {
            Doc::Pdf(v) => (Some(v.current_page + 1), Some(v.doc.page_count())),
            _ => (None, None),
        };
        let zoom = match &self.doc {
            Doc::Pdf(v) => Some(f64::from(v.zoom)),
            Doc::Image(v) => v.zoom.map(f64::from),
            _ => None,
        };
        let error = match &self.doc {
            Doc::Failed { error, .. } => Some(error.clone()),
            _ => None,
        };
        let search = self.search.as_ref().map(|s| {
            json!({
                "query": s.query,
                "hits": s.hits.len(),
                "current": s.current.map(|c| c + 1),
                "done": s.done,
            })
        });
        json!({
            "kind": kind,
            "file": path.map(|p| p.display().to_string()),
            "error": error,
            "index": if self.files.is_empty() || self.current >= self.files.len() { Value::Null } else { json!(self.current + 1) },
            "count": self.files.len(),
            "page": page,
            "pages": pages,
            "zoom": zoom,
            "night": self.night,
            "unsaved": self.has_unsaved_changes(),
            "search": search,
            "busy": self.control_opening || matches!(self.doc, Doc::Loading(_)),
        })
    }
}
