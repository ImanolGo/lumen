//! Lumen's own print dialog, for desktops without a print portal: pick a printer,
//! copies, paper and two-sided printing, and the job goes straight to CUPS.

use std::path::PathBuf;

use iced::widget::{
    button, center, checkbox, column, container, mouse_area, opaque, pick_list, row, space, stack, text, text_input,
};
use iced::{Alignment, Element, Font, Length, Task};

use super::{App, Message, blocking, err_string, muted};
use crate::cups::{self, Job, Printer};
use crate::i18n::tr;
use crate::style;

/// A paper size a printer offers.
#[derive(Debug, Clone, PartialEq)]
pub struct Media {
    keyword: String,
    label: String,
}

impl std::fmt::Display for Media {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

pub struct PrintDialog {
    /// What is printed (a copy with unsaved changes, possibly).
    path: PathBuf,
    printers: Option<Result<Vec<Printer>, String>>,
    printer: Option<Printer>,
    copies: String,
    media: Option<Media>,
    two_sided: bool,
    sending: bool,
}

#[derive(Debug, Clone)]
pub enum PrintMsg {
    Loaded(Result<Vec<Printer>, String>),
    Printer(Printer),
    Copies(String),
    Media(Media),
    TwoSided(bool),
    Print,
    Cancel,
    Done(Result<(), String>),
}

/// The paper sizes of `printer`, each once, the common ones first.
fn media_of(printer: &Printer) -> Vec<Media> {
    let mut out: Vec<Media> = Vec::new();
    for keyword in &printer.media {
        let label = cups::media_label(keyword);
        // Keep the ones with a name people know, and each name once (no borderless twins).
        if label != *keyword && !out.iter().any(|m| m.label == label) {
            out.push(Media { keyword: keyword.clone(), label });
        }
    }
    out
}

fn default_media(printer: &Printer) -> Option<Media> {
    let all = media_of(printer);
    let wanted = printer.media_default.as_deref().map(cups::media_label);
    all.iter().find(|m| Some(&m.label) == wanted.as_ref()).or(all.first()).cloned()
}

impl App {
    /// No print portal: show Lumen's own dialog for `path` and look for printers.
    pub(super) fn open_print_dialog(&mut self, path: PathBuf) -> Task<Message> {
        self.print_dialog = Some(PrintDialog {
            path,
            printers: None,
            printer: None,
            copies: "1".into(),
            media: None,
            two_sided: false,
            sending: false,
        });
        Task::perform(blocking(|| cups::printers().map_err(err_string)), |r| {
            Message::Printing(PrintMsg::Loaded(r.and_then(|r| r)))
        })
    }

    pub(super) fn printing_update(&mut self, msg: PrintMsg) -> Task<Message> {
        let Some(dialog) = &mut self.print_dialog else { return Task::none() };
        match msg {
            PrintMsg::Loaded(result) => {
                if let Ok(printers) = &result {
                    dialog.printer = printers.first().cloned();
                    dialog.media = dialog.printer.as_ref().and_then(default_media);
                }
                dialog.printers = Some(result);
            }
            PrintMsg::Printer(printer) => {
                dialog.media = default_media(&printer);
                dialog.two_sided &= printer.two_sided;
                dialog.printer = Some(printer);
            }
            PrintMsg::Copies(t) => dialog.copies = t.chars().filter(char::is_ascii_digit).take(3).collect(),
            PrintMsg::Media(m) => dialog.media = Some(m),
            PrintMsg::TwoSided(on) => dialog.two_sided = on,
            PrintMsg::Cancel => self.print_dialog = None,
            PrintMsg::Print => {
                let Some(printer) = dialog.printer.clone() else { return Task::none() };
                dialog.sending = true;
                let job = Job {
                    copies: dialog.copies.parse().unwrap_or(1).max(1),
                    media: dialog.media.as_ref().map(|m| m.keyword.clone()),
                    two_sided: dialog.two_sided,
                };
                let path = dialog.path.clone();
                return Task::perform(
                    blocking(move || crate::print::print_direct(&path, &printer.name, &job).map_err(err_string)),
                    |r| Message::Printing(PrintMsg::Done(r.and_then(|r| r))),
                );
            }
            PrintMsg::Done(result) => {
                self.print_dialog = None;
                return match result {
                    Ok(()) => self.notice(tr("Sent to the printer")),
                    Err(e) => self.toast(trf!("Could not print: {}", e)),
                };
            }
        }
        Task::none()
    }

