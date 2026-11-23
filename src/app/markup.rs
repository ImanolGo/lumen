//! Markup in the main window: the markup bar, how pointer events become
//! annotations, undo and redo, and saving.
//!
//! Each file being marked up has a [`DocMarkup`]. Flipping to another file
//! keeps unsaved markup aside (in memory) until you come back, and quitting
//! asks first while any is left.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use iced::widget::{Id, button, canvas, center, column, container, operation, row, space, text, text_editor};
use iced::{Alignment, Element, Font, Length, Task};

use super::overlay::{Capture, Overlay};
use super::{App, Doc, Message, blocking, err_string, file_name, icon_button, muted, placed, separator};
use crate::annotate::model::Style;
use crate::annotate::pdf::ObjRef;
use crate::annotate::{AnnotationLayer, Color, GeometryChange, History, Outcome, Point, Rect, Shape, Store, Tool};
use crate::{icons, pdf, style};

pub type Layers = BTreeMap<usize, AnnotationLayer>;

/// The colours offered for markup, from Lumen's icon and the GNOME palette.
pub const SWATCHES: [(&str, Color); 8] = [
    ("Amber", Color::AMBER),
    ("Yellow", Color::YELLOW),
    ("Coral", Color::rgba(0.906, 0.435, 0.325, 1.0)),
    ("Red", Color::rgba(0.878, 0.106, 0.141, 1.0)),
    ("Violet", Color::rgba(0.569, 0.255, 0.675, 1.0)),
    ("Blue", Color::rgba(0.208, 0.518, 0.894, 1.0)),
    ("Green", Color::rgba(0.180, 0.761, 0.494, 1.0)),
    ("Black", Color::rgba(0.141, 0.122, 0.192, 1.0)),
];
/// Pointer reach for picking annotations, in screen pixels.
const REACH: f32 = 6.0;

fn text_box_id() -> Id {
    Id::new("markup-text")
}

/// The markup of one file.
pub struct DocMarkup {
    pub store: Store,
    pub history: History<Layers>,
    /// What the file on disk holds, to tell what is unsaved.
    pub saved: Layers,
    /// PDF objects the layers replace, per page.
    pub refs: Vec<Vec<ObjRef>>,
    /// How big a document unit is: points for PDFs; for pictures, so that lines and
    /// text look the same size on a phone snapshot and on a 50-megapixel photo.
    pub unit: f64,
}

impl DocMarkup {
    fn new(layers: Layers, refs: Vec<Vec<ObjRef>>, unit: f64) -> Self {
        let mut store = Store::default();
        store.set_pages(layers.clone());
        Self { store, history: History::new(layers.clone()), saved: layers, refs, unit }
    }

    fn for_picture(width: u32, height: u32) -> Self {
        Self::new(Layers::new(), Vec::new(), (width.max(height) as f64 / 1000.0).max(1.0))
    }

    /// Pages whose annotations differ from the file's.
    pub fn dirty_pages(&self) -> Vec<usize> {
        let empty = AnnotationLayer::default();
        let now = &self.store.pages.layers;
        let mut pages: Vec<usize> = now.keys().chain(self.saved.keys()).copied().collect();
        pages.sort_unstable();
        pages.dedup();
        pages.retain(|p| !now.get(p).unwrap_or(&empty).same_as(self.saved.get(p).unwrap_or(&empty)));
        pages
    }

    pub fn is_dirty(&self) -> bool {
        !self.dirty_pages().is_empty()
    }

    pub fn has_items(&self) -> bool {
        !self.store.pages.is_empty()
    }

    /// The picture underneath turned: carry every state along.
    pub fn apply(&mut self, change: GeometryChange) {
        let turn = |layers: &mut Layers| layers.values_mut().for_each(|l| l.apply(change));
        self.history.map_all(turn);
        turn(&mut self.store.pages.layers);
        turn(&mut self.saved);
        self.store.draft = None;
    }
}

/// An owned copy of what to write into a PDF, to hand to a worker thread.
pub(super) struct Pending {
    layers: Layers,
    refs: Vec<Vec<ObjRef>>,
    pages: Vec<usize>,
    form: std::sync::Arc<crate::forms::Form>,
    values: Vec<crate::forms::Value>,
    password: Option<String>,
}

impl Pending {
    pub(super) fn changes(&self) -> pdf::Changes<'_> {
        pdf::Changes {
            layers: &self.layers,
            refs: &self.refs,
            pages: &self.pages,
            form: &self.form,
            values: &self.values,
            password: self.password.as_deref(),
        }
    }
}

/// The text box being typed into.
pub struct TextEdit {
    pub page: usize,
    pub id: u64,
    pub content: text_editor::Content,
}

#[derive(Debug, Clone)]
pub enum MarkupMsg {
    Toggle,
    Tool(Tool),
    Swatch(usize),
    /// The colour and size menu, and what is set in it (live while dragging, recorded on commit).
    StyleMenu,
    CustomColor,
    Pick {
        h: f32,
        s: f32,
        v: f32,
    },
    Hex(String),
    Opacity(f32),
    LineWidth(f32),
    TextSize(f32),
    TextSizeTyped(String),
    StyleCommit,
    Press {
        page: usize,
        x: f64,
        y: f64,
        clicks: u32,
    },
    Drag {
        x: f64,
        y: f64,
    },
    Release {
        x: f64,
        y: f64,
    },
    Undo,
    Redo,
    Delete,
    Nudge(f64, f64),
    Save,
    SaveAs,
    SaveTo(Result<Option<PathBuf>, String>),
    Prepared(u64, PathBuf, Result<pdf::Opened, pdf::OpenError>),
    PdfSaved(u64, PathBuf, Layers, Result<pdf::Opened, String>),
    PictureSaved(u64, PathBuf, Layers, Result<(), String>),
    TextAction(text_editor::Action),
    QuitAnyway,
    StayOpen,
}

