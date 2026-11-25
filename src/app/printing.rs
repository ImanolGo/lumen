//! Lumen's own print dialog, for desktops without a print portal: pick a printer,
//! copies, paper and two-sided printing, and the job goes straight to CUPS.

use iced::widget::{column, row, text};
use iced::{Alignment, Element, Task};

use super::{App, Message, TOOLBAR_H, blocking, err_string, muted};
use crate::cups::{self, Job, Printer};
use crate::i18n::tr;
use crate::style;
use crate::ui;

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
    job: crate::print::Job,
    printers: Option<Result<Vec<Printer>, String>>,
    printer: Option<Printer>,
    copies: String,
    media: Option<Media>,
    two_sided: bool,
    /// Pages, as "1-3, 5"; empty for all.
    pages: String,
    turned: Turned,
    sending: bool,
}

/// Which way round the paper is; automatic leaves it to the printer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turned {
    Auto,
    Portrait,
    Landscape,
}

#[derive(Debug, Clone)]
pub enum PrintMsg {
    Pages(String),
    Turned(Turned),
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
    /// No print portal: show Lumen's own dialog for `job` and look for printers.
    pub(super) fn open_print_dialog(&mut self, job: crate::print::Job) -> Task<Message> {
        self.print_dialog = Some(PrintDialog {
            job,
            printers: None,
            printer: None,
            copies: "1".into(),
            media: None,
            two_sided: false,
            pages: String::new(),
            turned: Turned::Auto,
            sending: false,
        });
        Task::perform(blocking(|| cups::printers().map_err(err_string)), |r| {
            Message::Printing(PrintMsg::Loaded(r.and_then(|r| r)))
        })
    }

    pub(super) fn printing_update(&mut self, msg: PrintMsg) -> Task<Message> {
        // The job may outlive the dialog (Escape closes it): its end is told all the same.
        if let PrintMsg::Done(result) = msg {
            if self.print_dialog.as_ref().is_some_and(|d| d.sending) {
                self.print_dialog = None;
            }
            return match result {
                Ok(()) => self.notice(tr("Sent to the printer")),
                Err(e) => self.toast(trf!("Could not print: {}", e)),
            };
        }
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
            PrintMsg::Pages(t) => dialog.pages = t,
            PrintMsg::Turned(way) => dialog.turned = way,
            PrintMsg::Cancel => self.print_dialog = None,
            PrintMsg::Print => {
                let Some(printer) = dialog.printer.clone() else { return Task::none() };
                let Some(pages) = cups::page_ranges(&dialog.pages) else { return Task::none() };
                if dialog.sending {
                    return Task::none();
                }
                dialog.sending = true;
                let job = Job {
                    copies: dialog.copies.parse().unwrap_or(1).max(1),
                    media: dialog.media.as_ref().map(|m| m.keyword.clone()),
                    two_sided: dialog.two_sided,
                    pages,
                    landscape: match dialog.turned {
                        Turned::Auto => None,
                        way => Some(way == Turned::Landscape),
                    },
                };
                let what = dialog.job.clone();
                return Task::perform(
                    blocking(move || crate::print::print_direct(&what.path, &printer.name, &job).map_err(err_string)),
                    |r| Message::Printing(PrintMsg::Done(r.and_then(|r| r))),
                );
            }
            PrintMsg::Done(_) => {}
        }
        Task::none()
    }

    pub(super) fn print_dialog_view(&self) -> Option<Element<'_, Message>> {
        let dialog = self.print_dialog.as_ref()?;
        let p = |m: PrintMsg| Message::Printing(m);
        let title = trf!("Print “{}”", super::file_name(&dialog.job.path));
        let label = |t: String| text(t).size(style::BODY).width(110);
        let body: Element<'_, Message> = match &dialog.printers {
            None => text(tr("Looking for printers…")).size(style::BODY).style(muted).into(),
            Some(Err(e)) => text(trf!("Printing is not available: {}", e)).size(style::BODY).into(),
            Some(Ok(printers)) if printers.is_empty() => {
                text(tr("No printers are set up. Add one in your system's printer settings.")).size(style::BODY).into()
            }
            Some(Ok(printers)) => {
                let printer = dialog.printer.as_ref();
                let media = printer.map(media_of).unwrap_or_default();
                let mut form = column![
                    row![
                        label(tr("Printer")),
                        ui::select(printers.clone(), dialog.printer.clone(), move |x| p(PrintMsg::Printer(x)))
                    ]
                    .align_y(Alignment::Center),
                    row![
                        label(tr("Copies")),
                        ui::input("1", &dialog.copies)
                            .on_input(move |t| p(PrintMsg::Copies(t)))
                            .on_submit(p(PrintMsg::Print))
                            .width(70)
                    ]
                    .align_y(Alignment::Center),
                ]
                .spacing(12);
                let is_pdf = crate::library::Kind::of(&dialog.job.path) == Some(crate::library::Kind::Pdf);
                if is_pdf {
                    let wrong = cups::page_ranges(&dialog.pages).is_none();
                    form = form.push(
                        row![
                            label(tr("Pages")),
                            ui::input(&tr("All (or 1-3, 5)"), &dialog.pages)
                                .on_input(move |t| p(PrintMsg::Pages(t)))
                                .on_submit(p(PrintMsg::Print))
                        ]
                        .align_y(Alignment::Center),
                    );
                    if wrong {
                        let say = tr("Write pages as 1-3, 5");
                        form = form.push(text(say).size(style::SMALL).style(super::problem));
                    }
                }
                let way = |name: String, way: Turned| (name, dialog.turned == way, p(PrintMsg::Turned(way)));
                let ways = vec![
                    way(tr("Automatic"), Turned::Auto),
                    way(tr("Portrait"), Turned::Portrait),
                    way(tr("Landscape"), Turned::Landscape),
                ];
                form = form.push(row![label(tr("Orientation")), ui::choices(ways)].align_y(Alignment::Center));
                if !media.is_empty() {
                    form = form.push(
                        row![
                            label(tr("Paper")),
                            ui::select(media, dialog.media.clone(), move |m| p(PrintMsg::Media(m)))
                        ]
                        .align_y(Alignment::Center),
                    );
                }
                if printer.is_some_and(|p| p.two_sided) {
                    form = form.push(
                        row![
                            label(String::new()),
                            ui::check(dialog.two_sided)
                                .label(tr("Print on both sides"))
                                .on_toggle(move |on| p(PrintMsg::TwoSided(on)))
                        ]
                        .align_y(Alignment::Center),
                    );
                }
                form.into()
            }
        };
        let ready = dialog.printer.is_some() && !dialog.sending && cups::page_ranges(&dialog.pages).is_some();
        let go = if dialog.sending { tr("Sending…") } else { tr("Print") };
        Some(
            ui::dialog(title, ui::FORM, body)
                .cancel(tr("Cancel"), p(PrintMsg::Cancel))
                .button(ui::Kind::Main, go, ready.then_some(p(PrintMsg::Print)))
                .view(TOOLBAR_H),
        )
    }
}
