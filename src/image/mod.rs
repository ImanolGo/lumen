//! Image documents: decoded pixels plus where they came from.

mod metadata;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use relm4::gtk::{gdk, gsk, prelude::*};

pub use metadata::Metadata;

#[derive(Debug, Clone)]
pub struct ImageDocument {
    pub path: PathBuf,
    pub texture: gdk::Texture,
    pub orientation: Orientation,
    pub metadata: Metadata,
}

impl ImageDocument {
    pub fn load(path: &Path) -> Result<Self> {
        let texture = gdk::Texture::from_filename(path)
            .with_context(|| format!("failed to decode {}", path.display()))?;
        let metadata = Metadata::read(path);
        Ok(Self {
            path: path.to_owned(),
            texture,
            orientation: metadata.orientation,
            metadata,
        })
    }

    /// Size in pixels as displayed, i.e. after applying the orientation.
    pub fn display_size(&self) -> (i32, i32) {
        let (w, h) = (self.texture.width(), self.texture.height());
        if self.orientation.swaps_axes() {
            (h, w)
        } else {
            (w, h)
        }
    }
}

/// EXIF orientation: how the stored pixels must be transformed to look upright.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Orientation(u8);

impl Orientation {
    pub const NORMAL: Self = Self(1);

    /// Values outside 1..=8 are treated as "normal".
    pub fn from_exif(value: u32) -> Self {
        match value {
            1..=8 => Self(value as u8),
            _ => Self::NORMAL,
        }
    }

    /// The EXIF value, 1 to 8.
    pub fn exif_value(self) -> u8 {
        self.0
    }

    /// The orientation as a 2×2 matrix from stored to displayed coordinates (y down).
    fn matrix(self) -> [[i8; 2]; 2] {
        match self.0 {
            2 => [[-1, 0], [0, 1]],
            3 => [[-1, 0], [0, -1]],
            4 => [[1, 0], [0, -1]],
            5 => [[0, 1], [1, 0]],
            6 => [[0, -1], [1, 0]],
            7 => [[0, -1], [-1, 0]],
            8 => [[0, 1], [-1, 0]],
            _ => [[1, 0], [0, 1]],
        }
    }

    /// The orientation that first applies `self`, then `op` to the displayed result.
    fn then(self, op: [[i8; 2]; 2]) -> Self {
        let m = self.matrix();
        let product = [
            [
                op[0][0] * m[0][0] + op[0][1] * m[1][0],
                op[0][0] * m[0][1] + op[0][1] * m[1][1],
            ],
            [
                op[1][0] * m[0][0] + op[1][1] * m[1][0],
                op[1][0] * m[0][1] + op[1][1] * m[1][1],
            ],
        ];
        (1..=8)
            .map(Self)
            .find(|candidate| candidate.matrix() == product)
            .expect("the eight orientations are closed under rotation and mirroring")
    }

    /// Turn the displayed image a quarter turn clockwise.
    pub fn rotated_cw(self) -> Self {
        self.then([[0, -1], [1, 0]])
    }

    /// Turn the displayed image a quarter turn counter-clockwise.
    pub fn rotated_ccw(self) -> Self {
        self.then([[0, 1], [-1, 0]])
    }

    /// Mirror the displayed image left to right.
    pub fn flipped_horizontally(self) -> Self {
        self.then([[-1, 0], [0, 1]])
    }

    /// Mirror the displayed image top to bottom.
    pub fn flipped_vertically(self) -> Self {
        self.then([[1, 0], [0, -1]])
    }

    /// Whether width and height trade places (the 90° and 270° cases).
    pub fn swaps_axes(self) -> bool {
        self.0 >= 5
    }

    /// Transform from the stored image, centred on the origin, to the displayed one.
    /// Coordinates are y-down, so positive rotations are clockwise.
    pub fn transform(self) -> gsk::Transform {
        let t = gsk::Transform::new();
        match self.0 {
            2 => t.scale(-1.0, 1.0),
            3 => t.rotate(180.0),
            4 => t.scale(1.0, -1.0),
            5 => t.rotate(90.0).scale(1.0, -1.0),
            6 => t.rotate(90.0),
            7 => t.rotate(90.0).scale(-1.0, 1.0),
            8 => t.rotate(270.0),
            _ => t,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where the stored top-left corner ends up for each orientation.
    #[test]
    fn corners_follow_the_exif_spec() {
        // Stored top-left corner of a 2×1 image, centred on the origin.
        let corner = (-1.0_f32, -0.5_f32);
        let expect = [
            (1, (-1.0, -0.5)), // normal: top-left
            (2, (1.0, -0.5)),  // mirrored: top-right
            (3, (1.0, 0.5)),   // rotated 180: bottom-right
            (4, (-1.0, 0.5)),  // flipped vertically: bottom-left
            (5, (-0.5, -1.0)), // transpose: stays top-left
            (6, (0.5, -1.0)),  // 90° CW: top-right
            (7, (0.5, 1.0)),   // transverse: bottom-right
            (8, (-0.5, 1.0)),  // 270° CW: bottom-left
        ];
        for (value, (x, y)) in expect {
            let p = Orientation::from_exif(value)
                .transform()
                .transform_point(&relm4::gtk::graphene::Point::new(corner.0, corner.1));
            assert!(
                (p.x() - x).abs() < 1e-4 && (p.y() - y).abs() < 1e-4,
                "orientation {value}: got ({}, {}), want ({x}, {y})",
                p.x(),
                p.y()
            );
        }
    }

    #[test]
    fn only_quarter_turns_swap_axes() {
        assert!(!Orientation::from_exif(3).swaps_axes());
        assert!(Orientation::from_exif(6).swaps_axes());
        assert_eq!(Orientation::from_exif(42), Orientation::NORMAL);
    }
}
