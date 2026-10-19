//! The editing logic behind the markup tools: what a press, drag and release do.
//!
//! No GTK here either. The canvas turns pointer events into document
//! coordinates and calls these methods; the returned `Outcome` says what the
//! caller has to do (redraw, record an undo step, open the text editor).

use std::collections::BTreeMap;

use super::model::{Annotation, AnnotationLayer, Handle, Pages, Point, Rect, Shape, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Select,
    Rectangle,
    Ellipse,
    Arrow,
    Line,
    Pen,
    Text,
    Highlight,
}

/// The annotation currently selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub page: usize,
    pub id: u64,
}

/// Everything drawn: the committed layers plus what is being drawn right now.
#[derive(Debug, Clone, Default)]
pub struct Store {
    pub pages: Pages,
    pub selection: Option<Selection>,
    /// A shape that is still being dragged out and is not part of the layer yet.
    pub draft: Option<(usize, Annotation)>,
}

impl Store {
    pub fn layer(&self, page: usize) -> Option<&AnnotationLayer> {
        self.pages.layers.get(&page)
    }

    pub fn selected(&self) -> Option<&Annotation> {
        let s = self.selection?;
        self.layer(s.page)?.get(s.id)
    }

    /// Replace all layers (after undo, redo or loading) and drop the selection if it vanished.
    pub fn set_pages(&mut self, layers: BTreeMap<usize, AnnotationLayer>) {
        self.pages.layers = layers;
        self.draft = None;
        if self.selected().is_none() {
            self.selection = None;
        }
    }
}

/// What the caller must do after an event.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    /// Something on screen changed.
    pub redraw: bool,
    /// The annotations changed for good: record an undo step.
    pub committed: bool,
    /// Open the text editor for this annotation (page, id).
    pub edit_text: Option<(usize, u64)>,
}

/// A drag in progress.
#[derive(Debug, Clone)]
enum Grab {
    Draw {
        page: usize,
        start: Point,
    },
    Move {
        page: usize,
        id: u64,
        start: Point,
        original: Annotation,
    },
    Resize {
        page: usize,
        id: u64,
        handle: Handle,
        original: Annotation,
    },
}

/// A drawn shape smaller than this (document units) counts as an accidental click.
const MIN_DRAG: f64 = 3.0;
/// Size of a new text box.
const TEXT_WIDTH: f64 = 180.0;

#[derive(Debug, Default)]
pub struct Editor {
    pub tool: Tool,
    pub style: Style,
    grab: Option<Grab>,
    /// An annotation created by the Text tool whose text has not been confirmed.
    unconfirmed_text: Option<(usize, u64)>,
}

impl Editor {
    pub fn is_dragging(&self) -> bool {
        self.grab.is_some()
    }

    /// The pointer went down at `p` on `page`. `tolerance` is the click reach in
    /// document units; `clicks` is 1, 2 or 3 for single, double and triple clicks.
    pub fn press(
        &mut self,
        store: &mut Store,
        page: usize,
        p: Point,
        tolerance: f64,
        clicks: u32,
    ) -> Outcome {
        match self.tool {
            Tool::Select => self.press_select(store, page, p, tolerance, clicks),
            Tool::Text => self.press_text(store, page, p),
            tool => {
                store.selection = None;
                let shape = match tool {
                    Tool::Rectangle => Shape::Rect(Rect::new(p.x, p.y, 0.0, 0.0)),
                    Tool::Ellipse => Shape::Ellipse(Rect::new(p.x, p.y, 0.0, 0.0)),
                    Tool::Highlight => Shape::Highlight(vec![Rect::new(p.x, p.y, 0.0, 0.0)]),
                    Tool::Line => Shape::Line(p, p),
                    Tool::Arrow => Shape::Arrow(p, p),
                    _ => Shape::Freehand(vec![p]),
                };
                store.draft = Some((
                    page,
                    Annotation {
                        id: 0,
                        shape,
                        style: self.draw_style(tool),
                    },
                ));
                self.grab = Some(Grab::Draw { page, start: p });
                Outcome {
                    redraw: true,
                    ..Outcome::default()
                }
            }
        }
    }

