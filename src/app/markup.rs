//! Markup in the main window: the toolbar's state, how pointer events turn into
//! annotations, and how annotation changes join the undo history.

use std::{cell::RefCell, collections::BTreeMap, rc::Rc, sync::Arc};

use relm4::{ComponentSender, gtk::prelude::*};

use super::{App, AppMsg};
use crate::{
    annotate::{AnnotationLayer, Editor, GeometryChange, History, Outcome, Rect, Shape, Store, Tool},
    edit::EditOp,
    pdf::HitRect,
    widgets::{MarkupEvent, MarkupMode},
};

/// Annotation layers by page.
pub type Layers = BTreeMap<usize, AnnotationLayer>;

/// One step of a PDF's history: the (working copy of the) file and its annotations.
/// Page operations change the file, drawing changes the layers; both undo as one.
#[derive(Debug, Clone)]
pub struct PdfState {
    pub file: std::path::PathBuf,
    pub layers: Layers,
}

pub struct MarkupState {
    pub open: bool,
    pub editor: Editor,
    /// What the canvas draws; shared with it.
    pub store: Rc<RefCell<Store>>,
    /// Undo history for a PDF's pages and annotations (images use the edit stack instead).
    pub pdf_history: History<PdfState>,
    /// How the PDF was when opened, to return to on "discard".
    pub pdf_baseline: PdfState,
    pub pdf_dirty: bool,
    /// A text selection should become a highlight as soon as its rectangles arrive.
    pub highlight_pending: bool,
}

impl Default for MarkupState {
    fn default() -> Self {
        Self {
            open: false,
            editor: Editor::default(),
            store: Rc::default(),
            pdf_history: History::new(PdfState { file: Default::default(), layers: Layers::new() }),
            pdf_baseline: PdfState { file: Default::default(), layers: Layers::new() },
            pdf_dirty: false,
            highlight_pending: false,
        }
    }
}

impl App {
    pub(super) fn markup_tool(&self) -> Tool {
        self.markup.editor.tool
    }

    /// Markup applies to images and PDFs, but not while a crop or colour tool is open.
    pub(super) fn can_markup(&self) -> bool {
        (self.image.is_some() || self.pdf.is_some()) && self.edit_mode == super::edit::EditMode::None
    }

    /// How the canvas should treat the pointer.
    pub(super) fn markup_canvas_mode(&self) -> MarkupMode {
        match (self.markup.open, self.pdf.is_some(), self.markup.editor.tool) {
            (false, ..) => MarkupMode::Off,
            (true, true, Tool::Highlight) => MarkupMode::TextSelect,
            (true, _, tool) => MarkupMode::Draw(tool),
        }
    }

    pub(super) fn toggle_markup(&mut self) {
        if self.markup.open {
            self.markup.open = false;
            self.markup.store.borrow_mut().selection = None;
        } else if self.can_markup() {
            self.markup.open = true;
            self.clear_selection();
        }
        self.canvas.queue_draw();
    }

    pub(super) fn set_tool(&mut self, tool: Tool) {
        self.markup.editor.tool = tool;
        self.markup.store.borrow_mut().draft = None;
        if tool != Tool::Select {
            self.markup.store.borrow_mut().selection = None;
        }
        self.clear_selection();
        self.canvas.queue_draw();
    }

    // --- pointer events -----------------------------------------------------------

    pub(super) fn markup_event(&mut self, event: MarkupEvent, sender: &ComponentSender<App>) {
        let outcome = {
            let mut store = self.markup.store.borrow_mut();
            let editor = &mut self.markup.editor;
            match event {
                MarkupEvent::Press { page, point, tolerance, clicks } => editor.press(&mut store, page, point, tolerance, clicks),
                MarkupEvent::Drag(point) => editor.drag(&mut store, point),
                MarkupEvent::Release(point) => editor.release(&mut store, point),
            }
        };
        self.finish_markup(outcome, sender);
    }

