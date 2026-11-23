//! The header's menus, as in the GTK Lumen: the open button's drop-down, and the main
//! menu behind ☰ with everything that does not need a button of its own. Also the
//! keyboard shortcuts and about dialogs they open.

use iced::widget::{button, center, column, container, mouse_area, opaque, row, rule, scrollable, space, stack, text};
use iced::{Alignment, Element, Font, Length, Padding, Task, Theme};

use super::edit::EditMsg;
use super::export::ExportMsg;
use super::markup::MarkupMsg;
use super::pages::PagesMsg;
use super::signature::SignMsg;
use super::{App, Doc, Message, TOOLBAR_H, muted};
use crate::style;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    Open,
    Main,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialog {
    Shortcuts,
    About,
    Properties,
    Export,
    Preferences,
}

/// One line of a menu: a label, its shortcut, and what it does (greyed out without).
type Item = (&'static str, &'static str, Option<Message>);

/// The keyboard shortcuts, in the groups the dialog shows them.
const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    (
        "Files",
        &[
            ("Ctrl+O", "Open files"),
            ("Ctrl+Shift+O", "Open a folder"),
            ("← →", "Previous / next file"),
            ("Home / End", "First / last file (page in a PDF)"),
            ("Ctrl+P", "Print"),
            ("Ctrl+S", "Save (a copy of a picture)"),
            ("Ctrl+Shift+S", "Save a copy"),
            ("Ctrl+Q", "Quit"),
        ],
    ),
    (
        "View",
        &[
            ("+ −, Ctrl+scroll, pinch", "Zoom in / out"),
            ("0", "Best fit"),
            ("1", "Actual size"),
            ("W", "Fit width (PDF)"),
            ("T", "Sidebar"),
            ("F, F11", "Fullscreen"),
            ("F5", "Slideshow (Space pauses, Esc leaves)"),
        ],
    ),
    (
        "PDF",
        &[
            ("Ctrl+F, F3", "Find, next match"),
            ("Ctrl+G", "Go to page"),
            ("Ctrl+Shift+N", "Night mode"),
            ("Page Up / Down", "Previous / next page"),
            ("Ctrl+C, Ctrl+A", "Copy text, select all"),
            ("G", "Organise pages"),
        ],
    ),
    (
        "Editing",
        &[
            ("M", "Markup"),
            ("E", "Edit the picture"),
            ("R, Shift+R", "Turn right / left"),
            ("Ctrl+Z, Ctrl+Shift+Z", "Undo, redo"),
            ("Delete", "Delete the selection"),
            ("Esc", "Cancel, close"),
        ],
    ),
];

impl App {
    pub(super) fn menu_update(&mut self, menu: Option<Menu>) -> Task<Message> {
        self.menu = if self.menu == menu { None } else { menu };
        Task::none()
    }

    /// A menu item was chosen: close the menu, then do it.
    pub(super) fn menu_pick(&mut self, msg: Message) -> Task<Message> {
        self.menu = None;
        self.update(msg)
    }

    /// Escape closes an open menu or dialog first.
    pub(super) fn menu_escape(&mut self) -> bool {
        self.menu.take().is_some() || self.dialog.take().is_some()
    }