    /// Highlights are always translucent yellow-ish; everything else uses the chosen style.
    fn draw_style(&self, tool: Tool) -> Style {
        let mut style = self.style.clone();
        if tool == Tool::Highlight {
            style.fill = Some(style.stroke);
            style.width = 0.0;
        }
        style
    }

    fn press_select(
        &mut self,
        store: &mut Store,
        page: usize,
        p: Point,
        tolerance: f64,
        clicks: u32,
    ) -> Outcome {
        // A grab point of the current selection comes first.
        if let Some(selected) = store
            .selected()
            .cloned()
            .filter(|_| store.selection.is_some_and(|s| s.page == page))
        {
            if let Some(handle) = selected.handle_at(p, tolerance) {
                let id = selected.id;
                self.grab = Some(Grab::Resize {
                    page,
                    id,
                    handle,
                    original: selected,
                });
                return Outcome::default();
            }
        }

        let hit = store.layer(page).and_then(|layer| layer.hit(p, tolerance));
        match hit {
            Some(id) => {
                store.selection = Some(Selection { page, id });
                let original = store
                    .layer(page)
                    .and_then(|l| l.get(id))
                    .cloned()
                    .expect("just hit");
                let is_text = matches!(original.shape, Shape::Text { .. });
                self.grab = Some(Grab::Move {
                    page,
                    id,
                    start: p,
                    original,
                });
                Outcome {
                    redraw: true,
                    edit_text: (is_text && clicks >= 2).then_some((page, id)),
                    ..Outcome::default()
                }
            }
            None => {
                let had_selection = store.selection.take().is_some();
                Outcome {
                    redraw: had_selection,
                    ..Outcome::default()
                }
            }
        }
    }

    fn press_text(&mut self, store: &mut Store, page: usize, p: Point) -> Outcome {
        let height = self.style.font_size * 1.5;
        let bounds = Rect::new(p.x, p.y - height / 2.0, TEXT_WIDTH, height);
        let mut style = self.style.clone();
        style.fill = None;
        let id = store.pages.layer_mut(page).add(
            Shape::Text {
                bounds,
                text: String::new(),
            },
            style,
        );
        store.selection = Some(Selection { page, id });
        self.unconfirmed_text = Some((page, id));
        Outcome {
            redraw: true,
            edit_text: Some((page, id)),
            ..Outcome::default()
        }
    }

    pub fn drag(&mut self, store: &mut Store, p: Point) -> Outcome {
        let Some(grab) = &self.grab else {
            return Outcome::default();
        };
        match grab {
            Grab::Draw { start, .. } => {
                if let Some((_, draft)) = &mut store.draft {
                    match &mut draft.shape {
                        Shape::Rect(r) | Shape::Ellipse(r) => *r = Rect::from_corners(*start, p),
                        Shape::Highlight(rects) => *rects = vec![Rect::from_corners(*start, p)],
                        Shape::Line(_, end) | Shape::Arrow(_, end) => *end = p,
                        Shape::Freehand(points) => {
                            if points
                                .last()
                                .is_none_or(|last| (last.x - p.x).hypot(last.y - p.y) >= 1.0)
                            {
                                points.push(p);
                            }
                        }
                        Shape::Text { .. } => {}
                    }
                }
            }
            Grab::Move {
                page,
                id,
                start,
                original,
            } => {
                let (page, id, dx, dy) = (*page, *id, p.x - start.x, p.y - start.y);
                let original = original.clone();
                if let Some(item) = store
                    .pages
                    .layers
                    .get_mut(&page)
                    .and_then(|l| l.get_mut(id))
                {
                    *item = original;
                    item.translate(dx, dy);
                }
            }
            Grab::Resize {
                page,
                id,
                handle,
                original,
            } => {
                let (page, id, handle) = (*page, *id, *handle);
                let original = original.clone();
                if let Some(item) = store
                    .pages
                    .layers
                    .get_mut(&page)
                    .and_then(|l| l.get_mut(id))
                {
                    item.drag_handle(&original, handle, p);
                }
            }
        }
        Outcome {
            redraw: true,
            ..Outcome::default()
        }
    }

