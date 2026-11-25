//! Several windows in one process, as a Mac has them: each is an [`App`] of its own, and this is
//! what they share. (On Linux and Windows every window is a process; see `newwindow.rs`.)

use std::path::PathBuf;

use iced::{Element, Subscription, Task, Theme, window};

use super::{App, Doc, Message};
use crate::control::Shared;
use crate::firstsize;

/// Whether this program has all its windows in one process.
pub fn in_process() -> bool {
    static IT_IS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    // The switch is for trying it elsewhere.
    *IT_IS.get_or_init(|| cfg!(target_os = "macos") || std::env::var_os("LUMEN_ONE_PROCESS").is_some())
}

/// What a window asks of the program.
#[derive(Debug, Clone, PartialEq)]
pub enum Ask {
    /// Show these in a window of their own.
    Open(Vec<PathBuf>),
    /// Close this window; whatever had to be asked has been.
    Close,
    /// Close them all.
    QuitAll,
}

#[derive(Debug, Clone)]
pub enum Msg {
    To(window::Id, Message),
    Closed(window::Id),
    MenuBar(String),
    Control(Shared),
}

pub struct Windows {
    list: Vec<(window::Id, App)>,
    focused: Option<window::Id>,
    /// Everything is to be closed, once what is not saved has been asked about.
    quitting: bool,
}

impl Windows {
    pub fn new(paths: Vec<PathBuf>) -> (Self, Task<Msg>) {
        let mut windows = Self { list: Vec::new(), focused: None, quitting: false };
        let opened = windows.open(paths);
        (windows, opened)
    }

    fn open(&mut self, paths: Vec<PathBuf>) -> Task<Msg> {
        // A file that has a window already is brought forward.
        if let [path] = paths.as_slice()
            && let Some((id, _)) = self.list.iter().find(|(_, app)| app.current_path() == Some(path))
        {
            return window::gain_focus(*id);
        }
        let first = firstsize::picture(&paths).map(firstsize::guess);
        let (id, opening) = window::open(crate::window_settings(first));
        let (app, started) = App::new(paths);
        self.list.push((id, app));
        self.focused = Some(id);
        Task::batch([opening.discard(), started.map(move |m| Msg::To(id, m))])
    }

    fn app(&self, id: window::Id) -> Option<&App> {
        self.list.iter().find(|(i, _)| *i == id).map(|(_, app)| app)
    }

    /// The window in use, or any.
    fn in_use(&self) -> Option<window::Id> {
        self.focused.filter(|id| self.app(*id).is_some()).or_else(|| self.list.last().map(|(id, _)| *id))
    }

    pub fn update(&mut self, message: Msg) -> Task<Msg> {
        match message {
            Msg::Closed(id) => self.forget(id),
            Msg::MenuBar(id) => match self.in_use() {
                Some(window) => self.send(window, Message::MenuBar(id)),
                None => Task::none(),
            },
            Msg::Control(call) => match self.in_use() {
                Some(window) => self.send(window, Message::Control(call)),
                None => Task::none(),
            },
            // Every window hears of every event; it is for one of them.
            Msg::To(id, Message::Raw(from, ..)) if from != id => Task::none(),
            Msg::To(id, message) => self.send(id, message),
        }
    }

    fn send(&mut self, id: window::Id, message: Message) -> Task<Msg> {
        if let Message::Raw(from, iced::Event::Window(window::Event::Focused), _) = &message {
            self.focused = Some(*from);
        }
        let Some((_, app)) = self.list.iter_mut().find(|(i, _)| *i == id) else { return Task::none() };
        let prefs = app.prefs.clone();
        let mut tasks = vec![app.update(message).map(move |m| Msg::To(id, m))];
        let asks = std::mem::take(&mut app.asks);
        let changed = (app.prefs != prefs).then(|| app.prefs.clone());
        if let Some(prefs) = changed {
            for (_, other) in self.list.iter_mut().filter(|(i, _)| *i != id) {
                other.prefs = prefs.clone();
                other.apply_theme();
            }
        }
        for ask in asks {
            tasks.push(match ask {
                Ask::Open(paths) => self.open(paths),
                Ask::Close => {
                    let closed = self.forget(id);
                    Task::batch([window::close(id), closed])
                }
                Ask::QuitAll => self.quit_all(),
            });
        }
        tasks.push(self.go_on_quitting());
        Task::batch(tasks)
    }

    /// A window is gone; without any the program ends.
    fn forget(&mut self, id: window::Id) -> Task<Msg> {
        self.list.retain(|(i, _)| *i != id);
        if self.list.is_empty() { iced::exit() } else { Task::none() }
    }

    /// Asks the windows with something unsaved what to do; the others wait for the answers.
    fn quit_all(&mut self) -> Task<Msg> {
        self.quitting = true;
        let mut tasks = Vec::new();
        for (id, app) in &mut self.list {
            if app.unsaved_files() > 0 {
                let id = *id;
                tasks.push(app.update(Message::Quit).map(move |m| Msg::To(id, m)));
                tasks.push(window::gain_focus(id));
            }
        }
        Task::batch(tasks)
    }

