//! The command palette (Ctrl+Shift+P): type a few words, pick a command, press Enter. It lists
//! what the menus offer (and a few things only keys and buttons offer), leaving out whatever
//! cannot be done right now.

use iced::widget::{button, column, container, mouse_area, opaque, row, space, stack, text};
use iced::{Alignment, Element, Length, Padding, Task};

use super::{App, Doc, Message, muted};
use crate::i18n::tr;
use crate::style;

/// How many matches are listed at once.
const SHOWN: usize = 8;

fn input_id() -> iced::widget::Id {
    iced::widget::Id::new("command-palette")
}

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

/// A command: what it is called, its shortcut and what it does.
pub type Command = (String, &'static str, Message);

/// The commands whose words all appear in `query`, best first: a command whose name starts
/// with the first word comes before one that only contains it.
pub fn filter(commands: Vec<Command>, query: &str) -> Vec<Command> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut found: Vec<(bool, Command)> = commands
        .into_iter()
        .filter_map(|c| {
            let name = c.0.to_lowercase();
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
    /// Every command that can be run now.
    fn commands(&self) -> Vec<Command> {
        let has_doc = matches!(self.doc, Doc::Image(_) | Doc::Pdf(_));
        let is_pdf = matches!(self.doc, Doc::Pdf(_));
        let multi = self.files.len() > 1;
        let mut all: Vec<Command> = Vec::new();
        for (label, keys, message) in Self::open_items().into_iter().chain(self.main_items()).flatten() {
            if let Some(message) = message {
                all.push((label, keys, message));
            }
        }
        let mut extra = |label: &str, keys: &'static str, on: bool, message: Message| {
            if on {
                all.push((tr(label), keys, message));
            }
        };
        extra("Zoom In", "+", has_doc, Message::ZoomIn);
        extra("Zoom Out", "-", has_doc, Message::ZoomOut);
        extra("Next File", "→", multi, Message::Next);
        extra("Previous File", "←", multi, Message::Prev);
        extra("Go to Page…", "Ctrl+G", is_pdf, Message::GoToOpen);
        extra("Next Page", "Page Down", is_pdf, Message::PageNext);
        extra("Previous Page", "Page Up", is_pdf, Message::PagePrev);
        all
    }

    pub(super) fn palette_matches(&self) -> Vec<Command> {
        let query = self.palette.as_ref().map_or("", |p| p.query.as_str());
        filter(self.commands(), query)
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
                iced::widget::operation::focus(input_id())
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
            Some((_, _, message)) => self.update(message),
            None => Task::none(),
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
        let mut list = column![
            crate::ui::bare_input(&hint, &palette.query)
                .id(input_id())
                .on_input(|q| Message::Palette(PaletteMsg::Input(q)))
                .on_submit(Message::Palette(PaletteMsg::Run))
                .size(style::TITLE)
                .width(Length::Fill)
        ]
        .spacing(6);
        if found.is_empty() {
            list = list.push(text(tr("No matching command")).size(style::BODY).style(muted));
        }
        for (i, (label, keys, _)) in found.iter().enumerate().take(SHOWN) {
            let line = row![
                text(label.clone()).size(style::BODY),
                space().width(Length::Fill),
                text(crate::platform::keys::show(keys)).size(style::SMALL).style(muted),
            ]
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

    fn list(names: &[&str]) -> Vec<Command> {
        names.iter().map(|n| (n.to_string(), "", Message::Next)).collect()
    }

    fn names(found: Vec<Command>) -> Vec<String> {
        found.into_iter().map(|c| c.0).collect()
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
