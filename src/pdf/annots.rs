//! Reading and writing real PDF annotations.
//!
//! The safe Poppler bindings cannot create most annotation kinds, so this file
//! uses the C API directly, in one place. Every pointer is checked and every
//! owned object released here. Page coordinates inside the model have y pointing
//! down; PDF has y pointing up, so every point is flipped with the page height.
//!
//! Supported kinds, which Lumen draws and edits itself: square, circle, ink,
//! free text and highlight. Lines and arrows are written as ink (Poppler has no
//! line-ending API), tagged in their contents so they come back as lines and
//! arrows. Other annotation kinds are left alone and keep being drawn by Poppler.

use std::{
    collections::BTreeMap,
    ffi::{CStr, CString, c_char},
    path::Path,
    ptr,
};

use anyhow::{Context, Result, bail};
use poppler::{Document, Page, ffi};
use relm4::gtk::glib::{
    self,
    translate::{ToGlibPtr, from_glib_full},
};

use super::{PdfSource, open_document, poppler_lock};
use crate::annotate::{
    Annotation, AnnotationLayer, Color, Point, Rect, Shape, Style, model::arrow_head,
};

const ARROW_TAG: &str = "lumen:arrow";
const LINE_TAG: &str = "lumen:line";
const DEFAULT_FONT_SIZE: f64 = 12.0;

fn is_supported(kind: ffi::PopplerAnnotType) -> bool {
    matches!(
        kind,
        ffi::POPPLER_ANNOT_SQUARE
            | ffi::POPPLER_ANNOT_CIRCLE
            | ffi::POPPLER_ANNOT_INK
            | ffi::POPPLER_ANNOT_FREE_TEXT
            | ffi::POPPLER_ANNOT_HIGHLIGHT
    )
}

/// The annotations of `page` that Lumen takes over, as raw owned references.
fn supported_annots(page: &Page) -> Vec<*mut ffi::PopplerAnnot> {
    page.annot_mapping()
        .iter()
        .filter_map(|mapping| {
            let raw: *const ffi::PopplerAnnotMapping = mapping.to_glib_none().0;
            // SAFETY: `raw` points into `mapping`, which lives for this closure; the
            // annotation object it holds is alive as long as the mapping is.
            let annot = unsafe { (*raw).annot };
            // SAFETY: a mapping's annotation pointer is a valid PopplerAnnot.
            let kind = unsafe { ffi::poppler_annot_get_annot_type(annot) };
            if !is_supported(kind) {
                return None;
            }
            // Keep it alive past the mapping.
            // SAFETY: `annot` is a live GObject.
            unsafe { glib::gobject_ffi::g_object_ref(annot as *mut _) };
            Some(annot)
        })
        .collect()
}

/// Stop Poppler from drawing the annotations Lumen draws itself (in this in-memory copy only).
pub fn hide_supported(page: &Page) {
    for annot in supported_annots(page) {
        // SAFETY: `annot` is a live annotation of `page`; our extra reference is released.
        unsafe {
            ffi::poppler_page_remove_annot(page.to_glib_none().0, annot);
            glib::gobject_ffi::g_object_unref(annot as *mut _);
        }
    }
}

// --- reading -------------------------------------------------------------------

/// Every supported annotation of the document, as editable layers.
pub fn read_all(doc: &Document, page_sizes: &[(f64, f64)]) -> BTreeMap<usize, AnnotationLayer> {
    let mut layers = BTreeMap::new();
    for (index, &(_, height)) in page_sizes.iter().enumerate() {
        let Some(page) = doc.page(index as i32) else {
            continue;
        };
        let mut layer = AnnotationLayer::default();
        for annot in supported_annots(&page) {
            // SAFETY: `annot` is a live annotation; released right after reading.
            unsafe {
                read_annot(annot, height, &mut layer);
                glib::gobject_ffi::g_object_unref(annot as *mut _);
            }
        }
        if !layer.is_empty() {
            layers.insert(index, layer);
        }
    }
    layers
}

fn flip(p: ffi::PopplerPoint, height: f64) -> Point {
    Point::new(p.x, height - p.y)
}

fn color_of(color: *mut ffi::PopplerColor) -> Option<Color> {
    if color.is_null() {
        return None;
    }
    // SAFETY: non-null colours returned by Poppler are valid and owned by the caller.
    let c = unsafe { *color };
    unsafe { glib::ffi::g_free(color as *mut _) };
    Some(Color::rgba(
        c.red as f32 / 65535.0,
        c.green as f32 / 65535.0,
        c.blue as f32 / 65535.0,
        1.0,
    ))
}

