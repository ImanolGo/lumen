//! Non-destructive editing: a list of operations applied to the original pixels.
//!
//! Nothing here touches the file. The stack records what the user did and can
//! undo and redo it; the displayed image is always "original + active
//! operations". Rotating and mirroring never need pixels at all (they only
//! change the EXIF orientation), so they stay instant and can be saved without
//! re-encoding a JPEG.

mod adjust;
pub mod crop;

use std::sync::Arc;

use image::{DynamicImage, imageops::FilterType};

use crate::{annotate::AnnotationLayer, image::Orientation};

pub use adjust::Adjustments;

/// A crop rectangle in pixels of the image as displayed at that point of the stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CropRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EditOp {
    RotateCw,
    RotateCcw,
    FlipH,
    FlipV,
    Crop(CropRect),
    Resize {
        w: u32,
        h: u32,
    },
    Adjust(Adjustments),
    /// The annotations changed. Changes no pixels itself: the layer is kept with the stack entry.
    Annotate,
}

impl EditOp {
    /// Whether the operation only changes the orientation, not the pixels.
    fn is_orientation_only(&self) -> bool {
        matches!(
            self,
            Self::RotateCw | Self::RotateCcw | Self::FlipH | Self::FlipV
        )
    }
}

/// The edits made to one image, with undo and redo.
#[derive(Debug, Default, Clone)]
pub struct EditStack {
    ops: Vec<EditOp>,
    /// For each operation, the annotations as they are after it, if it changed them.
    layers: Vec<Option<Arc<AnnotationLayer>>>,
    /// Number of operations that are applied; the rest can be redone.
    cursor: usize,
}

impl EditStack {
    #[cfg(test)]
    pub fn push(&mut self, op: EditOp) {
        self.push_with_layer(op, None);
    }

    /// Add an operation together with the annotations as they are after it.
    pub fn push_with_layer(&mut self, op: EditOp, layer: Option<Arc<AnnotationLayer>>) {
        self.ops.truncate(self.cursor);
        self.layers.truncate(self.cursor);
        self.ops.push(op);
        self.layers.push(layer);
        self.cursor += 1;
    }

    /// The annotations after the active operations, if any operation set them.
    pub fn current_layer(&self) -> Option<Arc<AnnotationLayer>> {
        self.layers[..self.cursor]
            .iter()
            .rev()
            .find_map(|layer| layer.clone())
    }

    pub fn undo(&mut self) -> bool {
        let can = self.can_undo();
        self.cursor -= usize::from(can);
        can
    }

    pub fn redo(&mut self) -> bool {
        let can = self.can_redo();
        self.cursor += usize::from(can);
        can
    }

    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_redo(&self) -> bool {
        self.cursor < self.ops.len()
    }

    /// The operations currently applied.
    pub fn active(&self) -> &[EditOp] {
        &self.ops[..self.cursor]
    }

    /// Whether there is anything to save.
    pub fn is_dirty(&self) -> bool {
        self.cursor > 0
    }

    /// The orientation after the active operations, if they change no pixels.
    pub fn orientation_only(&self, base: Orientation) -> Option<Orientation> {
        self.active()
            .iter()
            .try_fold(base, |orientation, op| match op {
                EditOp::RotateCw => Some(orientation.rotated_cw()),
                EditOp::RotateCcw => Some(orientation.rotated_ccw()),
                EditOp::FlipH => Some(orientation.flipped_horizontally()),
                EditOp::FlipV => Some(orientation.flipped_vertically()),
                // Annotations are drawn over the image and do not touch its pixels.
                EditOp::Annotate => Some(orientation),
                _ => None,
            })
    }

    /// Whether the active operations change the pixels (and so need a re-encode to save).
    pub fn changes_pixels(&self) -> bool {
        let has_annotations = self.current_layer().is_some_and(|layer| !layer.is_empty());
        has_annotations
            || self
                .active()
                .iter()
                .any(|op| !op.is_orientation_only() && *op != EditOp::Annotate)
    }
}

