//! The header's menus, as in the GTK Lumen: the open button's drop-down, and the main
//! menu behind ☰ with everything that does not need a button of its own. Also the
//! keyboard shortcuts and about dialogs they open.

use iced::widget::{button, center, column, container, mouse_area, opaque, row, rule, space, stack, svg, text};
use iced::{Alignment, Element, Font, Length, Point, Task, Theme};

use super::commands::{Command, Group, Place, gestures};
use super::markup::MarkupMsg;
use super::popup::{Anchor, popup};
use super::{App, Doc, Message, muted};
use crate::annotate::Tool;
use crate::i18n::{msgid, tr};
use crate::platform::keys;
use crate::{icons, style, ui};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    Open,
    /// Behind ☰, and the menus it leads to.
    Main,
    Recent,
    View,
    Export,
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialog {
    Shortcuts,
    About,
    Properties,
    Export,
    Preferences,
}

/// One line of a menu.
#[allow(clippy::large_enum_variant)] // Most of them are the large one.
pub enum Line {
    /// A command, or what else there is: a label, its shortcut, and what it does (greyed out without).
    Item(String, &'static str, Option<Message>),
    /// Another menu in this one's place.
    Into(Menu),
    /// The way back from one.
    Back(Menu),
    Rule,
}

impl Menu {
    fn name(self) -> &'static str {
        match self {
            Menu::Open | Menu::Main => msgid("Menu"),
            Menu::Recent => msgid("Open Recent"),
            Menu::View => msgid("View"),
            Menu::Export => msgid("Export"),
            Menu::File => msgid("File"),
        }
    }

    /// Whether it hangs from the ☰ button.
    pub fn is_main(self) -> bool {
        self != Menu::Open
    }
}

/// What was clicked with the right button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    /// The picture or page, or what is selected on it.
    View,
    /// A drawn annotation, which is selected now.
    Mark,
    /// A file in the sidebar.
    File(usize),
}

impl App {
    pub(super) fn menu_update(&mut self, menu: Option<Menu>) -> Task<Message> {
        self.context = None;
        self.menu = if self.menu == menu { None } else { menu };
        Task::none()
    }

    /// A menu item was chosen: close the menu, then do it.
    pub(super) fn menu_pick(&mut self, msg: Message) -> Task<Message> {
        self.menu = None;
        self.context = None;
        self.update(msg)
    }

    /// Escape closes an open menu or dialog first.
    pub(super) fn menu_escape(&mut self) -> bool {
        self.context.take().is_some() || self.menu.take().is_some() || self.dialog.take().is_some()
    }

    /// The right button went down on something.
    pub(super) fn context_open(&mut self, on: Context) -> Task<Message> {
        // The layer of annotations says it first, then the view it lies on.
        if std::mem::take(&mut self.context_taken) && on == Context::View {
            return Task::none();
        }
        if self.organizer.is_some() || self.crop.is_some() || self.slideshow.is_some() {
            return Task::none();
        }
        self.menu = None;
        let task = match on {
            Context::View if !matches!(self.doc, Doc::Image(_) | Doc::Pdf(_)) => return Task::none(),
            // What is done in the menu is done to the file on screen.
            Context::File(i) if i != self.current => self.update(Message::Select(i)),
            _ => Task::none(),
        };
        self.context_taken = on == Context::Mark;
        self.context = Some((on, self.right_press));
        task
    }

    /// Commands by their ids, as menu lines; "" is a rule. Ones that make no sense now are left out.
    fn lines(&self, ids: &[&str]) -> Vec<Line> {
        let all = self.commands();
        let mut lines: Vec<Line> = Vec::new();
        for id in ids {
            match all.iter().find(|c| c.id == *id) {
                Some(c) if c.message.is_some() => {
                    lines.push(Line::Item(tr(c.label), c.key(), c.message.clone()));
                }
                None if id.is_empty() && !matches!(lines.last(), None | Some(Line::Rule)) => lines.push(Line::Rule),
                _ => {}
            }
        }
        if let Some(Line::Rule) = lines.last() {
            lines.pop();
        }
        lines
    }