fn pt(p: hayro::kurbo::Rect) -> Rect {
    Rect::new(p.x0, p.y0, p.width(), p.height())
}

/// Somewhere for files that are printed with their unsaved markup.
fn print_copy(name: &str) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("lumen-rs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(name))
}

impl App {
    // ------------------------------------------------------------ queries

    /// Screen pixels per document unit for the shown file.
    fn markup_zoom(&self) -> f32 {
        match &self.doc {
            Doc::Pdf(v) => v.zoom,
            Doc::Image(v) => v.zoom.unwrap_or_else(|| self.image_fit_zoom(v)),
            _ => 1.0,
        }
    }

    /// Files with markup that is not saved yet.
    pub(super) fn unsaved_files(&self) -> usize {
        let elsewhere: std::collections::HashSet<&PathBuf> = self
            .stash
            .iter()
            .filter(|(_, m)| m.is_dirty())
            .map(|(p, _)| p)
            .chain(self.stash_forms.keys())
            .chain(self.stash_edits.iter().filter(|(_, e)| e.is_dirty()).map(|(p, _)| p))
            .collect();
        usize::from(self.has_unsaved_changes()) + elsewhere.len()
    }

    /// Markup or form values on screen that the file does not have yet.
    pub(super) fn has_unsaved_changes(&self) -> bool {
        self.marks.as_ref().is_some_and(DocMarkup::is_dirty)
            || self.form.as_ref().is_some_and(|f| f.is_dirty())
            || matches!(&self.doc, Doc::Image(v) if v.edit.is_dirty())
    }

    fn capture(&self, is_pdf: bool) -> Capture {
        if !self.markup_open || self.markup_busy.is_some() {
            return Capture::None;
        }
        match self.editor.tool {
            Tool::Select => Capture::Annotations,
            Tool::Highlight if is_pdf => Capture::OffText,
            _ => Capture::All,
        }
    }

    // ------------------------------------------------------------ attaching

    /// Gets markup ready for the file on screen, if markup is on or the file has some waiting.
    pub(super) fn attach_markup(&mut self) -> Task<Message> {
        if self.marks.is_some() {
            return Task::none();
        }
        match &self.doc {
            Doc::Image(view) => {
                let waiting = self.stash.remove(&view.path);
                if waiting.is_some() || self.markup_open {
                    let (w, h) = (view.shown.width, view.shown.height);
                    self.marks = Some(waiting.unwrap_or_else(|| DocMarkup::for_picture(w, h)));
                    self.apply_unit();
                }
                Task::none()
            }
            Doc::Pdf(view) if self.markup_open && self.markup_busy.is_none() => {
                // Reopen with the editable annotations hidden from the renderer, which Lumen draws instead.
                let path = view.doc.path().to_owned();
                self.markup_busy = Some("Preparing…");
                let generation = self.generation;
                let password = view.doc.password().map(str::to_owned);
                Task::perform(
                    {
                        let path = path.clone();
                        blocking(move || pdf::Document::open_full(&path, true, password.as_deref()))
                    },
                    move |r| {
                        let r = r.map_err(pdf::OpenError::Failed).and_then(|r| r.map_err(pdf::OpenError::from));
                        Message::Markup(MarkupMsg::Prepared(generation, path.clone(), r))
                    },
                )
            }
            _ => Task::none(),
        }
    }

    /// Whether `path`, about to be shown, should open ready for markup.
    pub(super) fn opens_for_markup(&self, path: &Path) -> bool {
        self.markup_open || self.stash.contains_key(path)
    }

    /// Puts the markup of the file being left aside, unless there is nothing unsaved.
    pub(super) fn detach_markup(&mut self) {
        let _ = self.finish_text();
        let Some(mut marks) = self.marks.take() else { return };
        self.editor.cancel(&mut marks.store);
        marks.store.selection = None;
        self.markup_busy = None;
        if !marks.is_dirty() {
            return;
        }
        let path = match &self.doc {
            // The picture's edits are put aside with it, so the markup stays measured on the edited picture.
            Doc::Image(view) => view.path.clone(),
            Doc::Pdf(view) => view.doc.path().to_owned(),
            _ => return,
        };
        self.stash.insert(path, marks);
    }

    /// A PDF was opened (or reopened for markup, or after saving): show it, with its form and markup.
    pub(super) fn install_opened(&mut self, path: PathBuf, opened: pdf::Opened) -> Task<Message> {
        let same = matches!(&self.doc, Doc::Pdf(v) if v.doc.path() == path);
        let pages = opened.doc.page_count();
        let shown = if same {
            self.swap_pdf(opened.doc)
        } else if matches!(&self.doc, Doc::Loading(p) if *p == path) {
            self.open_pdf_view(path.clone(), opened.doc)
        } else {
            return Task::none();
        };
        self.attach_form(&path, opened.form, pages);
        if let Some(imported) = opened.imported {
            let refs = imported.refs;
            match &mut self.marks {
                Some(marks) => marks.refs = refs,
                None => {
                    self.marks = Some(match self.stash.remove(&path) {
                        Some(mut waiting) => {
                            waiting.refs = refs;
                            waiting
                        }
                        None => DocMarkup::new(imported.layers, refs, 1.0),
                    });
                    self.apply_unit();
                }
            }
        }
        let signed = match self.pending_sign.take() {
            Some((signature, page, target)) => self.place_signature(signature, page, target),
            None => Task::none(),
        };
        Task::batch([shown, signed])
    }

