//! Every command, in one table: what it is called, its shortcuts, where it is found and what it
//! does now. The key handler, the menus, a Mac's menu bar, the command palette, the list of
//! shortcuts and the keyboard table in `docs/FEATURES.md` are all drawn from it.
//!
//! And the command palette (Ctrl+Shift+P): type a few words, pick a command, press Enter. It
//! leaves out whatever cannot be done right now.

use iced::keyboard::{Key, Modifiers, key::Named};
use iced::widget::{button, column, container, mouse_area, opaque, row, space, stack, svg, text};
use iced::{Alignment, Element, Length, Padding, Task};

use super::edit::EditMsg;
use super::export::ExportMsg;
use super::markup::MarkupMsg;
use super::menu::Dialog;
use super::pages::PagesMsg;
use super::region::RegionMsg;
use super::signature::SignMsg;
use super::{App, Doc, Message, muted};
use crate::annotate::Tool;
use crate::annotate::model::Figure;
use crate::i18n::{msgid, tr};
use crate::{icons, style};

/// How many matches are listed at once.
const SHOWN: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    File,
    Edit,
    View,
    Go,
    Tools,
    Help,
}

impl Group {
    pub const ALL: [Group; 6] = [Group::File, Group::Edit, Group::View, Group::Go, Group::Tools, Group::Help];

    pub fn name(self) -> &'static str {
        match self {
            Group::File => msgid("File"),
            Group::Edit => msgid("Edit"),
            Group::View => msgid("View"),
            Group::Go => msgid("Go"),
            Group::Tools => msgid("Tools"),
            Group::Help => msgid("Help"),
        }
    }
}

/// Where in the ☰ menu a command is. The rest are in the header, the islands and the palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// On the menu itself, in this part of it (the submenus come after the second).
    Main(u8),
    View,
    Export,
    File,
}

#[derive(Debug, Clone)]
pub struct Command {
    pub id: &'static str,
    /// What it is called, and what it is called now ("Stop Slideshow").
    pub name: &'static str,
    pub label: &'static str,
    pub group: Group,
    pub place: Option<Place>,
    /// As "Ctrl+Shift+S"; the first is the one shown.
    pub keys: &'static [&'static str],
    /// `None` when it cannot be done now.
    pub message: Option<Message>,
}

impl Command {
    /// Its main shortcut, empty for none.
    pub fn key(&self) -> &'static str {
        self.keys.first().copied().unwrap_or_default()
    }

    /// Whether a text box uses its shortcut, so that a menu bar must not take it away.
    pub fn text_key(&self) -> bool {
        matches!(self.id, "undo" | "redo" | "copy" | "select-all" | "delete")
    }
}

/// Keys and gestures that are not commands, for the list of shortcuts.
pub fn gestures() -> [(Group, &'static str, &'static str); 9] {
    [
        (Group::Edit, "← → ↑ ↓", msgid("Move the selected annotation (Shift: further)")),
        (Group::Edit, "Enter", msgid("Apply the crop")),
        (Group::Edit, "Esc", msgid("Cancel, close, and finally discard what is not saved")),
        (Group::View, "Ctrl+scroll, pinch", msgid("Zoom in / out")),
        (Group::View, "Space, Esc", msgid("Pause / leave the slideshow")),
        (Group::View, "Esc", msgid("Leave fullscreen")),
        (Group::Go, "↑ ↓", msgid("Scroll")),
        (Group::Go, "Enter, Shift+Enter", msgid("Next / previous match, in the Find box")),
        (Group::Tools, "Shift+click", msgid("Extend the selection")),
    ]
}

/// What the preferences are called on this system, in menus and on the dialog.
pub fn preferences_name() -> &'static str {
    if cfg!(target_os = "linux") { msgid("Preferences…") } else { msgid("Settings…") }
}

pub fn preferences_title() -> &'static str {
    if cfg!(target_os = "linux") { msgid("Preferences") } else { msgid("Settings") }
}

