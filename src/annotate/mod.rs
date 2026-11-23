//! Markup: shapes, arrows, freehand ink, text and highlights drawn over pictures and PDF pages.
//!
//! The model and the editing logic come from the GTK Lumen and are plain Rust,
//! with no UI code, so they are unit-tested on their own. `paint` turns an
//! annotation into paths that every output shares: the screen overlay, the
//! appearance streams written into PDFs, and pictures saved with their markup.

// Ported from the GTK Lumen with their tests; a few operations (flips, crops, placing a
// signature) have no use in this edition yet.
#[allow(dead_code)]
pub mod editor;
pub mod flatten;
#[allow(dead_code)]
pub mod model;
pub mod paint;
pub mod pdf;

pub use editor::{Editor, Outcome, Store, Tool};
pub use model::{AnnotationLayer, Color, GeometryChange, History, Point, Rect, Shape};