fn string_of(text: *mut c_char) -> Option<String> {
    if text.is_null() {
        return None;
    }
    // SAFETY: a non-null result is a NUL-terminated string we own.
    let value = unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned();
    unsafe { glib::ffi::g_free(text as *mut _) };
    Some(value)
}

/// SAFETY: `annot` must be a live annotation.
unsafe fn read_annot(annot: *mut ffi::PopplerAnnot, page_height: f64, layer: &mut AnnotationLayer) {
    unsafe {
        let kind = ffi::poppler_annot_get_annot_type(annot);
        let mut rect = ffi::PopplerRectangle {
            x1: 0.0,
            y1: 0.0,
            x2: 0.0,
            y2: 0.0,
        };
        ffi::poppler_annot_get_rectangle(annot, &mut rect);
        // A PDF rectangle has its lower-left corner first.
        let bounds = Rect::from_corners(
            Point::new(rect.x1, page_height - rect.y1),
            Point::new(rect.x2, page_height - rect.y2),
        );

        let stroke = color_of(ffi::poppler_annot_get_color(annot)).unwrap_or(Color::RED);
        let mut width = 0.0;
        if ffi::poppler_annot_get_border_width(annot, &mut width) == 0 || width <= 0.0 {
            width = 2.0;
        }
        let contents = string_of(ffi::poppler_annot_get_contents(annot));
        let style = Style {
            stroke,
            fill: None,
            width,
            font_size: DEFAULT_FONT_SIZE,
        };

        match kind {
            ffi::POPPLER_ANNOT_SQUARE | ffi::POPPLER_ANNOT_CIRCLE => {
                let interior = if kind == ffi::POPPLER_ANNOT_SQUARE {
                    ffi::poppler_annot_square_get_interior_color(annot as *mut _)
                } else {
                    ffi::poppler_annot_circle_get_interior_color(annot as *mut _)
                };
                // Written rectangles are padded by half the stroke; undo that.
                let inner = bounds.inflate(-width / 2.0);
                let shape = if kind == ffi::POPPLER_ANNOT_SQUARE {
                    Shape::Rect(inner)
                } else {
                    Shape::Ellipse(inner)
                };
                layer.add(
                    shape,
                    Style {
                        fill: color_of(interior),
                        ..style
                    },
                );
            }
            ffi::POPPLER_ANNOT_INK => {
                let mut count = 0usize;
                let list = ffi::poppler_annot_ink_get_ink_list(annot as *mut _, &mut count);
                let mut paths: Vec<Vec<Point>> = Vec::new();
                for i in 0..count {
                    let path = *list.add(i);
                    let mut n = 0usize;
                    let points = ffi::poppler_path_get_points(path, &mut n);
                    if !points.is_null() {
                        // The points belong to the path and go away with it.
                        paths.push((0..n).map(|j| flip(*points.add(j), page_height)).collect());
                    }
                    ffi::poppler_path_free(path);
                }
                if !list.is_null() {
                    glib::ffi::g_free(list as *mut _);
                }
                match (contents.as_deref(), paths.first()) {
                    (Some(ARROW_TAG), Some(shaft)) if shaft.len() >= 2 => {
                        layer.add(Shape::Arrow(shaft[0], shaft[shaft.len() - 1]), style);
                    }
                    (Some(LINE_TAG), Some(shaft)) if shaft.len() >= 2 => {
                        layer.add(Shape::Line(shaft[0], shaft[shaft.len() - 1]), style);
                    }
                    _ => {
                        for path in paths.into_iter().filter(|p| !p.is_empty()) {
                            layer.add(Shape::Freehand(path), style.clone());
                        }
                    }
                }
            }
            ffi::POPPLER_ANNOT_FREE_TEXT => {
                let text = contents.unwrap_or_default();
                let desc = ffi::poppler_annot_free_text_get_font_desc(annot as *mut _);
                let font_size = if desc.is_null() {
                    DEFAULT_FONT_SIZE
                } else {
                    (*desc).size_pt.max(4.0)
                };
                if !desc.is_null() {
                    ffi::poppler_font_description_free(desc);
                }
                let ink = color_of(ffi::poppler_annot_free_text_get_font_color(annot as *mut _))
                    .unwrap_or(stroke);
                layer.add(
                    Shape::Text { bounds, text },
                    Style {
                        stroke: ink,
                        font_size,
                        ..style
                    },
                );
            }
            ffi::POPPLER_ANNOT_HIGHLIGHT => {
                let quads = ffi::poppler_annot_text_markup_get_quadrilaterals(annot as *mut _);
                if quads.is_null() {
                    return;
                }
                let items = std::slice::from_raw_parts(
                    (*quads).data as *const ffi::PopplerQuadrilateral,
                    (*quads).len as usize,
                );
                let rects: Vec<Rect> = items
                    .iter()
                    .map(|q| {
                        let points = [q.p1, q.p2, q.p3, q.p4].map(|p| flip(p, page_height));
                        points[1..]
                            .iter()
                            .fold(Rect::new(points[0].x, points[0].y, 0.0, 0.0), |r, p| {
                                r.union(&Rect::new(p.x, p.y, 0.0, 0.0))
                            })
                    })
                    .collect();
                glib::ffi::g_array_free(quads, 1);
                layer.add(
                    Shape::Highlight(rects),
                    Style {
                        fill: Some(stroke),
                        width: 0.0,
                        ..style
                    },
                );
            }
            _ => {}
        }
    }
}

