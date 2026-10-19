//! Markup: drawing shapes, arrows, text and highlights over images and PDF pages.

pub mod editor;
pub mod model;
pub mod render;

pub use editor::{Editor, Outcome, Store, Tool};
pub use model::{
    Annotation, AnnotationLayer, Color, GeometryChange, History, Point, Rect, Shape, Style,
};