    /// Act on what an editing step asked for.
    fn finish_markup(&mut self, outcome: Outcome, sender: &ComponentSender<App>) {
        if outcome.committed {
            self.commit_markup(sender);
        }
        if let Some((page, id)) = outcome.edit_text {
            self.open_text_editor(page, id, sender);
        }
        if outcome.redraw || outcome.committed {
            self.canvas.queue_draw();
        }
    }

    fn open_text_editor(&self, page: usize, id: u64, sender: &ComponentSender<App>) {
        let (bounds, text) = {
            let store = self.markup.store.borrow();
            match store.layer(page).and_then(|l| l.get(id)).map(|a| (a.bounds(), a.shape.clone())) {
                Some((bounds, Shape::Text { text, .. })) => (bounds, text),
                _ => return,
            }
        };
        let sender = sender.clone();
        self.canvas.edit_text(page, bounds, &text, move |text| {
            sender.input(AppMsg::MarkupText { page, id, text });
        });
    }

    pub(super) fn markup_text(&mut self, page: usize, id: u64, text: &str, sender: &ComponentSender<App>) {
        let outcome = {
            let mut store = self.markup.store.borrow_mut();
            self.markup.editor.set_text(&mut store, page, id, text)
        };
        self.finish_markup(outcome, sender);
    }

    /// Place text directly, as the Text tool plus the editor would.
    pub(super) fn markup_add_text(&mut self, page: usize, x: f64, y: f64, text: &str, sender: &ComponentSender<App>) {
        let outcome = {
            let mut store = self.markup.store.borrow_mut();
            let editor = &mut self.markup.editor;
            editor.tool = Tool::Text;
            let placed = editor.press(&mut store, page, crate::annotate::Point::new(x, y), 4.0, 1);
            match placed.edit_text {
                Some((page, id)) => editor.set_text(&mut store, page, id, text),
                None => placed,
            }
        };
        self.markup.open = true;
        self.finish_markup(outcome, sender);
    }

    pub(super) fn markup_delete(&mut self, sender: &ComponentSender<App>) {
        if !self.markup.open {
            return;
        }
        let outcome = {
            let mut store = self.markup.store.borrow_mut();
            self.markup.editor.delete_selected(&mut store)
        };
        self.finish_markup(outcome, sender);
    }

    /// Nudge the selected annotation. Returns whether there was one to move.
    pub(super) fn markup_nudge(&mut self, dx: f64, dy: f64, sender: &ComponentSender<App>) -> bool {
        if !self.markup.open || self.markup.store.borrow().selection.is_none() {
            return false;
        }
        let outcome = {
            let mut store = self.markup.store.borrow_mut();
            self.markup.editor.nudge(&mut store, dx, dy)
        };
        self.finish_markup(outcome, sender);
        true
    }

    /// Deselect, if something is selected. Returns whether it was.
    pub(super) fn markup_deselect(&mut self) -> bool {
        let had = self.markup.store.borrow_mut().selection.take().is_some();
        if had {
            self.canvas.queue_draw();
        }
        had
    }

    /// Change the drawing style, and the selected annotation along with it.
    pub(super) fn set_markup_style(&mut self, change: impl FnOnce(&mut crate::annotate::Style), sender: &ComponentSender<App>) {
        change(&mut self.markup.editor.style);
        let outcome = {
            let mut store = self.markup.store.borrow_mut();
            self.markup.editor.restyle_selected(&mut store)
        };
        self.finish_markup(outcome, sender);
    }

    /// Text selected in a PDF becomes a highlight.
    pub(super) fn highlight_selection(&mut self, rects: &[(usize, Vec<HitRect>)], sender: &ComponentSender<App>) {
        self.markup.highlight_pending = false;
        let mut committed = false;
        for (page, hits) in rects {
            let rects: Vec<Rect> = hits.iter().map(|h| Rect::new(h.x1, h.y1, h.x2 - h.x1, h.y2 - h.y1)).collect();
            let outcome = {
                let mut store = self.markup.store.borrow_mut();
                self.markup.editor.add_highlight(&mut store, *page, rects)
            };
            committed |= outcome.committed;
        }
        self.clear_selection();
        if committed {
            self.commit_markup(sender);
        }
        self.canvas.queue_draw();
    }