/// Whether pressing `key` with `modifiers` is the shortcut `chord`.
pub fn pressed(chord: &str, key: &Key, modifiers: Modifiers) -> bool {
    let (mut ctrl, mut alt, mut shift, mut rest) = (false, false, false, chord);
    loop {
        // "Ctrl++" ends with the key "+".
        let held = [("Ctrl+", &mut ctrl), ("Alt+", &mut alt), ("Shift+", &mut shift)]
            .into_iter()
            .find_map(|(name, held)| rest.strip_prefix(name).map(|after| (held, after)));
        match held {
            Some((held, after)) if !after.is_empty() => {
                *held = true;
                rest = after;
            }
            _ => break,
        }
    }
    if modifiers.command() != ctrl || modifiers.alt() != alt {
        return false;
    }
    // A Mac's own Ctrl is not part of any shortcut.
    if cfg!(target_os = "macos") && modifiers.control() {
        return false;
    }
    let named = match key {
        Key::Character(c) => {
            let c = c.to_lowercase();
            // Where these are typed depends on the layout, with Shift or without.
            let same = match rest {
                "+" => c == "+" || c == "=",
                "-" => c == "-" || c == "_",
                _ => c == rest.to_lowercase(),
            };
            let letter = rest.chars().all(char::is_alphabetic);
            return same && (!letter || modifiers.shift() == shift);
        }
        Key::Named(named) => named,
        Key::Unidentified => return false,
    };
    let name = match named {
        Named::ArrowLeft => "←",
        Named::ArrowRight => "→",
        Named::ArrowUp => "↑",
        Named::ArrowDown => "↓",
        Named::Home => "Home",
        Named::End => "End",
        Named::PageUp => "Page Up",
        Named::PageDown => "Page Down",
        Named::Space => "Space",
        Named::Backspace => "Backspace",
        Named::Delete => "Delete",
        Named::Enter => "Enter",
        Named::F3 => "F3",
        Named::F5 => "F5",
        Named::F11 => "F11",
        _ => return false,
    };
    name == rest && modifiers.shift() == shift
}

/// What can be done by key while pages are being organised; the rest waits.
const ORGANISING: &[&str] = &[
    "save",
    "save-copy",
    "undo",
    "redo",
    "select-all",
    "delete",
    "rotate-right",
    "rotate-left",
    "organise",
    "move-earlier",
    "move-later",
    "fullscreen",
    "quit",
    "preferences",
];

#[derive(Debug, Default)]
pub struct Palette {
    query: String,
    /// The highlighted match.
    pub(super) selected: usize,
}

#[derive(Debug, Clone)]
pub enum PaletteMsg {
    Open,
    Close,
    Input(String),
    /// Move the highlight up (-1) or down (1).
    Move(i32),
    /// Run the highlighted command.
    Run,
    /// Run the one at this place in the list (clicked).
    Pick(usize),
}

/// The commands whose words all appear in `query`, best first: a command whose name starts
/// with the first word comes before one that only contains it.
pub fn filter(commands: Vec<Command>, query: &str) -> Vec<Command> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut found: Vec<(bool, Command)> = commands
        .into_iter()
        .filter_map(|c| {
            let name = tr(c.label).to_lowercase();
            if !words.iter().all(|w| name.contains(w.as_str())) {
                return None;
            }
            let starts = words.first().is_none_or(|w| name.starts_with(w.as_str()));
            Some((!starts, c))
        })
        .collect();
    found.sort_by_key(|(later, _)| *later); // stable: the menu's own order is kept within each group
    found.into_iter().map(|(_, c)| c).collect()
}