    fn go_on_quitting(&mut self) -> Task<Msg> {
        if !self.quitting {
            return Task::none();
        }
        // Someone thought better of it.
        if self.list.iter().any(|(_, app)| app.unsaved_files() > 0 && !app.confirm_quit && !app.quit_when_saved) {
            self.quitting = false;
            return Task::none();
        }
        if self.list.iter().any(|(_, app)| app.unsaved_files() > 0) {
            return Task::none();
        }
        for (_, app) in &mut self.list {
            app.before_leaving();
        }
        self.list.clear();
        iced::exit()
    }

    pub fn view(&self, id: window::Id) -> Element<'_, Msg> {
        match self.app(id) {
            Some(app) => app.view().map(move |m| Msg::To(id, m)),
            None => iced::widget::space().into(),
        }
    }

    pub fn title(&self, id: window::Id) -> String {
        self.app(id).map_or_else(|| "Lumen".into(), App::title)
    }

    pub fn theme(&self, id: window::Id) -> Option<Theme> {
        self.app(id).map(App::theme)
    }

    pub fn subscription(&self) -> Subscription<Msg> {
        let own = self.list.iter().map(|(id, app)| app.subscription().with(*id).map(|(id, m)| Msg::To(id, m)));
        let shared = [
            window::close_events().map(Msg::Closed),
            if crate::platform::caps().menu_bar {
                Subscription::run(crate::platform::menu_bar::chosen).map(Msg::MenuBar)
            } else {
                Subscription::none()
            },
            super::control_subscription().map(Msg::Control),
        ];
        Subscription::batch(own.chain(shared))
    }
}

impl App {
    /// Whether nothing has been opened here: what is opened from outside can come here.
    pub(super) fn is_blank(&self) -> bool {
        matches!(self.doc, Doc::Empty) && self.files.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
    }

    /// Two windows, each showing a file.
    fn two() -> Windows {
        let mut w = Windows { list: Vec::new(), focused: None, quitting: false };
        for name in ["pages.pdf", "form.pdf"] {
            let (mut app, _) = App::new(Vec::new());
            let _ = app.update(Message::Expanded { files: vec![file(name)], start: 0, append: false, alone: true });
            w.list.push((window::Id::unique(), app));
        }
        w
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Runtime::new().unwrap()
    }

    #[test]
    fn a_file_with_a_window_gets_no_other() {
        let runtime = runtime();
        let _inside = runtime.enter();
        let mut w = two();
        let _ = w.open(vec![file("form.pdf")]);
        assert_eq!(w.list.len(), 2);
        let _ = w.open(vec![file("encrypted.pdf")]);
        assert_eq!(w.list.len(), 3);
    }

    #[test]
    fn windows_are_worked_in_and_closed_on_their_own() {
        let runtime = runtime();
        let _inside = runtime.enter();
        let mut w = two();
        let (a, b) = (w.list[0].0, w.list[1].0);
        let _ = w.update(Msg::To(a, Message::ToggleFullscreen));
        assert!(w.app(a).unwrap().fullscreen && !w.app(b).unwrap().fullscreen);
        // An event is for the window it happened in.
        let focus = || iced::Event::Window(window::Event::Focused);
        let _ = w.update(Msg::To(a, Message::Raw(b, focus(), iced::event::Status::Ignored)));
        assert_eq!(w.focused, None);
        let _ = w.update(Msg::To(b, Message::Raw(b, focus(), iced::event::Status::Ignored)));
        assert_eq!(w.in_use(), Some(b));
        // A preference is everyone's.
        let _ = w.update(Msg::To(a, Message::Pref(super::super::prefs::PrefMsg::WrapAround(true))));
        assert!(w.app(b).unwrap().prefs.wrap_around);
        w.list[0].1.asks.push(Ask::Close);
        let _ = w.update(Msg::To(a, Message::Nothing));
        assert!(w.app(a).is_none() && w.app(b).is_some());
    }

    #[test]
    fn quitting_asks_about_every_window_before_closing_any() {
        let runtime = runtime();
        let _inside = runtime.enter();
        let mut w = two();
        let b = w.list[1].0;
        let path = file("form.pdf");
        w.list[1].1.stash_forms.insert(path.clone(), Vec::new());
        assert_eq!(w.list[1].1.unsaved_files(), 1);
        let _ = w.quit_all();
        let _ = w.go_on_quitting();
        assert!(w.list.len() == 2 && w.app(b).unwrap().confirm_quit, "both wait for the answer");
        // No: both stay.
        w.list[1].1.confirm_quit = false;
        let _ = w.go_on_quitting();
        assert!(!w.quitting && w.list.len() == 2);
        // Nothing unsaved any more: all close.
        w.list[1].1.stash_forms.clear();
        let _ = w.quit_all();
        let _ = w.go_on_quitting();
        assert!(w.list.is_empty());
    }
}
