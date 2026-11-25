//! PDF forms (AcroForm): reading the fields, and writing what was filled in.
//!
//! Fields are read with hayro's parser. While a form is open, the widgets of the
//! fields that can be filled in are hidden from the renderer and Lumen shows its
//! own inputs in their place. Saving sets each changed field's value and gives
//! its widgets a matching appearance, in the same incremental update as markup,
//! so every PDF viewer shows the filled-in form.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::rc::Rc;

use hayro::hayro_interpret::TransformExt;
use hayro::hayro_syntax::Pdf;
use hayro::hayro_syntax::object::{Array, Dict, MaybeRef, Name, Number, Object, String as PdfString};
use hayro::kurbo::{Affine, Rect};
use lopdf::{Object as LObject, dictionary};

use crate::annotate::pdf::{ObjRef, parse_da, pdf_text};
use crate::structure::decode_text;
use crate::textfont::{TextFont, wrapped};

const MAX_FIELDS: usize = 5_000;
const MAX_DEPTH: u8 = 16;
/// Bytes (about) that a whole form may take in memory: many fields can share one big array in
/// the file, but not here.
const MAX_FORM_BYTES: usize = 32 << 20;

const READ_ONLY: u32 = 1;
const MULTILINE: u32 = 1 << 12;
const PASSWORD: u32 = 1 << 13;
const COMB: u32 = 1 << 24;
const RADIO: u32 = 1 << 15;
const PUSH: u32 = 1 << 16;
#[cfg(test)]
const COMBO: u32 = 1 << 17;

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Text {
        multiline: bool,
        password: bool,
        max_len: Option<usize>,
    },
    Check,
    /// One of a group: choosing a widget turns the others off.
    Radio,
    /// A drop-down (combo) or list of options.
    Choice {
        options: Vec<Choice>,
    },
    /// A place to sign: Lumen signs it with ink.
    Signature,
    /// A button that runs a script; Lumen runs none and leaves it to the renderer.
    Push,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    /// What is stored in the field.
    pub export: String,
    /// What is shown.
    pub display: String,
}

/// What a field holds.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Text(String),
    /// A check box or radio group: the name of the state that is on, or `None` for off.
    State(Option<String>),
}

#[derive(Debug, Clone)]
pub struct Widget {
    pub page: usize,
    /// In view coordinates (points from the page's top-left).
    pub rect: Rect,
    pub obj: ObjRef,
    /// A check box's or radio button's "on" state name.
    pub on: Option<String>,
    /// Width and height in the widget's own space (unrotated), for its appearance.
    size: (f64, f64),
}

#[derive(Debug, Clone)]
pub struct Field {
    /// The full name, such as `address.city`.
    pub name: String,
    pub kind: Kind,
    pub read_only: bool,
    pub obj: ObjRef,
    pub widgets: Vec<Widget>,
    pub value: Value,
    /// From the default appearance: 0 means "fit the box".
    font_size: f64,
    color: [f32; 3],
    /// 0 left, 1 centred, 2 right.
    quadding: u8,
    /// Each character in a cell of its own.
    comb: Option<usize>,
}

impl Field {
    /// Whether Lumen shows its own input for this field (and hides the renderer's).
    pub fn fillable(&self) -> bool {
        !self.read_only && matches!(self.kind, Kind::Text { .. } | Kind::Check | Kind::Radio | Kind::Choice { .. })
    }

    /// Text size in points for a widget `height` points tall.
    pub fn text_size(&self, height: f64) -> f64 {
        if self.font_size > 0.0 {
            return self.font_size;
        }
        match self.kind {
            Kind::Text { multiline: true, .. } => 10.0,
            _ => ((height - 4.0) * 0.72).clamp(4.0, 14.0),
        }
    }

    pub fn color(&self) -> [f32; 3] {
        self.color
    }
}

#[derive(Debug, Clone, Default)]
pub struct Form {
    pub fields: Vec<Field>,
    /// There was more than is kept in memory; the rest is only shown, not filled in.
    pub truncated: bool,
}

impl Form {
    /// The widgets Lumen draws itself, to hide from the renderer.
    pub fn hidden_widgets(&self) -> Vec<ObjRef> {
        self.fields.iter().filter(|f| f.fillable()).flat_map(|f| f.widgets.iter().map(|w| w.obj)).collect()
    }

    pub fn values(&self) -> Vec<Value> {
        self.fields.iter().map(|f| f.value.clone()).collect()
    }
}

// ---------------------------------------------------------------- reading

fn obj_ref(dict: &Dict<'_>) -> Option<ObjRef> {
    let id = dict.obj_id()?;
    Some((id.obj_number, u16::try_from(id.gen_number).ok()?))
}