    pub(super) fn print_dialog_view(&self) -> Option<Element<'_, Message>> {
        let dialog = self.print_dialog.as_ref()?;
        let p = |m: PrintMsg| Message::Printing(m);
        let title = text(trf!("Print “{}”", super::file_name(&dialog.path)))
            .size(18)
            .font(Font { weight: iced::font::Weight::Semibold, ..crate::style::FONT });
        let label = |t: String| text(t).size(13).width(110);
        let body: Element<'_, Message> = match &dialog.printers {
            None => text(tr("Looking for printers…")).size(14).style(muted).into(),
            Some(Err(e)) => text(trf!("Printing is not available: {}", e)).size(14).into(),
            Some(Ok(printers)) if printers.is_empty() => {
                text(tr("No printers are set up. Add one in your system's printer settings.")).size(14).into()
            }
            Some(Ok(printers)) => {
                let printer = dialog.printer.as_ref();
                let media = printer.map(media_of).unwrap_or_default();
                let mut form = column![
                    row![
                        label(tr("Printer")),
                        pick_list(printers.clone(), dialog.printer.clone(), move |x| p(PrintMsg::Printer(x)))
                            .width(Length::Fill)
                            .text_size(14)
                    ]
                    .align_y(Alignment::Center),
                    row![
                        label(tr("Copies")),
                        text_input("1", &dialog.copies)
                            .on_input(move |t| p(PrintMsg::Copies(t)))
                            .on_submit(p(PrintMsg::Print))
                            .width(70)
                            .size(14)
                    ]
                    .align_y(Alignment::Center),
                ]
                .spacing(12);
                if !media.is_empty() {
                    form = form.push(
                        row![
                            label(tr("Paper")),
                            pick_list(media, dialog.media.clone(), move |m| p(PrintMsg::Media(m)))
                                .width(Length::Fill)
                                .text_size(14)
                        ]
                        .align_y(Alignment::Center),
                    );
                }
                if printer.is_some_and(|p| p.two_sided) {
                    form = form.push(
                        row![
                            label(String::new()),
                            checkbox(dialog.two_sided)
                                .label(tr("Print on both sides"))
                                .on_toggle(move |on| p(PrintMsg::TwoSided(on)))
                                .text_size(14)
                        ]
                        .align_y(Alignment::Center),
                    );
                }
                form.into()
            }
        };
        let ready = dialog.printer.is_some() && !dialog.sending;
        let card = container(
            column![
                title,
                text(tr("Printing directly, without the desktop's print dialog.")).size(12).style(muted),
                space().height(4),
                body,
                space().height(4),
                row![
                    space().width(Length::Fill),
                    button(text(tr("Cancel")).size(14))
                        .padding([8, 16])
                        .style(style::secondary)
                        .on_press(p(PrintMsg::Cancel)),
                    button(text(if dialog.sending { tr("Sending…") } else { tr("Print") }).size(14))
                        .padding([8, 16])
                        .style(style::primary)
                        .on_press_maybe(ready.then_some(p(PrintMsg::Print))),
                ]
                .spacing(10),
            ]
            .spacing(8)
            .width(440),
        )
        .padding(22)
        .style(style::panel);
        let backdrop = mouse_area(space().width(Length::Fill).height(Length::Fill)).on_press(p(PrintMsg::Cancel));
        Some(stack![container(backdrop).style(style::scrim), center(opaque(card))].into())
    }
}
