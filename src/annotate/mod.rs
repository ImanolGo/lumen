//! Markup: drawing shapes, arrows, text and highlights over images and PDF pages.

pub mod editor;
pub mod model;
pub mod render;

pub use editor::{Editor, Outcome, Selection, Store, Tool};
pub use model::{
    Annotation, AnnotationLayer, Color, GeometryChange, Handle, History, Pages, Point, Rect, Shape,
    Style,
};