    /// Replaces the open PDF's renderer, keeping the view where it is; pages re-render.
    fn swap_pdf(&mut self, doc: pdf::Document) -> Task<Message> {
        // Results still on their way belong to the old renderer.
        self.generation += 1;
        let Doc::Pdf(view) = &mut self.doc else { return Task::none() };
        view.doc = doc;
        for slot in &mut view.slots {
            if let Some((_, scale)) = &mut slot.shown {
                *scale = -1.0;
            }
            slot.pending = None;
            slot.text_pending = false;
        }
        self.request_pages()
    }

    fn apply_unit(&mut self) {
        let unit = self.marks.as_ref().map_or(1.0, |m| m.unit);
        self.editor.style.width = self.line_width * unit;
        self.editor.style.font_size = self.text_size * unit;
    }

    // ------------------------------------------------------------ editing

    fn apply(&mut self, outcome: Outcome) -> Task<Message> {
        let Some(marks) = &mut self.marks else { return Task::none() };
        if outcome.committed {
            marks.history.commit(marks.store.pages.layers.clone());
        }
        match outcome.edit_text {
            Some((page, id)) => {
                let text = match marks.store.layer(page).and_then(|l| l.get(id)).map(|a| &a.shape) {
                    Some(Shape::Text { text, .. }) => text.clone(),
                    _ => return Task::none(),
                };
                self.editing = Some(TextEdit { page, id, content: text_editor::Content::with_text(&text) });
                operation::focus(text_box_id())
            }
            None => Task::none(),
        }
    }

    /// Ends typing into a text box, keeping what was typed (an empty box goes away).
    pub(super) fn finish_text(&mut self) -> Task<Message> {
        let Some(edit) = self.editing.take() else { return Task::none() };
        let Some(marks) = &mut self.marks else { return Task::none() };
        let mut text = edit.content.text();
        while text.ends_with('\n') {
            text.pop();
        }
        let outcome = self.editor.set_text(&mut marks.store, edit.page, edit.id, &text);
        self.apply(outcome)
    }

    /// Turns the selected PDF text into highlights (one per page) and clears the selection.
    pub(super) fn highlight_selection(&mut self) -> Task<Message> {
        if self.marks.is_none() || !matches!(self.doc, Doc::Pdf(_)) {
            return Task::none();
        }
        let (Doc::Pdf(view), Some(sel), Some(marks)) = (&self.doc, self.selection.take(), &mut self.marks) else {
            return Task::none();
        };
        let (a, b) = sel.ordered();
        let mut committed = false;
        for page in a.0..=b.0 {
            let Some(text) = view.slots.get(page).and_then(|s| s.text.as_ref()) else { continue };
            let Some(range) = sel.on_page(page, text.len()) else { continue };
            let rects: Vec<Rect> = text.boxes(range).into_iter().map(pt).collect();
            committed |= self.editor.add_highlight(&mut marks.store, page, rects).committed;
        }
        if committed {
            marks.history.commit(marks.store.pages.layers.clone());
        }
        Task::none()
    }

    /// Changes the selected annotation's style now; the undo step is recorded by
    /// [`App::style_commit`], so a whole slider drag is one step.
    fn restyle_live(&mut self, change: impl FnOnce(&mut Shape, &mut Style)) {
        let Some(marks) = &mut self.marks else { return };
        let Some(sel) = marks.store.selection else { return };
        if self.style_from.is_none() {
            self.style_from = Some(marks.store.pages.layers.clone());
        }
        let Some(item) = marks.store.pages.layers.get_mut(&sel.page).and_then(|l| l.get_mut(sel.id)) else { return };
        change(&mut item.shape, &mut item.style);
    }

    fn style_commit(&mut self) {
        let (Some(from), Some(marks)) = (self.style_from.take(), &mut self.marks) else { return };
        if marks.store.pages.layers != from {
            marks.history.commit(marks.store.pages.layers.clone());
        }
    }

    /// Sets the colour of the tool, or of the selected annotation, keeping or setting its opacity.
    fn set_color(&mut self, color: Color) {
        let highlight = matches!(self.selected_shape(), Some(Shape::Highlight(_)))
            || (self.selected_shape().is_none() && self.editor.tool == Tool::Highlight);
        if highlight {
            self.editor.highlight_color = color;
        } else {
            self.editor.style.stroke = color;
        }
        self.hex_input = super::palette::to_hex(color);
        self.restyle_live(|shape, style| match shape {
            Shape::Highlight(_) => {
                style.stroke = color;
                style.fill = Some(color);
            }
            _ => style.stroke = color,
        });
    }

