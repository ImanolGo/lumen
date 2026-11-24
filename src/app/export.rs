//! Exporting: pages of a PDF as PNG images (72, 150 or 300 dpi) or as a new PDF, a
//! picture as a PDF, and "Combine Files…", which opens the page organiser and asks
//! for the PDFs to add.

use std::path::{Path, PathBuf};

use iced::widget::{button, column, row, space, text};
use iced::{Alignment, Element, Font, Length, Task};

use super::pages::PagesMsg;
use super::{App, Doc, Message, blocking, err_string, file_name, muted};
use crate::export;
use crate::i18n::tr;
use crate::pages::{self, Entry};
use crate::{pdf, style};

/// What the chooser's answer will be used for.
pub enum ExportJob {
    /// Pages as PNG files at this resolution.
    Png { pages: Vec<Entry>, docs: Vec<pdf::Document>, dpi: u32, stem: String },
    /// Pages as a new PDF.
    Pdf { pages: Vec<Entry>, docs: Vec<pdf::Document>, unsaved: Option<super::markup::Pending> },
    /// A picture as a PDF.
    Picture { path: PathBuf },
}

#[derive(Debug, Clone)]
pub enum ExportMsg {
    /// Open the Export dialog for the selected pages (or the page in view).
    Open,
    /// Export as PNG at this resolution.
    Png(u32),
    /// Export as a PDF.
    Pdf,
    /// A picture as a PDF.
    Picture,
    /// Where to write it (a file, or a folder for several PNGs); none if cancelled.
    Target(Result<Option<PathBuf>, String>),
    Done(Result<String, String>),
    /// Open the page organiser and ask for the PDFs to add.
    Combine,
}

impl App {
    pub(super) fn export_update(&mut self, msg: ExportMsg) -> Task<Message> {
        match msg {
            ExportMsg::Open => {
                if self.export_selection().is_none() {
                    return Task::none();
                }
                self.menu = None;
                self.dialog = Some(super::menu::Dialog::Export);
                Task::none()
            }
            ExportMsg::Png(dpi) => {
                let Some((pages, docs, stem)) = self.export_selection() else { return Task::none() };
                self.dialog = None;
                let many = pages.len() > 1;
                let suggested =
                    self.export_dir().join(export::numbered_names(&stem, &[pages[0].index], "png").remove(0));
                self.export_job = Some(ExportJob::Png { pages, docs, dpi, stem });
                let purpose =
                    if many { super::chooser::Purpose::ExportFolder } else { super::chooser::Purpose::Export };
                App::ask_for_files(purpose, Some(suggested))
            }
            ExportMsg::Pdf => {
                let Some((pages, docs, stem)) = self.export_selection() else { return Task::none() };
                self.dialog = None;
                let unsaved = self.has_unsaved_changes().then(|| self.pdf_changes());
                let suggested = self.export_dir().join(format!("{stem} (pages).pdf"));
                self.export_job = Some(ExportJob::Pdf { pages, docs, unsaved });
                App::ask_for_files(super::chooser::Purpose::Export, Some(suggested))
            }
            ExportMsg::Picture => {
                let Doc::Image(view) = &self.doc else { return Task::none() };
                self.menu = None;
                let stem = view.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let suggested = view.path.with_file_name(format!("{stem}.pdf"));
                self.export_job = Some(ExportJob::Picture { path: view.path.clone() });
                App::ask_for_files(super::chooser::Purpose::Export, Some(suggested))
            }
            ExportMsg::Target(Err(e)) => {
                self.export_job = None;
                self.toast(e)
            }
            ExportMsg::Target(Ok(None)) => {
                self.export_job = None;
                Task::none()
            }
            ExportMsg::Target(Ok(Some(target))) => {
                let Some(job) = self.export_job.take() else { return Task::none() };
                let work = blocking(move || run(job, &target).map_err(err_string));
                Task::perform(work, |r| Message::Export(ExportMsg::Done(r.and_then(|r| r))))
            }
            ExportMsg::Done(Ok(what)) => {
                self.export_running = self.export_running.saturating_sub(1);
                self.notice(what)
            }
            ExportMsg::Done(Err(e)) => {
                self.export_running = self.export_running.saturating_sub(1);
                self.toast(trf!("Could not export: {}", e))
            }
            ExportMsg::Combine => {
                if !matches!(self.doc, Doc::Pdf(_)) {
                    return Task::none();
                }
                self.menu = None;
                let open = if self.organizer.is_some() { Task::none() } else { self.pages_update(PagesMsg::Open) };
                Task::batch([open, Task::done(Message::Pages(PagesMsg::Insert))])
            }
        }
    }