impl App {
    /// Every command there is. Where two have the same key, the first that can be done is meant.
    pub(super) fn commands(&self) -> Vec<Command> {
        use Group::{Edit, File, Go, Help, Tools, View};
        let is_pdf = matches!(self.doc, Doc::Pdf(_));
        let is_image = matches!(self.doc, Doc::Image(_));
        let has_doc = is_pdf || is_image;
        let pages = self.organizer.is_some();
        let multi = self.browsed().len() > 1;
        let free = self.markup_busy.is_none();
        let drawing = self.editing.is_none();
        let marks = self.marks.as_ref().filter(|_| drawing);
        let selected = marks.is_some_and(|m| m.store.selection.is_some());
        let editing_picture = is_image && self.edit_open;
        let file = |m: Message| has_doc.then_some(m);
        let pdf = |m: Message| is_pdf.then_some(m);
        let image = |m: Message| is_image.then_some(m);
        let organising = |m: PagesMsg| pages.then_some(Message::Pages(m));
        let edit = |m: EditMsg| editing_picture.then_some(Message::Edit(m));
        let mark = |m: MarkupMsg| marks.map(|_| Message::Markup(m));
        let tool = |t: Tool| (has_doc && drawing && self.markup_open).then_some(Message::Markup(MarkupMsg::Tool(t)));
        let dialog = |d: Dialog| Some(Message::Dialog(Some(d)));

        let mut all = Vec::new();
        let mut add = |id, name: &'static str, group, place, keys, message| {
            all.push(Command { id, name, label: name, group, place, keys, message });
        };
        let main = |part| Some(Place::Main(part));

        add("open", msgid("Open Files…"), File, main(0), &["Ctrl+O"], Some(Message::Open { folder: false }));
        add("open-folder", msgid("Open Folder…"), File, None, &["Ctrl+Shift+O"], Some(Message::Open { folder: true }));
        let save = organising(PagesMsg::Save).or_else(|| {
            (has_doc && free && drawing && self.has_unsaved_changes()).then_some(Message::Markup(MarkupMsg::Save))
        });
        add("save", msgid("Save"), File, main(1), &["Ctrl+S"], save);
        let copy = organising(PagesMsg::SaveAs).or_else(|| file(Message::Markup(MarkupMsg::SaveAs)));
        add("save-copy", msgid("Save a Copy…"), File, main(1), &["Ctrl+Shift+S"], copy.filter(|_| free && drawing));
        add("print", msgid("Print…"), File, main(1), &["Ctrl+P"], file(Message::Print).filter(|_| !self.printing));
        add("export", msgid("Export Pages…"), File, Some(Place::Export), &[], pdf(Message::Export(ExportMsg::Open)));
        add(
            "to-pdf",
            msgid("Export as PDF…"),
            File,
            Some(Place::Export),
            &[],
            image(Message::Export(ExportMsg::Picture)),
        );
        let combine = pdf(Message::Export(ExportMsg::Combine)).filter(|_| free);
        add("combine", msgid("Combine Files…"), File, Some(Place::Export), &[], combine);
        let window = file(Message::NewWindow);
        add("new-window", msgid("Open in New Window"), File, Some(Place::File), &[], window);
        add("show", msgid("Show in Folder"), File, Some(Place::File), &[], file(Message::ShowInFolder));
        if crate::platform::caps().open_with {
            add("open-with", msgid("Open With…"), File, Some(Place::File), &[], file(Message::OpenWith));
        }
        add(
            "rename",
            msgid("Rename…"),
            File,
            Some(Place::File),
            &["F2"],
            file(Message::Rename(super::rename::RenameMsg::Open)),
        );
        add("trash", msgid("Move to Trash"), File, Some(Place::File), &[], file(Message::Trash));
        add("properties", msgid("Properties…"), File, Some(Place::File), &["Alt+Enter"], file(Message::ShowProperties));
        add("preferences", preferences_name(), File, main(2), &["Ctrl+,"], dialog(Dialog::Preferences));
        add("quit", msgid("Quit"), File, None, &["Ctrl+Q"], Some(Message::QuitAll));
        add("close", msgid("Close Window"), File, None, &["Ctrl+W"], Some(Message::Quit));

