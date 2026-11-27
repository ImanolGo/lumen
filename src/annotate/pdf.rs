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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

use anyhow::{Context, Result, bail};
use hayro::hayro_interpret::TransformExt;
use hayro::hayro_syntax::Pdf;
use hayro::hayro_syntax::object::{Array, Dict, Name, Number, String as PdfString};
use hayro::kurbo::{self, Affine, PathEl};
use lopdf::{Dictionary, Object, ObjectId, Stream, StringFormat, dictionary};

use super::model::{Annotation, AnnotationLayer, Color, Figure, Note, Point, Rect, Shape, Style, figure_outline};
use super::paint::{self, LINE_HEIGHT, TEXT_PAD};
use crate::structure::decode_text;
use crate::textfont::TextFont;

/// An annotation object: number and generation.
pub type ObjRef = (u32, u16);

/// Protects against hostile files.
const MAX_PER_PAGE: usize = 5_000;
const MAX_POINTS: usize = 100_000;
/// Points (and a point's worth of text) in all annotations of a document together: many of them can
/// share one big array in the file, but not in memory.
const MAX_TOTAL_POINTS: usize = 2_000_000;

/// The editable annotations of a document.
#[derive(Debug, Clone, Default)]
pub struct Imported {
    pub layers: BTreeMap<usize, AnnotationLayer>,
    /// Per page, the objects that became editable: hidden from hayro while they are
    /// edited, and replaced by the layer's contents when the page is saved.
    pub refs: Vec<Vec<ObjRef>>,
    /// There was more than is kept in memory; the rest is left to hayro, as it is in the file.
    pub truncated: bool,
}

impl Imported {
    #[cfg(test)]
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
    let mut out = Imported { layers: BTreeMap::new(), refs: vec![Vec::new(); pages.len()], truncated: false };
    let mut budget = MAX_TOTAL_POINTS;
    for (index, page) in pages.iter().enumerate() {
        let Some(annots) = page.raw().get::<Array<'_>>("Annots") else { continue };
        let to_view = page.initial_transform(true).to_kurbo();
        for annot in crate::structure::dicts(pdf, &annots).take(MAX_PER_PAGE) {
            // Inline dictionaries cannot be hidden or replaced by reference; leave them to hayro.
            let Some(id) = annot.obj_id() else { continue };
            let Ok(generation) = u16::try_from(id.gen_number) else { continue };
            // Hidden annotations stay hidden, and untouched.
            if annot.get::<u32>("F").unwrap_or(0) & 0b10 != 0 {
                continue;
            }
            let Ok(read) = read_one(&annot, to_view, &mut budget) else {
                out.truncated = true;
                return out;
            };
            let Some((annotation, note)) = read else { continue };
            let layer = out.layers.entry(index).or_default();
            let new_id = layer.add(annotation.shape, annotation.style);
            if note != Note::default() {
                layer.notes.insert(new_id, note);
            }
            layer.origins.insert(new_id, (id.obj_number, generation));
            out.refs[index].push((id.obj_number, generation));
        }
    }
    out
}

fn numbers(dict: &Dict<'_>, key: &str) -> Option<Vec<f64>> {
    let array = dict.get::<Array<'_>>(key)?;
    Some(array.iter::<Number>().take(MAX_FEW).map(|n| n.as_f64()).collect())
}

/// For arrays that are short by nature (a rectangle, a colour).
const MAX_FEW: usize = 64;

/// The document's annotations are too big to keep.
#[derive(Debug)]
struct OutOfBudget;

fn spend(budget: &mut usize, amount: usize) -> Result<(), OutOfBudget> {
    *budget = budget.checked_sub(amount).ok_or(OutOfBudget)?;
    Ok(())
}

/// The numbers of an array that may be long, paid for from `budget`.
fn many(array: &Array<'_>, budget: &mut usize) -> Result<Vec<f64>, OutOfBudget> {
    let v: Vec<f64> = array.iter::<Number>().take(MAX_POINTS).map(|n| n.as_f64()).collect();
    spend(budget, v.len() / 2 + 1)?;
    Ok(v)
}

/// One annotation as the model has it: `None` if Lumen does not edit this kind.
fn read_one(annot: &Dict<'_>, to_view: Affine, budget: &mut usize) -> Result<Option<(Annotation, Note)>, OutOfBudget> {
    let mut points = match annot.get::<Name<'_>>("Subtype").as_ref().map(Name::as_str) {
        Some("Ink") => {
            let Some(list) = annot.get::<Array<'_>>("InkList") else { return Ok(None) };
            list.iter::<Array<'_>>().take(1000).map(|stroke| many(&stroke, budget)).collect::<Result<_, _>>()?
        }
        // Other programs' polygons are not read: their corners are not counted against the budget.
        Some("Polygon") if annot.get::<Array<'_>>("LumenFigure").is_some() => {
            match annot.get::<Array<'_>>("Vertices") {
                Some(v) => vec![many(&v, budget)?],
                None => return Ok(None),
            }
        }
        Some("Highlight") => match annot.get::<Array<'_>>("QuadPoints") {
            Some(quads) => vec![many(&quads, budget)?],
            None => return Ok(None),
        },
        _ => Vec::new(),
    };
    let read = read_shape(annot, to_view, &mut points);
    if let Some((annotation, note)) = &read {
        let typed = if let Shape::Text { text, .. } = &annotation.shape { text.len() } else { 0 };
        let noted = [&note.contents, &note.author].into_iter().flatten().map(String::len).sum::<usize>();
        spend(budget, (typed + noted) / size_of::<Point>() + 1)?;
    }
    Ok(read)
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

