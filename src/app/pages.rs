//! Page management for PDFs: rotate, delete, reorder, insert and export pages.
//!
//! Every operation writes a new working copy of the document (`pdf::compose`) and
//! the viewer reloads from it. The working copies are the undo history: undoing
//! simply goes back to the previous file. Nothing touches the original until the
//! user saves.

use crate::i18n::tr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use relm4::{
    ComponentSender,
    gtk::{self, gio, prelude::*},
};

use super::{App, AppMsg, markup::PdfState};
use crate::{
    document::{self, DocumentKind, OpenError, Opened},
    pdf::{
        compose::{self, Source},
        pageops::{self, Entry, PageOp},
    },
};

/// A page operation the user asked for, before it is turned into a plan.
#[derive(Debug, Clone)]
pub enum PageRequest {
    Rotate {
        pages: Vec<usize>,
        degrees: i32,
    },
    Delete(Vec<usize>),
    /// Move pages in front of page `before`.
    Move {
        pages: Vec<usize>,
        before: usize,
    },
    /// Insert the pages of a PDF, or the picture, at `file` in front of page `at`.
    Insert {
        at: usize,
        file: PathBuf,
    },
}

/// A folder for the working copies, removed when it goes out of use.
#[derive(Default)]
pub struct WorkDir {
    dir: Option<PathBuf>,
    counter: u32,
}

impl WorkDir {
    pub fn dir(&mut self) -> PathBuf {
        self.dir
            .get_or_insert_with(|| {
                remove_stale_work_dirs();
                let dir = std::env::temp_dir().join(format!("lumen-work-{}", std::process::id()));
                let _ = std::fs::create_dir_all(&dir);
                dir
            })
            .clone()
    }

    /// A fresh file name, and the number it was made from.
    pub fn next_file(&mut self) -> (PathBuf, u32) {
        self.counter += 1;
        (
            self.dir().join(format!("{}.pdf", self.counter)),
            self.counter,
        )
    }
}

/// Work folders of sessions that ended without cleaning up (a crash or a kill).
fn remove_stale_work_dirs() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid = name
            .to_str()
            .and_then(|n| n.strip_prefix("lumen-work-"))
            .and_then(|p| p.parse::<u32>().ok());
        if pid.is_some_and(|pid| {
            pid != std::process::id() && !Path::new(&format!("/proc/{pid}")).exists()
        }) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        if let Some(dir) = &self.dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// A document to insert: the PDF itself, or a one-page PDF made from a picture.
fn prepare_insert(file: &Path, work_dir: &Path, name: u32) -> Result<(Source, usize)> {
    match document::detect_kind(file)? {
        DocumentKind::Pdf => {
            let doc = lopdf::Document::load(file)
                .with_context(|| format!("cannot read {}", file.display()))?;
            if doc.is_encrypted() {
                anyhow::bail!("{} is password protected", file.display());
            }
            Ok((
                Source {
                    path: file.to_owned(),
                    password: None,
                },
                doc.get_pages().len(),
            ))
        }
        DocumentKind::Image => {
            let converted = work_dir.join(format!("insert-{name}.pdf"));
            compose::image_to_pdf(file, &converted)?;
            Ok((
                Source {
                    path: converted,
                    password: None,
                },
                1,
            ))
        }
    }
}

/// Work out the plan for `request` and write the new document to `out`.
fn run_request(
    base: &Source,
    n_pages: usize,
    request: &PageRequest,
    out: &Path,
    work_dir: &Path,
    name: u32,
) -> Result<Vec<Entry>> {
    let (op, extras) = match request {
        PageRequest::Rotate { pages, degrees } => (
            PageOp::Rotate {
                pages: pages.clone(),
                degrees: *degrees,
            },
            Vec::new(),
        ),
        PageRequest::Delete(pages) => (PageOp::Delete(pages.clone()), Vec::new()),
        PageRequest::Move { pages, before } => (
            PageOp::Move {
                pages: pages.clone(),
                before: *before,
            },
            Vec::new(),
        ),
        PageRequest::Insert { at, file } => {
            let (source, count) = prepare_insert(file, work_dir, name)?;
            (
                PageOp::Insert {
                    at: *at,
                    source: 1,
                    count,
                },
                vec![source],
            )
        }
    };
    let plan = pageops::plan(&op, n_pages)?;
    compose::compose(base, &extras, &plan, out)?;
    Ok(plan)
}