    fn set_text_size(&mut self, size: f64) {
        let unit = self.marks.as_ref().map_or(1.0, |m| m.unit);
        self.text_size = size.clamp(4.0, 400.0);
        self.apply_unit();
        let size = self.text_size * unit;
        self.restyle_live(|shape, style| {
            if let Shape::Text { bounds, text } = shape {
                style.font_size = size;
                let lines = text.split('\n').count().max(1) as f64;
                let widest = text.split('\n').map(|l| l.chars().count()).max().unwrap_or(1) as f64;
                bounds.w = bounds.w.max(widest * size * 0.6);
                bounds.h = lines * size * 1.4;
            }
        });
    }

    fn selected_shape(&self) -> Option<&Shape> {
        self.marks.as_ref()?.store.selected().map(|a| &a.shape)
    }

    pub(super) fn markup_update(&mut self, msg: MarkupMsg) -> Task<Message> {
        match msg {
            MarkupMsg::Toggle => {
                let done = self.finish_text();
                self.markup_open = !self.markup_open;
                if self.markup_open {
                    // One bar at a time.
                    self.edit_open = false;
                    self.crop = None;
                    self.adjust_open = false;
                }
                if !self.markup_open {
                    if let Some(marks) = &mut self.marks {
                        self.editor.cancel(&mut marks.store);
                        marks.store.selection = None;
                    }
                    return done;
                }
                Task::batch([done, self.attach_markup()])
            }
            MarkupMsg::Tool(tool) => {
                let done = self.finish_text();
                self.editor.tool = tool;
                if let Some(marks) = &mut self.marks {
                    self.editor.cancel(&mut marks.store);
                    if tool != Tool::Select {
                        marks.store.selection = None;
                    }
                }
                let opened = if self.markup_open { Task::none() } else { self.markup_update(MarkupMsg::Toggle) };
                // Picking the highlighter with text selected highlights it.
                if tool == Tool::Highlight && self.selection.is_some_and(|s| !s.is_empty()) {
                    let _ = self.highlight_selection();
                }
                Task::batch([done, opened])
            }
            MarkupMsg::Swatch(i) => {
                self.swatch = i;
                let alpha = self.current_color().a;
                let color = Color { a: alpha, ..SWATCHES[i].1 };
                self.set_color(color);
                self.style_commit();
                Task::none()
            }
            MarkupMsg::StyleMenu => {
                self.style_menu = !self.style_menu;
                if self.style_menu {
                    let c = self.current_color();
                    self.hex_input = super::palette::to_hex(c);
                    self.size_input = format!("{:.0}", self.current_size());
                    self.picker_hue = super::palette::to_hsv(c).0;
                } else {
                    self.style_commit();
                }
                Task::none()
            }
            MarkupMsg::CustomColor => {
                self.picker_open = !self.picker_open;
                Task::none()
            }
            MarkupMsg::Pick { h, s, v } => {
                self.picker_hue = h;
                let alpha = self.current_color().a;
                self.set_color(super::palette::from_hsv(h, s, v, alpha));
                Task::none()
            }
            MarkupMsg::Hex(t) => {
                self.hex_input = t.chars().filter(|c| c.is_ascii_hexdigit() || *c == '#').take(7).collect();
                if let Some(c) = super::palette::from_hex(&self.hex_input) {
                    let alpha = self.current_color().a;
                    let hex = self.hex_input.clone();
                    self.set_color(Color { a: alpha, ..c });
                    self.hex_input = hex;
                }
                Task::none()
            }
            MarkupMsg::Opacity(a) => {
                let c = self.current_color();
                self.set_color(Color { a: a.clamp(0.05, 1.0), ..c });
                Task::none()
            }
            MarkupMsg::LineWidth(w) => {
                self.line_width = (w as f64).clamp(0.25, 100.0);
                self.apply_unit();
                let width = self.editor.style.width;
                self.restyle_live(|shape, style| {
                    if !matches!(shape, Shape::Highlight(_) | Shape::Text { .. }) {
                        style.width = width;
                    }
                });
                Task::none()
            }
            MarkupMsg::TextSize(size) => {
                self.size_input = format!("{size:.0}");
                self.set_text_size(size as f64);
                Task::none()
            }
            MarkupMsg::TextSizeTyped(t) => {
                self.size_input = t.chars().filter(|c| c.is_ascii_digit() || *c == '.').take(5).collect();
                if let Ok(size) = self.size_input.parse::<f64>()
                    && size >= 1.0
                {
                    self.set_text_size(size);
                }
                Task::none()
            }
            MarkupMsg::StyleCommit => {
                self.style_commit();
                Task::none()
            }
            MarkupMsg::Press { page, x, y, clicks } => {
                // Drawing closes the style menu.
                if std::mem::take(&mut self.style_menu) {
                    self.style_commit();
                }
                let done = self.finish_text();
                let tolerance = (REACH / self.markup_zoom()) as f64;
                let Some(marks) = &mut self.marks else { return done };
                self.selection = None;
                let outcome = self.editor.press(&mut marks.store, page, Point::new(x, y), tolerance, clicks);
                self.last_drag = Some((x, y));
                Task::batch([done, self.apply(outcome)])
            }
            MarkupMsg::Drag { x, y } => {
                let Some(marks) = &mut self.marks else { return Task::none() };
                self.last_drag = Some((x, y));
                self.editor.drag(&mut marks.store, Point::new(x, y));
                Task::none()
            }
            MarkupMsg::Release { x, y } => {
                let Some(marks) = &mut self.marks else { return Task::none() };
                let outcome = self.editor.release(&mut marks.store, Point::new(x, y));
                self.apply(outcome)
            }
            MarkupMsg::Undo | MarkupMsg::Redo => {
                let done = self.finish_text();
                let Some(marks) = &mut self.marks else { return done };
                self.editor.cancel(&mut marks.store);
                let state = if matches!(msg, MarkupMsg::Undo) { marks.history.undo() } else { marks.history.redo() };
                if let Some(layers) = state.cloned() {
                    marks.store.set_pages(layers);
                }
                done
            }
            MarkupMsg::Delete => {
                let Some(marks) = &mut self.marks else { return Task::none() };
                self.editing = None;
                let outcome = self.editor.delete_selected(&mut marks.store);
                self.apply(outcome)
            }
            MarkupMsg::Nudge(dx, dy) => {
                let Some(marks) = &mut self.marks else { return Task::none() };
                let outcome = self.editor.nudge(&mut marks.store, dx, dy);
                self.apply(outcome)
            }
            MarkupMsg::TextAction(action) => {
                if let Some(edit) = &mut self.editing {
                    edit.content.perform(action);
                }
                Task::none()
            }

            MarkupMsg::Prepared(generation, path, result) => {
                self.markup_busy = None;
                if generation != self.generation {
                    return Task::none();
                }
                match result {
                    Ok(opened) => {
                        let thumb = self.unlocked_thumb(&path);
                        Task::batch([self.install_opened(path, opened), thumb])
                    }
                    Err(error) if matches!(&self.doc, Doc::Loading(p) if *p == path) => self.pdf_failed(path, error),
                    Err(pdf::OpenError::Failed(e)) => self.toast(format!("Markup is not available for this PDF: {e}")),
                    Err(pdf::OpenError::Password { .. }) => self.toast("Markup is not available for this PDF"),
                }
            }
            MarkupMsg::Save => {
                let done = self.finish_text();
                match &self.doc {
                    Doc::Pdf(view) if self.has_unsaved_changes() => {
                        let path = view.doc.path().to_owned();
                        Task::batch([done, self.save_pdf(path)])
                    }
                    Doc::Pdf(_) => done,
                    Doc::Image(_) => Task::batch([done, self.markup_update(MarkupMsg::SaveAs)]),
                    _ => done,
                }
            }
            MarkupMsg::SaveAs => {
                let done = self.finish_text();
                let suggested = match &self.doc {
                    Doc::Pdf(view) => {
                        let p = view.doc.path();
                        let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                        p.with_file_name(format!("{stem} (marked up).pdf"))
                    }
                    Doc::Image(view) => {
                        let p = &view.path;
                        let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                        let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
                        let ext = match ext.as_str() {
                            "jpg" | "jpeg" | "png" | "webp" | "tif" | "tiff" | "bmp" => ext,
                            _ => "png".into(),
                        };
                        let what = if view.edit.edits.is_none() { "marked up" } else { "edited" };
                        p.with_file_name(format!("{stem} ({what}).{ext}"))
                    }
                    _ => return done,
                };
                let ask = App::ask_for_files(super::chooser::Purpose::SaveCopy, Some(suggested));
                Task::batch([done, ask])
            }
            MarkupMsg::SaveTo(Ok(None)) => Task::none(),
            MarkupMsg::SaveTo(Err(e)) => self.toast(e),
            MarkupMsg::SaveTo(Ok(Some(target))) => match &self.doc {
                Doc::Pdf(_) => self.save_pdf(target),
                Doc::Image(_) => self.save_picture(target),
                _ => Task::none(),
            },
            MarkupMsg::PdfSaved(generation, target, layers, result) => {
                self.markup_busy = None;
                match result {
                    Ok(opened) if generation == self.generation => {
                        if self.files.get(self.current).is_some_and(|p| *p != target) {
                            self.files[self.current] = target.clone();
                        }
                        let pages = opened.doc.page_count();
                        let shown = self.swap_pdf(opened.doc);
                        self.attach_form(&target, opened.form, pages);
                        if let (Some(marks), Some(imported)) = (&mut self.marks, opened.imported) {
                            marks.refs = imported.refs;
                            marks.saved = layers;
                        }
                        Task::batch([shown, self.notice(format!("Saved “{}”", file_name(&target)))])
                    }
                    Ok(_) => self.notice(format!("Saved “{}”", file_name(&target))),
                    Err(e) => self.toast(format!("Could not save: {e}")),
                }
            }
            MarkupMsg::PictureSaved(generation, target, layers, result) => {
                self.markup_busy = None;
                if let Err(e) = result {
                    return self.toast(format!("Could not save: {e}"));
                }
                let saved = self.notice(format!("Saved a copy as “{}”", file_name(&target)));
                if generation != self.generation {
                    return saved;
                }
                if self.current_path() == Some(target.as_path()) {
                    // The markup is part of the picture now; show the new file as it is.
                    self.marks = None;
                    self.pictures.retain(|(p, _)| *p != target);
                    self.thumbs.remove(&target);
                    let index = self.current;
                    self.current = usize::MAX;
                    return Task::batch([saved, self.show(index)]);
                }
                if let Some(marks) = &mut self.marks {
                    marks.saved = layers;
                }
                if let Doc::Image(view) = &mut self.doc {
                    view.edit.saved = view.edit.edits;
                }
                saved
            }
            MarkupMsg::QuitAnyway => iced::exit(),
            MarkupMsg::StayOpen => {
                self.confirm_quit = false;
                Task::none()
            }
        }
    }