    pub fn release(&mut self, store: &mut Store, p: Point) -> Outcome {
        self.drag(store, p);
        let Some(grab) = self.grab.take() else {
            return Outcome::default();
        };
        match grab {
            Grab::Draw { page, .. } => {
                let Some((_, draft)) = store.draft.take() else {
                    return Outcome::default();
                };
                if !is_substantial(&draft.shape) {
                    return Outcome {
                        redraw: true,
                        ..Outcome::default()
                    };
                }
                let id = store.pages.layer_mut(page).add(draft.shape, draft.style);
                // Drawing something new leaves the pen/tool active, with the new item selected
                // only in the Select tool; here selection would hide the shape under handles.
                store.selection = None;
                let _ = id;
                Outcome {
                    redraw: true,
                    committed: true,
                    ..Outcome::default()
                }
            }
            Grab::Move {
                page, id, start, ..
            } => {
                let moved = (p.x - start.x).hypot(p.y - start.y) >= 0.5;
                let _ = (page, id);
                Outcome {
                    redraw: true,
                    committed: moved,
                    ..Outcome::default()
                }
            }
            Grab::Resize {
                original, page, id, ..
            } => {
                let changed = store
                    .layer(page)
                    .and_then(|l| l.get(id))
                    .is_some_and(|now| *now != original);
                Outcome {
                    redraw: true,
                    committed: changed,
                    ..Outcome::default()
                }
            }
        }
    }

    /// Delete key: remove the selected annotation.
    pub fn delete_selected(&mut self, store: &mut Store) -> Outcome {
        let Some(Selection { page, id }) = store.selection.take() else {
            return Outcome::default();
        };
        let removed = store
            .pages
            .layers
            .get_mut(&page)
            .and_then(|l| l.remove(id))
            .is_some();
        Outcome {
            redraw: true,
            committed: removed,
            ..Outcome::default()
        }
    }

    /// Arrow keys: nudge the selected annotation.
    pub fn nudge(&mut self, store: &mut Store, dx: f64, dy: f64) -> Outcome {
        let Some(Selection { page, id }) = store.selection else {
            return Outcome::default();
        };
        match store
            .pages
            .layers
            .get_mut(&page)
            .and_then(|l| l.get_mut(id))
        {
            Some(item) => {
                item.translate(dx, dy);
                Outcome {
                    redraw: true,
                    committed: true,
                    ..Outcome::default()
                }
            }
            None => Outcome::default(),
        }
    }

    /// The text editor closed with `text`. Empty text removes the annotation.
    pub fn set_text(&mut self, store: &mut Store, page: usize, id: u64, text: &str) -> Outcome {
        let is_new = self.unconfirmed_text == Some((page, id));
        if is_new {
            self.unconfirmed_text = None;
        }
        let Some(layer) = store.pages.layers.get_mut(&page) else {
            return Outcome::default();
        };
        if text.trim().is_empty() {
            let removed = layer.remove(id).is_some();
            if store.selection == Some(Selection { page, id }) {
                store.selection = None;
            }
            // Dropping a text box that never had text leaves no trace in the history.
            return Outcome {
                redraw: true,
                committed: removed && !is_new,
                ..Outcome::default()
            };
        }
        let Some(item) = layer.get_mut(id) else {
            return Outcome::default();
        };
        let changed = match &mut item.shape {
            Shape::Text { text: old, bounds } => {
                let changed = old != text || is_new;
                *old = text.to_owned();
                // Grow the box to fit the longest line and the number of lines.
                let lines: Vec<&str> = text.lines().collect();
                let widest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(1) as f64;
                bounds.w = bounds.w.max(widest * item.style.font_size * 0.6);
                bounds.h = bounds
                    .h
                    .max(lines.len().max(1) as f64 * item.style.font_size * 1.4);
                changed
            }
            _ => false,
        };
        Outcome {
            redraw: true,
            committed: changed,
            ..Outcome::default()
        }
    }