// --- writing -------------------------------------------------------------------

fn pdf_color(color: Color) -> ffi::PopplerColor {
    let channel = |c: f32| (c.clamp(0.0, 1.0) * 65535.0).round() as u16;
    ffi::PopplerColor {
        red: channel(color.r),
        green: channel(color.g),
        blue: channel(color.b),
    }
}

fn pdf_point(p: Point, height: f64) -> ffi::PopplerPoint {
    ffi::PopplerPoint {
        x: p.x,
        y: height - p.y,
    }
}

/// A PDF rectangle (lower-left corner first) covering `r` in model coordinates.
fn pdf_rect(r: Rect, height: f64) -> ffi::PopplerRectangle {
    ffi::PopplerRectangle {
        x1: r.x,
        y1: height - r.bottom(),
        x2: r.right(),
        y2: height - r.y,
    }
}

/// Write `layers` into a copy of the document at `dest`: the supported annotations
/// that were in the file are replaced by exactly what the layers hold.
pub fn write(
    source: &PdfSource,
    layers: &BTreeMap<usize, AnnotationLayer>,
    dest: &Path,
) -> Result<()> {
    let _poppler = poppler_lock();
    let doc = open_document(source)?;
    let n_pages = doc.n_pages().max(0) as usize;
    for index in 0..n_pages {
        let page = doc
            .page(index as i32)
            .with_context(|| format!("page {} is missing", index + 1))?;
        let height = page.size().1;
        hide_supported(&page);
        let Some(layer) = layers.get(&index) else {
            continue;
        };
        for annotation in &layer.items {
            // SAFETY: `doc` and `page` are live; the annotation created is handed to the
            // page, which keeps its own reference, and ours is released.
            unsafe { add_annotation(&doc, &page, annotation, height)? };
        }
    }

    let uri = glib::filename_to_uri(dest, None).context("cannot make a URI for the output file")?;
    let uri = CString::new(uri.as_str())?;
    let mut error = ptr::null_mut();
    // SAFETY: `doc` is live and `uri` is NUL-terminated.
    let saved =
        unsafe { ffi::poppler_document_save(doc.to_glib_none().0, uri.as_ptr(), &mut error) };
    if saved == 0 {
        // SAFETY: on failure Poppler sets an owned error.
        let err: glib::Error = unsafe { from_glib_full(error) };
        bail!("cannot write the PDF: {err}");
    }
    Ok(())
}

