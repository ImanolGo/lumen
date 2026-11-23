//! Real PDF annotations: reading them into the model, and writing the model back.
//!
//! The kinds Lumen draws itself (square, circle, line, ink, free text and
//! highlight) become editable. Everything else (links, form fields, stamps,
//! sticky notes…) is left exactly as it is and keeps being drawn by hayro.
//!
//! Saving appends an *incremental update* to the file (via `lopdf`): only the
//! pages that changed get a new annotation list, and new annotation objects are
//! added after the original bytes, which stay untouched. Each annotation has an
//! appearance stream, so every PDF viewer shows it the same way.
//!
//! Positions in the model are view coordinates (points, origin at the top-left
//! of the page as displayed, y down), the same space as text and links. Each
//! page's `to_view` transform (hayro's initial transform, which also handles
//! /Rotate) maps PDF user space to it.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;

use anyhow::{Context, Result, bail};
use hayro::hayro_interpret::TransformExt;
use hayro::hayro_syntax::Pdf;
use hayro::hayro_syntax::object::{Array, Dict, Name, Number, String as PdfString};
use hayro::kurbo::{self, Affine, PathEl};
use lopdf::{Dictionary, Object, ObjectId, Stream, StringFormat, dictionary};

use super::model::{Annotation, AnnotationLayer, Color, Note, Point, Rect, Shape, Style};
use super::paint::{self, LINE_HEIGHT, TEXT_PAD};
use crate::structure::decode_text;

/// An annotation object: number and generation.
pub type ObjRef = (u32, u16);

/// Protects against hostile files.
const MAX_PER_PAGE: usize = 5_000;
const MAX_POINTS: usize = 100_000;

/// The editable annotations of a document.
#[derive(Debug, Clone, Default)]
pub struct Imported {
    pub layers: BTreeMap<usize, AnnotationLayer>,
    /// Per page, the objects that became editable: hidden from hayro while they are
    /// edited, and replaced by the layer's contents when the page is saved.
    pub refs: Vec<Vec<ObjRef>>,
}

impl Imported {
    pub fn is_empty(&self) -> bool {
        self.refs.iter().all(Vec::is_empty)
    }
}

/// PDF user space → view, per page.
pub fn transforms(pdf: &Pdf) -> Vec<Affine> {
    pdf.pages().iter().map(|p| p.initial_transform(true).to_kurbo()).collect()
}

// ---------------------------------------------------------------- reading

/// Reads the annotations Lumen can edit. Cheap: no page content is interpreted.
pub fn import(pdf: &Pdf) -> Imported {
    let pages = pdf.pages();
    let mut out = Imported { layers: BTreeMap::new(), refs: vec![Vec::new(); pages.len()] };
    for (index, page) in pages.iter().enumerate() {
        let Some(annots) = page.raw().get::<Array<'_>>("Annots") else { continue };
        let to_view = page.initial_transform(true).to_kurbo();
        for annot in annots.iter::<Dict<'_>>().take(MAX_PER_PAGE) {
            // Inline dictionaries cannot be hidden or replaced by reference; leave them to hayro.
            let Some(id) = annot.obj_id() else { continue };
            let Ok(generation) = u16::try_from(id.gen_number) else { continue };
            // Hidden annotations stay hidden, and untouched.
            if annot.get::<u32>("F").unwrap_or(0) & 0b10 != 0 {
                continue;
            }
            let Some((annotation, note)) = read_one(&annot, to_view) else { continue };
            let layer = out.layers.entry(index).or_default();
            let new_id = layer.add(annotation.shape, annotation.style);
            if note != Note::default() {
                layer.notes.insert(new_id, note);
            }
            out.refs[index].push((id.obj_number, generation));
        }
    }
    out
}

fn numbers(dict: &Dict<'_>, key: &str) -> Option<Vec<f64>> {
    let array = dict.get::<Array<'_>>(key)?;
    Some(array.iter::<Number>().take(MAX_POINTS).map(|n| n.as_f64()).collect())
}