    /// Add a highlight over `rects` at once (for text selected in a PDF).
    pub fn add_highlight(&mut self, store: &mut Store, page: usize, rects: Vec<Rect>) -> Outcome {
        if rects.is_empty() {
            return Outcome::default();
        }
        let style = self.draw_style(Tool::Highlight);
        store
            .pages
            .layer_mut(page)
            .add(Shape::Highlight(rects), style);
        Outcome {
            redraw: true,
            committed: true,
            ..Outcome::default()
        }
    }

    /// Apply the current style to the selected annotation (colour or width changed).
    pub fn restyle_selected(&mut self, store: &mut Store) -> Outcome {
        let Some(Selection { page, id }) = store.selection else {
            return Outcome::default();
        };
        let new = self.style.clone();
        let Some(item) = store
            .pages
            .layers
            .get_mut(&page)
            .and_then(|l| l.get_mut(id))
        else {
            return Outcome::default();
        };
        let before = item.style.clone();
        item.style.stroke = new.stroke;
        if matches!(item.shape, Shape::Highlight(_)) {
            item.style.fill = Some(new.stroke);
        } else {
            item.style.width = new.width;
            item.style.font_size = new.font_size;
        }
        Outcome {
            redraw: true,
            committed: item.style != before,
            ..Outcome::default()
        }
    }
}