    fn main_items(&self) -> Vec<Option<Item>> {
        let has_doc = matches!(self.doc, Doc::Image(_) | Doc::Pdf(_));
        let is_pdf = matches!(self.doc, Doc::Pdf(_));
        let is_image = matches!(self.doc, Doc::Image(_));
        let doc = |m: Message| has_doc.then_some(m);
        let mut items: Vec<Option<Item>> = vec![
            Some(("Print…", "Ctrl+P", (has_doc && !self.printing).then_some(Message::Print))),
            None,
            Some(("Best Fit", "0", doc(Message::ZoomFit))),
        ];
        if is_pdf {
            items.push(Some(("Fit Width", "W", Some(Message::ZoomFitWidth))));
        }
        items.push(Some(("Actual Size", "1", doc(Message::ZoomActual))));
        if is_image {
            items.push(Some(("Rotate Right", "R", Some(Message::Rotate(1)))));
            items.push(Some(("Rotate Left", "Shift+R", Some(Message::Rotate(-1)))));
        }
        items.push(Some((
            if self.slideshow.is_some() { "Stop Slideshow" } else { "Slideshow" },
            "F5",
            (self.can_slideshow() || self.slideshow.is_some()).then_some(Message::Slideshow),
        )));
        items.push(Some((
            if self.fullscreen { "Leave Fullscreen" } else { "Fullscreen" },
            "F",
            Some(Message::ToggleFullscreen),
        )));
        if self.sidebar_shows().is_some() {
            items.push(Some((
                if self.show_sidebar { "Hide Sidebar" } else { "Show Sidebar" },
                "T",
                Some(Message::ToggleSidebar),
            )));
        }
        items.push(None);
        if is_pdf {
            items.push(Some(("Find…", "Ctrl+F", Some(Message::SearchOpen))));
            items.push(Some((
                if self.night { "Leave Night Mode" } else { "Night Mode" },
                "Ctrl+Shift+N",
                Some(Message::ToggleNight),
            )));
            items.push(Some((
                "Organise Pages",
                "G",
                self.markup_busy.is_none().then_some(Message::Pages(PagesMsg::Open)),
            )));
        }
        if is_image {
            items.push(Some(("Edit Picture", "E", Some(Message::Edit(EditMsg::Toggle)))));
        }
        items.push(Some(("Markup", "M", doc(Message::Markup(MarkupMsg::Toggle)))));
        items.push(Some(("Sign…", "", doc(Message::Sign(SignMsg::Menu)))));
        items.push(None);
        if is_pdf {
            let save = self.has_unsaved_changes() && self.markup_busy.is_none();
            items.push(Some(("Save", "Ctrl+S", save.then_some(Message::Markup(MarkupMsg::Save)))));
        }
        if self.picture_in_place().is_some() {
            let changed = matches!(&self.doc, Doc::Image(v) if v.edit.is_dirty())
                || self.marks.as_ref().is_some_and(|m| m.has_items());
            items.push(Some((
                "Save",
                "Ctrl+S",
                (changed && self.markup_busy.is_none()).then_some(Message::Markup(MarkupMsg::Save)),
            )));
        }
        items.push(Some(("Save a Copy…", "Ctrl+Shift+S", doc(Message::Markup(MarkupMsg::SaveAs)))));
        if is_pdf {
            items.push(Some(("Export Pages…", "", Some(Message::Export(ExportMsg::Open)))));
            items.push(Some((
                "Combine Files…",
                "",
                self.markup_busy.is_none().then_some(Message::Export(ExportMsg::Combine)),
            )));
        }
        if is_image {
            items.push(Some(("Export as PDF…", "", Some(Message::Export(ExportMsg::Picture)))));
        }
        items.push(None);
        items.push(Some(("Properties…", "", doc(Message::ShowProperties))));
        items.push(None);
        items.push(Some(("Show in Folder", "", doc(Message::ShowInFolder))));
        items.push(Some(("Open With…", "", doc(Message::OpenWith))));
        items.push(Some(("Move to Trash", "", doc(Message::Trash))));
        items.push(None);
        items.push(Some(("Preferences", "", Some(Message::Dialog(Some(Dialog::Preferences))))));
        items.push(Some(("Keyboard Shortcuts", "", Some(Message::Dialog(Some(Dialog::Shortcuts))))));
        items.push(Some(("About Lumen", "", Some(Message::Dialog(Some(Dialog::About))))));
        items
    }

    fn open_items() -> Vec<Option<Item>> {
        vec![
            Some(("Open Files…", "Ctrl+O", Some(Message::Open { folder: false }))),
            Some(("Open Folder…", "Ctrl+Shift+O", Some(Message::Open { folder: true }))),
        ]
    }