    pub(super) fn context_lines(&self, on: Context) -> Vec<Line> {
        match on {
            Context::Mark => {
                let front = Line::Item(tr("Bring to Front"), "", Some(Message::Markup(MarkupMsg::ToFront)));
                let mut lines = self.lines(&["delete"]);
                lines.push(front);
                lines
            }
            Context::File(_) => self.lines(&["show", "open-with", "properties", "", "trash"]),
            Context::View => {
                let picture = matches!(self.doc, Doc::Image(_));
                let text = matches!(self.doc, Doc::Pdf(_)) && self.selection.is_some_and(|s| !s.is_empty());
                let mut lines = if picture || text { self.lines(&["copy"]) } else { Vec::new() };
                if text {
                    let mark = Message::Markup(MarkupMsg::Tool(Tool::Highlight));
                    lines.push(Line::Item(tr("Highlight"), "", Some(mark)));
                }
                if picture && self.region.is_some() {
                    lines.extend(self.lines(&["crop"]));
                }
                if !lines.is_empty() {
                    lines.push(Line::Rule);
                }
                lines.extend(self.lines(&["fit", "fit-width", "actual"]));
                if picture {
                    lines.extend(self.lines(&["rotate-right", "rotate-left"]));
                }
                lines.push(Line::Rule);
                lines.extend(self.lines(&["show", "properties"]));
                lines
            }
        }
    }

    pub(super) fn menu_lines(&self, menu: Menu) -> Vec<Line> {
        let all = self.commands();
        let placed = |place: Place| {
            all.iter()
                .filter(move |c| c.place == Some(place))
                .map(|c| Line::Item(tr(c.label), c.key(), c.message.clone()))
                .collect::<Vec<_>>()
        };
        let sub = |place: Place| {
            let mut lines = vec![Line::Back(menu), Line::Rule];
            lines.extend(placed(place));
            lines
        };
        match menu {
            Menu::Open => self.lines(&["open", "open-folder"]),
            Menu::View => sub(Place::View),
            Menu::Export => sub(Place::Export),
            Menu::File => sub(Place::File),
            Menu::Recent => {
                let mut lines = vec![Line::Back(menu), Line::Rule];
                lines.extend(
                    self.state
                        .listed()
                        .map(|r| Line::Item(super::file_name(&r.path), "", Some(Message::OpenRecent(r.path.clone())))),
                );
                lines.push(Line::Rule);
                lines.push(Line::Item(tr("Clear"), "", Some(Message::ClearRecent)));
                lines
            }
            Menu::Main => {
                let mut lines = placed(Place::Main(0));
                if self.state.listed().next().is_some() {
                    lines.push(Line::Into(Menu::Recent));
                }
                lines.push(Line::Rule);
                lines.extend(placed(Place::Main(1)));
                lines.push(Line::Rule);
                lines.extend([Menu::View, Menu::Export, Menu::File].map(Line::Into));
                lines.push(Line::Rule);
                lines.extend(placed(Place::Main(2)));
                lines
            }
        }
    }

    /// The menus of the system's menu bar, where there is one: the program's own, then every command.
    pub(super) fn menu_bar(&self) -> Vec<crate::platform::MenuGroup> {
        use crate::platform::{MenuEntry, MenuGroup};
        let own = ["about", "preferences", "quit"];
        let entry = |c: &Command| {
            // Plain letters, and what text boxes use, would be taken away from typing.
            let keys = if c.key().starts_with("Ctrl+") && !c.text_key() && !c.key().ends_with(['←', '→']) {
                c.key()
            } else {
                ""
            };
            Some(MenuEntry { id: c.id, label: tr(c.name), keys })
        };
        let all = self.commands();
        let mut program: Vec<_> = Vec::new();
        for id in own {
            program.extend(all.iter().find(|c| c.id == id).map(entry));
            program.push(None);
        }
        program.pop();
        let mut groups = vec![MenuGroup { title: "Lumen".into(), items: program }];
        groups.extend(Group::ALL.into_iter().map(|group| {
            let items = all.iter().filter(|c| c.group == group && !own.contains(&c.id)).map(entry).collect();
            MenuGroup { title: tr(group.name()), items }
        }));
        groups
    }

