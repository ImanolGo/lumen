//! Picture editing: turning, mirroring, cropping and colour adjustments.
//!
//! Non-destructive: the decoded picture is kept as it was opened, and what is
//! shown (and saved) is always that picture with the current [`Edits`] applied.
//! An edit also says how it moves the picture's pixels, so markup drawn on the
//! picture can follow it, and undo can move it back.

pub mod adjust;
pub mod crop;
pub mod select;
pub mod straighten;

use std::sync::Arc;

use image::{DynamicImage, RgbaImage, imageops};

pub use adjust::Adjustments;

use crate::annotate::GeometryChange;

/// A crop rectangle in pixels of the turned and mirrored picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CropRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Everything done to a picture, in the order it is applied: quarter turns clockwise,
/// then a horizontal mirror, then the crop, then the new size, then the colours.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Edits {
    pub turns: u8,
    pub flip: bool,
    pub crop: Option<CropRect>,
    /// Width and height after cropping, when the picture was resized.
    pub resize: Option<(u32, u32)>,
    pub adjust: Adjustments,
}

/// Largest side a picture can be resized to.
pub const MAX_SIDE: u32 = 16_384;

/// One edit: the new state, and how it moved what was on the picture (and back).
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub before: Edits,
    pub after: Edits,
    pub forward: Vec<GeometryChange>,
    pub back: Vec<GeometryChange>,
    /// Pixels changed outright (a part cleared, say): the picture the edits start from, before and after.
    pub baked: Option<Arc<Baked>>,
}

/// A new picture to start from, which undo can take back.
#[derive(Debug)]
pub struct Baked {
    /// Tells it from every other.
    pub id: u64,
    pub before: crate::raster::Picture,
    pub after: crate::raster::Picture,
}

impl PartialEq for Baked {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Step {
    /// The picture as shown becomes `after`, to start from with no edits.
    pub fn bake(edits: Edits, before: crate::raster::Picture, after: crate::raster::Picture) -> Self {
        static IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self {
            before: edits,
            after: Edits::default(),
            forward: Vec::new(),
            back: Vec::new(),
            baked: Some(Arc::new(Baked { id, before, after })),
        }
    }
}

fn inverse(change: GeometryChange, before: (f64, f64)) -> GeometryChange {
    match change {
        // A clockwise turn of a picture `h` tall leaves it `h` wide; turning back uses that width.
        GeometryChange::RotateCw { .. } => GeometryChange::RotateCcw { width: before.1 },
        GeometryChange::RotateCcw { .. } => GeometryChange::RotateCw { height: before.0 },
        GeometryChange::Crop { x, y } => GeometryChange::Crop { x: -x, y: -y },
        GeometryChange::Scale { sx, sy } => GeometryChange::Scale { sx: 1.0 / sx, sy: 1.0 / sy },
        same => same,
    }
}

impl Edits {
    pub fn is_none(&self) -> bool {
        *self == Self::default()
    }

    /// Size of the picture after turning and mirroring, before cropping.
    fn oriented(&self, base: (u32, u32)) -> (u32, u32) {
        if self.turns % 2 == 1 { (base.1, base.0) } else { base }
    }

    /// Size of the edited picture before resizing.
    pub fn unscaled(&self, base: (u32, u32)) -> (u32, u32) {
        match self.crop {
            Some(c) => (c.w, c.h),
            None => self.oriented(base),
        }
    }

    /// Size of the edited picture.
    pub fn size(&self, base: (u32, u32)) -> (u32, u32) {
        self.resize.unwrap_or_else(|| self.unscaled(base))
    }