fn color(dict: &Dict<'_>, key: &str, alpha: f32) -> Option<Color> {
    let c = numbers(dict, key)?;
    let c: Vec<f32> = c.iter().map(|v| v.clamp(0.0, 1.0) as f32).collect();
    let (r, g, b) = match c.as_slice() {
        [gray] => (*gray, *gray, *gray),
        [r, g, b] => (*r, *g, *b),
        [c, m, y, k] => ((1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)),
        _ => return None,
    };
    Some(Color::rgba(r, g, b, alpha))
}

fn text_string(dict: &Dict<'_>, key: &str) -> Option<String> {
    let s = dict.get::<PdfString<'_>>(key)?;
    let text = decode_text(s.as_bytes()).replace("\r\n", "\n").replace('\r', "\n");
    (!text.trim().is_empty()).then_some(text)
}

fn view_rect(to_view: Affine, r: [f64; 4]) -> Rect {
    let k = to_view.transform_rect_bbox(kurbo::Rect::new(r[0], r[1], r[2], r[3]));
    Rect::new(k.x0, k.y0, k.width(), k.height())
}

fn view_point(to_view: Affine, x: f64, y: f64) -> Point {
    let p = to_view * kurbo::Point::new(x, y);
    Point::new(p.x, p.y)
}

fn read_one(annot: &Dict<'_>, to_view: Affine) -> Option<(Annotation, Note)> {
    let subtype = annot.get::<Name<'_>>("Subtype")?;
    let rect: [f64; 4] = numbers(annot, "Rect")?.try_into().ok()?;
    let alpha = annot.get::<f32>("CA").unwrap_or(1.0).clamp(0.0, 1.0);
    let width = annot
        .get::<Dict<'_>>("BS")
        .and_then(|bs| bs.get::<f64>("W"))
        .or_else(|| numbers(annot, "Border").and_then(|b| b.get(2).copied()))
        .unwrap_or(1.0)
        .clamp(0.0, 200.0);
    let stroke = color(annot, "C", alpha);
    let mut style = Style { stroke: stroke.unwrap_or(Color::AMBER), fill: None, width, ..Style::default() };
    let note = Note { contents: text_string(annot, "Contents"), author: text_string(annot, "T") };
    let shape = match subtype.as_str() {
        "Square" | "Circle" => {
            if stroke.is_none() {
                style.width = 0.0;
            }
            style.fill = color(annot, "IC", alpha);
            if style.width == 0.0 && style.fill.is_none() {
                return None;
            }
            // The geometry is the rectangle minus /RD, or minus half the border.
            let inset = numbers(annot, "RD").filter(|d| d.len() == 4).unwrap_or(vec![width / 2.0; 4]);
            let r = [rect[0].min(rect[2]), rect[1].min(rect[3]), rect[0].max(rect[2]), rect[1].max(rect[3])];
            let r = [r[0] + inset[0], r[1] + inset[3], r[2] - inset[2], r[3] - inset[1]];
            let r = view_rect(to_view, r);
            if subtype.as_str() == "Square" { Shape::Rect(r) } else { Shape::Ellipse(r) }
        }
        "Line" => {
            let l: [f64; 4] = numbers(annot, "L")?.try_into().ok()?;
            let (a, b) = (view_point(to_view, l[0], l[1]), view_point(to_view, l[2], l[3]));
            let ends: Vec<String> = annot
                .get::<Array<'_>>("LE")
                .map(|a| a.iter::<Name<'_>>().map(|n| n.as_str().to_owned()).collect())
                .unwrap_or_default();
            let arrow = |n: Option<&String>| n.is_some_and(|n| n.ends_with("Arrow"));
            match (arrow(ends.first()), arrow(ends.get(1))) {
                (_, true) => Shape::Arrow(a, b),
                (true, false) => Shape::Arrow(b, a),
                _ => Shape::Line(a, b),
            }
        }
        "Ink" => {
            let list = annot.get::<Array<'_>>("InkList")?;
            let strokes: Vec<Vec<Point>> = list
                .iter::<Array<'_>>()
                .take(1000)
                .map(|s| {
                    let v: Vec<f64> = s.iter::<Number>().take(MAX_POINTS).map(|n| n.as_f64()).collect();
                    v.as_chunks::<2>().0.iter().map(|[x, y]| view_point(to_view, *x, *y)).collect::<Vec<_>>()
                })
                .filter(|s: &Vec<Point>| !s.is_empty())
                .collect();
            match strokes.len() {
                0 => return None,
                1 => Shape::Freehand(strokes.into_iter().next()?),
                _ => Shape::Ink(strokes),
            }
        }
        "Highlight" => {
            let quads = numbers(annot, "QuadPoints").filter(|q| q.len() >= 8)?;
            let rects = quads
                .as_chunks::<8>()
                .0
                .iter()
                .map(|q| {
                    let xs = [q[0], q[2], q[4], q[6]];
                    let ys = [q[1], q[3], q[5], q[7]];
                    let min = |v: [f64; 4]| v.into_iter().fold(f64::INFINITY, f64::min);
                    let max = |v: [f64; 4]| v.into_iter().fold(f64::NEG_INFINITY, f64::max);
                    view_rect(to_view, [min(xs), min(ys), max(xs), max(ys)])
                })
                .collect();
            let c = stroke.unwrap_or(Color::YELLOW);
            style = Style { stroke: c, fill: Some(c), width: 0.0, ..Style::default() };
            Shape::Highlight(rects)
        }
        "FreeText" => {
            let (size, text_color) = parse_da(annot.get::<PdfString<'_>>("DA").map(|s| s.as_bytes().to_vec()));
            style.font_size = size;
            style.stroke = text_color.unwrap_or(Color::rgba(0.0, 0.0, 0.0, 1.0));
            style.stroke.a = alpha;
            style.width = 0.0;
            let text = note.contents.clone().unwrap_or_default();
            let bounds = view_rect(to_view, rect);
            // The text lives in the shape; do not keep a second copy as a comment.
            let note = Note { contents: None, ..note };
            return Some((Annotation { id: 0, shape: Shape::Text { bounds, text }, style }, note));
        }
        _ => return None,
    };
    Some((Annotation { id: 0, shape, style }, note))
}

/// Font size and colour from a default appearance string such as `/Helv 12 Tf 1 0 0 rg`.
fn parse_da(da: Option<Vec<u8>>) -> (f64, Option<Color>) {
    let da = da.map(|d| String::from_utf8_lossy(&d).into_owned()).unwrap_or_default();
    let tokens: Vec<&str> = da.split_whitespace().collect();
    let mut size = 12.0;
    let mut color = None;
    let num = |i: usize| tokens.get(i).and_then(|t| t.parse::<f32>().ok());
    for (i, t) in tokens.iter().enumerate() {
        match *t {
            "Tf" => size = i.checked_sub(1).and_then(num).map_or(12.0, |s| s as f64),
            "rg" if i >= 3 => {
                if let (Some(r), Some(g), Some(b)) = (num(i - 3), num(i - 2), num(i - 1)) {
                    color = Some(Color::rgba(r, g, b, 1.0));
                }
            }
            "g" if i >= 1 => color = num(i - 1).map(|v| Color::rgba(v, v, v, 1.0)),
            _ => {}
        }
    }
    // A size of 0 means "auto"; pick something readable.
    (if size <= 0.0 { 12.0 } else { size.min(400.0) }, color)
}

// ---------------------------------------------------------------- writing

fn load(bytes: &[u8]) -> Result<lopdf::IncrementalDocument> {
    let doc = lopdf::IncrementalDocument::load_from(bytes).context("the PDF could not be read for saving")?;
    if doc.get_prev_documents().is_encrypted() {
        bail!("markup cannot be saved into this encrypted PDF");
    }
    Ok(doc)
}

fn finish(mut doc: lopdf::IncrementalDocument) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(doc.get_prev_documents_bytes().len() + 64 * 1024);
    doc.new_document.compress();
    doc.save_to(&mut out).context("the PDF could not be written")?;
    Ok(out)
}

/// The document with the given annotations hidden, so hayro does not draw them while
/// Lumen draws (and edits) them instead. Only ever held in memory.
pub fn hide(bytes: &[u8], refs: &[ObjRef]) -> Result<Vec<u8>> {
    let mut doc = load(bytes)?;
    for &id in refs {
        if doc.opt_clone_object_to_new_document(id).is_err() {
            continue;
        }
        if let Ok(dict) = doc.new_document.get_dictionary_mut(id) {
            let flags = dict.get(b"F").and_then(Object::as_i64).unwrap_or(0);
            dict.set("F", flags | 0b10);
        }
    }
    finish(doc)
}

/// One page to rewrite: what is on it now, and the objects it replaces.
pub struct PageEdit<'a> {
    pub page: usize,
    pub layer: Option<&'a AnnotationLayer>,
    pub replaces: &'a [ObjRef],
}

