//! Page operations as plans: which pages the edited document is made of, in
//! which order, and how far each is turned.
//!
//! Pure functions, tested without any PDF. `compose` turns a plan into a file.

use std::collections::BTreeMap;

use crate::annotate::{AnnotationLayer, GeometryChange};

/// One page of the new document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// 0 is the document being edited; 1 and up are documents that were inserted.
    pub source: usize,
    /// Page of that document, counting from 0.
    pub index: usize,
    /// Degrees to turn the page clockwise, a multiple of 90 in `0..360`.
    pub rotate: i32,
}

impl Entry {
    fn original(index: usize) -> Self {
        Self { source: 0, index, rotate: 0 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageOp {
    /// Turn pages by `degrees` (a multiple of 90, negative for counter-clockwise).
    Rotate { pages: Vec<usize>, degrees: i32 },
    Delete(Vec<usize>),
    /// Move pages (keeping their order) in front of the page that is now at `before`;
    /// `before` equal to the page count means the end.
    Move { pages: Vec<usize>, before: usize },
    /// Put `count` pages of inserted document `source` in front of page `at`.
    Insert { at: usize, source: usize, count: usize },
}

#[derive(Debug, PartialEq, Eq)]
pub enum PlanError {
    /// The operation would leave no pages.
    EmptyResult,
    /// A page number is out of range.
    NoSuchPage(usize),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyResult => write!(f, "a document needs at least one page"),
            Self::NoSuchPage(p) => write!(f, "page {} does not exist", p + 1),
        }
    }
}

impl std::error::Error for PlanError {}

/// The pages of the new document after `op`, for a document of `n_pages`.
pub fn plan(op: &PageOp, n_pages: usize) -> Result<Vec<Entry>, PlanError> {
    let identity = || (0..n_pages).map(Entry::original).collect::<Vec<_>>();
    let check = |pages: &[usize]| pages.iter().try_for_each(|&p| if p < n_pages { Ok(()) } else { Err(PlanError::NoSuchPage(p)) });

    match op {
        PageOp::Rotate { pages, degrees } => {
            check(pages)?;
            let mut entries = identity();
            for &page in pages {
                let e = &mut entries[page];
                e.rotate = (e.rotate + degrees).rem_euclid(360);
            }
            Ok(entries)
        }
        PageOp::Delete(pages) => {
            check(pages)?;
            let entries: Vec<Entry> = identity().into_iter().filter(|e| !pages.contains(&e.index)).collect();
            if entries.is_empty() { Err(PlanError::EmptyResult) } else { Ok(entries) }
        }
        PageOp::Move { pages, before } => {
            check(pages)?;
            if *before > n_pages {
                return Err(PlanError::NoSuchPage(*before));
            }
            let moved: Vec<Entry> = identity().into_iter().filter(|e| pages.contains(&e.index)).collect();
            let mut rest: Vec<Entry> = identity().into_iter().filter(|e| !pages.contains(&e.index)).collect();
            // Where the target page sits once the moved ones are out of the way.
            let position = rest.iter().position(|e| e.index >= *before).unwrap_or(rest.len());
            rest.splice(position..position, moved);
            Ok(rest)
        }
        PageOp::Insert { at, source, count } => {
            if *at > n_pages {
                return Err(PlanError::NoSuchPage(*at));
            }
            let mut entries = identity();
            let new = (0..*count).map(|index| Entry { source: *source, index, rotate: 0 });
            entries.splice(*at..*at, new);
            Ok(entries)
        }
    }
}

/// Annotation layers by page, as the editor keeps them.
pub type Layers = BTreeMap<usize, AnnotationLayer>;

/// Carry the annotations along to the new page numbers, turning them with their pages.
/// `sizes` are the sizes of the pages *before* the operation, in points.
pub fn remap_layers(layers: &Layers, plan: &[Entry], sizes: &[(f64, f64)]) -> Layers {
    let mut moved = Layers::new();
    for (new_index, entry) in plan.iter().enumerate() {
        if entry.source != 0 {
            continue;
        }
        let Some(layer) = layers.get(&entry.index) else { continue };
        let mut layer = layer.clone();
        let (mut w, mut h) = sizes.get(entry.index).copied().unwrap_or((0.0, 0.0));
        for _ in 0..entry.rotate / 90 {
            layer.apply(GeometryChange::RotateCw { height: h });
            (w, h) = (h, w);
        }
        let _ = (w, h);
        if !layer.is_empty() {
            moved.insert(new_index, layer);
        }
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotate::{Rect, Shape, Style};

    fn indexes(entries: &[Entry]) -> Vec<usize> {
        entries.iter().map(|e| e.index).collect()
    }

    #[test]
    fn rotating_turns_only_the_chosen_pages_and_wraps_around() {
        let entries = plan(&PageOp::Rotate { pages: vec![1, 3], degrees: 90 }, 5).unwrap();
        assert_eq!(entries.iter().map(|e| e.rotate).collect::<Vec<_>>(), [0, 90, 0, 90, 0]);
        assert_eq!(indexes(&entries), [0, 1, 2, 3, 4]);
        // Counter-clockwise becomes 270, and four turns are a full circle.
        let entries = plan(&PageOp::Rotate { pages: vec![0], degrees: -90 }, 2).unwrap();
        assert_eq!(entries[0].rotate, 270);
        let mut e = Entry::original(0);
        for _ in 0..4 {
            e.rotate = (e.rotate + 90).rem_euclid(360);
        }
        assert_eq!(e.rotate, 0);
    }

    #[test]
    fn deleting_removes_pages_but_never_all_of_them() {
        let entries = plan(&PageOp::Delete(vec![0, 2]), 4).unwrap();
        assert_eq!(indexes(&entries), [1, 3]);
        // Duplicates in the selection do no harm.
        assert_eq!(indexes(&plan(&PageOp::Delete(vec![1, 1]), 3).unwrap()), [0, 2]);
        assert_eq!(plan(&PageOp::Delete(vec![0, 1]), 2), Err(PlanError::EmptyResult));
        assert_eq!(plan(&PageOp::Delete(vec![5]), 2), Err(PlanError::NoSuchPage(5)));
    }

    #[test]
    fn moving_a_block_keeps_its_order() {
        // Move pages 0 and 1 to the end.
        let entries = plan(&PageOp::Move { pages: vec![0, 1], before: 5 }, 5).unwrap();
        assert_eq!(indexes(&entries), [2, 3, 4, 0, 1]);
        // Move page 4 to the front.
        assert_eq!(indexes(&plan(&PageOp::Move { pages: vec![4], before: 0 }, 5).unwrap()), [4, 0, 1, 2, 3]);
        // Move 0 and 3 in front of page 2.
        assert_eq!(indexes(&plan(&PageOp::Move { pages: vec![0, 3], before: 2 }, 5).unwrap()), [1, 0, 3, 2, 4]);
        // Dropping a block in front of one of its own pages leaves things as they were.
        assert_eq!(indexes(&plan(&PageOp::Move { pages: vec![2], before: 2 }, 4).unwrap()), [0, 1, 2, 3]);
        assert_eq!(indexes(&plan(&PageOp::Move { pages: vec![2], before: 3 }, 4).unwrap()), [0, 1, 2, 3]);
        assert_eq!(plan(&PageOp::Move { pages: vec![0], before: 9 }, 3), Err(PlanError::NoSuchPage(9)));
    }

    #[test]
    fn moving_keeps_each_pages_rotation_out_of_the_plan() {
        // A move never turns anything.
        let entries = plan(&PageOp::Move { pages: vec![1], before: 0 }, 3).unwrap();
        assert!(entries.iter().all(|e| e.rotate == 0 && e.source == 0));
    }

    #[test]
    fn inserting_puts_foreign_pages_in_place() {
        let entries = plan(&PageOp::Insert { at: 1, source: 1, count: 2 }, 3).unwrap();
        let shape: Vec<(usize, usize)> = entries.iter().map(|e| (e.source, e.index)).collect();
        assert_eq!(shape, [(0, 0), (1, 0), (1, 1), (0, 1), (0, 2)]);
        // At the end and at the start.
        assert_eq!(plan(&PageOp::Insert { at: 3, source: 1, count: 1 }, 3).unwrap().last().unwrap().source, 1);
        assert_eq!(plan(&PageOp::Insert { at: 0, source: 2, count: 1 }, 3).unwrap()[0].source, 2);
        assert_eq!(plan(&PageOp::Insert { at: 4, source: 1, count: 1 }, 3), Err(PlanError::NoSuchPage(4)));
    }

    fn layer_with_dot(x: f64, y: f64) -> AnnotationLayer {
        let mut layer = AnnotationLayer::default();
        layer.add(Shape::Rect(Rect::new(x, y, 10.0, 10.0)), Style::default());
        layer
    }

    #[test]
    fn annotations_follow_their_pages() {
        let layers = Layers::from([(0, layer_with_dot(5.0, 5.0)), (2, layer_with_dot(7.0, 7.0))]);
        let sizes = [(100.0, 50.0); 3];

        // Reordered to [2, 0, 1]: the annotations of old pages 2 and 0 are now on pages 0 and 1.
        let entries = plan(&PageOp::Move { pages: vec![2], before: 0 }, 3).unwrap();
        let moved = remap_layers(&layers, &entries, &sizes);
        assert_eq!(moved.keys().copied().collect::<Vec<_>>(), [0, 1]);
        assert_eq!(moved[&0].items[0].bounds().x, 7.0);
        assert_eq!(moved[&1].items[0].bounds().x, 5.0);

        // Deleted pages lose their annotations; the others renumber.
        let entries = plan(&PageOp::Delete(vec![0]), 3).unwrap();
        let moved = remap_layers(&layers, &entries, &sizes);
        assert_eq!(moved.keys().copied().collect::<Vec<_>>(), [1]);

        // Inserted pages start empty and push the rest down.
        let entries = plan(&PageOp::Insert { at: 0, source: 1, count: 1 }, 3).unwrap();
        let moved = remap_layers(&layers, &entries, &sizes);
        assert_eq!(moved.keys().copied().collect::<Vec<_>>(), [1, 3]);
    }

    #[test]
    fn annotations_turn_with_a_rotated_page() {
        // A 100×50 page turned clockwise becomes 50×100; a mark near its top-left goes to the top-right.
        let layers = Layers::from([(0, layer_with_dot(10.0, 5.0))]);
        let entries = plan(&PageOp::Rotate { pages: vec![0], degrees: 90 }, 1).unwrap();
        let turned = remap_layers(&layers, &entries, &[(100.0, 50.0)]);
        let b = turned[&0].items[0].bounds();
        assert_eq!((b.x, b.y, b.w, b.h), (35.0, 10.0, 10.0, 10.0));
    }
}