    fn step(self, base: (u32, u32), after: Edits, forward: Vec<GeometryChange>) -> Step {
        // Undoing runs the changes backwards, each measured on the picture as it was after it.
        let mut sizes = vec![self.size(base)];
        for change in &forward {
            let (w, h) = *sizes.last().unwrap_or(&(0, 0));
            sizes.push(match change {
                GeometryChange::RotateCw { .. } | GeometryChange::RotateCcw { .. } => (h, w),
                _ => (w, h),
            });
        }
        let back = forward
            .iter()
            .enumerate()
            .rev()
            .map(|(i, c)| {
                let (w, h) = sizes[i];
                inverse(*c, (w as f64, h as f64))
            })
            .collect();
        Step { before: self, after, forward, back, baked: None }
    }

    /// Turn the shown picture a quarter clockwise.
    pub fn rotate_cw(self, base: (u32, u32)) -> Step {
        let (_, oh) = self.oriented(base);
        let (_, sh) = self.size(base);
        let mut after = self;
        // Turning after a mirror is the mirror after turning the other way.
        after.turns = if self.flip { (self.turns + 3) % 4 } else { (self.turns + 1) % 4 };
        after.crop = self.crop.map(|c| CropRect { x: oh - c.y - c.h, y: c.x, w: c.h, h: c.w });
        after.resize = self.resize.map(|(w, h)| (h, w));
        self.step(base, after, vec![GeometryChange::RotateCw { height: sh as f64 }])
    }

    pub fn rotate_ccw(self, base: (u32, u32)) -> Step {
        let s1 = self.rotate_cw(base);
        let s2 = s1.after.rotate_cw(base);
        let s3 = s2.after.rotate_cw(base);
        let forward = [s1.forward, s2.forward, s3.forward].concat();
        self.step(base, s3.after, forward)
    }

    /// Mirror the shown picture left to right.
    pub fn flip_h(self, base: (u32, u32)) -> Step {
        let (ow, _) = self.oriented(base);
        let (sw, _) = self.size(base);
        let mut after = self;
        after.flip = !self.flip;
        after.crop = self.crop.map(|c| CropRect { x: ow - c.x - c.w, ..c });
        self.step(base, after, vec![GeometryChange::FlipH { width: sw as f64 }])
    }

    /// Mirror the shown picture top to bottom: a left-right mirror turned half way.
    pub fn flip_v(self, base: (u32, u32)) -> Step {
        let s1 = self.flip_h(base);
        let s2 = s1.after.rotate_cw(base);
        let s3 = s2.after.rotate_cw(base);
        let forward = [s1.forward, s2.forward, s3.forward].concat();
        self.step(base, s3.after, forward)
    }

    /// Crop to `rect`, given in pixels of the shown picture.
    pub fn crop(self, base: (u32, u32), rect: CropRect) -> Step {
        let (x0, y0) = self.crop.map_or((0, 0), |c| (c.x, c.y));
        let mut after = self;
        // On a resized picture the crop is taken from the full-size one, and the
        // result keeps the same scale.
        let (uw, uh) = self.unscaled(base);
        let (sw, sh) = self.size(base);
        let (fx, fy) = (uw as f64 / sw.max(1) as f64, uh as f64 / sh.max(1) as f64);
        let scaled = |v: u32, f: f64| (v as f64 * f).round() as u32;
        let full = CropRect {
            x: scaled(rect.x, fx),
            y: scaled(rect.y, fy),
            w: scaled(rect.w, fx).max(1),
            h: scaled(rect.h, fy).max(1),
        };
        let full = CropRect { w: full.w.min(uw - full.x.min(uw - 1)), h: full.h.min(uh - full.y.min(uh - 1)), ..full };
        after.crop = Some(CropRect { x: x0 + full.x, y: y0 + full.y, ..full });
        after.resize = self.resize.map(|_| (rect.w, rect.h));
        self.step(base, after, vec![GeometryChange::Crop { x: rect.x as f64, y: rect.y as f64 }])
    }

    /// Resize the picture to `width`×`height` (going back to the full size when that is it).
    pub fn resized(self, base: (u32, u32), width: u32, height: u32) -> Step {
        let (width, height) = (width.clamp(1, MAX_SIDE), height.clamp(1, MAX_SIDE));
        let (sw, sh) = self.size(base);
        let mut after = self;
        after.resize = (self.unscaled(base) != (width, height)).then_some((width, height));
        let change = GeometryChange::Scale { sx: width as f64 / sw as f64, sy: height as f64 / sh as f64 };
        self.step(base, after, vec![change])
    }