fn is_substantial(shape: &Shape) -> bool {
    match shape {
        Shape::Rect(r) | Shape::Ellipse(r) => r.w >= MIN_DRAG && r.h >= MIN_DRAG,
        Shape::Highlight(rects) => rects.iter().any(|r| r.w >= MIN_DRAG && r.h >= MIN_DRAG),
        Shape::Line(a, b) | Shape::Arrow(a, b) => (a.x - b.x).hypot(a.y - b.y) >= MIN_DRAG,
        Shape::Freehand(points) => points.len() >= 2,
        Shape::Text { .. } => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOL: f64 = 4.0;

    fn drag_out(
        editor: &mut Editor,
        store: &mut Store,
        from: (f64, f64),
        to: (f64, f64),
    ) -> Outcome {
        editor.press(store, 0, Point::new(from.0, from.1), TOL, 1);
        editor.drag(
            store,
            Point::new((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0),
        );
        editor.release(store, Point::new(to.0, to.1))
    }

    fn editor(tool: Tool) -> (Editor, Store) {
        (
            Editor {
                tool,
                ..Editor::default()
            },
            Store::default(),
        )
    }

    #[test]
    fn every_drawing_tool_adds_its_shape() {
        for (tool, check) in [
            (
                Tool::Rectangle,
                (|s: &Shape| matches!(s, Shape::Rect(r) if r.w == 100.0 && r.h == 50.0))
                    as fn(&Shape) -> bool,
            ),
            (Tool::Ellipse, |s| matches!(s, Shape::Ellipse(_))),
            (
                Tool::Line,
                |s| matches!(s, Shape::Line(a, b) if a.x == 10.0 && b.x == 110.0),
            ),
            (Tool::Arrow, |s| matches!(s, Shape::Arrow(..))),
            (
                Tool::Highlight,
                |s| matches!(s, Shape::Highlight(r) if r.len() == 1),
            ),
            (
                Tool::Pen,
                |s| matches!(s, Shape::Freehand(p) if p.len() >= 3),
            ),
        ] {
            let (mut editor, mut store) = editor(tool);
            let outcome = drag_out(&mut editor, &mut store, (10.0, 10.0), (110.0, 60.0));
            assert!(outcome.committed, "{tool:?}");
            let layer = store.layer(0).unwrap();
            assert_eq!(layer.items.len(), 1, "{tool:?}");
            assert!(
                check(&layer.items[0].shape),
                "{tool:?}: {:?}",
                layer.items[0].shape
            );
            assert!(store.draft.is_none());
        }
    }

    #[test]
    fn rectangles_can_be_dragged_in_any_direction() {
        let (mut editor, mut store) = editor(Tool::Rectangle);
        drag_out(&mut editor, &mut store, (110.0, 60.0), (10.0, 10.0));
        let Shape::Rect(r) = &store.layer(0).unwrap().items[0].shape else {
            panic!()
        };
        assert_eq!(*r, Rect::new(10.0, 10.0, 100.0, 50.0));
    }

    #[test]
    fn accidental_clicks_draw_nothing() {
        let (mut editor, mut store) = editor(Tool::Rectangle);
        let outcome = drag_out(&mut editor, &mut store, (10.0, 10.0), (11.0, 11.0));
        assert!(!outcome.committed);
        assert!(store.layer(0).is_none_or(AnnotationLayer::is_empty));
        let (mut editor, mut store) = (
            Editor {
                tool: Tool::Pen,
                ..Editor::default()
            },
            Store::default(),
        );
        editor.press(&mut store, 0, Point::new(5.0, 5.0), TOL, 1);
        assert!(
            !editor.release(&mut store, Point::new(5.0, 5.0)).committed,
            "a pen needs a stroke"
        );
    }

    #[test]
    fn select_move_and_resize() {
        let (mut editor, mut store) = editor(Tool::Rectangle);
        drag_out(&mut editor, &mut store, (10.0, 10.0), (110.0, 60.0));
        editor.tool = Tool::Select;

        // Click on the outline selects; dragging moves.
        let outcome = editor.press(&mut store, 0, Point::new(60.0, 10.0), TOL, 1);
        assert!(outcome.redraw && store.selection.is_some());
        editor.drag(&mut store, Point::new(70.0, 30.0));
        let outcome = editor.release(&mut store, Point::new(70.0, 30.0));
        assert!(outcome.committed);
        assert_eq!(
            store.selected().unwrap().bounds(),
            Rect::new(20.0, 30.0, 100.0, 50.0)
        );

        // The bottom-right handle resizes.
        editor.press(&mut store, 0, Point::new(120.0, 80.0), TOL, 1);
        editor.drag(&mut store, Point::new(140.0, 100.0));
        let outcome = editor.release(&mut store, Point::new(140.0, 100.0));
        assert!(outcome.committed);
        assert_eq!(
            store.selected().unwrap().bounds(),
            Rect::new(20.0, 30.0, 120.0, 70.0)
        );
    }

    #[test]
    fn clicking_without_moving_commits_nothing() {
        let (mut editor, mut store) = editor(Tool::Rectangle);
        drag_out(&mut editor, &mut store, (10.0, 10.0), (110.0, 60.0));
        editor.tool = Tool::Select;
        editor.press(&mut store, 0, Point::new(60.0, 10.0), TOL, 1);
        assert!(!editor.release(&mut store, Point::new(60.0, 10.0)).committed);
    }

    #[test]
    fn clicking_empty_space_deselects() {
        let (mut editor, mut store) = editor(Tool::Rectangle);
        drag_out(&mut editor, &mut store, (10.0, 10.0), (110.0, 60.0));
        editor.tool = Tool::Select;
        editor.press(&mut store, 0, Point::new(60.0, 10.0), TOL, 1);
        editor.release(&mut store, Point::new(60.0, 10.0));
        assert!(store.selection.is_some());
        let outcome = editor.press(&mut store, 0, Point::new(300.0, 300.0), TOL, 1);
        assert!(outcome.redraw && store.selection.is_none());
    }

    #[test]
    fn delete_and_nudge_the_selection() {
        let (mut editor, mut store) = editor(Tool::Rectangle);
        drag_out(&mut editor, &mut store, (10.0, 10.0), (110.0, 60.0));
        editor.tool = Tool::Select;
        editor.press(&mut store, 0, Point::new(60.0, 10.0), TOL, 1);
        editor.release(&mut store, Point::new(60.0, 10.0));

        let outcome = editor.nudge(&mut store, 1.0, 0.0);
        assert!(outcome.committed);
        assert_eq!(store.selected().unwrap().bounds().x, 11.0);

        let outcome = editor.delete_selected(&mut store);
        assert!(outcome.committed);
        assert!(store.layer(0).unwrap().is_empty() && store.selection.is_none());
        assert!(
            !editor.delete_selected(&mut store).committed,
            "nothing left to delete"
        );
    }

    #[test]
    fn text_tool_asks_for_text_and_commits_it() {
        let (mut editor, mut store) = editor(Tool::Text);
        let outcome = editor.press(&mut store, 0, Point::new(50.0, 50.0), TOL, 1);
        let (page, id) = outcome.edit_text.expect("opens the editor");
        assert!(
            !outcome.committed,
            "nothing to undo until text is confirmed"
        );

        let outcome = editor.set_text(&mut store, page, id, "Hello\nworld");
        assert!(outcome.committed);
        let Shape::Text { text, bounds } = &store.layer(0).unwrap().items[0].shape else {
            panic!()
        };
        assert_eq!(text, "Hello\nworld");
        assert!(
            bounds.h >= 2.0 * editor.style.font_size,
            "two lines fit: {bounds:?}"
        );
    }

    #[test]
    fn empty_text_leaves_nothing_behind() {
        let (mut editor, mut store) = editor(Tool::Text);
        let (page, id) = editor
            .press(&mut store, 0, Point::new(50.0, 50.0), TOL, 1)
            .edit_text
            .unwrap();
        let outcome = editor.set_text(&mut store, page, id, "   ");
        assert!(
            !outcome.committed,
            "a box that never had text is not an undo step"
        );
        assert!(store.layer(0).unwrap().is_empty() && store.selection.is_none());
    }

    #[test]
    fn double_clicking_text_edits_it_and_emptying_it_deletes_it() {
        let (mut editor, mut store) = editor(Tool::Text);
        let (page, id) = editor
            .press(&mut store, 0, Point::new(50.0, 50.0), TOL, 1)
            .edit_text
            .unwrap();
        editor.set_text(&mut store, page, id, "Note");

        editor.tool = Tool::Select;
        let outcome = editor.press(&mut store, 0, Point::new(60.0, 50.0), TOL, 2);
        assert_eq!(outcome.edit_text, Some((0, id)));
        editor.release(&mut store, Point::new(60.0, 50.0));

        let outcome = editor.set_text(&mut store, 0, id, "");
        assert!(outcome.committed, "removing existing text is undoable");
        assert!(store.layer(0).unwrap().is_empty());
    }

    #[test]
    fn restyling_changes_the_selected_annotation() {
        let (mut editor, mut store) = editor(Tool::Rectangle);
        drag_out(&mut editor, &mut store, (10.0, 10.0), (110.0, 60.0));
        editor.tool = Tool::Select;
        editor.press(&mut store, 0, Point::new(60.0, 10.0), TOL, 1);
        editor.release(&mut store, Point::new(60.0, 10.0));

        editor.style.width = 8.0;
        let outcome = editor.restyle_selected(&mut store);
        assert!(outcome.committed);
        assert_eq!(store.selected().unwrap().style.width, 8.0);
        assert!(
            !editor.restyle_selected(&mut store).committed,
            "same style again changes nothing"
        );
    }

    #[test]
    fn highlights_take_the_colour_as_fill_and_no_outline() {
        let (mut editor, mut store) = editor(Tool::Highlight);
        editor.style.stroke = super::super::model::Color::YELLOW;
        let outcome = editor.add_highlight(
            &mut store,
            2,
            vec![
                Rect::new(0.0, 0.0, 50.0, 10.0),
                Rect::new(0.0, 12.0, 30.0, 10.0),
            ],
        );
        assert!(outcome.committed);
        let item = &store.layer(2).unwrap().items[0];
        assert_eq!(item.style.fill, Some(super::super::model::Color::YELLOW));
        assert_eq!(item.style.width, 0.0);
    }

    #[test]
    fn annotations_stay_on_the_page_they_were_drawn_on() {
        let (mut editor, mut store) = editor(Tool::Rectangle);
        editor.press(&mut store, 3, Point::new(10.0, 10.0), TOL, 1);
        editor.release(&mut store, Point::new(60.0, 60.0));
        assert!(store.layer(3).is_some_and(|l| l.items.len() == 1));
        assert!(store.layer(0).is_none_or(AnnotationLayer::is_empty));
        // Hit-testing on another page finds nothing.
        editor.tool = Tool::Select;
        editor.press(&mut store, 0, Point::new(10.0, 35.0), TOL, 1);
        assert!(store.selection.is_none());
    }
}