    /// What to write into the PDF on screen: unsaved markup and form values.
    pub(super) fn pdf_changes(&self) -> Pending {
        let (layers, refs, pages) = match &self.marks {
            Some(m) => (m.store.pages.layers.clone(), m.refs.clone(), m.dirty_pages()),
            None => Default::default(),
        };
        let (form, values) = match &self.form {
            Some(f) => (f.form.clone(), f.values.clone()),
            None => Default::default(),
        };
        let password = match &self.doc {
            Doc::Pdf(view) => view.doc.password().map(str::to_owned),
            _ => None,
        };
        Pending { layers, refs, pages, form, values, password }
    }

    fn save_pdf(&mut self, target: PathBuf) -> Task<Message> {
        let Doc::Pdf(view) = &self.doc else { return Task::none() };
        if self.markup_busy.is_some() {
            return Task::none();
        }
        self.markup_busy = Some("Saving…");
        let source = view.doc.path().to_owned();
        let markup = self.marks.is_some();
        let pending = self.pdf_changes();
        let generation = self.generation;
        let saved = pending.layers.clone();
        Task::perform(
            {
                let target = target.clone();
                blocking(move || {
                    pdf::Document::save_changes(&source, &target, &pending.changes(), markup).map_err(err_string)
                })
            },
            move |r| Message::Markup(MarkupMsg::PdfSaved(generation, target.clone(), saved.clone(), r.and_then(|r| r))),
        )
    }