/// `bytes` with the pages in `edits` carrying their new annotations.
pub fn save(bytes: &[u8], edits: &[PageEdit<'_>]) -> Result<Vec<u8>> {
    let hayro_doc = Pdf::new(bytes.to_vec()).map_err(|_| anyhow::anyhow!("the PDF could not be read"))?;
    let to_view = transforms(&hayro_doc);
    drop(hayro_doc);

    let mut doc = load(bytes)?;
    let pages = doc.get_prev_documents().get_pages();
    if pages.len() != to_view.len() {
        bail!("the pages of this PDF could not be matched up for saving");
    }
    for edit in edits {
        let Some(&page_id) = pages.get(&(edit.page as u32 + 1)) else { continue };
        let replaced: HashSet<ObjectId> = edit.replaces.iter().copied().collect();
        let prev = doc.get_prev_documents();
        let old: Vec<Object> = prev
            .get_dictionary(page_id)
            .ok()
            .and_then(|page| page.get(b"Annots").ok())
            .and_then(|a| prev.dereference(a).ok())
            .and_then(|(_, a)| a.as_array().ok().cloned())
            .unwrap_or_default();
        // Keep everything that was not replaced, except pop-up notes belonging to what was.
        let mut annots: Vec<Object> = old
            .into_iter()
            .filter(|o| match o {
                Object::Reference(id) if replaced.contains(id) => false,
                Object::Reference(id) => !prev
                    .get_dictionary(*id)
                    .ok()
                    .and_then(|d| d.get(b"Parent").ok())
                    .and_then(|p| p.as_reference().ok())
                    .is_some_and(|parent| replaced.contains(&parent)),
                _ => true,
            })
            .collect();

        let to_view = to_view[edit.page];
        let to_user = to_view.inverse();
        if let Some(layer) = edit.layer {
            for a in &layer.items {
                let note = layer.notes.get(&a.id);
                let (dict, appearance) = annotation_dict(a, note, page_id, to_view, to_user);
                let ap = doc.new_document.add_object(appearance);
                let mut dict = dict;
                dict.set("AP", dictionary! { "N" => ap });
                annots.push(Object::Reference(doc.new_document.add_object(dict)));
            }
        }
        doc.opt_clone_object_to_new_document(page_id)?;
        let page = doc.new_document.get_dictionary_mut(page_id)?;
        if annots.is_empty() {
            page.remove(b"Annots");
        } else {
            page.set("Annots", annots);
        }
    }
    finish(doc)
}

fn real(v: f64) -> Object {
    Object::Real(((v * 1000.0).round() / 1000.0) as f32)
}

fn rgb(c: Color) -> Object {
    Object::Array(vec![real(c.r as f64), real(c.g as f64), real(c.b as f64)])
}

fn user_rect(to_user: Affine, r: Rect) -> Vec<Object> {
    let k = to_user.transform_rect_bbox(kurbo::Rect::new(r.x, r.y, r.right(), r.bottom()));
    vec![real(k.x0), real(k.y0), real(k.x1), real(k.y1)]
}

fn user_point(to_user: Affine, p: Point) -> [Object; 2] {
    let k = to_user * kurbo::Point::new(p.x, p.y);
    [real(k.x), real(k.y)]
}

/// A PDF text string: plain when it is ASCII, UTF-16 otherwise.
fn pdf_text(s: &str) -> Object {
    if s.is_ascii() {
        Object::String(s.as_bytes().to_vec(), StringFormat::Literal)
    } else {
        let mut bytes = vec![0xFE, 0xFF];
        bytes.extend(s.encode_utf16().flat_map(u16::to_be_bytes));
        Object::String(bytes, StringFormat::Hexadecimal)
    }
}

fn annotation_dict(
    a: &Annotation,
    note: Option<&Note>,
    page: ObjectId,
    to_view: Affine,
    to_user: Affine,
) -> (Dictionary, Stream) {
    let style = &a.style;
    let outer = paint::outer_bounds(a);
    let mut d = dictionary! {
        "Type" => "Annot",
        "Rect" => user_rect(to_user, outer),
        "P" => page,
        "F" => 4, // print
        "NM" => pdf_text(&format!("lumen-{:x}-{}", std::process::id(), a.id)),
    };
    if let Some(note) = note {
        if let Some(c) = &note.contents {
            d.set("Contents", pdf_text(c));
        }
        if let Some(t) = &note.author {
            d.set("T", pdf_text(t));
        }
    }
    let border = dictionary! { "W" => real(style.width), "S" => "S" };
    if style.stroke.a < 1.0 {
        d.set("CA", real(style.stroke.a as f64));
    }
    match &a.shape {
        Shape::Rect(r) | Shape::Ellipse(r) => {
            d.set("Subtype", if matches!(a.shape, Shape::Rect(_)) { "Square" } else { "Circle" });
            if style.width > 0.0 {
                d.set("C", rgb(style.stroke));
            }
            if let Some(fill) = style.fill {
                d.set("IC", rgb(fill));
            }
            d.set("BS", border);
            // How far the geometry sits inside /Rect, so it reads back exactly.
            let k = to_user.transform_rect_bbox(kurbo::Rect::new(outer.x, outer.y, outer.right(), outer.bottom()));
            let g = to_user.transform_rect_bbox(kurbo::Rect::new(r.x, r.y, r.right(), r.bottom()));
            d.set("RD", vec![real(g.x0 - k.x0), real(k.y1 - g.y1), real(k.x1 - g.x1), real(g.y0 - k.y0)]);
        }
        Shape::Line(p, q) | Shape::Arrow(p, q) => {
            d.set("Subtype", "Line");
            d.set("C", rgb(style.stroke));
            d.set("BS", border);
            let mut l = Vec::with_capacity(4);
            l.extend(user_point(to_user, *p));
            l.extend(user_point(to_user, *q));
            d.set("L", l);
            if matches!(a.shape, Shape::Arrow(..)) {
                d.set("LE", vec![Object::Name(b"None".to_vec()), Object::Name(b"OpenArrow".to_vec())]);
            }
        }
        Shape::Freehand(points) => {
            d.set("Subtype", "Ink");
            d.set("C", rgb(style.stroke));
            d.set("BS", border);
            d.set("InkList", vec![Object::Array(points.iter().flat_map(|p| user_point(to_user, *p)).collect())]);
        }
        Shape::Ink(strokes) => {
            d.set("Subtype", "Ink");
            d.set("C", rgb(style.stroke));
            d.set("BS", border);
            let list: Vec<Object> = strokes
                .iter()
                .map(|s| Object::Array(s.iter().flat_map(|p| user_point(to_user, *p)).collect()))
                .collect();
            d.set("InkList", list);
        }
        Shape::Highlight(rects) => {
            d.set("Subtype", "Highlight");
            d.set("C", rgb(style.fill.unwrap_or(style.stroke)));
            let mut quads = Vec::with_capacity(rects.len() * 8);
            for r in rects {
                // Upper left, upper right, lower left, lower right, as other viewers expect.
                for p in [
                    Point::new(r.x, r.y),
                    Point::new(r.right(), r.y),
                    Point::new(r.x, r.bottom()),
                    Point::new(r.right(), r.bottom()),
                ] {
                    quads.extend(user_point(to_user, p));
                }
            }
            d.set("QuadPoints", quads);
        }
        Shape::Text { text, .. } => {
            d.set("Subtype", "FreeText");
            d.set("Contents", pdf_text(text));
            let c = style.stroke;
            d.set(
                "DA",
                Object::String(
                    format!("/Helv {:.2} Tf {:.3} {:.3} {:.3} rg", style.font_size, c.r, c.g, c.b).into_bytes(),
                    StringFormat::Literal,
                ),
            );
            d.set("BS", dictionary! { "W" => 0 });
        }
    }
    (d, appearance(a, outer, to_view))
}

/// The appearance stream: drawn in view coordinates, with a /Matrix that maps them to
/// user space, so the same code serves rotated and unrotated pages.
fn appearance(a: &Annotation, outer: Rect, to_view: Affine) -> Stream {
    let mut c = String::new();
    let mut resources = Dictionary::new();
    let mut states = Dictionary::new();
    let alpha = a.style.stroke.a;
    let prims = paint::prims(a);
    if prims.iter().any(|p| p.marker) {
        states.set("M", dictionary! { "Type" => "ExtGState", "BM" => "Multiply", "ca" => real(alpha as f64) });
    } else if alpha < 1.0 {
        states.set("A", dictionary! { "Type" => "ExtGState", "CA" => real(alpha as f64), "ca" => real(alpha as f64) });
    }
    for prim in &prims {
        c.push_str("q 1 J 1 j\n");
        if prim.marker {
            c.push_str("/M gs\n");
        } else if alpha < 1.0 {
            c.push_str("/A gs\n");
        }
        for el in prim.path.elements() {
            let _ = match el {
                PathEl::MoveTo(p) => writeln!(c, "{:.2} {:.2} m", p.x, p.y),
                PathEl::LineTo(p) => writeln!(c, "{:.2} {:.2} l", p.x, p.y),
                PathEl::QuadTo(p1, p2) => writeln!(c, "{:.2} {:.2} {:.2} {:.2} v", p1.x, p1.y, p2.x, p2.y),
                PathEl::CurveTo(p1, p2, p3) => {
                    writeln!(c, "{:.2} {:.2} {:.2} {:.2} {:.2} {:.2} c", p1.x, p1.y, p2.x, p2.y, p3.x, p3.y)
                }
                PathEl::ClosePath => writeln!(c, "h"),
            };
        }
        if let Some(f) = prim.fill {
            let _ = writeln!(c, "{:.3} {:.3} {:.3} rg", f.r, f.g, f.b);
        }
        if let Some((s, w)) = prim.stroke {
            let _ = writeln!(c, "{:.3} {:.3} {:.3} RG {:.2} w", s.r, s.g, s.b, w);
        }
        c.push_str(match (prim.fill.is_some(), prim.stroke.is_some()) {
            (true, true) => "B\n",
            (true, false) => "f\n",
            (false, true) => "S\n",
            (false, false) => "n\n",
        });
        c.push_str("Q\n");
    }
    if let Some(block) = paint::text(a) {
        resources.set(
            "Font",
            dictionary! { "Helv" => dictionary! {
                "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding",
            }},
        );
        let col = block.color;
        let _ = writeln!(c, "q BT /Helv {:.2} Tf {:.3} {:.3} {:.3} rg", block.size, col.r, col.g, col.b);
        if alpha < 1.0 {
            c.push_str("/A gs\n");
        }
        let pad = block.size * TEXT_PAD;
        for (i, line) in block.lines.iter().enumerate() {
            let baseline = block.bounds.y + pad + block.size * (i as f64 * LINE_HEIGHT + 0.95);
            // y is flipped in view space, so the glyphs are flipped back.
            let _ = write!(c, "1 0 0 -1 {:.2} {:.2} Tm <", block.bounds.x + pad, baseline);
            for b in line.chars().map(win_ansi) {
                let _ = write!(c, "{b:02X}");
            }
            c.push_str("> Tj\n");
        }
        c.push_str("ET Q\n");
    }
    if !states.is_empty() {
        resources.set("ExtGState", states);
    }
    let m = to_view.inverse().as_coeffs();
    let form = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Form",
        "BBox" => vec![real(outer.x), real(outer.y), real(outer.right()), real(outer.bottom())],
        "Matrix" => m.iter().map(|v| real(*v)).collect::<Vec<_>>(),
        "Resources" => resources,
    };
    let mut stream = Stream::new(form, c.into_bytes());
    let _ = stream.compress();
    stream
}