        let undo = organising(PagesMsg::Undo).or_else(|| edit(EditMsg::Undo)).or_else(|| mark(MarkupMsg::Undo));
        let discard = (has_doc && free && self.has_unsaved_changes()).then_some(Message::Discard);
        add("discard", msgid("Discard Changes…"), File, Some(Place::File), &[], discard);
        add("undo", msgid("Undo"), Edit, None, &["Ctrl+Z"], undo);
        let redo = organising(PagesMsg::Redo).or_else(|| edit(EditMsg::Redo)).or_else(|| mark(MarkupMsg::Redo));
        add("redo", msgid("Redo"), Edit, None, &["Ctrl+Shift+Z", "Ctrl+Y"], redo);
        add("copy", msgid("Copy"), Edit, None, &["Ctrl+C"], pdf(Message::Copy).or_else(|| image(Message::CopyPicture)));
        let everything = organising(PagesMsg::SelectAll)
            .or_else(|| pdf(Message::SelectAll))
            .or_else(|| image(Message::Region(RegionMsg::All)));
        add("select-all", msgid("Select All"), Edit, None, &["Ctrl+A"], everything);
        let crop = self.region.is_some().then_some(Message::Region(RegionMsg::Crop));
        let area = pdf(Message::Area(super::area::AreaMsg::Toggle));
        add("select-area", msgid("Select an Area"), Edit, None, &[], area);
        let invert = self.region.is_some().then_some(Message::Region(RegionMsg::Invert));
        add("invert", msgid("Invert Selection"), Edit, None, &["Ctrl+Shift+I"], invert);
        add("crop", msgid("Crop to Selection"), Edit, None, &["Ctrl+K"], crop);
        let delete = organising(PagesMsg::Delete)
            .or(selected.then_some(Message::Markup(MarkupMsg::Delete)))
            .or(self.region.is_some().then_some(Message::Region(RegionMsg::Delete)));
        add("delete", msgid("Delete Selection"), Edit, None, &["Delete", "Backspace"], delete);

        let view = Some(Place::View);
        add("zoom-in", msgid("Zoom In"), View, None, &["+", "Ctrl++"], file(Message::ZoomIn));
        add("zoom-out", msgid("Zoom Out"), View, None, &["-", "Ctrl+-"], file(Message::ZoomOut));
        add("fit", msgid("Best Fit"), View, view, &["0", "Ctrl+0"], file(Message::ZoomFit));
        add("fit-width", msgid("Fit Width"), View, view, &["W"], pdf(Message::ZoomFitWidth));
        add("actual", msgid("Actual Size"), View, view, &["1", "Ctrl+1"], file(Message::ZoomActual));
        add("fullscreen", msgid("Fullscreen"), View, view, &["F", "F11"], Some(Message::ToggleFullscreen));
        let slideshow = (self.can_slideshow() || self.slideshow.is_some()).then_some(Message::Slideshow);
        add("slideshow", msgid("Slideshow"), View, view, &["F5"], slideshow);
        let sidebar = self.sidebar_shows().map(|_| Message::ToggleSidebar);
        add("sidebar", msgid("Sidebar"), View, view, &["T"], sidebar);
        add("night", msgid("Night Mode"), View, view, &["Ctrl+Shift+N"], pdf(Message::ToggleNight));