    /// The edited picture and its markup, unless the edits are still being worked out.
    fn picture_with_markup(&self) -> Option<(image::RgbaImage, AnnotationLayer)> {
        let Doc::Image(view) = &self.doc else { return None };
        if view.edit.is_busy() {
            return None;
        }
        let pic = &view.shown.picture;
        let rgba = image::RgbaImage::from_raw(pic.width, pic.height, Vec::from(pic.rgba.clone()))?;
        let layer = self.marks.as_ref().and_then(|m| m.store.layer(0).cloned()).unwrap_or_default();
        Some((rgba, layer))
    }

    fn save_picture(&mut self, target: PathBuf) -> Task<Message> {
        if self.markup_busy.is_some() {
            return Task::none();
        }
        let Some((rgba, layer)) = self.picture_with_markup() else { return Task::none() };
        self.markup_busy = Some("Saving…");
        let generation = self.generation;
        let saved = self.marks.as_ref().map(|m| m.store.pages.layers.clone()).unwrap_or_default();
        let source = match &self.doc {
            Doc::Image(view) => view.path.clone(),
            _ => return Task::none(),
        };
        Task::perform(
            {
                let target = target.clone();
                blocking(move || {
                    // The copy keeps the original's EXIF data and colour profile.
                    let meta = crate::metadata::Metadata::for_copy(&source, rgba.width(), rgba.height());
                    crate::annotate::flatten::save(rgba, &layer, &target, &meta).map_err(err_string)
                })
            },
            move |r| {
                Message::Markup(MarkupMsg::PictureSaved(generation, target.clone(), saved.clone(), r.and_then(|r| r)))
            },
        )
    }

    /// For printing: a copy of the file with its markup, when it has markup the file lacks.
    pub(super) fn printable_copy(&self) -> Option<Task<Result<PathBuf, String>>> {
        match &self.doc {
            // Unsaved changes are printed too; a protected PDF is printed unlocked (the print system cannot open it).
            Doc::Pdf(view) if self.has_unsaved_changes() || view.doc.password().is_some() => {
                let source = view.doc.path().to_owned();
                let pending = self.pdf_changes();
                let unsaved = self.has_unsaved_changes();
                Some(Task::perform(
                    blocking(move || {
                        let mut bytes = if unsaved {
                            pdf::Document::with_changes(&source, &pending.changes()).map_err(err_string)?
                        } else {
                            std::fs::read(&source).map_err(|e| e.to_string())?
                        };
                        if let Some(password) = &pending.password {
                            bytes = pdf::decrypted(&bytes, password).map_err(err_string)?;
                        }
                        let copy = print_copy(&file_name(&source))?;
                        std::fs::write(&copy, bytes).map_err(|e| e.to_string())?;
                        Ok(copy)
                    }),
                    |r| r.and_then(|r| r),
                ))
            }
            Doc::Image(view) if self.marks.as_ref().is_some_and(DocMarkup::has_items) || !view.edit.edits.is_none() => {
                let (rgba, layer) = self.picture_with_markup()?;
                let stem = view.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                Some(Task::perform(
                    blocking(move || {
                        let copy = print_copy(&format!("{stem}.png"))?;
                        crate::annotate::flatten::save(rgba, &layer, &copy, &Default::default()).map_err(err_string)?;
                        Ok(copy)
                    }),
                    |r| r.and_then(|r| r),
                ))
            }
            _ => None,
        }
    }