    /// What an item of the menu bar does, if it can be done now.
    pub(super) fn menu_bar_message(&self, id: &str) -> Option<Message> {
        // A question or a chooser on screen is answered first.
        let asked = self.confirm.is_some()
            || self.confirm_quit
            || self.chooser.is_some()
            || self.print_dialog.is_some()
            || self.pad.is_some();
        if asked && id != "quit" {
            return None;
        }
        self.command(id)?.message
    }

    fn line_view<'a>(line: Line) -> Element<'a, Message> {
        let item = |content: Element<'a, Message>, press: Option<Message>| -> Element<'a, Message> {
            button(content).padding([7, 10]).width(Length::Fill).style(style::menu_item).on_press_maybe(press).into()
        };
        let glyph = |handle| svg(handle).width(14).height(14).style(style::icon_muted);
        match line {
            Line::Rule => container(rule::horizontal(1)).padding([4, 6]).into(),
            Line::Item(label, keys, message) => {
                let enabled = message.is_some();
                let name = text(label)
                    .size(style::BODY)
                    .width(Length::Fill)
                    .style(move |t: &Theme| text::Style { color: (!enabled).then(|| style::unavailable(t)) });
                let line = row![name, text(keys::show(keys)).size(style::SMALL).style(muted)]
                    .spacing(12)
                    .align_y(Alignment::Center);
                item(line.into(), message.map(|m| Message::MenuPick(Box::new(m))))
            }
            Line::Into(menu) => {
                let line = row![text(tr(menu.name())).size(style::BODY).width(Length::Fill), glyph(icons::next())]
                    .align_y(Alignment::Center);
                item(line.into(), Some(Message::Menu(Some(menu))))
            }
            Line::Back(menu) => {
                let name = text(tr(menu.name())).size(style::BODY).font(style::font(iced::font::Weight::Semibold));
                let line = row![glyph(icons::prev()), name].spacing(6).align_y(Alignment::Center);
                item(line.into(), Some(Message::Menu(Some(Menu::Main))))
            }
        }
    }

    fn panel_of<'a>(lines: Vec<Line>) -> Element<'a, Message> {
        let list = column(lines.into_iter().map(Self::line_view)).spacing(1);
        container(ui::scroll(list)).width(262).padding(6).style(style::panel).into()
    }