    /// Exports without asking where to, for the control interface (see `crate::control`).
    pub(super) fn control_export(&mut self, kind: &str, path: PathBuf, dpi: u32) -> Result<Task<Message>, String> {
        let first = match kind {
            "png" if export::DPIS.contains(&dpi) => ExportMsg::Png(dpi),
            "png" => return Err(format!("dpi must be one of {:?}", export::DPIS)),
            "pdf" => ExportMsg::Pdf,
            "picture-pdf" => ExportMsg::Picture,
            other => return Err(format!("unknown export “{other}” (png, pdf, picture-pdf)")),
        };
        // This sets up the job and would ask where to put it; the answer is already known.
        let _asking = self.export_update(first);
        if self.export_job.is_none() {
            return Err("there is nothing to export".into());
        }
        self.export_running += 1;
        Ok(self.export_update(ExportMsg::Target(Ok(Some(path)))))
    }

    /// The pages to export, the documents they come from, and a name for the files:
    /// what is selected in the page organiser, or else the page in view.
    pub(super) fn export_selection(&self) -> Option<(Vec<Entry>, Vec<pdf::Document>, String)> {
        let Doc::Pdf(view) = &self.doc else { return None };
        let stem = view.doc.path().file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match &self.organizer {
            Some(o) => {
                let pages = o.selected_entries();
                (!pages.is_empty()).then(|| (pages, o.sources.clone(), stem))
            }
            None => {
                Some((vec![Entry { source: 0, index: view.current_page, rotate: 0 }], vec![view.doc.clone()], stem))
            }
        }
    }

    /// The folder next to the open file, where exports are suggested.
    fn export_dir(&self) -> PathBuf {
        let path = match &self.doc {
            Doc::Pdf(v) => v.doc.path().to_path_buf(),
            Doc::Image(v) => v.path.clone(),
            _ => PathBuf::new(),
        };
        path.parent().map(Path::to_path_buf).unwrap_or_default()
    }

    pub(super) fn export_dialog<'a>(
        &'a self,
        title: Element<'a, Message>,
        close: Element<'a, Message>,
    ) -> Element<'a, Message> {
        let count = self.export_selection().map_or(0, |(pages, ..)| pages.len());
        let what = if count == 1 { "1 page".to_string() } else { format!("{count} pages") };
        let dpi = |d: u32| {
            button(text(format!("{d} dpi")).size(13).width(Length::Fill).align_x(Alignment::Center))
                .padding([6, 10])
                .width(Length::Fill)
                .style(style::secondary)
                .on_press(Message::Export(ExportMsg::Png(d)))
        };
        let label =
            |t: String| text(t).size(13).font(Font { weight: iced::font::Weight::Semibold, ..crate::style::FONT });
        column![
            title,
            text(what).size(13).style(muted),
            label(tr("As PNG images")),
            row(export::DPIS.iter().map(|&d| dpi(d).into())).spacing(6),
            text(tr("Pages are drawn at that resolution; several pages go into a folder you choose."))
                .size(12)
                .style(muted),
            label(tr("As a PDF")),
            button(text(tr("Save the pages as a new PDF…")).size(13))
                .padding([6, 12])
                .style(style::secondary)
                .on_press(Message::Export(ExportMsg::Pdf)),
            row![space().width(Length::Fill), close],
        ]
        .spacing(10)
        .width(380)
        .into()
    }
}

/// Does the export. Blocking.
fn run(job: ExportJob, target: &Path) -> anyhow::Result<String> {
    match job {
        ExportJob::Png { pages, docs, dpi, stem } => {
            export::need_pages(pages.len())?;
            if let [one] = pages.as_slice() {
                export::page_to_png(&docs[one.source], one.index, one.rotate, dpi, target)?;
                return Ok(trf!("Saved “{}”", file_name(target)));
            }
            let indexes: Vec<usize> = (0..pages.len()).collect();
            let names = export::numbered_names(&stem, &indexes, "png");
            for (entry, name) in pages.iter().zip(&names) {
                export::page_to_png(
                    &docs[entry.source],
                    entry.index,
                    entry.rotate,
                    dpi,
                    &export::free_path(target, name),
                )?;
            }
            Ok(trf!("Saved {} images in “{}”", pages.len(), file_name(target)))
        }
        ExportJob::Pdf { pages, docs, unsaved } => {
            export::need_pages(pages.len())?;
            let base = &docs[0];
            let bytes = match &unsaved {
                Some(pending) => pdf::Document::with_changes(base.path(), &pending.changes())?,
                None => std::fs::read(base.path())?,
            };
            let extras = docs[1..]
                .iter()
                .map(|d| {
                    Ok(pages::Source { bytes: std::fs::read(d.path())?, password: d.password().map(str::to_owned) })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let out = pages::compose(
                &pages::Source { bytes, password: base.password().map(str::to_owned) },
                &extras,
                &pages,
            )?;
            pdf::write_atomic(target, &out)?;
            Ok(trf!("Saved “{}”", file_name(target)))
        }
        ExportJob::Picture { path } => {
            let out = crate::print::picture_to_pdf_native(&path, 150.0)?;
            pdf::write_atomic(target, &out)?;
            Ok(trf!("Saved “{}”", file_name(target)))
        }
    }
}