    /// Keys that mean something while marking up; `None` for the rest.
    pub(super) fn markup_key(
        &mut self,
        key: &iced::keyboard::Key,
        modifiers: iced::keyboard::Modifiers,
    ) -> Option<Task<Message>> {
        use iced::keyboard::{Key, key::Named};
        if self.editing.is_some() || !matches!(self.doc, Doc::Image(_) | Doc::Pdf(_)) {
            return None;
        }
        let ctrl = modifiers.command();
        let has_marks = self.marks.is_some();
        let selected = self.marks.as_ref().is_some_and(|m| m.store.selection.is_some());
        let unit = self.marks.as_ref().map_or(1.0, |m| m.unit) * if modifiers.shift() { 10.0 } else { 1.0 };
        let msg = match key.as_ref() {
            Key::Character(c) => match (c.to_lowercase().as_str(), ctrl, modifiers.shift()) {
                ("z", true, false) if has_marks => MarkupMsg::Undo,
                ("z", true, true) | ("y", true, false) if has_marks => MarkupMsg::Redo,
                ("s", true, false) if has_marks || self.has_unsaved_changes() => MarkupMsg::Save,
                ("s", true, true) if has_marks || self.form.is_some() => MarkupMsg::SaveAs,
                ("m", false, false) => MarkupMsg::Toggle,
                _ => return None,
            },
            // Delete is heard by the app whatever has focus (see `markup_delete_key`).
            Key::Named(Named::Backspace) if selected => MarkupMsg::Delete,
            Key::Named(Named::ArrowLeft) if selected => MarkupMsg::Nudge(-unit, 0.0),
            Key::Named(Named::ArrowRight) if selected => MarkupMsg::Nudge(unit, 0.0),
            Key::Named(Named::ArrowUp) if selected => MarkupMsg::Nudge(0.0, -unit),
            Key::Named(Named::ArrowDown) if selected => MarkupMsg::Nudge(0.0, unit),
            _ => return None,
        };
        Some(self.markup_update(msg))
    }

    /// Escape, while marking up: stop typing or close the style menu, then deselect, then
    /// leave markup.
    pub(super) fn markup_escape(&mut self) -> Option<Task<Message>> {
        if self.editing.is_some() {
            return Some(self.finish_text());
        }
        if std::mem::take(&mut self.style_menu) {
            self.style_commit();
            return Some(Task::none());
        }
        if let Some(marks) = &mut self.marks {
            if self.editor.is_dragging() {
                self.editor.cancel(&mut marks.store);
                return Some(Task::none());
            }
            if marks.store.selection.take().is_some() {
                return Some(Task::none());
            }
        }
        if self.markup_open {
            return Some(self.markup_update(MarkupMsg::Toggle));
        }
        None
    }

    /// The Delete key, heard whether or not a widget took it: deletes the selected annotation.
    pub(super) fn markup_delete_key(&mut self) -> Task<Message> {
        let selected = self.marks.as_ref().is_some_and(|m| m.store.selection.is_some());
        if self.editing.is_none() && selected {
            return self.markup_update(MarkupMsg::Delete);
        }
        Task::none()
    }

    /// The left button went up somewhere: finish a highlight of selected text, or a drag
    /// whose release the page never saw (it scrolled away).
    pub(super) fn markup_mouse_up(&mut self) -> Option<Task<Message>> {
        if let Some(marks) = &mut self.marks
            && self.editor.is_dragging()
        {
            let (x, y) = self.last_drag.unwrap_or_default();
            let outcome = self.editor.release(&mut marks.store, Point::new(x, y));
            return Some(self.apply(outcome));
        }
        if self.markup_open && self.editor.tool == Tool::Highlight && self.selecting {
            self.selecting = false;
            return Some(self.highlight_selection());
        }
        None
    }

    /// A press on the page that the markup layer let through (text selection or panning).
    pub(super) fn markup_page_press(&mut self) -> Task<Message> {
        let done = self.finish_text();
        if let Some(marks) = &mut self.marks {
            marks.store.selection = None;
        }
        done
    }

    // ------------------------------------------------------------ view