    /// `base`, a button of the header, with the menu `which` says hangs from it, if it is open.
    pub(super) fn with_menu<'a>(
        &self,
        base: Element<'a, Message>,
        which: fn(Menu) -> bool,
        anchor: Anchor,
    ) -> Element<'a, Message> {
        let panel = self.menu.filter(|m| which(*m)).map(|m| Self::panel_of(self.menu_lines(m)));
        popup(base, panel, anchor, Message::Menu(None)).into()
    }

    /// The window, with the context menu over it.
    pub(super) fn with_context<'a>(&self, base: Element<'a, Message>) -> Element<'a, Message> {
        let panel = self.context.map(|(on, _)| Self::panel_of(self.context_lines(on)));
        let at = self.context.map_or(Point::ORIGIN, |(_, at)| at);
        popup(base, panel, Anchor::At(at), Message::Menu(None)).on_right_press(Message::RightPress).into()
    }

    /// The shortcuts and what they do, by group.
    fn shortcuts(&self) -> Vec<(Group, Vec<(String, String)>)> {
        let all = self.commands();
        Group::ALL
            .into_iter()
            .map(|group| {
                let commands = all.iter().filter(|c| c.group == group && !c.keys.is_empty());
                let mut entries: Vec<_> = commands.map(|c| (keys::show(&c.keys.join(", ")), tr(c.name))).collect();
                let more = gestures().into_iter().filter(|g| g.0 == group);
                entries.extend(more.map(|(_, keys, what)| (keys::show(keys), tr(what))));
                (group, entries)
            })
            .filter(|(_, entries)| !entries.is_empty())
            .collect()
    }

    pub(super) fn dialog_view(&self) -> Option<Element<'_, Message>> {
        let dialog = self.dialog?;
        let title = |t: String| {
            text(t).size(style::HEADING).font(Font { weight: iced::font::Weight::Semibold, ..crate::style::FONT })
        };
        let close = button(text(tr("Close")).size(style::BODY))
            .padding([8, 16])
            .style(style::secondary)
            .on_press(Message::Dialog(None));
        let body: Element<'_, Message> = match dialog {
            Dialog::Shortcuts => {
                let mut groups = column![].spacing(16);
                for (group, entries) in self.shortcuts() {
                    let mut list = column![
                        text(tr(group.name())).size(style::BODY).font(style::font(iced::font::Weight::Semibold))
                    ]
                    .spacing(6);
                    for (key, what) in entries {
                        list = list.push(
                            row![
                                text(what).size(style::BODY).width(Length::Fill),
                                text(key).size(style::SMALL).style(muted)
                            ]
                            .spacing(12),
                        );
                    }
                    groups = groups.push(list);
                }
                column![
                    title(tr("Keyboard Shortcuts")),
                    ui::scroll(groups).height(Length::Shrink),
                    row![space().width(Length::Fill), close]
                ]
                .spacing(14)
                .width(420)
                .into()
            }
            Dialog::Preferences => self.prefs_dialog(title(tr("Preferences")).into(), close.into()),
            Dialog::Export => self.export_dialog(title(tr("Export")).into(), close.into()),
            Dialog::Properties => {
                let mut sections = column![].spacing(16);
                match &self.properties {
                    None => sections = sections.push(text(tr("Reading…")).size(style::BODY).style(muted)),
                    Some(Err(e)) => sections = sections.push(text(e.as_str()).size(style::BODY)),
                    Some(Ok(sheet)) => {
                        for (name, rows) in &sheet.sections {
                            let mut group = column![
                                text(name.as_str())
                                    .size(style::BODY)
                                    .font(Font { weight: iced::font::Weight::Semibold, ..crate::style::FONT })
                            ]
                            .spacing(6);
                            for (key, value) in rows {
                                group = group.push(
                                    row![
                                        text(key.as_str()).size(style::SMALL).style(muted).width(104),
                                        text(value.as_str()).size(style::BODY).width(Length::Fill),
                                    ]
                                    .spacing(10),
                                );
                            }
                            sections = sections.push(group);
                        }
                    }
                }
                column![
                    title(tr("Properties")),
                    ui::scroll(sections).height(Length::Shrink),
                    row![space().width(Length::Fill), close]
                ]
                .spacing(14)
                .width(460)
                .into()
            }
            Dialog::About => column![
                center(iced::widget::svg(crate::icons::app()).width(80).height(80)).height(90),
                container(title(tr("Lumen"))).center_x(Length::Fill),
                container(text(trf!("Version {}", env!("CARGO_PKG_VERSION"))).size(style::BODY).style(muted))
                    .center_x(Length::Fill),
                container(
                    text(tr("A fast, safe viewer and editor for pictures and PDFs, in pure Rust with iced."))
                        .size(style::BODY)
                        .align_x(Alignment::Center)
                )
                .center_x(Length::Fill),
                container(text(tr("GPL-3.0-or-later · github.com/ImanolGo/lumen-rs")).size(style::SMALL).style(muted))
                    .center_x(Length::Fill),
                row![space().width(Length::Fill), close],
            ]
            .spacing(10)
            .width(360)
            .into(),
        };
        let card = container(body).padding(22).style(style::panel).max_height(self.area.height.max(300.0));
        let backdrop = mouse_area(space().width(Length::Fill).height(Length::Fill)).on_press(Message::Dialog(None));
        Some(stack![container(backdrop).style(style::scrim), center(opaque(card))].into())
    }
}