    // --- history ------------------------------------------------------------------

    fn store_layers(&self) -> Layers {
        self.markup.store.borrow().pages.layers.clone()
    }

    /// The annotations changed for good: record an undo step.
    fn commit_markup(&mut self, _sender: &ComponentSender<App>) {
        if self.pdf.is_some() {
            let state = PdfState { file: self.markup.pdf_history.present().file.clone(), layers: self.store_layers() };
            self.markup.pdf_history.commit(state);
            self.markup.pdf_dirty = true;
        } else if let Some(session) = &mut self.edit {
            let layer = self.markup.store.borrow().pages.layers.get(&0).cloned().unwrap_or_default();
            session.stack.push_with_layer(EditOp::Annotate, Some(Arc::new(layer)));
        }
    }

    /// Put the store back in line with the history after an undo or redo.
    pub(super) fn sync_markup_from_history(&mut self) {
        let layers = if self.pdf.is_some() {
            self.markup.pdf_history.present().layers.clone()
        } else {
            let layer = self.edit.as_ref().and_then(|e| e.stack.current_layer());
            layer.map(|l| BTreeMap::from([(0, (*l).clone())])).unwrap_or_default()
        };
        self.markup.store.borrow_mut().set_pages(layers);
        self.canvas.queue_draw();
    }

    /// The PDF history moved: show the annotations, and the file, of its present state.
    pub(super) fn pdf_history_changed(&mut self, sender: &ComponentSender<App>) {
        self.sync_markup_from_history();
        let file = self.markup.pdf_history.present().file.clone();
        if let Some(pdf) = &self.pdf
            && pdf.source.path != file
        {
            let page = pdf.page;
            self.reload_pdf(file, page, sender);
        }
    }

    /// Start from the PDF's own annotations (or none, for an image).
    pub(super) fn load_markup(&mut self, annotations: Option<Layers>) {
        let layers = annotations.unwrap_or_default();
        let file = self.pdf.as_ref().map(|p| p.source.path.clone()).unwrap_or_default();
        let state = PdfState { file, layers: layers.clone() };
        self.markup.pdf_baseline = state.clone();
        self.markup.pdf_history = History::new(state);
        self.markup.pdf_dirty = false;
        self.markup.highlight_pending = false;
        let mut store = self.markup.store.borrow_mut();
        store.selection = None;
        store.draft = None;
        store.set_pages(layers);
    }

    /// Go back to how the PDF was when it was opened (discarding edits).
    pub(super) fn reset_markup(&mut self) {
        let baseline = self.markup.pdf_baseline.clone();
        self.markup.pdf_history = History::new(baseline.clone());
        self.markup.pdf_dirty = false;
        self.markup.store.borrow_mut().set_pages(baseline.layers);
    }

    // --- geometry edits under annotations -----------------------------------------

    /// The layer after `op`, moved along with the image, and the change it applied.
    pub(super) fn layer_after_geometry(&self, op: EditOp, size: (f64, f64)) -> Option<Arc<AnnotationLayer>> {
        let layer = self.markup.store.borrow().pages.layers.get(&0).filter(|l| !l.is_empty())?.clone();
        let (w, h) = size;
        let change = match op {
            EditOp::RotateCw => Some(GeometryChange::RotateCw { height: h }),
            EditOp::RotateCcw => Some(GeometryChange::RotateCcw { width: w }),
            EditOp::FlipH => Some(GeometryChange::FlipH { width: w }),
            EditOp::FlipV => Some(GeometryChange::FlipV { height: h }),
            EditOp::Crop(r) => Some(GeometryChange::Crop { x: r.x as f64, y: r.y as f64 }),
            EditOp::Resize { w: nw, h: nh } => Some(GeometryChange::Scale { sx: nw as f64 / w, sy: nh as f64 / h }),
            EditOp::Adjust(_) | EditOp::Annotate => None,
        };
        let mut moved = layer;
        if let Some(change) = change {
            moved.apply(change);
        }
        Some(Arc::new(moved))
    }
}