    /// The markup layer of `page`, drawn at `zoom` over a `w`×`h` area.
    pub(super) fn markup_layers<'a>(&'a self, page: usize, zoom: f32, w: f32, h: f32) -> Vec<Element<'a, Message>> {
        let Some(marks) = &self.marks else { return Vec::new() };
        let is_pdf = matches!(self.doc, Doc::Pdf(_));
        let store = &marks.store;
        let editing = self.editing.as_ref().filter(|e| e.page == page);
        let overlay = Overlay {
            page,
            zoom,
            layer: store.layer(page),
            draft: store.draft.as_ref().filter(|(p, _)| *p == page).map(|(_, a)| a),
            selected: store.selection.filter(|s| s.page == page).and_then(|_| store.selected()),
            hidden: editing.map(|e| e.id),
            capture: self.capture(is_pdf),
            over_text: is_pdf && self.hover_text && self.hover.is_some_and(|(p, _)| p == page),
            text_tool: self.editor.tool == Tool::Text,
        };
        let mut layers = vec![canvas(overlay).width(w).height(h).into()];
        if let Some(edit) = editing
            && let Some(a) = store.layer(page).and_then(|l| l.get(edit.id))
        {
            let b = a.bounds();
            let c = a.style.stroke;
            let size = (a.style.font_size * zoom as f64) as f32;
            let editor = text_editor(&edit.content)
                .id(text_box_id())
                .placeholder("Type here")
                .on_action(|action| Message::Markup(MarkupMsg::TextAction(action)))
                .size(size)
                .line_height(iced::widget::text::LineHeight::Relative(crate::annotate::paint::LINE_HEIGHT as f32))
                .padding((a.style.font_size * crate::annotate::paint::TEXT_PAD) as f32 * zoom)
                .width((b.w as f32 * zoom).max(size * 8.0))
                .style(style::text_box(iced::Color::from_rgb(c.r, c.g, c.b)));
            let at = hayro::kurbo::Rect::new(b.x, b.y, b.right(), b.bottom());
            layers.push(placed(&at, zoom, editor.into()));
        }
        layers
    }

    pub(super) fn markup_bar(&self) -> Element<'_, Message> {
        let tool = |t: Tool, icon: iced::widget::svg::Handle, label: &'static str| {
            icon_button(icon, label, Some(Message::Markup(MarkupMsg::Tool(t))), self.editor.tool == t)
        };
        let is_pdf = matches!(self.doc, Doc::Pdf(_));
        let highlight_label = if is_pdf { "Highlight text" } else { "Highlight" };
        let tools = row![
            tool(Tool::Select, icons::select(), "Select, move and resize"),
            tool(Tool::Highlight, icons::highlighter(), highlight_label),
            tool(Tool::Pen, icons::pen(), "Pen"),
            tool(Tool::Rectangle, icons::rectangle(), "Rectangle"),
            tool(Tool::Ellipse, icons::ellipse(), "Ellipse"),
            tool(Tool::Line, icons::line(), "Line"),
            tool(Tool::Arrow, icons::arrow(), "Arrow"),
            tool(Tool::Text, icons::text_tool(), "Text"),
        ]
        .spacing(2)
        .align_y(Alignment::Center);
        let marks = self.marks.as_ref();
        let can_undo = marks.is_some_and(|m| m.history.can_undo());
        let can_redo = marks.is_some_and(|m| m.history.can_redo());
        let selected = marks.is_some_and(|m| m.store.selection.is_some());
        let edit = row![
            separator(),
            icon_button(icons::undo(), "Undo (Ctrl+Z)", can_undo.then_some(Message::Markup(MarkupMsg::Undo)), false),
            icon_button(
                icons::redo(),
                "Redo (Ctrl+Shift+Z)",
                can_redo.then_some(Message::Markup(MarkupMsg::Redo)),
                false
            ),
            icon_button(icons::delete(), "Delete (Del)", selected.then_some(Message::Markup(MarkupMsg::Delete)), false),
        ]
        .spacing(2)
        .align_y(Alignment::Center);

        let dirty = self.has_unsaved_changes();
        let status = match self.markup_busy {
            Some(s) => s,
            None if dirty => "Unsaved",
            None => "",
        };
        let ready = self.markup_busy.is_none() && marks.is_some();
        let (save_label, save_msg, save_tip) = if is_pdf {
            ("Save", (ready && dirty).then_some(Message::Markup(MarkupMsg::Save)), "Save into the PDF (Ctrl+S)")
        } else {
            (
                "Save a Copy…",
                (ready
                    && (matches!(&self.doc, Doc::Image(v) if !v.edit.edits.is_none())
                        || marks.is_some_and(DocMarkup::has_items)))
                .then_some(Message::Markup(MarkupMsg::SaveAs)),
                "Save the picture with its markup as a new file (Ctrl+S)",
            )
        };
        let save = button(text(save_label).size(13)).padding([6, 14]).style(style::primary).on_press_maybe(save_msg);
        let mut end = row![text(status).size(12).style(muted), super::tip(save.into(), save_tip)]
            .spacing(6)
            .align_y(Alignment::Center);
        if is_pdf {
            end = end.push(icon_button(
                icons::save_copy(),
                "Save a copy… (Ctrl+Shift+S)",
                ready.then_some(Message::Markup(MarkupMsg::SaveAs)),
                false,
            ));
        }

        // A floating island at the top of the view, and under it, while drawing or with
        // something selected, a second one for colour and size.
        let main = iced::widget::opaque(
            container(
                row![tools, separator(), self.style_button(), edit, separator(), end]
                    .spacing(4)
                    .align_y(Alignment::Center),
            )
            .padding([4, 6])
            .style(style::panel),
        );
        let mut islands = column![main].spacing(8).align_x(Alignment::Center);
        if self.style_menu {
            islands = islands.push(self.style_menu_view());
        }
        islands.into()
    }

    pub(super) fn quit_dialog(&self) -> Element<'_, Message> {
        let n = self.unsaved_files();
        let what = if n == 1 { "One file has".to_string() } else { format!("{n} files have") };
        let card = container(
            column![
                text("Quit without saving?")
                    .size(18)
                    .font(Font { weight: iced::font::Weight::Semibold, ..crate::style::FONT }),
                text(format!("{what} markup that is not saved. It will be lost.")).size(14).style(muted),
                space().height(6),
                row![
                    space().width(Length::Fill),
                    button(text("Cancel").size(14))
                        .padding([8, 16])
                        .style(style::secondary)
                        .on_press(Message::Markup(MarkupMsg::StayOpen)),
                    button(text("Quit Without Saving").size(14))
                        .padding([8, 16])
                        .style(iced::widget::button::danger)
                        .on_press(Message::Markup(MarkupMsg::QuitAnyway)),
                ]
                .spacing(10),
            ]
            .spacing(8)
            .width(400),
        )
        .padding(22)
        .style(style::panel);
        center(card).style(style::scrim).into()
    }
}