impl App {
    /// Page operations work on PDFs, one at a time.
    pub(super) fn can_pages(&self) -> bool {
        self.pdf.is_some() && !self.page_busy && self.edit_mode == super::edit::EditMode::None
    }

    /// The selected pages, or the page in view if none is selected.
    fn target_pages(&self) -> Vec<usize> {
        let selected = self.page_strip.selected();
        match (&self.pdf, selected.is_empty()) {
            (Some(pdf), true) => vec![pdf.page],
            _ => selected,
        }
    }

    pub(super) fn rotate_pages(&mut self, degrees: i32, sender: &ComponentSender<App>) {
        let pages = self.target_pages();
        self.page_request(PageRequest::Rotate { pages, degrees }, sender);
    }

    pub(super) fn delete_pages(&mut self, sender: &ComponentSender<App>) {
        let pages = self.target_pages();
        self.page_request(PageRequest::Delete(pages), sender);
    }

    /// Insert files after the page in view (or in front of `at`).
    pub(super) fn insert_files(
        &mut self,
        paths: Vec<PathBuf>,
        at: Option<usize>,
        sender: &ComponentSender<App>,
    ) {
        let Some(pdf) = &self.pdf else { return };
        self.insert_at = at.unwrap_or(pdf.page + 1);
        let mut queue = paths;
        queue.reverse();
        self.insert_queue = queue;
        self.next_insert(sender);
    }

    fn next_insert(&mut self, sender: &ComponentSender<App>) {
        if let Some(file) = self.insert_queue.pop() {
            let at = self.insert_at;
            self.page_request(PageRequest::Insert { at, file }, sender);
        }
    }

