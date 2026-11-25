//! Running the control interface's commands (see `crate::control`).

use iced::Task;
use serde_json::{Value, json};

use super::markup::MarkupMsg;
use super::{App, Doc, Message};
use crate::control::{Command, Shared};

/// What `Show` says in a toast.
const SAMPLE_NOTICE: &str = "Saved “report.pdf”";
const SAMPLE_PROBLEM: &str = "Could not save: the disk is full";

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
            Command::Show(what) => match self.control_show(what) {
                Some(task) => task,
                None => return (Err(format!("there is no “{what}” to show")), Task::none()),
            },
            Command::Quit => iced::exit(),
            Command::GetForm => return (Ok(self.control_form().to_string()), Task::none()),
            Command::GetAnnotations => return (Ok(self.control_annotations().to_string()), Task::none()),
            Command::SetField { name, value } => {
                if let Err(why) = self.control_set_field(name, value) {
                    return (Err(why), Task::none());
                }
                Task::none()
            }
            Command::SetMarkup(on) => {
                if !matches!(self.doc, Doc::Image(_) | Doc::Pdf(_)) {
                    return (Err("nothing is open".into()), Task::none());
                }
                self.control_markup(*on)
            }
            Command::Annotate { kind, page, rect, text, color } => {
                if *page == 0 {
                    return (Err("pages are counted from 1".into()), Task::none());
                }
                match self.control_annotate(kind, *page as usize - 1, *rect, text, color) {
                    Ok(task) => task,
                    Err(why) => return (Err(why), Task::none()),
                }
            }
            Command::Edit { op, arg } => match self.control_edit(op, arg) {
                Ok(task) => task,
                Err(why) => return (Err(why), Task::none()),
            },
            Command::Save => {
                if !matches!(self.doc, Doc::Image(_) | Doc::Pdf(_)) {
                    return (Err("nothing is open".into()), Task::none());
                }
                if !self.has_unsaved_changes() {
                    return (Err("there is nothing to save".into()), Task::none());
                }
                if matches!(self.doc, Doc::Image(_)) && self.picture_in_place().is_none() {
                    return (Err("this kind of picture cannot be saved in place: use SaveCopy".into()), Task::none());
                }
                if self.changed_on_disk() {
                    return (Err("the file changed on disk: use SaveCopy".into()), Task::none());
                }
                self.markup_update(MarkupMsg::Save)
            }
            Command::SaveCopy(path) => {
                if !matches!(self.doc, Doc::Image(_) | Doc::Pdf(_)) {
                    return (Err("nothing is open".into()), Task::none());
                }
                self.markup_update(MarkupMsg::SaveTo(Ok(Some(path.clone()))))
            }
            Command::Export { kind, path, dpi } => match self.control_export(kind, path.clone(), *dpi) {
                Ok(task) => task,
                Err(why) => return (Err(why), Task::none()),
            },
        };
        (Ok(self.control_state().to_string()), task)
    }

    /// Shows a part of the interface, the way a click would.
    fn control_show(&mut self, what: &str) -> Option<Task<Message>> {
        use super::commands::PaletteMsg;
        use super::edit::EditMsg;
        use super::menu::{Dialog, Menu};
        use super::pages::PagesMsg;
        use super::region::RegionMsg;
        use super::signature::SignMsg;
        use super::{Confirm, ExportMsg, SidebarTab, chooser::Purpose};
        let message = match what {
            "menu" => Message::Menu(Some(Menu::Main)),
            "open-menu" => Message::Menu(Some(Menu::Open)),
            "prefs" => Message::Dialog(Some(Dialog::Preferences)),
            "keys" => Message::Dialog(Some(Dialog::Shortcuts)),
            "about" => Message::Dialog(Some(Dialog::About)),
            "props" => Message::ShowProperties,
            "search" => {
                let open = self.update(Message::SearchOpen);
                return Some(Task::batch([open, self.update(Message::SearchInput("page".into()))]));
            }
            "goto" => Message::GoToOpen,
            "palette" => Message::Palette(PaletteMsg::Open),
            "export" => Message::Export(ExportMsg::Open),
            "markup" => Message::Markup(MarkupMsg::Toggle),
            "colour" => Message::Markup(MarkupMsg::StyleMenu),
            "sign" => Message::Sign(SignMsg::Menu),
            "pad" => Message::Sign(SignMsg::Draw),
            "edit" => Message::Edit(EditMsg::Toggle),
            "crop" => Message::Edit(EditMsg::CropStart),
            "resize" => Message::Edit(EditMsg::ResizeOpen),
            "adjust" => Message::Edit(EditMsg::AdjustOpen),
            "organise" => Message::Pages(PagesMsg::Open),
            "region" => {
                let drag = [(60.0, 40.0), (260.0, 190.0)];
                let _ = self.region_update(RegionMsg::Press(drag[0].0, drag[0].1));
                let _ = self.region_update(RegionMsg::Drag(drag[1].0, drag[1].1));
                return Some(self.region_update(RegionMsg::Release));
            }
            "discard" => {
                self.confirm = Some(Confirm::Discard);
                return Some(Task::none());
            }
            "pages-tab" => Message::SidebarTab(SidebarTab::Pages),
            "chooser" => return Some(App::ask_for_files(Purpose::OpenFiles, None)),
            "print" => {
                let path = self.current_path()?.to_path_buf();
                return Some(self.open_print_dialog(crate::print::Job::file(path)));
            }
            "quit" => {
                self.confirm_quit = true;
                return Some(Task::none());
            }
            "overwrite" => {
                self.confirm = Some(Confirm::Overwrite);
                return Some(Task::none());
            }
            "trash" => {
                self.confirm = Some(Confirm::Trash);
                return Some(Task::none());
            }
            "toast" => return Some(self.notice(SAMPLE_NOTICE)),
            "error" => return Some(self.toast(SAMPLE_PROBLEM)),
            "slideshow" => Message::Slideshow,
            _ => return None,
        };
        Some(self.update(message))
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
        let markup = self.marks.as_ref().map(|m| {
            let annotations: usize = m.store.pages.layers.values().map(|l| l.items.len()).sum();
            json!({ "annotations": annotations, "dirty": m.is_dirty() })
        });
        let form = self.form.as_ref().map(|f| json!({ "fields": f.form.fields.len(), "dirty": f.is_dirty() }));
        let edit = match &self.doc {
            Doc::Image(v) => Some(json!({
                "dirty": v.edit.is_dirty(),
                "width": v.shown.width,
                "height": v.shown.height,
                "original": [v.original.width, v.original.height],
            })),
            _ => None,
        };
        let editing = matches!(&self.doc, Doc::Image(v) if v.edit.is_busy());
        let notice = self.last_toast.as_ref().map(|(text, error)| json!({ "text": text, "error": error }));
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
            "markup_open": self.markup_open,
            "markup": markup,
            "form": form,
            "edit": edit,
            "notice": notice,
            "busy": self.control_opening
                || matches!(self.doc, Doc::Loading(_))
                // A picture is loaded behind the one still on screen; a failed load never clears this.
                || (self.load_started.is_some() && !matches!(self.doc, Doc::Failed { .. }))
                || self.markup_busy.is_some()
                || self.export_running > 0
                || editing,
        })
    }
}