/// Apply `ops` to the stored pixels of an image whose EXIF orientation is `base`.
/// The result is upright: the orientation is baked in.
pub fn render(stored: &DynamicImage, base: Orientation, ops: &[EditOp]) -> DynamicImage {
    let mut image = stored.clone();
    if let Some(orientation) = image::metadata::Orientation::from_exif(base.exif_value()) {
        image.apply_orientation(orientation);
    }
    for op in ops {
        image = match *op {
            EditOp::RotateCw => image.rotate90(),
            EditOp::RotateCcw => image.rotate270(),
            EditOp::FlipH => image.fliph(),
            EditOp::FlipV => image.flipv(),
            EditOp::Crop(r) => image.crop_imm(r.x, r.y, r.w, r.h),
            EditOp::Resize { w, h } => image.resize_exact(w.max(1), h.max(1), FilterType::Lanczos3),
            EditOp::Adjust(a) => adjust::apply(image, a),
            EditOp::Annotate => image,
        };
    }
    image
}

#[cfg(test)]
mod tests {
    use image::{Rgba, RgbaImage};

    use super::*;

    /// A 3×2 image whose pixels are all different, so any mix-up shows.
    fn sample() -> DynamicImage {
        let mut img = RgbaImage::new(3, 2);
        for (i, pixel) in img.pixels_mut().enumerate() {
            *pixel = Rgba([i as u8 * 40, 255 - i as u8 * 40, i as u8, 255]);
        }
        DynamicImage::ImageRgba8(img)
    }

    fn pixels(img: &DynamicImage) -> (u32, u32, Vec<u8>) {
        (img.width(), img.height(), img.to_rgba8().into_raw())
    }

    fn all_orientations() -> impl Iterator<Item = Orientation> {
        (1..=8).map(Orientation::from_exif)
    }

    #[test]
    fn four_quarter_turns_are_the_identity() {
        for base in all_orientations() {
            let turned = (0..4).fold(base, |o, _| o.rotated_cw());
            assert_eq!(turned, base);
            let turned = (0..4).fold(base, |o, _| o.rotated_ccw());
            assert_eq!(turned, base);
        }
    }

    #[test]
    fn mirroring_twice_and_inverse_turns_cancel() {
        for base in all_orientations() {
            assert_eq!(base.flipped_horizontally().flipped_horizontally(), base);
            assert_eq!(base.flipped_vertically().flipped_vertically(), base);
            assert_eq!(base.rotated_cw().rotated_ccw(), base);
            // Two quarter turns equal both mirrors.
            assert_eq!(
                base.rotated_cw().rotated_cw(),
                base.flipped_horizontally().flipped_vertically()
            );
        }
    }

    /// The orientation algebra must agree with what the `image` crate does to pixels.
    #[test]
    fn orientation_changes_match_pixel_operations() {
        let stored = sample();
        for base in all_orientations() {
            let ops_and_results: [(EditOp, Orientation); 4] = [
                (EditOp::RotateCw, base.rotated_cw()),
                (EditOp::RotateCcw, base.rotated_ccw()),
                (EditOp::FlipH, base.flipped_horizontally()),
                (EditOp::FlipV, base.flipped_vertically()),
            ];
            for (op, new_orientation) in ops_and_results {
                let by_pixels = render(&stored, base, &[op]);
                let by_orientation = render(&stored, new_orientation, &[]);
                assert_eq!(
                    pixels(&by_pixels),
                    pixels(&by_orientation),
                    "{base:?} then {op:?}"
                );
            }
        }
    }

    #[test]
    fn undo_and_redo_walk_the_stack() {
        let mut stack = EditStack::default();
        assert!(!stack.can_undo() && !stack.is_dirty());
        stack.push(EditOp::RotateCw);
        stack.push(EditOp::FlipH);
        assert_eq!(stack.active().len(), 2);
        assert!(stack.undo());
        assert!(stack.redo() && !stack.redo());
        stack.undo();
        stack.undo();
        assert!(!stack.undo() && !stack.is_dirty());
        // A new edit after undoing discards the redo branch.
        stack.redo();
        stack.push(EditOp::FlipV);
        assert!(!stack.can_redo());
        assert_eq!(stack.active(), &[EditOp::RotateCw, EditOp::FlipV]);
    }