/// SAFETY: `doc` and `page` must be live and belong together.
unsafe fn add_annotation(doc: &Document, page: &Page, a: &Annotation, height: f64) -> Result<()> {
    unsafe {
        let doc_ptr: *mut ffi::PopplerDocument = doc.to_glib_none().0;
        let pad = a.style.width / 2.0;
        let mut color = pdf_color(a.style.stroke);
        let mut strokes: Option<Vec<Vec<Point>>> = None;

        let annot: *mut ffi::PopplerAnnot = match &a.shape {
            Shape::Rect(r) | Shape::Ellipse(r) => {
                let mut rect = pdf_rect(r.inflate(pad), height);
                let is_rect = matches!(a.shape, Shape::Rect(_));
                let annot = if is_rect {
                    ffi::poppler_annot_square_new(doc_ptr, &mut rect)
                } else {
                    ffi::poppler_annot_circle_new(doc_ptr, &mut rect)
                };
                if let Some(fill) = a.style.fill {
                    let mut interior = pdf_color(fill);
                    if is_rect {
                        ffi::poppler_annot_square_set_interior_color(
                            annot as *mut _,
                            &mut interior,
                        );
                    } else {
                        ffi::poppler_annot_circle_set_interior_color(
                            annot as *mut _,
                            &mut interior,
                        );
                    }
                }
                annot
            }
            Shape::Line(from, to) => {
                let paths = vec![vec![*from, *to]];
                let annot = ink(doc_ptr, &paths, height, a.style.width, Some(LINE_TAG));
                strokes = Some(paths);
                annot
            }
            Shape::Arrow(from, to) => {
                let [left, right] = arrow_head(*from, *to, a.style.width);
                let paths = vec![vec![*from, *to], vec![left, *to, right]];
                let annot = ink(doc_ptr, &paths, height, a.style.width, Some(ARROW_TAG));
                strokes = Some(paths);
                annot
            }
            Shape::Freehand(points) => {
                let paths = vec![points.clone()];
                let annot = ink(doc_ptr, &paths, height, a.style.width, None);
                strokes = Some(paths);
                annot
            }
            Shape::Text { bounds, text } => {
                let mut rect = pdf_rect(*bounds, height);
                let annot = ffi::poppler_annot_free_text_new(doc_ptr, &mut rect);
                let contents = CString::new(text.replace('\0', ""))?;
                ffi::poppler_annot_set_contents(annot, contents.as_ptr());
                let font = CString::new("Helvetica")?;
                let desc = ffi::poppler_font_description_new(font.as_ptr());
                (*desc).size_pt = a.style.font_size;
                ffi::poppler_annot_free_text_set_font_desc(annot as *mut _, desc);
                ffi::poppler_font_description_free(desc);
                ffi::poppler_annot_free_text_set_font_color(annot as *mut _, &mut color);
                annot
            }
            Shape::Highlight(rects) => {
                let Some(first) = rects.first() else {
                    return Ok(());
                };
                let all = rects.iter().fold(*first, |acc, r| acc.union(r));
                let mut rect = pdf_rect(all, height);
                let quads = glib::ffi::g_array_new(
                    0,
                    0,
                    std::mem::size_of::<ffi::PopplerQuadrilateral>() as u32,
                );
                for r in rects {
                    // Corner order of PDF's QuadPoints: top-left, top-right, bottom-left, bottom-right.
                    let quad = ffi::PopplerQuadrilateral {
                        p1: pdf_point(Point::new(r.x, r.y), height),
                        p2: pdf_point(Point::new(r.right(), r.y), height),
                        p3: pdf_point(Point::new(r.x, r.bottom()), height),
                        p4: pdf_point(Point::new(r.right(), r.bottom()), height),
                    };
                    glib::ffi::g_array_append_vals(quads, &quad as *const _ as *const _, 1);
                }
                let annot = ffi::poppler_annot_text_markup_new_highlight(doc_ptr, &mut rect, quads);
                glib::ffi::g_array_free(quads, 1);
                annot
            }
        };
        if annot.is_null() {
            bail!("Poppler could not create an annotation");
        }

        ffi::poppler_annot_set_color(annot, &mut color);
        if !matches!(a.shape, Shape::Text { .. } | Shape::Highlight(_)) {
            ffi::poppler_annot_set_border_width(annot, a.style.width);
        }
        ffi::poppler_page_add_annot(page.to_glib_none().0, annot);
        if let Some(paths) = strokes {
            set_ink_paths(annot, &paths, height);
        }
        glib::gobject_ffi::g_object_unref(annot as *mut _);
        Ok(())
    }
}

/// An ink annotation big enough for `paths` (model coordinates). The strokes are
/// set separately, once the annotation is on its page.
///
/// SAFETY: `doc` must be a live document.
unsafe fn ink(
    doc: *mut ffi::PopplerDocument,
    paths: &[Vec<Point>],
    height: f64,
    width: f64,
    tag: Option<&str>,
) -> *mut ffi::PopplerAnnot {
    unsafe {
        let bounds = paths
            .iter()
            .flatten()
            .map(|p| Rect::new(p.x, p.y, 0.0, 0.0))
            .reduce(|a, b| a.union(&b))
            .unwrap_or_default()
            .inflate(width / 2.0 + 1.0);
        let mut rect = pdf_rect(bounds, height);
        let annot = ffi::poppler_annot_ink_new(doc, &mut rect);
        if let (false, Some(tag)) = (annot.is_null(), tag.and_then(|t| CString::new(t).ok())) {
            ffi::poppler_annot_set_contents(annot, tag.as_ptr());
        }
        annot
    }
}