fn text(dict: &Dict<'_>, key: &str) -> Option<String> {
    dict.get::<PdfString<'_>>(key).map(|s| decode_text(s.as_bytes()))
}

/// What a field inherits from its parents.
#[derive(Clone, Default)]
struct Inherited {
    ft: Option<String>,
    ff: u32,
    v: Option<Value>,
    da: Option<Vec<u8>>,
    q: u8,
    max_len: Option<usize>,
    options: Option<Rc<Vec<Choice>>>,
}

struct Reader<'a> {
    annot_pages: HashMap<ObjRef, usize>,
    to_view: Vec<Affine>,
    pdf: &'a Pdf,
    seen: HashSet<ObjRef>,
    fields: Vec<Field>,
    /// What is left of [`MAX_FORM_BYTES`]; `None` once it has run out.
    budget: Option<usize>,
}

/// Reads the form of `pdf`; empty if it has none.
pub fn read(pdf: &Pdf) -> Form {
    let xref = pdf.xref();
    let Some(acro) = xref.get::<Dict<'_>>(xref.root_id()).and_then(|c| c.get::<Dict<'_>>("AcroForm")) else {
        return Form::default();
    };
    let Some(fields) = acro.get::<Array<'_>>("Fields") else { return Form::default() };
    let mut annot_pages = HashMap::new();
    for (i, page) in pdf.pages().iter().enumerate() {
        if let Some(annots) = page.raw().get::<Array<'_>>("Annots") {
            for item in annots.raw_iter() {
                if let MaybeRef::Ref(r) = item
                    && let Ok(generation) = u16::try_from(r.gen_number)
                {
                    annot_pages.insert((r.obj_number, generation), i);
                }
            }
        }
    }
    let to_view = pdf.pages().iter().map(|p| p.initial_transform(true).to_kurbo()).collect();
    let mut reader =
        Reader { annot_pages, to_view, pdf, seen: HashSet::new(), fields: Vec::new(), budget: Some(MAX_FORM_BYTES) };
    let base = Inherited {
        da: acro.get::<PdfString<'_>>("DA").map(|s| s.as_bytes().to_vec()),
        q: acro.get::<u32>("Q").unwrap_or(0).min(2) as u8,
        ..Inherited::default()
    };
    for field in crate::structure::dicts(pdf, &fields) {
        reader.walk(&field, &base, "", 0);
    }
    Form { truncated: reader.budget.is_none(), fields: reader.fields }
}

fn choice(option: Object<'_>) -> Option<Choice> {
    match option {
        Object::String(s) => {
            let t = decode_text(s.as_bytes());
            Some(Choice { export: t.clone(), display: t })
        }
        Object::Array(pair) => {
            let mut it = pair.iter::<PdfString<'_>>();
            let export = decode_text(it.next()?.as_bytes());
            let display = it.next().map(|d| decode_text(d.as_bytes())).unwrap_or_else(|| export.clone());
            Some(Choice { export, display })
        }
        _ => None,
    }
}

fn choice_bytes(c: &Choice) -> usize {
    size_of::<Choice>() + c.export.len() + c.display.len()
}

/// The name of a button's state, or `None` for "Off". Names are bytes, not text: each byte becomes the
/// character of that number, so that any name survives.
fn state_of(name: &Name<'_>) -> Option<String> {
    let bytes: &[u8] = name.as_ref();
    (bytes != b"Off").then(|| bytes.iter().map(|b| char::from(*b)).collect())
}

fn state_bytes(state: &str) -> Vec<u8> {
    state.chars().map(|c| u8::try_from(c).unwrap_or(b'?')).collect()
}

fn value_of(dict: &Dict<'_>) -> Option<Value> {
    match dict.get::<Object<'_>>("V")? {
        Object::String(s) => Some(Value::Text(decode_text(s.as_bytes()))),
        Object::Name(n) => Some(Value::State(state_of(&n))),
        Object::Array(a) => a.iter::<PdfString<'_>>().next().map(|s| Value::Text(decode_text(s.as_bytes()))),
        _ => None,
    }
}

impl<'a> Reader<'a> {
    /// Takes `bytes` from the budget; false (from then on) when it has run out.
    fn spend(&mut self, bytes: usize) -> bool {
        self.budget = self.budget.and_then(|left| left.checked_sub(bytes));
        self.budget.is_some()
    }