    #[test]
    fn annotation_layers_follow_undo_and_redo() {
        use crate::annotate::{Rect, Shape, Style};
        let layer_with = |n: usize| {
            let mut layer = AnnotationLayer::default();
            for i in 0..n {
                layer.add(
                    Shape::Rect(Rect::new(i as f64, 0.0, 1.0, 1.0)),
                    Style::default(),
                );
            }
            Arc::new(layer)
        };
        let mut stack = EditStack::default();
        assert!(stack.current_layer().is_none());
        stack.push_with_layer(EditOp::Annotate, Some(layer_with(1)));
        stack.push(EditOp::RotateCw);
        stack.push_with_layer(EditOp::Annotate, Some(layer_with(2)));
        assert_eq!(stack.current_layer().unwrap().items.len(), 2);
        // A rotation that kept the layer unchanged still sees the earlier snapshot.
        stack.undo();
        assert_eq!(stack.current_layer().unwrap().items.len(), 1);
        stack.undo();
        assert_eq!(stack.current_layer().unwrap().items.len(), 1);
        stack.undo();
        assert!(stack.current_layer().is_none());
        stack.redo();
        stack.redo();
        stack.redo();
        assert_eq!(stack.current_layer().unwrap().items.len(), 2);
        // A new edit after undoing drops the discarded branch's layers too.
        stack.undo();
        stack.push(EditOp::FlipH);
        assert_eq!(stack.current_layer().unwrap().items.len(), 1);
    }

    #[test]
    fn annotations_force_a_re_encode_only_while_they_exist() {
        use crate::annotate::{Rect, Shape, Style};
        let mut with_shape = AnnotationLayer::default();
        with_shape.add(Shape::Rect(Rect::new(0.0, 0.0, 5.0, 5.0)), Style::default());

        let mut stack = EditStack::default();
        stack.push(EditOp::RotateCw);
        assert!(!stack.changes_pixels());
        stack.push_with_layer(EditOp::Annotate, Some(Arc::new(with_shape)));
        assert!(stack.changes_pixels(), "a drawn shape has to be burned in");
        assert!(
            stack.orientation_only(Orientation::NORMAL).is_some(),
            "but the pixels are still the file's"
        );
        // Deleting it again makes the rotation lossless once more.
        stack.push_with_layer(EditOp::Annotate, Some(Arc::new(AnnotationLayer::default())));
        assert!(!stack.changes_pixels());
    }

    #[test]
    fn rotations_alone_never_change_pixels() {
        let mut stack = EditStack::default();
        for op in [EditOp::RotateCw, EditOp::FlipH, EditOp::RotateCcw] {
            stack.push(op);
        }
        assert!(!stack.changes_pixels());
        let base = Orientation::from_exif(6);
        let composed = stack.orientation_only(base).expect("orientation only");
        assert_eq!(
            pixels(&render(&sample(), base, stack.active())),
            pixels(&render(&sample(), composed, &[]))
        );

        stack.push(EditOp::Crop(CropRect {
            x: 0,
            y: 0,
            w: 1,
            h: 1,
        }));
        assert!(stack.changes_pixels() && stack.orientation_only(base).is_none());
    }

    #[test]
    fn crop_and_resize_compose_in_order() {
        let ops = [
            EditOp::Crop(CropRect {
                x: 1,
                y: 0,
                w: 2,
                h: 2,
            }),
            EditOp::Resize { w: 4, h: 4 },
            EditOp::RotateCw,
        ];
        let out = render(&sample(), Orientation::NORMAL, &ops);
        assert_eq!((out.width(), out.height()), (4, 4));
        // A quarter turn swaps width and height, and a stored orientation of 6 already did.
        let turned = render(&sample(), Orientation::from_exif(6), &[EditOp::RotateCw]);
        assert_eq!((turned.width(), turned.height()), (3, 2));
    }
}