    /// The open menu, floating under its button, with a backdrop that closes it.
    pub(super) fn menu_view(&self) -> Option<Element<'_, Message>> {
        let menu = self.menu?;
        let (items, align, pad) = match menu {
            Menu::Open => (Self::open_items(), Alignment::Start, Padding { left: 44.0, ..Padding::ZERO }),
            Menu::Main => (self.main_items(), Alignment::End, Padding { right: 46.0, ..Padding::ZERO }),
        };
        let mut list = column![].spacing(1).width(250);
        for item in items {
            list = match item {
                None => list.push(container(rule::horizontal(1)).padding([4, 6])),
                Some((label, keys, msg)) => {
                    let enabled = msg.is_some();
                    let line = row![
                        text(label).size(14).width(Length::Fill).style(move |t: &Theme| text::Style {
                            color: Some(t.extended_palette().background.base.text.scale_alpha(if enabled {
                                1.0
                            } else {
                                0.4
                            })),
                        }),
                        text(keys).size(12).style(muted),
                    ]
                    .align_y(Alignment::Center);
                    list.push(
                        button(line)
                            .padding([7, 10])
                            .width(Length::Fill)
                            .style(style::menu_item)
                            .on_press_maybe(msg.map(|m| Message::MenuPick(Box::new(m)))),
                    )
                }
            };
        }
        let panel = opaque(container(list).padding(6).style(style::panel));
        let backdrop = mouse_area(space().width(Length::Fill).height(Length::Fill)).on_press(Message::Menu(None));
        let placed =
            container(panel).width(Length::Fill).align_x(align).padding(Padding { top: TOOLBAR_H - 4.0, ..pad });
        Some(stack![backdrop, placed].into())
    }

    pub(super) fn dialog_view(&self) -> Option<Element<'_, Message>> {
        let dialog = self.dialog?;
        let title = |t: &'static str| {
            text(t).size(18).font(Font { weight: iced::font::Weight::Semibold, ..crate::style::FONT })
        };
        let close =
            button(text("Close").size(14)).padding([8, 16]).style(style::secondary).on_press(Message::Dialog(None));
        let body: Element<'_, Message> = match dialog {
            Dialog::Shortcuts => {
                let mut groups = column![].spacing(16);
                for (name, keys) in SHORTCUTS {
                    let mut group = column![
                        text(*name).size(13).font(Font { weight: iced::font::Weight::Semibold, ..crate::style::FONT })
                    ]
                    .spacing(6);
                    for (key, what) in *keys {
                        group = group.push(
                            row![text(*what).size(13).width(Length::Fill), text(*key).size(12).style(muted)]
                                .spacing(12),
                        );
                    }
                    groups = groups.push(group);
                }
                column![
                    title("Keyboard Shortcuts"),
                    scrollable(groups).height(Length::Shrink),
                    row![space().width(Length::Fill), close]
                ]
                .spacing(14)
                .width(420)
                .into()
            }
            Dialog::Preferences => self.prefs_dialog(title("Preferences").into(), close.into()),
            Dialog::Export => self.export_dialog(title("Export").into(), close.into()),
            Dialog::Properties => {
                let mut sections = column![].spacing(16);
                match &self.properties {
                    None => sections = sections.push(text("Reading…").size(13).style(muted)),
                    Some(Err(e)) => sections = sections.push(text(e.as_str()).size(13)),
                    Some(Ok(sheet)) => {
                        for (name, rows) in &sheet.sections {
                            let mut group = column![
                                text(name.as_str())
                                    .size(13)
                                    .font(Font { weight: iced::font::Weight::Semibold, ..crate::style::FONT })
                            ]
                            .spacing(6);
                            for (key, value) in rows {
                                group = group.push(
                                    row![
                                        text(key.as_str()).size(12).style(muted).width(104),
                                        text(value.as_str()).size(13).width(Length::Fill),
                                    ]
                                    .spacing(10),
                                );
                            }
                            sections = sections.push(group);
                        }
                    }
                }
                column![
                    title("Properties"),
                    scrollable(container(sections).padding([0, 12])).height(Length::Shrink),
                    row![space().width(Length::Fill), close]
                ]
                .spacing(14)
                .width(460)
                .into()
            }
            Dialog::About => column![
                center(iced::widget::svg(crate::icons::app()).width(80).height(80)).height(90),
                container(title("Lumen")).center_x(Length::Fill),
                container(text(format!("Version {}", env!("CARGO_PKG_VERSION"))).size(13).style(muted))
                    .center_x(Length::Fill),
                container(
                    text("A fast, safe viewer and editor for pictures and PDFs, in pure Rust with iced.")
                        .size(14)
                        .align_x(Alignment::Center)
                )
                .center_x(Length::Fill),
                container(text("GPL-3.0-or-later · github.com/ImanolGo/lumen-rs").size(12).style(muted))
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