    pub fn adjusted(self, base: (u32, u32), adjust: Adjustments) -> Step {
        let mut after = self;
        after.adjust = adjust;
        self.step(base, after, Vec::new())
    }

    /// Back to the picture as it was opened.
    pub fn reset(self, base: (u32, u32)) -> Step {
        // Undo the size, then the crop, then the mirror, then the turns.
        let mut forward = Vec::new();
        if let Some((rw, rh)) = self.resize {
            let (uw, uh) = self.unscaled(base);
            forward.push(GeometryChange::Scale { sx: uw as f64 / rw as f64, sy: uh as f64 / rh as f64 });
        }
        if let Some(c) = self.crop {
            forward.push(GeometryChange::Crop { x: -(c.x as f64), y: -(c.y as f64) });
        }
        let mut now = Edits { crop: None, resize: None, ..self };
        if now.flip {
            let s = now.flip_h(base);
            forward.extend(s.forward);
            now = s.after;
        }
        while now.turns != 0 {
            let s = now.rotate_cw(base);
            forward.extend(s.forward);
            now = s.after;
        }
        self.step(base, Edits::default(), forward)
    }
}

/// `base` with `edits` applied.
pub fn apply(base: &RgbaImage, edits: &Edits) -> RgbaImage {
    let turned = match edits.turns % 4 {
        1 => imageops::rotate90(base),
        2 => imageops::rotate180(base),
        3 => imageops::rotate270(base),
        _ => base.clone(),
    };
    let mirrored = if edits.flip { imageops::flip_horizontal(&turned) } else { turned };
    let cropped = match edits.crop {
        Some(c) if c.x + c.w <= mirrored.width() && c.y + c.h <= mirrored.height() => {
            imageops::crop_imm(&mirrored, c.x, c.y, c.w.max(1), c.h.max(1)).to_image()
        }
        _ => mirrored,
    };
    let sized = match edits.resize {
        Some((w, h)) if (w, h) != cropped.dimensions() => {
            imageops::resize(&cropped, w.clamp(1, MAX_SIDE), h.clamp(1, MAX_SIDE), imageops::FilterType::CatmullRom)
        }
        _ => cropped,
    };
    if edits.adjust.is_neutral() {
        sized
    } else {
        adjust::apply(DynamicImage::ImageRgba8(sized), edits.adjust).into_rgba8()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotate::model::{Annotation, Point, Shape, Style};

    /// A 4×2 picture whose pixels say where they came from.
    fn picture() -> RgbaImage {
        RgbaImage::from_fn(4, 2, |x, y| image::Rgba([x as u8, y as u8, 0, 255]))
    }

    fn origin(img: &RgbaImage, x: u32, y: u32) -> (u8, u8) {
        let p = img.get_pixel(x, y);
        (p[0], p[1])
    }

    /// A dot on the source pixel (`x`, `y`), carried along by `steps`.
    fn follow(steps: &[&Step], x: f64, y: f64) -> (f64, f64) {
        let mut a = Annotation { id: 1, shape: Shape::Freehand(vec![Point::new(x, y)]), style: Style::default() };
        for s in steps {
            s.forward.iter().for_each(|c| a.apply(*c));
        }
        let Shape::Freehand(p) = &a.shape else { unreachable!() };
        (p[0].x, p[0].y)
    }

    #[test]
    fn every_edit_moves_markup_with_the_pixels_and_back() {
        let base = picture();
        let size = base.dimensions();
        let start = Edits::default();
        let r = start.rotate_cw(size);
        let f = r.after.flip_h(size);
        let c = f.after.crop(size, CropRect { x: 0, y: 1, w: 2, h: 3 });
        let v = c.after.flip_v(size);
        let k = v.after.rotate_ccw(size);
        let steps = [&r, &f, &c, &v, &k];
        let shown = apply(&base, &k.after);
        assert_eq!(shown.dimensions(), k.after.size(size));
        // Wherever a pixel ends up, a dot drawn on it (at its centre) ends up there too.
        for (sx, sy) in [(0, 0), (1, 1), (2, 0), (3, 1)] {
            let (dx, dy) = follow(&steps, sx as f64 + 0.5, sy as f64 + 0.5);
            if dx < 0.0 || dy < 0.0 || dx >= shown.width() as f64 || dy >= shown.height() as f64 {
                continue; // cropped away
            }
            assert_eq!(origin(&shown, dx as u32, dy as u32), (sx, sy), "pixel {sx},{sy} went to {dx},{dy}");
        }
        // Undoing every step brings the dot home.
        let mut a = Annotation { id: 1, shape: Shape::Freehand(vec![Point::new(1.5, 0.5)]), style: Style::default() };
        for s in steps {
            s.forward.iter().for_each(|c| a.apply(*c));
        }
        for s in steps.iter().rev() {
            s.back.iter().for_each(|c| a.apply(*c));
        }
        let Shape::Freehand(p) = &a.shape else { unreachable!() };
        assert!((p[0].x - 1.5).abs() < 1e-9 && (p[0].y - 0.5).abs() < 1e-9, "{:?}", p[0]);
        // And resetting is the picture as it was.
        let reset = k.after.reset(size);
        assert_eq!(apply(&base, &reset.after), base);
        assert_eq!(follow(&[&r, &f, &c, &v, &k, &reset], 2.5, 1.5), (2.5, 1.5));
    }

    #[test]
    fn resizing_scales_markup_and_later_crops_keep_the_scale() {
        let base = RgbaImage::from_fn(400, 200, |x, y| image::Rgba([(x / 2) as u8, y as u8, 0, 255]));
        let size = base.dimensions();
        let half = Edits::default().resized(size, 200, 100);
        assert_eq!(half.after.size(size), (200, 100));
        assert_eq!(follow(&[&half], 100.0, 50.0), (50.0, 25.0));
        // A crop of the half-size picture is taken from the full one and stays half size.
        let crop = half.after.crop(size, CropRect { x: 50, y: 25, w: 100, h: 50 });
        assert_eq!(crop.after.crop, Some(CropRect { x: 100, y: 50, w: 200, h: 100 }));
        assert_eq!(crop.after.size(size), (100, 50));
        let turned = crop.after.rotate_cw(size);
        assert_eq!(turned.after.size(size), (50, 100));
        let shown = apply(&base, &turned.after);
        assert_eq!(shown.dimensions(), (50, 100));
        // A dot on source pixel (240, 120) is at half size, cropped, then turned.
        let (x, y) = follow(&[&half, &crop, &turned], 240.0, 120.0);
        assert!((x - (50.0 - 35.0)).abs() < 1e-9 && (y - 70.0).abs() < 1e-9, "{x},{y}");
        // Back to the original, markup included.
        let reset = turned.after.reset(size);
        assert_eq!(apply(&base, &reset.after), base);
        let (x, y) = follow(&[&half, &crop, &turned, &reset], 240.0, 120.0);
        assert!((x - 240.0).abs() < 1e-9 && (y - 120.0).abs() < 1e-9, "{x},{y}");
        // Resizing back to the full size is no resize at all.
        assert_eq!(half.after.resized(size, 400, 200).after.resize, None);
    }

    #[test]
    fn crops_add_up() {
        let size = (100, 50);
        let a = Edits::default().crop(size, CropRect { x: 10, y: 5, w: 50, h: 30 });
        let b = a.after.crop(size, CropRect { x: 4, y: 2, w: 20, h: 10 });
        assert_eq!(b.after.crop, Some(CropRect { x: 14, y: 7, w: 20, h: 10 }));
        assert_eq!(b.after.size(size), (20, 10));
    }
}