        add("find", msgid("Find…"), Go, None, &["Ctrl+F"], pdf(Message::SearchOpen));
        add("find-next", msgid("Next Match"), Go, None, &["F3"], pdf(Message::SearchNext));
        add("find-previous", msgid("Previous Match"), Go, None, &["Shift+F3"], pdf(Message::SearchPrev));
        add("goto", msgid("Go to Page…"), Go, None, &["Ctrl+G"], pdf(Message::GoToOpen));
        add("back", msgid("Back"), Go, None, &["Alt+←"], pdf(Message::LinkBack));
        add("forward", msgid("Forward"), Go, None, &["Alt+→"], pdf(Message::LinkForward));
        add("next-page", msgid("Next Page"), Go, None, &["Page Down"], pdf(Message::PageNext));
        add("previous-page", msgid("Previous Page"), Go, None, &["Page Up"], pdf(Message::PagePrev));
        add("first-page", msgid("First Page"), Go, None, &["Home"], pdf(Message::PageAt(0)));
        add("last-page", msgid("Last Page"), Go, None, &["End"], pdf(Message::PageAt(usize::MAX)));
        // A screen at a time, keeping a little of the last one in view.
        let screen = self.area.height * 0.9;
        let scroll = |y| pdf(Message::ScrollBy(y)).filter(|_| self.slideshow.is_none());
        add("screen-down", msgid("Scroll Down a Screen"), Go, None, &["Space"], scroll(screen));
        add("screen-up", msgid("Scroll Up a Screen"), Go, None, &["Shift+Space"], scroll(-screen));
        let several = |m: Message| multi.then_some(m).filter(|_| self.slideshow.is_none());
        add("next", msgid("Next File"), Go, None, &["→", "Space", "Page Down"], multi.then_some(Message::Next));
        add(
            "previous",
            msgid("Previous File"),
            Go,
            None,
            &["←", "Backspace", "Page Up"],
            multi.then_some(Message::Prev),
        );
        add("first", msgid("First File"), Go, None, &["Home"], several(Message::First));
        add("last", msgid("Last File"), Go, None, &["End"], several(Message::Last));

        // With the markup bar open, letters pick the tool (as in the GTK Lumen).
        add("tool-select", msgid("Select"), Tools, None, &["V"], tool(Tool::Select));
        add("tool-highlight", msgid("Highlighter"), Tools, None, &["H"], tool(Tool::Highlight));
        add("tool-pen", msgid("Pen"), Tools, None, &["P"], tool(Tool::Pen));
        add("tool-sketch", msgid("Sketch"), Tools, None, &["S"], tool(Tool::Sketch));
        add("tool-rectangle", msgid("Rectangle"), Tools, None, &["R"], tool(Tool::Rectangle));
        add("tool-ellipse", msgid("Ellipse"), Tools, None, &["O"], tool(Tool::Ellipse));
        add("tool-line", msgid("Line"), Tools, None, &["L"], tool(Tool::Line));
        add("tool-arrow", msgid("Arrow"), Tools, None, &["A"], tool(Tool::Arrow));
        add("tool-double-arrow", msgid("Double Arrow"), Tools, None, &[], tool(Tool::DoubleArrow));
        add("tool-rounded", msgid("Rounded Rectangle"), Tools, None, &[], tool(Tool::Figure(Figure::Rounded)));
        add("tool-star", msgid("Star"), Tools, None, &[], tool(Tool::Figure(Figure::Star)));
        add("tool-polygon", msgid("Polygon"), Tools, None, &[], tool(Tool::Figure(Figure::Polygon)));
        add("tool-bubble", msgid("Speech Bubble"), Tools, None, &[], tool(Tool::Figure(Figure::Bubble)));
        add("tool-text", msgid("Text"), Tools, None, &["T"], tool(Tool::Text));
        let markup = (drawing && !pages).then_some(Message::Markup(MarkupMsg::Toggle)).filter(|_| has_doc);
        add("markup", msgid("Markup"), Tools, None, &["M"], markup);
        let cropping = image(Message::Edit(EditMsg::CropAlone));
        add("crop-tool", msgid("Crop…"), Tools, None, &[], cropping);
        add("edit", msgid("Edit Picture"), Tools, None, &["E"], image(Message::Edit(EditMsg::Toggle)));
        let organise = organising(PagesMsg::Close).or_else(|| pdf(Message::Pages(PagesMsg::Open)).filter(|_| free));
        add("organise", msgid("Organise Pages"), Tools, None, &["G"], organise);
        add("sign", msgid("Sign…"), Tools, None, &[], file(Message::Sign(SignMsg::Menu)));
        let right = organising(PagesMsg::Rotate(90)).or_else(|| image(Message::Rotate(1)));
        add("rotate-right", msgid("Rotate Right"), Tools, None, &["R"], right);
        let left = organising(PagesMsg::Rotate(-90)).or_else(|| image(Message::Rotate(-1)));
        add("rotate-left", msgid("Rotate Left"), Tools, None, &["Shift+R"], left);
        let keys = &["Ctrl+←", "Ctrl+↑"];
        add("move-earlier", msgid("Move Pages Earlier"), Tools, None, keys, organising(PagesMsg::MoveBy(-1)));
        let keys = &["Ctrl+→", "Ctrl+↓"];
        add("move-later", msgid("Move Pages Later"), Tools, None, keys, organising(PagesMsg::MoveBy(1)));