    pub(super) fn insert_dialog(&self, sender: &ComponentSender<App>) {
        if !self.can_pages() {
            return;
        }
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(&tr("PDF documents and pictures")));
        filter.add_mime_type("application/pdf");
        filter.add_pixbuf_formats();
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title(tr("Insert Pages"))
            .modal(true)
            .filters(&filters)
            .build();
        let sender = sender.clone();
        dialog.open_multiple(
            super::edit::main_window().as_ref(),
            gio::Cancellable::NONE,
            move |result| {
                let Ok(files) = result else { return };
                let paths: Vec<PathBuf> = files
                    .iter::<gio::File>()
                    .flatten()
                    .filter_map(|f| f.path())
                    .collect();
                if !paths.is_empty() {
                    sender.input(AppMsg::InsertFiles { paths, at: None });
                }
            },
        );
    }

    /// Start a page operation in the background.
    pub(super) fn page_request(&mut self, request: PageRequest, sender: &ComponentSender<App>) {
        if !self.can_pages() {
            return;
        }
        let Some(pdf) = &self.pdf else { return };
        let base = Source {
            path: pdf.source.path.clone(),
            password: pdf.source.password.clone(),
        };
        let n_pages = pdf.n_pages;
        let goto = match &request {
            PageRequest::Insert { at, .. } => *at,
            _ => pdf.page,
        };
        let (out, name) = self.work.next_file();
        let work_dir = self.work.dir();
        self.page_busy = true;
        self.loading = true;
        let sender = sender.clone();
        relm4::spawn_blocking(move || {
            let result = run_request(&base, n_pages, &request, &out, &work_dir, name)
                .map_err(|e| format!("{e:#}"));
            sender.input(AppMsg::PageOpDone { out, goto, result });
        });
    }

    pub(super) fn page_op_done(
        &mut self,
        out: PathBuf,
        goto: usize,
        result: Result<Vec<Entry>, String>,
        sender: &ComponentSender<App>,
    ) {
        match result {
            Ok(plan) => {
                let Some(pdf) = &self.pdf else { return };
                let sizes: Vec<(f64, f64)> = pdf.layout.rects.iter().map(|r| r.size()).collect();
                let layers = self.markup.store.borrow().pages.layers.clone();
                let moved = pageops::remap_layers(&layers, &plan, &sizes);
                // The new file and the re-numbered annotations are one undo step.
                self.markup.store.borrow_mut().set_pages(moved.clone());
                self.markup.pdf_history.commit(PdfState {
                    file: out.clone(),
                    layers: moved,
                });
                self.markup.pdf_dirty = true;
                let inserted = plan.iter().filter(|e| e.source != 0).count();
                self.insert_at = goto + inserted;
                self.reload_pdf(out, goto.min(plan.len() - 1), sender);
            }
            Err(message) => {
                self.page_busy = false;
                self.loading = false;
                self.insert_queue.clear();
                super::edit::show_error(&tr("Could Not Change the Pages"), &message);
            }
        }
    }

    /// Show the document at `path` (a working copy) again, keeping the view near `goto`.
    pub(super) fn reload_pdf(&mut self, path: PathBuf, goto: usize, sender: &ComponentSender<App>) {
        self.reload_generation += 1;
        let generation = self.reload_generation;
        self.page_busy = true;
        self.loading = true;
        let sender = sender.clone();
        relm4::spawn_blocking(move || {
            let result = document::open(&path, None);
            sender.input(AppMsg::PdfReloaded {
                generation,
                goto,
                path,
                result,
            });
        });
    }

    pub(super) fn pdf_reloaded(
        &mut self,
        generation: u64,
        goto: usize,
        path: PathBuf,
        result: Result<Opened, OpenError>,
        sender: &ComponentSender<App>,
    ) {
        if generation != self.reload_generation {
            return;
        }
        self.page_busy = false;
        self.loading = false;
        let (Some(old), Ok(opened)) = (&self.pdf, result) else {
            super::edit::show_error(
                &tr("Could Not Reload the Document"),
                &tr("The edited document could not be opened."),
            );
            return;
        };
        let Some(info) = opened.pdf else { return };
        let original = old.original.clone();
        let view = self.make_pdf_view(&path, original, &info, sender);
        self.page_strip.set_document(
            &view
                .layout
                .rects
                .iter()
                .map(|r| r.size())
                .collect::<Vec<_>>(),
        );
        self.outline.set_outline(&info.outline);
        self.properties.set_pdf(
            &view.original,
            &info.metadata,
            info.n_pages,
            info.page_sizes[0],
        );
        self.pdf = Some(view);
        self.sync_canvas();
        self.canvas.scroll_to_page(goto);
        self.clear_selection();
        self.restart_search();
        // Several files were queued for insertion: carry on with the next.
        self.next_insert(sender);
    }

    // --- export --------------------------------------------------------------------

    pub(super) fn export_pages_dialog(&self, sender: &ComponentSender<App>) {
        let Some(pdf) = &self.pdf else { return };
        let pages = self.target_pages();
        let stem = pdf
            .original
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = if pages.len() == 1 {
            format!("{stem}-page-{}.pdf", pages[0] + 1)
        } else {
            format!("{stem}-pages.pdf")
        };
        let sender = sender.clone();
        choose_file(
            &tr("Export Pages"),
            &name,
            &tr("PDF documents"),
            "application/pdf",
            &pdf.original,
            move |dest| {
                sender.input(AppMsg::ExportPagesTo {
                    dest,
                    pages: pages.clone(),
                });
            },
        );
    }

    pub(super) fn export_pages(
        &mut self,
        dest: PathBuf,
        pages: Vec<usize>,
        sender: &ComponentSender<App>,
    ) {
        let Some(pdf) = &self.pdf else { return };
        let base = Source {
            path: pdf.source.path.clone(),
            password: pdf.source.password.clone(),
        };
        let plan: Vec<Entry> = pages
            .iter()
            .map(|&index| Entry {
                source: 0,
                index,
                rotate: 0,
            })
            .collect();
        let sender = sender.clone();
        relm4::spawn_blocking(move || {
            let result = compose::compose(&base, &[], &plan, &dest).map_err(|e| format!("{e:#}"));
            sender.input(AppMsg::Exported(result));
        });
    }

    pub(super) fn export_png_dialog(&self, sender: &ComponentSender<App>) {
        let Some(pdf) = &self.pdf else { return };
        let stem = pdf
            .original
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = format!("{stem}-page-{}.png", pdf.page + 1);
        let sender = sender.clone();
        choose_file(
            &tr("Export Page as PNG"),
            &name,
            &tr("PNG images"),
            "image/png",
            &pdf.original,
            move |dest| {
                sender.input(AppMsg::ExportPngTo { dest });
            },
        );
    }

    /// Render the page in view at 300 dpi, with its annotations, into a PNG.
    pub(super) fn export_png(&mut self, dest: PathBuf, sender: &ComponentSender<App>) {
        let Some(pdf) = &self.pdf else { return };
        let (source, page) = (pdf.source.clone(), pdf.page);
        let layer = self.markup.store.borrow().layer(page).cloned();
        let sender = sender.clone();
        relm4::spawn_blocking(move || {
            let result = crate::pdf::export_png(&source, page, 300.0 / 72.0, layer.as_ref(), &dest)
                .map_err(|e| format!("{e:#}"));
            sender.input(AppMsg::Exported(result));
        });
    }

    pub(super) fn export_image_pdf_dialog(&self, sender: &ComponentSender<App>) {
        let Some(session) = &self.edit else { return };
        let stem = session
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let sender = sender.clone();
        choose_file(
            &tr("Export as PDF"),
            &format!("{stem}.pdf"),
            &tr("PDF documents"),
            "application/pdf",
            &session.path,
            move |dest| {
                sender.input(AppMsg::ExportImagePdfTo { dest });
            },
        );
    }

    /// Make a one-page PDF of the image, with the edits and annotations as they are on screen.
    pub(super) fn export_image_pdf(&mut self, dest: PathBuf, sender: &ComponentSender<App>) {
        let Some(session) = &self.edit else { return };
        let (path, base, stack) = (session.path.clone(), session.base, session.stack.clone());
        let layer = self.markup.store.borrow().pages.layers.get(&0).cloned();
        let temp_dir = self.work.dir();
        let sender = sender.clone();
        relm4::spawn_blocking(move || {
            let result = (|| -> Result<()> {
                if !stack.is_dirty() && layer.as_ref().is_none_or(|l| l.is_empty()) {
                    return compose::image_to_pdf(&path, &dest);
                }
                // Render what is on screen into a temporary PNG, which becomes the page.
                let stored = image::open(&path)
                    .with_context(|| format!("cannot read {}", path.display()))?;
                let mut shown = crate::edit::render(&stored, base, stack.active());
                if let Some(layer) = layer {
                    shown = crate::annotate::render::flatten(&shown, &layer)
                        .context("cannot draw the annotations")?;
                }
                let temp = temp_dir.join("export-image.png");
                shown.save(&temp).context("cannot prepare the image")?;
                let made = compose::image_to_pdf(&temp, &dest);
                let _ = std::fs::remove_file(&temp);
                made
            })()
            .map_err(|e| format!("{e:#}"));
            sender.input(AppMsg::Exported(result));
        });
    }

    pub(super) fn exported(&self, result: Result<(), String>) {
        if let Err(message) = result {
            super::edit::show_error(&tr("Could Not Export"), &message);
        }
    }
}

/// A save dialog for one kind of file, suggesting `name` next to `near`.
fn choose_file(
    title: &str,
    name: &str,
    filter_name: &str,
    mime: &str,
    near: &Path,
    then: impl Fn(PathBuf) + 'static,
) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some(filter_name));
    filter.add_mime_type(mime);
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let dialog = gtk::FileDialog::builder()
        .title(title)
        .modal(true)
        .filters(&filters)
        .initial_name(name)
        .build();
    if let Some(folder) = near.parent() {
        dialog.set_initial_folder(Some(&gio::File::for_path(folder)));
    }
    dialog.save(
        super::edit::main_window().as_ref(),
        gio::Cancellable::NONE,
        move |result| {
            if let Some(dest) = result.ok().and_then(|file| file.path()) {
                then(dest);
            }
        },
    );
}