    fn options(&mut self, array: &Array<'_>) -> Vec<Choice> {
        let mut out = Vec::new();
        for option in array.iter::<Object<'_>>().take(10_000) {
            let Some(choice) = choice(option) else { continue };
            if !self.spend(choice_bytes(&choice)) {
                break;
            }
            out.push(choice);
        }
        out
    }

    fn walk(&mut self, dict: &Dict<'a>, parent: &Inherited, prefix: &str, depth: u8) {
        if depth > MAX_DEPTH || self.fields.len() >= MAX_FIELDS || self.budget.is_none() {
            return;
        }
        let Some(obj) = obj_ref(dict) else { return };
        if !self.seen.insert(obj) {
            return;
        }
        let mut inherited = parent.clone();
        if let Some(ft) = dict.get::<Name<'_>>("FT") {
            inherited.ft = Some(ft.as_str().to_owned());
        }
        if let Some(ff) = dict.get::<u32>("Ff") {
            inherited.ff = ff;
        }
        if let Some(v) = value_of(dict) {
            inherited.v = Some(v);
        }
        if let Some(da) = dict.get::<PdfString<'_>>("DA") {
            inherited.da = Some(da.as_bytes().to_vec());
        }
        if let Some(q) = dict.get::<u32>("Q") {
            inherited.q = q.min(2) as u8;
        }
        if let Some(n) = dict.get::<u32>("MaxLen") {
            inherited.max_len = Some(n as usize);
        }
        if let Some(opt) = dict.get::<Array<'_>>("Opt") {
            inherited.options = Some(Rc::new(self.options(&opt)));
        }
        let name = match (text(dict, "T"), prefix.is_empty()) {
            (Some(t), true) => t,
            (Some(t), false) => format!("{prefix}.{t}"),
            (None, _) => prefix.to_owned(),
        };
        let kids: Vec<Dict<'a>> = dict
            .get::<Array<'_>>("Kids")
            .map(|k| crate::structure::dicts(self.pdf, &k).take(MAX_FIELDS).collect())
            .unwrap_or_default();
        let (children, widgets): (Vec<_>, Vec<_>) = kids.into_iter().partition(|k| k.contains_key("T"));
        for child in &children {
            self.walk(child, &inherited, &name, depth + 1);
        }
        if !children.is_empty() && widgets.is_empty() {
            return;
        }
        // A field without widget kids is its own widget.
        let widgets: Vec<Dict<'_>> =
            if widgets.is_empty() && dict.get::<Name<'_>>("Subtype").is_some_and(|s| s.as_str() == "Widget") {
                vec![dict.clone()]
            } else {
                widgets
            };
        let widgets: Vec<Widget> = widgets.iter().filter_map(|w| self.widget(w)).collect();
        if widgets.is_empty() {
            return;
        }
        let choices = inherited.options.iter().flat_map(|o| o.iter()).map(choice_bytes).sum::<usize>();
        let typed = if let Some(Value::Text(t)) = &inherited.v { t.len() } else { 0 };
        let shapes = widgets.iter().map(|w| size_of::<Widget>() + w.on.as_ref().map_or(0, String::len)).sum::<usize>();
        if !self.spend(size_of::<Field>() + name.len() + choices + typed + shapes) {
            return;
        }
        let ff = inherited.ff;
        let kind = match inherited.ft.as_deref() {
            Some("Tx") => Kind::Text {
                multiline: ff & MULTILINE != 0,
                password: ff & PASSWORD != 0,
                max_len: inherited.max_len.filter(|&n| n > 0),
            },
            Some("Btn") if ff & PUSH != 0 => Kind::Push,
            Some("Btn") if ff & RADIO != 0 => Kind::Radio,
            Some("Btn") => Kind::Check,
            Some("Ch") => Kind::Choice { options: inherited.options.as_deref().cloned().unwrap_or_default() },
            Some("Sig") => Kind::Signature,
            _ => return,
        };
        let value = match (&kind, inherited.v.clone()) {
            (Kind::Check | Kind::Radio, Some(v @ Value::State(_))) => v,
            // No value: a check box's widgets tell whether it is on.
            (Kind::Check | Kind::Radio, _) => Value::State(widgets.iter().find_map(|w| {
                let state = self
                    .pdf
                    .xref()
                    .get::<Dict<'_>>(hayro::hayro_syntax::object::ObjectIdentifier::new(w.obj.0, w.obj.1 as i32))?;
                let state = state.get::<Name<'_>>("AS")?;
                state_of(&state)
            })),
            (_, Some(Value::Text(t))) => Value::Text(t),
            _ => Value::Text(String::new()),
        };
        let (size, color) = parse_da(inherited.da.clone());
        let size = if inherited.da.as_ref().is_some_and(|d| String::from_utf8_lossy(d).contains(" 0 Tf")) {
            0.0
        } else {
            size
        };
        let color = color.map_or([0.0; 3], |c| [c.r, c.g, c.b]);
        self.fields.push(Field {
            name,
            kind,
            read_only: ff & READ_ONLY != 0,
            obj,
            widgets,
            value,
            font_size: size,
            color,
            quadding: inherited.q.min(2),
            comb: inherited.max_len.filter(|n| ff & COMB != 0 && ff & (MULTILINE | PASSWORD) == 0 && *n > 0),
        });
    }

    fn widget(&self, dict: &Dict<'_>) -> Option<Widget> {
        let obj = obj_ref(dict)?;
        let page = *self.annot_pages.get(&obj)?;
        let r: [f64; 4] =
            dict.get::<Array<'_>>("Rect")?.iter::<Number>().map(|n| n.as_f64()).collect::<Vec<_>>().try_into().ok()?;
        let user = Rect::new(r[0], r[1], r[2], r[3]).abs();
        if dict.get::<u32>("F").unwrap_or(0) & 0b10 != 0 || user.width() < 1.0 || user.height() < 1.0 {
            return None;
        }
        let rect = self.to_view[page].transform_rect_bbox(user);
        let on = dict
            .get::<Dict<'_>>("AP")
            .and_then(|ap| ap.get::<Dict<'_>>("N"))
            .and_then(|n| n.keys().find_map(|k| state_of(&k)));
        Some(Widget { page, rect, obj, on, size: (user.width(), user.height()) })
    }
}