/// SAFETY: `annot` must be a live ink annotation that is already on its page.
unsafe fn set_ink_paths(annot: *mut ffi::PopplerAnnot, paths: &[Vec<Point>], height: f64) {
    unsafe {
        let mut owned: Vec<*mut ffi::PopplerPath> = paths
            .iter()
            .map(|path| {
                // A path takes ownership of its points, so they must come from g_malloc.
                let bytes = (path.len().max(1) * std::mem::size_of::<ffi::PopplerPoint>()) as _;
                let points = glib::ffi::g_malloc(bytes) as *mut ffi::PopplerPoint;
                for (i, p) in path.iter().enumerate() {
                    points.add(i).write(pdf_point(*p, height));
                }
                ffi::poppler_path_new_from_array(points, path.len())
            })
            .collect();
        ffi::poppler_annot_ink_set_ink_list(annot as *mut _, owned.as_mut_ptr(), owned.len());
        // The annotation keeps its own copies.
        for path in owned {
            ffi::poppler_path_free(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::{load_info, poppler_lock};

    fn fixture() -> PdfSource {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/selection.pdf");
        PdfSource::new(&path, None)
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lumen-annots-{name}-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join("out.pdf")
    }

    fn solid(color: Color) -> Style {
        Style {
            stroke: color,
            fill: Some(color),
            width: 2.0,
            font_size: 14.0,
        }
    }

    fn sample_layers() -> BTreeMap<usize, AnnotationLayer> {
        let mut layer = AnnotationLayer::default();
        let blue = Color::rgba(0.0, 0.0, 1.0, 1.0);
        layer.add(
            Shape::Rect(Rect::new(20.0, 30.0, 60.0, 40.0)),
            solid(Color::RED),
        );
        layer.add(
            Shape::Ellipse(Rect::new(120.0, 30.0, 50.0, 40.0)),
            Style {
                fill: None,
                ..solid(blue)
            },
        );
        layer.add(
            Shape::Line(Point::new(20.0, 100.0), Point::new(120.0, 100.0)),
            Style {
                fill: None,
                ..solid(blue)
            },
        );
        layer.add(
            Shape::Arrow(Point::new(20.0, 130.0), Point::new(120.0, 130.0)),
            Style {
                fill: None,
                ..solid(blue)
            },
        );
        layer.add(
            Shape::Freehand(vec![
                Point::new(200.0, 30.0),
                Point::new(230.0, 60.0),
                Point::new(260.0, 30.0),
            ]),
            Style {
                fill: None,
                ..solid(Color::RED)
            },
        );
        layer.add(
            Shape::Text {
                bounds: Rect::new(200.0, 100.0, 120.0, 30.0),
                text: "Hello PDF".into(),
            },
            solid(blue),
        );
        layer.add(
            Shape::Highlight(vec![
                Rect::new(72.0, 180.0, 100.0, 14.0),
                Rect::new(72.0, 200.0, 80.0, 14.0),
            ]),
            Style {
                fill: Some(Color::YELLOW),
                width: 0.0,
                ..solid(Color::YELLOW)
            },
        );
        BTreeMap::from([(0, layer)])
    }

    #[test]
    fn annotations_survive_a_write_and_read_round_trip() {
        let dest = scratch("roundtrip");
        write(&fixture(), &sample_layers(), &dest).expect("writes");

        let info = load_info(&PdfSource::new(&dest, None)).expect("reopens");
        let layer = info.annotations.get(&0).expect("annotations on page 1");
        let shapes: Vec<&Shape> = layer.items.iter().map(|a| &a.shape).collect();
        assert_eq!(shapes.len(), 7, "{shapes:#?}");

        let find = |f: fn(&&Shape) -> bool| {
            layer
                .items
                .iter()
                .find(|a| f(&&a.shape))
                .unwrap_or_else(|| panic!("missing shape in {shapes:#?}"))
        };

        // Rectangle: same place (to within rounding), same fill.
        let rect = find(|s| matches!(s, Shape::Rect(_)));
        let Shape::Rect(r) = rect.shape else {
            unreachable!()
        };
        assert!(
            (r.x - 20.0).abs() < 0.5
                && (r.y - 30.0).abs() < 0.5
                && (r.w - 60.0).abs() < 0.5
                && (r.h - 40.0).abs() < 0.5,
            "{r:?}"
        );
        assert!(rect.style.fill.is_some_and(|c| c.r > 0.85 && c.g < 0.2), "fill {:?}", rect.style);

        // Lines and arrows come back as themselves, with the same end points.
        let line = find(|s| matches!(s, Shape::Line(..)));
        let Shape::Line(a, b) = line.shape else {
            unreachable!()
        };
        assert!(
            (a.x - 20.0).abs() < 0.5 && (a.y - 100.0).abs() < 0.5 && (b.x - 120.0).abs() < 0.5,
            "{a:?} {b:?}"
        );
        let arrow = find(|s| matches!(s, Shape::Arrow(..)));
        let Shape::Arrow(a, b) = arrow.shape else {
            unreachable!()
        };
        assert!(
            (a.y - 130.0).abs() < 0.5 && (b.x - 120.0).abs() < 0.5,
            "{a:?} {b:?}"
        );

        let pen = find(|s| matches!(s, Shape::Freehand(_)));
        let Shape::Freehand(points) = &pen.shape else {
            unreachable!()
        };
        assert_eq!(points.len(), 3);
        assert!(
            (points[1].y - 60.0).abs() < 0.5,
            "y is measured from the top: {points:?}"
        );

        let text = find(|s| matches!(s, Shape::Text { .. }));
        let Shape::Text { text: t, bounds } = &text.shape else {
            unreachable!()
        };
        assert_eq!(t, "Hello PDF");
        assert!(
            (bounds.y - 100.0).abs() < 0.5 && (text.style.font_size - 14.0).abs() < 0.5,
            "{bounds:?} {}",
            text.style.font_size
        );

        let marker = find(|s| matches!(s, Shape::Highlight(_)));
        let Shape::Highlight(rects) = &marker.shape else {
            unreachable!()
        };
        assert_eq!(rects.len(), 2);
        assert!(
            (rects[0].y - 180.0).abs() < 0.5 && (rects[0].w - 100.0).abs() < 0.5,
            "{rects:?}"
        );
        let _ = std::fs::remove_dir_all(dest.parent().unwrap());
    }

    #[test]
    fn written_annotations_are_visible_to_other_readers() {
        // A plain Poppler render (which draws annotations) must show the red square.
        let dest = scratch("visible");
        write(&fixture(), &sample_layers(), &dest).expect("writes");

        let _poppler = poppler_lock();
        let doc = open_document(&PdfSource::new(&dest, None)).unwrap();
        let page = doc.page(0).unwrap();
        let mut surface = poppler::ffi::PopplerRectangle {
            x1: 0.0,
            y1: 0.0,
            x2: 0.0,
            y2: 0.0,
        };
        let _ = &mut surface;
        let image =
            relm4::gtk::cairo::ImageSurface::create(relm4::gtk::cairo::Format::ARgb32, 400, 300)
                .unwrap();
        {
            let cr = relm4::gtk::cairo::Context::new(&image).unwrap();
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.paint().unwrap();
            page.render(&cr);
        }
        let mut image = image;
        let stride = image.stride() as usize;
        let data = image.data().unwrap();
        let at = |x: usize, y: usize| {
            let o = y * stride + x * 4;
            [data[o + 2], data[o + 1], data[o]]
        };
        let [r, g, b] = at(50, 50);
        assert!(
            r > 200 && g < 80 && b < 80,
            "inside the red square: {r} {g} {b}"
        );
        let [r, g, b] = at(380, 280);
        assert!(r > 240 && g > 240 && b > 240, "blank paper elsewhere");
        let _ = std::fs::remove_dir_all(dest.parent().unwrap());
    }

    #[test]
    fn rewriting_replaces_instead_of_duplicating() {
        let first = scratch("rewrite1");
        write(&fixture(), &sample_layers(), &first).unwrap();
        // Read them back, write again from what was read: still seven, not fourteen.
        let info = load_info(&PdfSource::new(&first, None)).unwrap();
        let second = scratch("rewrite2");
        write(&PdfSource::new(&first, None), &info.annotations, &second).unwrap();
        let again = load_info(&PdfSource::new(&second, None)).unwrap();
        assert_eq!(again.annotations[&0].items.len(), 7);

        // Deleting everything removes them from the file.
        let cleared = scratch("rewrite3");
        write(&PdfSource::new(&second, None), &BTreeMap::new(), &cleared).unwrap();
        assert!(
            load_info(&PdfSource::new(&cleared, None))
                .unwrap()
                .annotations
                .is_empty()
        );
        for p in [first, second, cleared] {
            let _ = std::fs::remove_dir_all(p.parent().unwrap());
        }
    }
}