        let palette = Some(Message::Palette(PaletteMsg::Open));
        add("palette", msgid("Command Palette"), Help, None, &["Ctrl+Shift+P"], palette);
        add("shortcuts", msgid("Keyboard Shortcuts"), Help, main(2), &[], dialog(Dialog::Shortcuts));
        add("about", msgid("About Lumen"), Help, main(2), &[], dialog(Dialog::About));

        // The first choice of a letter goes to the drawing tools: they come before what it means
        // without the bar ("T" is the sidebar).
        all.sort_by_key(|c: &Command| !(c.id.starts_with("tool-") && c.message.is_some()));
        for c in &mut all {
            c.label = match c.id {
                "fullscreen" if self.fullscreen => msgid("Leave Fullscreen"),
                "slideshow" if self.slideshow.is_some() => msgid("Stop Slideshow"),
                "sidebar" if self.show_sidebar => msgid("Hide Sidebar"),
                "select-area" if self.area_mode => msgid("Stop Selecting Areas"),
                "sidebar" if self.sidebar_tabs() == [super::SidebarTab::Files] => msgid("Show Folder"),
                "sidebar" => msgid("Show Sidebar"),
                "night" if self.night => msgid("Leave Night Mode"),
                _ => c.name,
            };
        }
        all
    }

    pub(super) fn command(&self, id: &str) -> Option<Command> {
        self.commands().into_iter().find(|c| c.id == id)
    }

    /// What a key does now.
    pub(super) fn key_message(&self, key: &Key, modifiers: Modifiers) -> Option<Message> {
        let organising = self.organizer.is_some();
        let is_pdf = matches!(self.doc, Doc::Pdf(_));
        self.commands()
            .into_iter()
            .filter(|c| !organising || ORGANISING.contains(&c.id))
            // In a PDF Backspace is not a way to leave the file.
            .filter(|c| !(is_pdf && c.id == "previous" && *key == Key::Named(Named::Backspace)))
            .filter(|c| c.keys.iter().any(|chord| pressed(chord, key, modifiers)))
            .find_map(|c| c.message)
    }

    pub(super) fn palette_matches(&self) -> Vec<Command> {
        let query = self.palette.as_ref().map_or("", |p| p.query.as_str());
        let can = self.commands().into_iter().filter(|c| c.message.is_some() && c.id != "palette");
        filter(can.collect(), query)
    }

    pub(super) fn palette_update(&mut self, msg: PaletteMsg) -> Task<Message> {
        match msg {
            PaletteMsg::Open => {
                if self.palette.is_some() {
                    self.palette = None;
                    return Task::none();
                }
                self.palette = Some(Palette::default());
                self.menu = None;
                iced::widget::operation::focus(self.widget_id("command-palette"))
            }
            PaletteMsg::Close => {
                self.palette = None;
                Task::none()
            }
            PaletteMsg::Input(query) => {
                if let Some(p) = &mut self.palette {
                    p.query = query;
                    p.selected = 0;
                }
                Task::none()
            }
            PaletteMsg::Move(delta) => {
                let n = self.palette_matches().len().min(SHOWN);
                if let Some(p) = &mut self.palette
                    && n > 0
                {
                    p.selected = (p.selected as i32 + delta).rem_euclid(n as i32) as usize;
                }
                Task::none()
            }
            PaletteMsg::Run => {
                let at = self.palette.as_ref().map_or(0, |p| p.selected);
                self.palette_run(at)
            }
            PaletteMsg::Pick(at) => self.palette_run(at),
        }
    }

    fn palette_run(&mut self, at: usize) -> Task<Message> {
        let found = self.palette_matches();
        self.palette = None;
        match found.into_iter().nth(at) {
            Some(Command { message: Some(message), .. }) => self.update(message),
            _ => Task::none(),
        }
    }

    /// Up and Down move the highlight while the palette is open (the text box leaves them alone).
    pub(super) fn palette_key(&mut self, key: &iced::keyboard::Key) -> Option<Task<Message>> {
        use iced::keyboard::{Key, key::Named};
        self.palette.as_ref()?;
        let delta = match key.as_ref() {
            Key::Named(Named::ArrowDown) => 1,
            Key::Named(Named::ArrowUp) => -1,
            _ => return None,
        };
        Some(self.palette_update(PaletteMsg::Move(delta)))
    }

    pub(super) fn palette_view(&self) -> Option<Element<'_, Message>> {
        let palette = self.palette.as_ref()?;
        let found = self.palette_matches();
        let hint = tr("Type a command");
        let input = crate::ui::bare_input(&hint, &palette.query)
            .id(self.widget_id("command-palette"))
            .on_input(|q| Message::Palette(PaletteMsg::Input(q)))
            .on_submit(Message::Palette(PaletteMsg::Run))
            .size(style::TITLE);
        let glass = svg(icons::search()).width(16).height(16).style(style::icon_muted);
        let top = row![glass, input].spacing(6).align_y(Alignment::Center);
        let mut list = column![container(top).padding(Padding { left: 10.0, ..Padding::ZERO })].spacing(2);
        if found.is_empty() {
            list = list.push(text(tr("No matching command")).size(style::BODY).style(muted));
        }
        for (i, command) in found.iter().enumerate().take(SHOWN) {
            let line = row![
                text(tr(command.label)).size(style::BODY),
                text(tr(command.group.name())).size(style::SMALL).style(muted),
                space().width(Length::Fill),
                text(crate::platform::keys::show(command.key())).size(style::SMALL).style(muted),
            ]
            .spacing(8)
            .align_y(Alignment::Center);
            list = list.push(
                button(line)
                    .padding([6, 10])
                    .width(Length::Fill)
                    .style(if i == palette.selected { style::list_row } else { style::tool })
                    .on_press(Message::Palette(PaletteMsg::Pick(i))),
            );
        }
        let panel = container(list).width(440).padding(10).style(style::panel);
        Some(
            stack![
                mouse_area(space().width(Length::Fill).height(Length::Fill))
                    .on_press(Message::Palette(PaletteMsg::Close)),
                container(opaque(panel))
                    .width(Length::Fill)
                    .align_x(Alignment::Center)
                    .padding(Padding { top: 64.0, ..Padding::ZERO }),
            ]
            .into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(name: &'static str) -> Command {
        let (group, message) = (Group::Go, Some(Message::Next));
        Command { id: "", name, label: name, group, place: None, keys: &[], message }
    }

    fn list(names: &[&'static str]) -> Vec<Command> {
        names.iter().map(|n| command(n)).collect()
    }

    fn names(found: Vec<Command>) -> Vec<String> {
        found.into_iter().map(|c| c.label.to_string()).collect()
    }

    #[test]
    fn every_word_must_appear_in_any_order_and_any_case() {
        let all = list(&["Open Files…", "Open Folder…", "Fit Width", "Show in Folder", "Move to Trash"]);
        assert_eq!(names(filter(all.clone(), "folder")), ["Open Folder…", "Show in Folder"]);
        assert_eq!(names(filter(all.clone(), "FOLDER open")), ["Open Folder…"]);
        assert_eq!(names(filter(all.clone(), "  ")).len(), 5, "no words, no filter");
        assert!(filter(all, "zzz").is_empty());
    }

    #[test]
    fn a_name_starting_with_the_word_comes_first() {
        let all = list(&["Show in Folder", "Open Folder…", "Fold Pages", "Print…"]);
        assert_eq!(names(filter(all, "fold")), ["Fold Pages", "Show in Folder", "Open Folder…"]);
    }
}