// ---------------------------------------------------------------- writing

/// Sets `field` to `value` in `doc`: the value, and a matching appearance for each widget.
pub(crate) fn fill(doc: &mut lopdf::IncrementalDocument, field: &Field, value: &Value) -> lopdf::Result<()> {
    doc.opt_clone_object_to_new_document(field.obj)?;
    let v = match value {
        Value::Text(t) => pdf_text(t),
        Value::State(Some(s)) => LObject::Name(state_bytes(s)),
        Value::State(None) => LObject::Name(b"Off".to_vec()),
    };
    doc.new_document.get_dictionary_mut(field.obj)?.set("V", v);
    for widget in &field.widgets {
        doc.opt_clone_object_to_new_document(widget.obj)?;
        match value {
            Value::State(state) => {
                let on = widget.on.as_deref().unwrap_or("Yes");
                let shown = if state.as_deref() == Some(on) { on } else { "Off" };
                doc.new_document.get_dictionary_mut(widget.obj)?.set("AS", LObject::Name(state_bytes(shown)));
            }
            Value::Text(t) => {
                let shown = match &field.kind {
                    Kind::Choice { options } => {
                        options.iter().find(|o| o.export == *t).map_or(t.clone(), |o| o.display.clone())
                    }
                    Kind::Text { password: true, .. } => "•".repeat(t.chars().count()),
                    _ => t.clone(),
                };
                let look = Look::of(doc, widget);
                let font = TextFont::for_text(&mut doc.new_document, &shown);
                let stream = text_appearance(field, widget, &shown, &look, &font);
                let ap = doc.new_document.add_object(stream);
                doc.new_document.get_dictionary_mut(widget.obj)?.set("AP", dictionary! { "N" => ap });
            }
        }
    }
    Ok(())
}

/// How a widget looks besides its text, from its `/MK` and `/BS`.
#[derive(Default)]
struct Look {
    /// Degrees the text is turned against the page, counterclockwise.
    turn: i64,
    background: Option<String>,
    border: Option<String>,
    border_width: f64,
    dashed: bool,
}

impl Look {
    fn of(doc: &lopdf::IncrementalDocument, widget: &Widget) -> Self {
        let mut look = Self { border_width: 1.0, ..Self::default() };
        let prev = doc.get_prev_documents();
        let Ok(dict) = prev.get_dictionary(widget.obj) else { return look };
        let sub = |key: &[u8]| {
            let found = prev.dereference(dict.get(key).ok()?).ok()?.1;
            found.as_dict().ok().cloned()
        };
        if let Some(style) = sub(b"BS") {
            look.border_width = style.get(b"W").and_then(LObject::as_float).map_or(1.0, f64::from).clamp(0.0, 20.0);
            look.dashed = style.get(b"S").and_then(LObject::as_name).is_ok_and(|s| s == b"D");
        }
        if let Some(mk) = sub(b"MK") {
            look.turn = mk.get(b"R").and_then(LObject::as_float).map_or(0, |r| (r as i64).rem_euclid(360));
            let color = |key: &[u8], op: [&str; 3]| {
                let parts = prev.dereference(mk.get(key).ok()?).ok()?.1.as_array().ok()?;
                let parts: Vec<f32> = parts.iter().filter_map(|v| v.as_float().ok()).collect();
                let op = match parts.len() {
                    1 => op[0],
                    3 => op[1],
                    4 => op[2],
                    _ => return None,
                };
                Some(parts.iter().fold(String::new(), |s, v| format!("{s}{v:.3} ")) + op)
            };
            look.background = color(b"BG", ["g", "rg", "k"]);
            look.border = color(b"BC", ["G", "RG", "K"]);
        }
        look
    }
}