/// The WinAnsiEncoding byte for `c` (the standard fonts' encoding), or `?`.
fn win_ansi(c: char) -> u8 {
    match c as u32 {
        0x20..=0x7E | 0xA0..=0xFF => c as u8,
        _ => match c {
            '€' => 0x80,
            '‚' => 0x82,
            '„' => 0x84,
            '…' => 0x85,
            '‘' => 0x91,
            '’' => 0x92,
            '“' => 0x93,
            '”' => 0x94,
            '•' => 0x95,
            '–' => 0x96,
            '—' => 0x97,
            '™' => 0x99,
            '\t' => b' ',
            _ => b'?',
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-page PDF (rotated when `rotate` is set) with a link and an existing highlight.
    pub fn fixture(rotate: bool) -> Vec<u8> {
        use pdf_writer::types::AnnotationType;
        use pdf_writer::{Content, Finish, Name, Rect as PRect, Ref, Str};
        let r = Ref::new;
        let mut w = pdf_writer::Pdf::new();
        w.catalog(r(1)).pages(r(2));
        w.pages(r(2)).kids([r(3)]).count(1);
        let mut page = w.page(r(3));
        page.parent(r(2)).media_box(PRect::new(0.0, 0.0, 600.0, 800.0)).contents(r(4));
        if rotate {
            page.rotate(90);
        }
        page.resources().fonts().pair(Name(b"F1"), r(5));
        page.annotations([r(6), r(7)]);
        page.finish();
        let mut c = Content::new();
        c.begin_text().set_font(Name(b"F1"), 20.0).next_line(72.0, 700.0).show(Str(b"Hello")).end_text();
        w.stream(r(4), &c.finish());
        w.type1_font(r(5)).base_font(Name(b"Helvetica"));
        let mut link = w.annotation(r(6));
        link.subtype(AnnotationType::Link).rect(PRect::new(72.0, 690.0, 200.0, 720.0));
        link.action().action_type(pdf_writer::types::ActionType::Uri).uri(Str(b"https://example.com/"));
        link.finish();
        let mut mark = w.annotation(r(7));
        mark.subtype(AnnotationType::Highlight)
            .rect(PRect::new(70.0, 695.0, 130.0, 720.0))
            .quad_points([70.0, 720.0, 130.0, 720.0, 70.0, 695.0, 130.0, 695.0])
            .color_rgb(1.0, 1.0, 0.0)
            .contents(pdf_writer::TextStr("Important"));
        mark.finish();
        w.finish()
    }

    fn read(bytes: &[u8]) -> Imported {
        import(&Pdf::new(bytes.to_vec()).unwrap())
    }

    #[test]
    fn reads_highlights_and_leaves_links_alone() {
        let imported = read(&fixture(false));
        assert_eq!(imported.refs, vec![vec![(7, 0)]]);
        let layer = &imported.layers[&0];
        let Shape::Highlight(rects) = &layer.items[0].shape else { panic!("{:?}", layer.items) };
        // 720 pt from the bottom of an 800 pt page is 80 pt from the top.
        let r = rects[0];
        assert!((r.x - 70.0).abs() < 1e-3 && (r.y - 80.0).abs() < 1e-3 && (r.h - 25.0).abs() < 1e-3, "{r:?}");
        assert_eq!(layer.notes[&layer.items[0].id].contents.as_deref(), Some("Important"));
    }

    #[test]
    fn hiding_keeps_the_rest_of_the_document() {
        let bytes = fixture(false);
        let hidden = hide(&bytes, &read(&bytes).refs[0]).unwrap();
        assert!(hidden.starts_with(&bytes), "an incremental update only appends");
        let again = read(&hidden);
        assert!(again.is_empty(), "hidden annotations are not editable twice");
        let pdf = Pdf::new(hidden).unwrap();
        assert_eq!(crate::structure::read(&pdf).1[0].len(), 1, "the link is still there");
    }

    #[test]
    fn saves_every_shape_and_reads_it_back() {
        for rotate in [false, true] {
            let bytes = fixture(rotate);
            let imported = read(&bytes);
            let mut layer = imported.layers[&0].clone();
            let style = Style::default();
            let shapes = [
                Shape::Rect(Rect::new(100.0, 100.0, 80.0, 40.0)),
                Shape::Ellipse(Rect::new(200.0, 100.0, 60.0, 60.0)),
                Shape::Line(Point::new(100.0, 300.0), Point::new(200.0, 320.0)),
                Shape::Arrow(Point::new(100.0, 400.0), Point::new(250.0, 380.0)),
                Shape::Freehand(vec![Point::new(300.0, 300.0), Point::new(310.0, 320.0), Point::new(330.0, 310.0)]),
                Shape::Text { bounds: Rect::new(100.0, 500.0, 200.0, 30.0), text: "Größe – ok\nline two".into() },
            ];
            for shape in shapes.clone() {
                layer.add(shape, style.clone());
            }
            let edit = PageEdit { page: 0, layer: Some(&layer), replaces: &imported.refs[0] };
            let saved = save(&bytes, &[edit]).unwrap();
            assert!(saved.starts_with(&bytes));

            let back = read(&saved);
            let items = &back.layers[&0].items;
            assert_eq!(items.len(), 1 + shapes.len(), "rotate={rotate}: {items:?}");
            assert!(matches!(items[0].shape, Shape::Highlight(_)));
            assert_eq!(back.layers[&0].notes.values().next().unwrap().contents.as_deref(), Some("Important"));
            for (got, want) in items[1..].iter().zip(&shapes) {
                let (g, w) = (got.bounds(), Annotation { id: 0, shape: want.clone(), style: style.clone() }.bounds());
                let close = (g.x - w.x).abs() < 0.05 && (g.y - w.y).abs() < 0.05 && (g.w - w.w).abs() < 0.05;
                assert!(close, "rotate={rotate}: {want:?} came back as {:?}", got.shape);
                assert_eq!(std::mem::discriminant(&got.shape), std::mem::discriminant(want));
            }
            let Shape::Text { text, .. } = &items[6].shape else { panic!() };
            assert_eq!(text, "Größe – ok\nline two");
            assert!((items[6].style.font_size - style.font_size).abs() < 0.01);

            // The link is untouched, and the page still renders.
            let pdf = Pdf::new(saved.clone()).unwrap();
            assert_eq!(crate::structure::read(&pdf).1[0].len(), 1);
            let page = &pdf.pages()[0];
            let settings = hayro::PixmapSettings {
                x_scale: 0.5,
                y_scale: 0.5,
                bg_color: hayro::vello_cpu::color::palette::css::WHITE,
            };
            let cache = hayro::RenderCache::new();
            let pixmap = hayro::render(
                page,
                &cache,
                &hayro::hayro_interpret::InterpreterSettings::default(),
                &hayro::RenderSettings::default(),
                &settings,
            );
            let rgba = pixmap.take_rgba8(hayro::vello_cpu::peniko::ImageAlphaType::Alpha);
            assert!(rgba.chunks(4).any(|p| p[0] > 150 && p[2] < 100), "the amber shapes are drawn");

            // Deleting everything removes the list.
            let empty = save(&saved, &[PageEdit { page: 0, layer: None, replaces: &back.refs[0] }]).unwrap();
            assert!(read(&empty).is_empty());
            assert_eq!(crate::structure::read(&Pdf::new(empty).unwrap()).1[0].len(), 1);
        }
    }

    #[test]
    fn reads_default_appearance_strings() {
        assert_eq!(parse_da(Some(b"/Helv 14 Tf 1 0 0 rg".to_vec())), (14.0, Some(Color::rgba(1.0, 0.0, 0.0, 1.0))));
        assert_eq!(parse_da(Some(b"0.5 g /F1 0 Tf".to_vec())), (12.0, Some(Color::rgba(0.5, 0.5, 0.5, 1.0))));
        assert_eq!(parse_da(None), (12.0, None));
    }
}