/// `points`: the strokes of ink, the corners of a highlight, or a polygon's vertices.
fn read_shape(annot: &Dict<'_>, to_view: Affine, points: &mut Vec<Vec<f64>>) -> Option<(Annotation, Note)> {
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
    let dashed = annot.get::<Dict<'_>>("BS").and_then(|bs| bs.get::<Name<'_>>("S")).is_some_and(|s| s.as_str() == "D");
    let mut style = Style { stroke: stroke.unwrap_or(Color::AMBER), fill: None, width, dashed, ..Style::default() };
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
                (true, true) => Shape::DoubleArrow(a, b),
                (_, true) => Shape::Arrow(a, b),
                (true, false) => Shape::Arrow(b, a),
                _ => Shape::Line(a, b),
            }
        }
        // Only Lumen's own figures: other polygons are left as they are.
        "Polygon" => {
            let figure = annot.get::<Array<'_>>("LumenFigure")?;
            let mut parts = figure.flex_iter();
            let kind = Figure::from_name(parts.next::<Name<'_>>()?.as_str())?;
            let corners = parts.next::<f64>().filter(|v| v.is_finite()).unwrap_or(0.0).clamp(0.0, 12.0) as u8;
            let v = points.pop().filter(|v| v.len() >= 6)?;
            let points: Vec<Point> = v.as_chunks::<2>().0.iter().map(|[x, y]| view_point(to_view, *x, *y)).collect();
            let bounds = points.iter().map(|p| Rect::new(p.x, p.y, 0.0, 0.0)).reduce(|a, b| a.union(&b))?;
            if stroke.is_none() {
                style.width = 0.0;
            }
            style.fill = color(annot, "IC", alpha);
            if style.width == 0.0 && style.fill.is_none() {
                return None;
            }
            Shape::Figure { bounds, kind, corners }
        }
        "Ink" => {
            let strokes: Vec<Vec<Point>> = std::mem::take(points)
                .into_iter()
                .map(|v| v.as_chunks::<2>().0.iter().map(|[x, y]| view_point(to_view, *x, *y)).collect::<Vec<_>>())
                .filter(|s: &Vec<Point>| !s.is_empty())
                .collect();
            match strokes.len() {
                0 => return None,
                1 => Shape::Freehand(strokes.into_iter().next()?),
                _ => Shape::Ink(strokes),
            }
        }
        "Highlight" => {
            let quads = points.pop().filter(|q| q.len() >= 8)?;
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
pub(crate) fn parse_da(da: Option<Vec<u8>>) -> (f64, Option<Color>) {
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

fn load(bytes: &[u8], password: Option<&str>) -> Result<lopdf::IncrementalDocument> {
    // An encrypted PDF is decrypted for editing; lopdf encrypts what it appends the same way.
    let options = lopdf::LoadOptions { password: password.map(str::to_owned), ..lopdf::LoadOptions::default() };
    let prev =
        lopdf::Document::load_mem_with_options(bytes, options).context("the PDF could not be read for saving")?;
    if prev.is_encrypted() {
        bail!("changes cannot be saved into this encrypted PDF");
    }
    Ok(lopdf::IncrementalDocument::create_from(bytes.to_vec(), prev))
}

fn finish(mut doc: lopdf::IncrementalDocument) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(doc.get_prev_documents_bytes().len() + 64 * 1024);
    doc.new_document.compress();
    doc.save_to(&mut out).context("the PDF could not be written")?;
    Ok(out)
}

/// The document with the given annotations hidden, so hayro does not draw them while
/// Lumen draws (and edits) them instead. Only ever held in memory.
pub fn hide(bytes: &[u8], refs: &[ObjRef], password: Option<&str>) -> Result<Vec<u8>> {
    let mut doc = load(bytes, password)?;
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

/// A form field to set.
pub struct Fill<'a> {
    pub field: &'a crate::forms::Field,
    pub value: &'a crate::forms::Value,
}

/// Which object each annotation that was not in the file before became: its page, its id, the object.
pub type Placed = Vec<(usize, u64, ObjRef)>;

/// `bytes` with the pages in `edits` carrying their new annotations, and the fields in `fills` filled
/// in. An annotation that was not changed keeps its object as it is; one that was keeps
/// everything about it that Lumen does not model.
pub fn save(
    bytes: &[u8],
    edits: &[PageEdit<'_>],
    fills: &[Fill<'_>],
    password: Option<&str>,
) -> Result<(Vec<u8>, Placed)> {
    let hayro_doc = Pdf::new_with_password(bytes.to_vec(), password.unwrap_or(""))
        .map_err(|_| anyhow::anyhow!("the PDF could not be read"))?;
    let to_view = transforms(&hayro_doc);
    let before = if edits.is_empty() { Imported::default() } else { import(&hayro_doc) };
    drop(hayro_doc);

    let mut doc = load(bytes, password)?;
    let pages = doc.get_prev_documents().get_pages();
    if pages.len() != to_view.len() {
        bail!("the pages of this PDF could not be matched up for saving");
    }
    let mut placed = Placed::new();
    for edit in edits {
        let Some(&page_id) = pages.get(&(edit.page as u32 + 1)) else { continue };
        let to_view = to_view[edit.page];
        let to_user = to_view.inverse();
        // The annotations of the file, as they were read.
        let originals: HashMap<ObjRef, (&Annotation, Option<&Note>)> = before
            .layers
            .get(&edit.page)
            .into_iter()
            .flat_map(|l| l.items.iter().map(|a| (l.origins[&a.id], (a, l.notes.get(&a.id)))))
            .collect();

        // First what each annotation becomes: the object it was, changed or not, or a new one.
        let mut now: Vec<ObjectId> = Vec::new();
        for a in edit.layer.iter().flat_map(|l| &l.items) {
            let layer = edit.layer.expect("there are items");
            let note = layer.notes.get(&a.id);
            let wanted = annotation_dict(a, note, page_id, to_user);
            let origin = layer.origins.get(&a.id).filter(|o| {
                // A copy of an annotation is a new one.
                !now.contains(o) && doc.get_prev_documents().get_dictionary(**o).is_ok()
            });
            let was = origin.and_then(|o| originals.get(o)).map(|(a, n)| annotation_dict(a, *n, page_id, to_user));
            if let (Some(&origin), Some(was)) = (origin, &was)
                && same_dict(was, &wanted)
            {
                now.push(origin);
                continue;
            }
            let text = if let Shape::Text { text, .. } = &a.shape { text.as_str() } else { "" };
            let font = TextFont::for_text(&mut doc.new_document, text);
            let ap = doc.new_document.add_object(appearance(a, to_view, &font));
            let changed = match (origin, was) {
                (Some(&origin), Some(was)) if same(was.get(b"Subtype").ok(), wanted.get(b"Subtype").ok()) => {
                    let old = doc.get_prev_documents().get_dictionary(origin)?.clone();
                    Some((origin, overlaid(old, &was, &wanted)))
                }
                _ => None,
            };
            let id = match changed {
                Some((origin, mut dict)) => {
                    dict.set("AP", dictionary! { "N" => ap });
                    dict.remove(b"AS");
                    doc.new_document.set_object(origin, dict);
                    origin
                }
                None => {
                    let mut dict = wanted;
                    dict.set("AP", dictionary! { "N" => ap });
                    let id = doc.new_document.add_object(dict);
                    placed.push((edit.page, a.id, id));
                    id
                }
            };
            now.push(id);
        }

        // Then the page's list: what is gone is taken out (with its pop-up note), the rest keeps
        // its place, and what is new comes last.
        let kept: HashSet<ObjectId> = now.iter().copied().collect();
        let gone: HashSet<ObjectId> = edit.replaces.iter().copied().filter(|r| !kept.contains(r)).collect();
        let prev = doc.get_prev_documents();
        let old: Vec<Object> = prev
            .get_dictionary(page_id)
            .ok()
            .and_then(|page| page.get(b"Annots").ok())
            .and_then(|a| prev.dereference(a).ok())
            .and_then(|(_, a)| a.as_array().ok().cloned())
            .unwrap_or_default();
        let mut annots: Vec<Object> = old
            .into_iter()
            .filter(|o| match o {
                Object::Reference(id) if gone.contains(id) => false,
                Object::Reference(id) => !prev
                    .get_dictionary(*id)
                    .ok()
                    .and_then(|d| d.get(b"Parent").ok())
                    .and_then(|p| p.as_reference().ok())
                    .is_some_and(|parent| gone.contains(&parent)),
                _ => true,
            })
            .collect();
        let listed: HashSet<ObjectId> = annots.iter().filter_map(|o| o.as_reference().ok()).collect();
        annots.extend(now.iter().filter(|id| !listed.contains(id)).map(|id| Object::Reference(*id)));
        // Those Lumen edits are drawn in the layer's order, whichever places they have.
        let mut in_order = now.iter();
        for slot in &mut annots {
            if slot.as_reference().is_ok_and(|id| kept.contains(&id))
                && let Some(id) = in_order.next()
            {
                *slot = Object::Reference(*id);
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
    for fill in fills {
        crate::forms::fill(&mut doc, fill.field, fill.value).context("a form field could not be filled in")?;
    }
    Ok((finish(doc)?, placed))
}

/// What says nothing about how an annotation looks, or is new every time.
const NOT_MODELLED: [&[u8]; 4] = [b"NM", b"P", b"F", b"Type"];

/// Whether two values are the same but for rounding.
fn same(a: Option<&Object>, b: Option<&Object>) -> bool {
    match (a, b) {
        (Some(Object::Array(a)), Some(Object::Array(b))) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(Some(a), Some(b)))
        }
        (Some(Object::Dictionary(a)), Some(Object::Dictionary(b))) => same_dict(a, b),
        (Some(a), Some(b)) => match (a.as_float(), b.as_float()) {
            (Ok(a), Ok(b)) => (a - b).abs() < 0.02,
            _ => a == b,
        },
        (None, None) => true,
        _ => false,
    }
}

fn same_dict(a: &Dictionary, b: &Dictionary) -> bool {
    a.iter()
        .map(|(k, _)| k)
        .chain(b.iter().map(|(k, _)| k))
        .all(|key| NOT_MODELLED.contains(&key.as_slice()) || same(a.get(key).ok(), b.get(key).ok()))
}

/// The annotation `old` of the file, which Lumen read as `was`, now that it should be `wanted`:
/// only what differs between those two is changed.
fn overlaid(old: Dictionary, was: &Dictionary, wanted: &Dictionary) -> Dictionary {
    let mut out = old.clone();
    let differs = |key: &[u8]| !same(was.get(key).ok(), wanted.get(key).ok());
    let moved = differs(b"Rect");
    let keys: Vec<Vec<u8>> = was.iter().chain(wanted.iter()).map(|(k, _)| k.clone()).collect();
    for key in keys {
        // The inset goes with the rectangle: the file's own need not be the one Lumen would write.
        let inset = key == b"RD" && moved;
        if NOT_MODELLED.contains(&key.as_slice()) || !(differs(&key) || inset) {
            continue;
        }
        match wanted.get(&key) {
            // The width changes; whether it is dashed does not.
            Ok(Object::Dictionary(border)) if key == b"BS" => {
                let mut style = old.get(b"BS").and_then(Object::as_dict).cloned().unwrap_or_else(|_| border.clone());
                if let Ok(width) = border.get(b"W") {
                    style.set("W", width.clone());
                }
                // Dashed or not is Lumen's to change; a dashed line's own pattern is kept.
                let dash = |d: &Dictionary| d.get(b"S").and_then(Object::as_name).is_ok_and(|s| s == b"D");
                let was_dashed = was.get(b"BS").and_then(Object::as_dict).is_ok_and(dash);
                if dash(border) != was_dashed {
                    for key in [&b"S"[..], b"D"] {
                        match border.get(key) {
                            Ok(v) => style.set(key, v.clone()),
                            Err(_) => drop(style.remove(key)),
                        }
                    }
                }
                out.set("BS", style);
            }
            Ok(value) => out.set(key, value.clone()),
            Err(_) => drop(out.remove(&key)),
        }
    }
    if wanted.get(b"Subtype").and_then(Object::as_name).is_ok_and(|s| s == b"Line") {
        // Read backwards, because only its start has an arrow head.
        let heads = old.get(b"LE").and_then(Object::as_array).cloned().unwrap_or_default();
        let head = |i: usize| heads.get(i).and_then(|o| o.as_name().ok()).is_some_and(|n| n.ends_with(b"Arrow"));
        if !differs(b"LE")
            && head(0)
            && !head(1)
            && let Ok(Object::Array(l)) = out.get(b"L").cloned()
            && differs(b"L")
            && let [a, b, c, d] = l.as_slice()
        {
            out.set("L", vec![c.clone(), d.clone(), a.clone(), b.clone()]);
        }
    }
    if differs(b"Contents") {
        // The same text, formatted.
        out.remove(b"RC");
    }
    if moved && wanted.get(b"Subtype").and_then(Object::as_name).is_ok_and(|s| s == b"FreeText") {
        // A callout line would point from where the box was.
        for key in [&b"CL"[..], b"RD"] {
            out.remove(key);
        }
        if out.get(b"IT").and_then(Object::as_name).is_ok_and(|n| n == b"FreeTextCallout") {
            out.remove(b"IT");
        }
    }
    out
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
pub(crate) fn pdf_text(s: &str) -> Object {
    if s.is_ascii() {
        Object::String(s.as_bytes().to_vec(), StringFormat::Literal)
    } else {
        let mut bytes = vec![0xFE, 0xFF];
        bytes.extend(s.encode_utf16().flat_map(u16::to_be_bytes));
        Object::String(bytes, StringFormat::Hexadecimal)
    }
}

fn annotation_dict(a: &Annotation, note: Option<&Note>, page: ObjectId, to_user: Affine) -> Dictionary {
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
    let border = if style.dashed {
        let [on, off] = paint::dashes(style.width);
        dictionary! { "W" => real(style.width), "S" => "D", "D" => vec![real(on), real(off)] }
    } else {
        dictionary! { "W" => real(style.width), "S" => "S" }
    };
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
        Shape::Figure { bounds, kind, corners } => {
            d.set("Subtype", "Polygon");
            if style.width > 0.0 {
                d.set("C", rgb(style.stroke));
            }
            if let Some(fill) = style.fill {
                d.set("IC", rgb(fill));
            }
            d.set("BS", border);
            let outline = figure_outline(*bounds, *kind, *corners);
            d.set("Vertices", outline.iter().flat_map(|p| user_point(to_user, *p)).collect::<Vec<_>>());
            // Which figure it is, for Lumen to read back; other programs see a polygon.
            d.set("LumenFigure", vec![Object::Name(kind.name().as_bytes().to_vec()), Object::Integer(*corners as i64)]);
        }
        Shape::Line(p, q) | Shape::Arrow(p, q) | Shape::DoubleArrow(p, q) => {
            d.set("Subtype", "Line");
            d.set("C", rgb(style.stroke));
            d.set("BS", border);
            let mut l = Vec::with_capacity(4);
            l.extend(user_point(to_user, *p));
            l.extend(user_point(to_user, *q));
            d.set("L", l);
            let start = if matches!(a.shape, Shape::DoubleArrow(..)) { "OpenArrow" } else { "None" };
            if !matches!(a.shape, Shape::Line(..)) {
                d.set("LE", vec![Object::Name(start.as_bytes().to_vec()), Object::Name(b"OpenArrow".to_vec())]);
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
    d
}

/// The appearance stream: drawn in view coordinates, with a /Matrix that maps them to
/// user space, so the same code serves rotated and unrotated pages.
fn appearance(a: &Annotation, to_view: Affine, font: &TextFont) -> Stream {
    let outer = paint::outer_bounds(a);
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
        let mut at = kurbo::Point::ZERO;
        for el in prim.path.elements() {
            let _ = match el {
                PathEl::MoveTo(p) => writeln!(c, "{:.2} {:.2} m", p.x, p.y),
                PathEl::LineTo(p) => writeln!(c, "{:.2} {:.2} l", p.x, p.y),
                // A PDF has cubic curves only.
                PathEl::QuadTo(q, p2) => {
                    let c1 = at + (*q - at) * (2.0 / 3.0);
                    let c2 = *p2 + (*q - *p2) * (2.0 / 3.0);
                    let (x1, y1, x2, y2) = (c1.x, c1.y, c2.x, c2.y);
                    writeln!(c, "{x1:.2} {y1:.2} {x2:.2} {y2:.2} {:.2} {:.2} c", p2.x, p2.y)
                }
                PathEl::CurveTo(p1, p2, p3) => {
                    writeln!(c, "{:.2} {:.2} {:.2} {:.2} {:.2} {:.2} c", p1.x, p1.y, p2.x, p2.y, p3.x, p3.y)
                }
                PathEl::ClosePath => writeln!(c, "h"),
            };
            at = el.end_point().unwrap_or(at);
        }
        if let Some(f) = prim.fill {
            let _ = writeln!(c, "{:.3} {:.3} {:.3} rg", f.r, f.g, f.b);
        }
        if let Some((s, w)) = prim.stroke {
            let _ = writeln!(c, "{:.3} {:.3} {:.3} RG {:.2} w", s.r, s.g, s.b, w);
            if prim.dashed {
                let [on, off] = paint::dashes(w);
                let _ = writeln!(c, "[{on:.2} {off:.2}] 0 d");
            }
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
        resources.set("Font", dictionary! { "Helv" => font.resource() });
        let col = block.color;
        let _ = writeln!(c, "q BT /Helv {:.2} Tf {:.3} {:.3} {:.3} rg", block.size, col.r, col.g, col.b);
        if alpha < 1.0 {
            c.push_str("/A gs\n");
        }
        let pad = block.size * TEXT_PAD;
        for (i, line) in block.lines.iter().enumerate() {
            let baseline = block.bounds.y + pad + block.size * (i as f64 * LINE_HEIGHT + 0.95);
            // y is flipped in view space, so the glyphs are flipped back.
            let _ = writeln!(c, "1 0 0 -1 {:.2} {:.2} Tm <{}> Tj", block.bounds.x + pad, baseline, font.encoded(line));
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

    /// `n` ink annotations that all draw the same stroke of `points` points.
    fn shared_ink(n: usize, points: usize) -> Vec<u8> {
        let mut doc = lopdf::Document::with_version("1.7");
        let stroke = doc.add_object(vec![Object::Real(5.0); points * 2]);
        let pages = doc.new_object_id();
        let annots: Vec<Object> = (0..n)
            .map(|_| {
                let rect: Vec<Object> = vec![0.into(), 0.into(), 10.into(), 10.into()];
                let ink = dictionary! { "Subtype" => "Ink", "Rect" => rect, "InkList" => vec![stroke.into()] };
                doc.add_object(ink).into()
            })
            .collect();
        let media: Vec<Object> = vec![0.into(), 0.into(), 100.into(), 100.into()];
        let page = doc
            .add_object(dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => media, "Annots" => annots });
        let tree = dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 };
        doc.objects.insert(pages, tree.into());
        let root = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
        doc.trailer.set("Root", root);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    #[test]
    fn many_annotations_sharing_one_stroke_do_not_fill_the_memory() {
        let small = read(&shared_ink(20, 50));
        assert!(!small.truncated && small.layers[&0].items.len() == 20);
        let hostile = read(&shared_ink(200, 50_000));
        assert!(hostile.truncated);
        let kept: usize = hostile.layers[&0].items.len();
        assert!(kept > 0 && kept * 50_000 <= MAX_TOTAL_POINTS, "{kept}");
        assert_eq!(hostile.refs[0].len(), kept, "what was not read is not replaced when saving");
    }

    #[test]
    fn hiding_keeps_the_rest_of_the_document() {
        let bytes = fixture(false);
        let hidden = hide(&bytes, &read(&bytes).refs[0], None).unwrap();
        assert!(hidden.starts_with(&bytes), "an incremental update only appends");
        let again = read(&hidden);
        assert!(again.is_empty(), "hidden annotations are not editable twice");
        let pdf = Pdf::new(hidden).unwrap();
        assert_eq!(crate::structure::read(&pdf).1[0].len(), 1, "the link is still there");
    }

    #[test]
    fn figures_double_arrows_and_dashes_come_back_as_they_were() {
        for rotate in [false, true] {
            let bytes = fixture(rotate);
            let imported = read(&bytes);
            let mut layer = imported.layers[&0].clone();
            let dashed = Style { dashed: true, ..Style::default() };
            let shapes = [
                (Shape::Figure { bounds: Rect::new(100.0, 100.0, 80.0, 60.0), kind: Figure::Star, corners: 7 }, false),
                (
                    Shape::Figure { bounds: Rect::new(200.0, 100.0, 60.0, 60.0), kind: Figure::Polygon, corners: 6 },
                    true,
                ),
                (
                    Shape::Figure { bounds: Rect::new(100.0, 200.0, 120.0, 70.0), kind: Figure::Bubble, corners: 0 },
                    false,
                ),
                (
                    Shape::Figure { bounds: Rect::new(250.0, 200.0, 90.0, 40.0), kind: Figure::Rounded, corners: 0 },
                    true,
                ),
                (Shape::DoubleArrow(Point::new(100.0, 400.0), Point::new(250.0, 380.0)), true),
                (Shape::Rect(Rect::new(300.0, 400.0, 50.0, 50.0)), true),
            ];
            for (shape, dash) in shapes.clone() {
                layer.add(shape, if dash { dashed.clone() } else { Style::default() });
            }
            let edit = PageEdit { page: 0, layer: Some(&layer), replaces: &imported.refs[0] };
            let (saved, _) = save(&bytes, &[edit], &[], None).unwrap();
            let back = read(&saved);
            let items = &back.layers[&0].items[1..];
            assert_eq!(items.len(), shapes.len());
            for (got, (want, dash)) in items.iter().zip(&shapes) {
                assert_eq!(got.style.dashed, *dash, "rotate={rotate}: {want:?}");
                match (&got.shape, want) {
                    (
                        Shape::Figure { bounds: g, kind: gk, corners: gc },
                        Shape::Figure { bounds: w, kind: wk, corners: wc },
                    ) => {
                        assert_eq!((gk, gc), (wk, wc));
                        let close =
                            [(g.x, w.x), (g.y, w.y), (g.w, w.w), (g.h, w.h)].iter().all(|(a, b)| (a - b).abs() < 0.05);
                        assert!(close, "rotate={rotate}: {g:?} {w:?}");
                    }
                    (Shape::DoubleArrow(a, b), Shape::DoubleArrow(c, d)) => {
                        assert!((a.x - c.x).abs() < 0.05 && (b.y - d.y).abs() < 0.05);
                    }
                    (Shape::Rect(_), Shape::Rect(_)) => {}
                    other => panic!("{other:?}"),
                }
            }
            // Other programs see a polygon with its corners, and a line with two heads.
            let doc = lopdf::Document::load_mem(&saved).unwrap();
            let kinds: Vec<Vec<u8>> = doc
                .objects
                .values()
                .filter_map(|o| o.as_dict().ok())
                .filter_map(|d| d.get(b"Subtype").and_then(Object::as_name).ok().map(<[u8]>::to_vec))
                .collect();
            assert_eq!(kinds.iter().filter(|k| k.as_slice() == b"Polygon").count(), 4);
            let line = doc
                .objects
                .values()
                .filter_map(|o| o.as_dict().ok())
                .find(|d| d.get(b"Subtype").and_then(Object::as_name).is_ok_and(|s| s == b"Line"))
                .unwrap();
            let ends = line.get(b"LE").and_then(Object::as_array).unwrap();
            assert!(ends.iter().all(|e| e.as_name().is_ok_and(|n| n == b"OpenArrow")));
            let border = line.get(b"BS").and_then(Object::as_dict).unwrap();
            assert_eq!(border.get(b"S").and_then(Object::as_name).unwrap(), b"D");
        }
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
            let (saved, placed) = save(&bytes, &[edit], &[], None).unwrap();
            assert_eq!(placed.len(), shapes.len());
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
            let nothing = PageEdit { page: 0, layer: None, replaces: &back.refs[0] };
            let empty = save(&saved, &[nothing], &[], None).unwrap().0;
            assert!(read(&empty).is_empty());
            assert_eq!(crate::structure::read(&Pdf::new(empty).unwrap()).1[0].len(), 1);
        }
    }

    /// A page with annotations as another program writes them (and a null among them).
    fn foreign() -> Vec<u8> {
        use pdf_writer::types::AnnotationType;
        use pdf_writer::{Date, Finish, Name, Rect as PRect, Ref, TextStr};
        let r = Ref::new;
        let mut w = pdf_writer::Pdf::new();
        w.catalog(r(1)).pages(r(2));
        w.pages(r(2)).kids([r(3)]).count(1);
        let mut page = w.page(r(3));
        page.parent(r(2)).media_box(PRect::new(0.0, 0.0, 600.0, 800.0));
        let mut list = page.insert(Name(b"Annots")).array();
        list.item(r(4)).item(pdf_writer::Null).item(r(99)).item(r(5)).item(r(6)).item(r(7));
        list.finish();
        page.finish();
        let mut square = w.annotation(r(4));
        square
            .subtype(AnnotationType::Square)
            .rect(PRect::new(100.0, 100.0, 200.0, 150.0))
            .color_rgb(1.0, 0.0, 0.0)
            .modified(Date::new(2020).month(5).day(6))
            .contents(TextStr("look here"));
        square.pair(Name(b"CreationDate"), pdf_writer::Str(b"D:20200101"));
        square.insert(Name(b"BE")).dict().pair(Name(b"S"), Name(b"C"));
        square.insert(Name(b"BS")).dict().pair(Name(b"W"), 2).pair(Name(b"S"), Name(b"D"));
        square.insert(Name(b"AP")).dict().pair(Name(b"N"), r(8));
        square.finish();
        let mut line = w.annotation(r(5));
        line.subtype(AnnotationType::Line).rect(PRect::new(10.0, 10.0, 90.0, 60.0)).color_rgb(0.0, 0.0, 1.0);
        line.insert(Name(b"L")).array().items([10.0, 10.0, 90.0, 60.0]);
        line.insert(Name(b"LE")).array().items([Name(b"ClosedArrow"), Name(b"Circle")]);
        line.finish();
        let mut popup = w.annotation(r(6));
        popup.pair(Name(b"Subtype"), Name(b"Popup"));
        popup.rect(PRect::new(300.0, 300.0, 400.0, 400.0));
        popup.pair(Name(b"Parent"), r(4));
        popup.finish();
        let mut link = w.annotation(r(7));
        link.subtype(AnnotationType::Link).rect(PRect::new(72.0, 690.0, 200.0, 720.0));
        link.action().action_type(pdf_writer::types::ActionType::Uri).uri(pdf_writer::Str(b"https://example.com/"));
        link.finish();
        w.form_xobject(r(8), b"1 0 0 RG 0 0 100 50 re S").bbox(PRect::new(0.0, 0.0, 100.0, 50.0));
        w.finish()
    }

    fn object(bytes: &[u8], id: ObjectId) -> Dictionary {
        lopdf::Document::load_mem(bytes).unwrap().get_dictionary(id).unwrap().clone()
    }

    fn annots(bytes: &[u8]) -> Vec<Object> {
        let page = object(bytes, (3, 0));
        page.get(b"Annots").unwrap().as_array().unwrap().clone()
    }

    #[test]
    fn a_null_among_the_annotations_does_not_hide_the_rest() {
        let imported = read(&foreign());
        assert_eq!(imported.refs[0], [(4, 0), (5, 0)]);
        assert_eq!(crate::structure::read(&Pdf::new(foreign()).unwrap()).1[0].len(), 1, "the link too");
    }

    #[test]
    fn annotations_of_other_programs_keep_what_lumen_does_not_model() {
        let bytes = foreign();
        let imported = read(&bytes);
        let mut layer = imported.layers[&0].clone();
        // Something new on the page: the others are not touched at all.
        let new = layer.add(Shape::Rect(Rect::new(400.0, 400.0, 20.0, 20.0)), Style::default());
        let replaces = &imported.refs[0];
        let (saved, placed) = save(&bytes, &[PageEdit { page: 0, layer: Some(&layer), replaces }], &[], None).unwrap();
        assert_eq!((placed.len(), placed[0].1), (1, new));
        for id in [(4, 0), (5, 0), (6, 0), (7, 0)] {
            assert_eq!(object(&saved, id), object(&bytes, id), "{id:?}");
        }
        let appended = String::from_utf8_lossy(&saved[bytes.len()..]).into_owned();
        assert!(!appended.contains("4 0 obj") && !appended.contains("5 0 obj"), "nor written again");
        assert_eq!(annots(&saved)[..annots(&bytes).len()], annots(&bytes), "and the list keeps its order");

        // Moved, and wider: only that changes.
        let style = &mut layer.items[0].style;
        style.width = 5.0;
        layer.items[0].shape = Shape::Rect(Rect::new(120.0, 620.0, 100.0, 50.0));
        let (saved, _) = save(&bytes, &[PageEdit { page: 0, layer: Some(&layer), replaces }], &[], None).unwrap();
        let (old, now) = (object(&bytes, (4, 0)), object(&saved, (4, 0)));
        for key in [&b"M"[..], b"CreationDate", b"BE", b"Contents", b"C"] {
            assert_eq!(now.get(key).unwrap(), old.get(key).unwrap(), "{}", String::from_utf8_lossy(key));
        }
        let border = now.get(b"BS").unwrap().as_dict().unwrap();
        assert_eq!(border.get(b"S").unwrap().as_name().unwrap(), b"D", "still dashed");
        assert_eq!(border.get(b"W").unwrap().as_float().unwrap(), 5.0);
        assert_ne!(now.get(b"AP").unwrap(), old.get(b"AP").unwrap());
        assert!(annots(&saved).contains(&Object::Reference((6, 0))), "its pop-up stays");
        let Shape::Rect(r) = read(&saved).layers[&0].items[0].shape else { panic!() };
        assert!((r.x - 120.0).abs() < 0.1 && (r.y - 620.0).abs() < 0.1 && (r.w - 100.0).abs() < 0.1, "{r:?}");

        // A line with its arrow at the start keeps it there.
        let mut layer = imported.layers[&0].clone();
        let Shape::Arrow(tail, tip) = layer.items[1].shape else { panic!() };
        let moved = |p: Point| Point::new(p.x + 10.0, p.y);
        layer.items[1].shape = Shape::Arrow(moved(tail), moved(tip));
        let (saved, _) = save(&bytes, &[PageEdit { page: 0, layer: Some(&layer), replaces }], &[], None).unwrap();
        let line = object(&saved, (5, 0));
        assert_eq!(line.get(b"LE").unwrap(), object(&bytes, (5, 0)).get(b"LE").unwrap());
        assert_eq!(read(&saved).layers[&0].items[1].shape, layer.items[1].shape);

        // Deleted: it goes, with its pop-up.
        layer.remove(layer.items[0].id);
        let (saved, _) = save(&bytes, &[PageEdit { page: 0, layer: Some(&layer), replaces }], &[], None).unwrap();
        assert!(!annots(&saved).contains(&Object::Reference((4, 0))));
        assert!(!annots(&saved).contains(&Object::Reference((6, 0))));
    }

    #[test]
    fn text_in_other_scripts_reads_back_from_the_page() {
        let mut layer = AnnotationLayer::default();
        let text = "Škoda œuvre Привет";
        layer.add(
            Shape::Text { bounds: Rect::new(50.0, 50.0, 400.0, 40.0), text: text.into() },
            Style { font_size: 20.0, ..Style::default() },
        );
        let edit = PageEdit { page: 0, layer: Some(&layer), replaces: &[] };
        let (saved, _) = save(&fixture(false), &[edit], &[], None).unwrap();
        let Shape::Text { text: back, .. } = &read(&saved).layers[&0].items[1].shape else { panic!() };
        assert_eq!(back, text);
        let doc = lopdf::Document::load_mem(&saved).unwrap();
        let embedded = doc.objects.values().any(|o| {
            o.as_dict().is_ok_and(|d| d.get(b"Subtype").and_then(Object::as_name).is_ok_and(|s| s == b"Type0"))
        });
        // Every machine the tests run on need not have a Cyrillic font.
        eprintln!("embedded a font: {embedded}");
    }

    #[test]
    fn reads_default_appearance_strings() {
        assert_eq!(parse_da(Some(b"/Helv 14 Tf 1 0 0 rg".to_vec())), (14.0, Some(Color::rgba(1.0, 0.0, 0.0, 1.0))));
        assert_eq!(parse_da(Some(b"0.5 g /F1 0 Tf".to_vec())), (12.0, Some(Color::rgba(0.5, 0.5, 0.5, 1.0))));
        assert_eq!(parse_da(None), (12.0, None));
    }
}