fn text_appearance(field: &Field, widget: &Widget, shown: &str, look: &Look, font: &TextFont) -> lopdf::Stream {
    let (rect_w, rect_h) = widget.size;
    // The text runs along the widget as the page is read, which may be turned.
    let (w, h) = if look.turn % 180 == 90 { (rect_h, rect_w) } else { (rect_w, rect_h) };
    let matrix = match look.turn {
        90 => [0.0, 1.0, -1.0, 0.0, rect_w, 0.0],
        180 => [-1.0, 0.0, 0.0, -1.0, rect_w, rect_h],
        270 => [0.0, -1.0, 1.0, 0.0, 0.0, rect_h],
        _ => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
    };
    let size = field.text_size(h);
    let [r, g, b] = field.color;
    let mut c = String::new();
    if let Some(fill) = &look.background {
        let _ = writeln!(c, "q {fill} 0 0 {w:.2} {h:.2} re f Q");
    }
    let edge = if look.border.is_some() { look.border_width } else { 0.0 };
    if let Some(stroke) = look.border.as_ref().filter(|_| edge > 0.0) {
        let dash = if look.dashed { "[3] 0 d" } else { "" };
        let (x, bw, bh) = (edge / 2.0, w - edge, h - edge);
        let _ = writeln!(c, "q {stroke} {edge:.2} w {dash} {x:.2} {x:.2} {bw:.2} {bh:.2} re S Q");
    }
    let inset = 1.0 + edge;
    let _ = writeln!(
        c,
        "/Tx BMC q {inset:.2} {inset:.2} {:.2} {:.2} re W n BT /Helv {size:.2} Tf {r:.3} {g:.3} {b:.3} rg",
        (w - 2.0 * inset).max(0.0),
        (h - 2.0 * inset).max(0.0)
    );
    let pad = inset + 1.0;
    let multiline = matches!(field.kind, Kind::Text { multiline: true, .. });
    let middle = (h - size * 0.7) / 2.0;
    let mut show = |x: f64, y: f64, line: &str| {
        let _ = writeln!(c, "1 0 0 1 {x:.2} {y:.2} Tm <{}> Tj", font.encoded(line));
    };
    if let Some(cells) = field.comb {
        let cell = w / cells as f64;
        // The text sits at the side (or in the middle) of the row of cells.
        let count = shown.chars().count().min(cells);
        let first = match field.quadding {
            1 => (cells - count) / 2,
            2 => cells - count,
            _ => 0,
        };
        for (i, ch) in shown.chars().take(cells).enumerate() {
            let ch = ch.to_string();
            show(cell * ((first + i) as f64 + 0.5) - font.width(&ch) * size / 2.0, middle, &ch);
        }
    } else {
        let lines = if multiline {
            wrapped(shown, font, (w - 2.0 * pad).max(size) / size)
        } else {
            vec![shown.lines().next().unwrap_or("").to_owned()]
        };
        for (i, line) in lines.iter().enumerate() {
            let line = line.trim_end();
            let width = font.width(line) * size;
            let x = match field.quadding {
                1 => (w - width) / 2.0,
                2 => w - pad - width,
                _ => pad,
            }
            .max(pad);
            let y = if multiline { h - pad - size * (i as f64 + 1.0) * 1.15 + size * 0.15 } else { middle };
            show(x, y, line);
        }
    }
    c.push_str("ET Q EMC\n");
    let dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Form",
        "BBox" => vec![0.into(), 0.into(), LObject::Real(w as f32), LObject::Real(h as f32)],
        "Matrix" => matrix.iter().map(|v: &f64| LObject::Real(*v as f32)).collect::<Vec<_>>(),
        "Resources" => dictionary! { "Font" => dictionary! { "Helv" => font.resource() } },
    };
    let mut stream = lopdf::Stream::new(dict, c.into_bytes());
    let _ = stream.compress();
    stream
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use lopdf::{Document, Object as O, Stream};

    /// One page with a text field, a check box, a radio group of two, a drop-down and a signature field.
    pub fn fixture() -> Vec<u8> {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let page_id = doc.new_object_id();
        let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
        let content = doc.add_object(Stream::new(dictionary! {}, b"BT /F1 12 Tf 72 760 Td (Name:) Tj ET".to_vec()));
        let on = doc.add_object(Stream::new(
            dictionary! { "BBox" => vec![0.into(), 0.into(), 12.into(), 12.into()] },
            b"0 0 m 12 12 l S".to_vec(),
        ));
        let off = doc.add_object(Stream::new(
            dictionary! { "BBox" => vec![0.into(), 0.into(), 12.into(), 12.into()] },
            Vec::new(),
        ));
        let ap = |state: &str| dictionary! { "N" => dictionary! { state => on, "Off" => off } };
        let rect = |x0: i64, y0: i64, x1: i64, y1: i64| vec![O::from(x0), y0.into(), x1.into(), y1.into()];
        let name = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => O::string_literal("name"),
            "Rect" => rect(130, 750, 330, 772), "P" => page_id, "DA" => O::string_literal("/Helv 0 Tf 0 g"),
            "V" => O::string_literal("Ada"),
        });
        let check = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "FT" => "Btn", "T" => O::string_literal("agree"),
            "Rect" => rect(130, 700, 142, 712), "P" => page_id, "AS" => "Off", "AP" => ap("Yes"),
        });
        let radio = doc.new_object_id();
        let r1 = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "Parent" => radio, "Rect" => rect(130, 650, 142, 662),
            "P" => page_id, "AS" => "tea", "AP" => ap("tea"),
        });
        let r2 = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "Parent" => radio, "Rect" => rect(160, 650, 172, 662),
            "P" => page_id, "AS" => "Off", "AP" => ap("coffee"),
        });
        doc.objects.insert(radio, O::Dictionary(dictionary! {
            "FT" => "Btn", "Ff" => RADIO as i64, "T" => O::string_literal("drink"), "V" => "tea", "Kids" => vec![r1.into(), r2.into()],
        }));
        let choice = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "FT" => "Ch", "Ff" => COMBO as i64, "T" => O::string_literal("size"),
            "Rect" => rect(130, 600, 230, 620), "P" => page_id,
            "Opt" => vec![vec![O::string_literal("s"), O::string_literal("Small")].into(), vec![O::string_literal("l"), O::string_literal("Large")].into()],
        });
        let sig = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "FT" => "Sig", "T" => O::string_literal("sign"),
            "Rect" => rect(130, 500, 330, 550), "P" => page_id,
        });
        doc.objects.insert(
            page_id,
            O::Dictionary(dictionary! {
                "Type" => "Page", "Parent" => pages_id, "MediaBox" => rect(0, 0, 600, 800), "Contents" => content,
                "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
                "Annots" => vec![name.into(), check.into(), r1.into(), r2.into(), choice.into(), sig.into()],
            }),
        );
        doc.objects.insert(
            pages_id,
            O::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1 }),
        );
        let acro = doc.add_object(dictionary! {
            "Fields" => vec![name.into(), check.into(), radio.into(), choice.into(), sig.into()],
            "DA" => O::string_literal("/Helv 0 Tf 0 g"),
        });
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id, "AcroForm" => acro });
        doc.trailer.set("Root", catalog);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    pub fn read_bytes(bytes: &[u8]) -> Form {
        read(&Pdf::new(bytes.to_vec()).unwrap())
    }

    /// A form of one page with `fields` (each its own widget), and a null before them.
    fn form_of(fields: Vec<lopdf::Dictionary>) -> Vec<u8> {
        let mut doc = Document::with_version("1.7");
        let mut ids: Vec<O> = vec![O::Null, O::Reference((999, 0))];
        ids.extend(fields.into_iter().map(|f| O::Reference(doc.add_object(f))));
        let pages = doc.new_object_id();
        let media: Vec<O> = vec![0.into(), 0.into(), 300.into(), 300.into()];
        let page = dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => media, "Annots" => ids.clone() };
        let page = doc.add_object(page);
        let tree = dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 };
        doc.objects.insert(pages, tree.into());
        let acro = doc.add_object(dictionary! { "Fields" => ids, "DA" => O::string_literal("/Helv 10 Tf 0 g") });
        let root = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages, "AcroForm" => acro });
        doc.trailer.set("Root", root);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    fn rect(w: i64, h: i64) -> Vec<O> {
        vec![10.into(), 10.into(), (10 + w).into(), (10 + h).into()]
    }

    /// `bytes` with its first field set, and the appearance that gave it.
    fn filled(bytes: &[u8], value: Value) -> (Vec<u8>, lopdf::Stream) {
        let form = read_bytes(bytes);
        let fill = crate::annotate::pdf::Fill { field: &form.fields[0], value: &value };
        let (saved, _) = crate::annotate::pdf::save(bytes, &[], &[fill], None).unwrap();
        let doc = Document::load_mem(&saved).unwrap();
        let widget = doc.get_dictionary(form.fields[0].obj).unwrap();
        let ap = widget.get(b"AP").unwrap().as_dict().unwrap().get(b"N").unwrap().as_reference();
        let mut look = ap
            .ok()
            .and_then(|id| doc.get_object(id).ok()?.as_stream().ok().cloned())
            .unwrap_or(Stream::new(dictionary! {}, Vec::new()));
        let _ = look.decompress();
        (saved, look)
    }

    #[test]
    fn a_state_whose_name_is_not_text_survives() {
        let name = b"S\xed".to_vec();
        let on = O::Dictionary(lopdf::Dictionary::from_iter([(name.clone(), O::Null), (b"Off".to_vec(), O::Null)]));
        let bytes = form_of(vec![dictionary! {
            "Subtype" => "Widget", "FT" => "Btn", "T" => O::string_literal("c"), "Rect" => rect(12, 12),
            "AP" => dictionary! { "N" => on },
        }]);
        let form = read_bytes(&bytes);
        assert_eq!(form.fields.len(), 1, "the null before it does not end the form");
        let state = form.fields[0].widgets[0].on.clone().unwrap();
        let value = Value::State(Some(state));
        let fill = crate::annotate::pdf::Fill { field: &form.fields[0], value: &value };
        let (saved, _) = crate::annotate::pdf::save(&bytes, &[], &[fill], None).unwrap();
        let doc = Document::load_mem(&saved).unwrap();
        let widget = doc.get_dictionary(form.fields[0].obj).unwrap();
        assert_eq!(widget.get(b"V").unwrap().as_name().unwrap(), name);
        assert_eq!(widget.get(b"AS").unwrap().as_name().unwrap(), name);
        assert_eq!(read_bytes(&saved).fields[0].value, value);
    }

    #[test]
    fn a_saved_field_keeps_its_turn_its_border_and_its_background() {
        let colors = |v: &[f32]| v.iter().map(|v| O::Real(*v)).collect::<Vec<_>>();
        let bytes = form_of(vec![dictionary! {
            "Subtype" => "Widget", "FT" => "Tx", "T" => O::string_literal("t"), "Rect" => rect(20, 100),
            "BS" => dictionary! { "W" => 2, "S" => "D" },
            "MK" => dictionary! { "R" => 90, "BG" => colors(&[1.0, 1.0, 0.5]), "BC" => colors(&[0.25]) },
        }]);
        let (saved, look) = filled(&bytes, Value::Text("turned".into()));
        let numbers = |key: &[u8]| -> Vec<f32> {
            look.dict.get(key).unwrap().as_array().unwrap().iter().map(|v| v.as_float().unwrap()).collect()
        };
        assert_eq!(numbers(b"BBox"), [0.0, 0.0, 100.0, 20.0]);
        assert_eq!(numbers(b"Matrix"), [0.0, 1.0, -1.0, 0.0, 20.0, 0.0]);
        let drawn = String::from_utf8(look.content).unwrap();
        assert!(drawn.contains("1.000 1.000 0.500 rg 0 0 100.00 20.00 re f"), "{drawn}");
        assert!(drawn.contains("0.250 G 2.00 w [3] 0 d"), "{drawn}");
        Pdf::new(saved).unwrap();
    }

    #[test]
    fn long_text_is_wrapped_and_combs_are_filled_cell_by_cell() {
        let bytes = form_of(vec![dictionary! {
            "Subtype" => "Widget", "FT" => "Tx", "T" => O::string_literal("t"), "Rect" => rect(100, 100),
            "Ff" => MULTILINE as i64,
        }]);
        let text = "several words that do not fit on a single line of this box";
        let drawn = String::from_utf8(filled(&bytes, Value::Text(text.into())).1.content).unwrap();
        assert!(drawn.matches(" Tj").count() >= 3, "{drawn}");
        let bytes = form_of(vec![dictionary! {
            "Subtype" => "Widget", "FT" => "Tx", "T" => O::string_literal("t"), "Rect" => rect(100, 20),
            "Ff" => COMB as i64, "MaxLen" => 5,
        }]);
        let drawn = String::from_utf8(filled(&bytes, Value::Text("123".into())).1.content).unwrap();
        // Digits are 5.56 wide at size 10, in the middle of cells 20 wide.
        for x in ["7.22", "27.22", "47.22"] {
            assert!(drawn.contains(&format!("1 0 0 1 {x} ")), "{drawn}");
        }
    }

    #[test]
    fn many_fields_sharing_one_list_of_options_do_not_fill_the_memory() {
        let mut doc = Document::with_version("1.7");
        let list: Vec<O> = (0..10_000).map(|i| O::string_literal(format!("option number {i}"))).collect();
        let list = doc.add_object(list);
        let rect: Vec<O> = vec![0.into(), 0.into(), 50.into(), 20.into()];
        let fields: Vec<O> = (0..500)
            .map(|i| {
                doc.add_object(dictionary! {
                    "Subtype" => "Widget", "FT" => "Ch", "T" => O::string_literal(format!("f{i}")),
                    "Rect" => rect.clone(), "Opt" => list,
                })
                .into()
            })
            .collect();
        let pages = doc.new_object_id();
        let media: Vec<O> = vec![0.into(), 0.into(), 100.into(), 100.into()];
        let page = dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => media, "Annots" => fields.clone() };
        let page = doc.add_object(page);
        let tree = dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 };
        doc.objects.insert(pages, tree.into());
        let acro = doc.add_object(dictionary! { "Fields" => fields });
        let root = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages, "AcroForm" => acro });
        doc.trailer.set("Root", root);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        let form = read_bytes(&bytes);
        assert!(form.truncated);
        let kept = form.fields.iter().map(|f| if let Kind::Choice { options } = &f.kind { options.len() } else { 0 });
        let kept: usize = kept.sum();
        assert!(kept > 0 && kept < MAX_FORM_BYTES / size_of::<Choice>(), "{kept}");
        assert!(!read_bytes(&fixture()).truncated);
    }

    #[test]
    fn reads_every_kind_of_field() {
        let form = read_bytes(&fixture());
        let names: Vec<&str> = form.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["name", "agree", "drink", "size", "sign"]);
        let f = &form.fields;
        assert!(matches!(f[0].kind, Kind::Text { multiline: false, .. }));
        assert_eq!(f[0].value, Value::Text("Ada".into()));
        // 772 pt from the bottom of an 800 pt page is 28 pt from the top.
        assert!((f[0].widgets[0].rect.y0 - 28.0).abs() < 1e-6 && (f[0].widgets[0].rect.x0 - 130.0).abs() < 1e-6);
        assert_eq!((f[1].kind.clone(), f[1].value.clone()), (Kind::Check, Value::State(None)));
        assert_eq!(f[1].widgets[0].on.as_deref(), Some("Yes"));
        assert_eq!(f[2].kind, Kind::Radio);
        assert_eq!(f[2].value, Value::State(Some("tea".into())));
        assert_eq!(f[2].widgets.iter().map(|w| w.on.clone().unwrap()).collect::<Vec<_>>(), ["tea", "coffee"]);
        let Kind::Choice { options } = &f[3].kind else { panic!() };
        assert_eq!(options[1], Choice { export: "l".into(), display: "Large".into() });
        assert_eq!(f[4].kind, Kind::Signature);
        assert!(!f[4].fillable() && f[0].fillable());
        assert_eq!(form.hidden_widgets().len(), 5);
        // A size of 0 in the default appearance means "fit the box".
        assert!((f[0].text_size(22.0) - 12.96).abs() < 0.01);
    }

    #[test]
    fn filled_in_values_are_saved_and_read_back() {
        let bytes = fixture();
        let form = read_bytes(&bytes);
        let values = vec![
            Value::Text("Grace Hopper – Größe".into()),
            Value::State(Some("Yes".into())),
            Value::State(Some("coffee".into())),
            Value::Text("l".into()),
            Value::Text(String::new()),
        ];
        let fills: Vec<_> = form
            .fields
            .iter()
            .zip(&values)
            .filter(|(f, v)| f.value != **v)
            .map(|(field, value)| crate::annotate::pdf::Fill { field, value })
            .collect();
        assert_eq!(fills.len(), 4);
        let saved = crate::annotate::pdf::save(&bytes, &[], &fills, None).unwrap().0;
        assert!(saved.starts_with(&bytes));
        let back = read_bytes(&saved);
        assert_eq!(back.values(), values);
        // Each radio button shows its own state; the text field has an appearance now.
        let pdf = Pdf::new(saved.clone()).unwrap();
        let xref = pdf.xref();
        let state = |r: ObjRef| {
            let d = xref.get::<Dict<'_>>(hayro::hayro_syntax::object::ObjectIdentifier::new(r.0, r.1 as i32)).unwrap();
            d.get::<Name<'_>>("AS").map(|n| n.as_str().to_owned())
        };
        assert_eq!(state(back.fields[2].widgets[0].obj).as_deref(), Some("Off"));
        assert_eq!(state(back.fields[2].widgets[1].obj).as_deref(), Some("coffee"));
        assert_eq!(state(back.fields[1].widgets[0].obj).as_deref(), Some("Yes"));
        let name =
            xref.get::<Dict<'_>>(hayro::hayro_syntax::object::ObjectIdentifier::new(back.fields[0].obj.0, 0)).unwrap();
        assert!(
            name.get::<Dict<'_>>("AP").and_then(|ap| ap.get::<hayro::hayro_syntax::object::Stream<'_>>("N")).is_some()
        );
        // Hiding the widgets for editing keeps the values.
        let hidden = crate::annotate::pdf::hide(&saved, &back.hidden_widgets(), None).unwrap();
        assert_eq!(read_bytes(&hidden).fields.len(), 1, "hidden widgets are not offered again; the signature stays");
    }
}
