//! Signatures: drawn once, kept, and stamped onto documents as ink.
//!
//! They are stored in `~/.local/share/lumen/signatures.toml` as strokes scaled to a height
//! of 1, so one can be fitted into any box without distortion.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::annotate::{Point, Rect, Shape};

/// At most this many signatures are kept; adding one more drops the oldest.
const MAX_SIGNATURES: usize = 8;

/// A signature: strokes of (x, y) points, 1 unit tall and `aspect` units wide.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Signature {
    pub strokes: Vec<Vec<[f64; 2]>>,
}

impl Signature {
    /// Normalise strokes drawn in any coordinates; `None` if nothing was drawn.
    pub fn from_strokes(strokes: &[Vec<(f64, f64)>]) -> Option<Self> {
        let points = || strokes.iter().flatten();
        let min_x = points().map(|p| p.0).reduce(f64::min)?;
        let min_y = points().map(|p| p.1).reduce(f64::min)?;
        let max_x = points().map(|p| p.0).reduce(f64::max)?;
        let max_y = points().map(|p| p.1).reduce(f64::max)?;
        // A flat stroke (a line) still needs some height to be scaled by.
        let height = (max_y - min_y).max((max_x - min_x) / 20.0).max(1e-6);
        let strokes = strokes
            .iter()
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.iter()
                    .map(|&(x, y)| [(x - min_x) / height, (y - min_y) / height])
                    .collect()
            })
            .collect();
        Some(Self { strokes })
    }

    /// Width for a height of 1.
    pub fn aspect(&self) -> f64 {
        self.strokes
            .iter()
            .flatten()
            .map(|p| p[0])
            .fold(0.0, f64::max)
            .max(0.1)
    }

    /// The signature as ink, as large as fits in `target` and centred in it.
    pub fn shape_in(&self, target: Rect) -> Shape {
        let aspect = self.aspect();
        let height = target.h.min(target.w / aspect);
        let width = height * aspect;
        let (x, y) = (
            target.x + (target.w - width) / 2.0,
            target.y + (target.h - height) / 2.0,
        );
        Shape::Ink(
            self.strokes
                .iter()
                .map(|s| {
                    s.iter()
                        .map(|p| Point::new(x + p[0] * height, y + p[1] * height))
                        .collect()
                })
                .collect(),
        )
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    signatures: Vec<Signature>,
}

/// The saved signatures, newest first.
pub struct Signatures {
    path: PathBuf,
    pub items: Vec<Signature>,
}

impl Signatures {
    pub fn user() -> Self {
        Self::load(
            relm4::gtk::glib::user_data_dir()
                .join("lumen")
                .join("signatures.toml"),
        )
    }

    pub fn load(path: PathBuf) -> Self {
        let file: File = crate::settings::load(&path);
        Self {
            path,
            items: file.signatures,
        }
    }

    /// Keep `signature` as the newest one.
    pub fn add(&mut self, signature: Signature) {
        self.items.retain(|s| *s != signature);
        self.items.insert(0, signature);
        self.items.truncate(MAX_SIGNATURES);
        self.save();
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.items.len() {
            self.items.remove(index);
            self.save();
        }
    }

    fn save(&self) {
        let file = File {
            signatures: self.items.clone(),
        };
        if let Err(err) = crate::settings::save(&self.path, &file) {
            tracing::warn!("cannot save the signatures: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drawn() -> Vec<Vec<(f64, f64)>> {
        // Two strokes in widget pixels, 300 wide and 100 tall overall.
        vec![
            vec![(100.0, 200.0), (250.0, 150.0), (400.0, 250.0)],
            vec![(150.0, 160.0), (160.0, 180.0)],
        ]
    }

    #[test]
    fn strokes_are_scaled_to_a_height_of_one() {
        let s = Signature::from_strokes(&drawn()).unwrap();
        assert!((s.aspect() - 3.0).abs() < 1e-9);
        assert_eq!(s.strokes[0][0], [0.0, 0.5]);
        assert_eq!(s.strokes[0][2], [3.0, 1.0]);
        assert!(Signature::from_strokes(&[]).is_none());
    }

    #[test]
    fn a_signature_fits_its_box_without_distortion() {
        let s = Signature::from_strokes(&drawn()).unwrap();
        // A tall, narrow box: the width decides, and the signature is centred vertically.
        let shape = s.shape_in(Rect::new(10.0, 20.0, 60.0, 100.0));
        let Shape::Ink(strokes) = &shape else {
            panic!("ink")
        };
        let ys: Vec<f64> = strokes.iter().flatten().map(|p| p.y).collect();
        let xs: Vec<f64> = strokes.iter().flatten().map(|p| p.x).collect();
        let (top, bottom) = (
            ys.iter().cloned().fold(f64::MAX, f64::min),
            ys.iter().cloned().fold(f64::MIN, f64::max),
        );
        assert!((xs.iter().cloned().fold(f64::MIN, f64::max) - 70.0).abs() < 1e-9);
        assert!((bottom - top - 20.0).abs() < 1e-9);
        assert!(((top + bottom) / 2.0 - 70.0).abs() < 1e-9);
    }

    #[test]
    fn newest_first_and_saved() {
        let dir = std::env::temp_dir().join(format!("lumen-signatures-{}", std::process::id()));
        let path = dir.join("signatures.toml");
        let mut store = Signatures::load(path.clone());
        let a = Signature::from_strokes(&drawn()).unwrap();
        let b = Signature::from_strokes(&[vec![(0.0, 0.0), (10.0, 5.0)]]).unwrap();
        store.add(a.clone());
        store.add(b.clone());
        store.add(a.clone());
        assert_eq!(store.items, vec![a.clone(), b.clone()]);
        assert_eq!(Signatures::load(path.clone()).items, vec![a, b.clone()]);
        store.remove(0);
        assert_eq!(Signatures::load(path).items, vec![b]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
