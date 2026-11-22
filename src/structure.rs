//! What a PDF says beyond its pixels: the outline (table of contents), the
//! links on each page, and the text on each page (for search).
//!
//! Everything is read with hayro's pure-Rust parser. Positions are in the same
//! space the pages are drawn in: PDF points, origin at the top-left of the
//! page as displayed (after /Rotate), y growing downwards.

use std::collections::{HashMap, HashSet};

use hayro::hayro_interpret::font::{Glyph, GlyphRun};
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps, InterpreterCache,
    InterpreterSettings, SoftMask, TransformExt, interpret_page,
};
use hayro::hayro_syntax::Pdf;
use hayro::hayro_syntax::object::{Array, Dict, MaybeRef, Name, ObjRef, Object, String as PdfString};
use hayro::hayro_syntax::page::Page;
use hayro::kurbo::{Affine, BezPath, Point, Rect};

/// Where a link or outline entry leads.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// A page, optionally scrolled so that `top` (points from the page's top edge) is at the top of the view.
    Page { page: usize, top: Option<f32> },
    /// A web or mail address.
    Uri(String),
}

#[derive(Debug, Clone)]
pub struct OutlineItem {
    pub title: String,
    pub depth: u16,
    pub target: Option<Target>,
}

#[derive(Debug, Clone)]
pub struct Link {
    pub rect: Rect,
    pub target: Target,
}

/// Outline entries beyond this are ignored; protects against hostile files.
const MAX_OUTLINE: usize = 20_000;
const MAX_LINKS_PER_PAGE: usize = 2_000;
const MAX_DEPTH: u16 = 32;

/// Reads the outline and every page's links. Cheap: no page content is interpreted.
pub fn read(pdf: &Pdf) -> (Vec<OutlineItem>, Vec<Vec<Link>>) {
    let resolver = Resolver::new(pdf);
    let outline = resolver.outline();
    let links = pdf.pages().iter().map(|page| resolver.links(page)).collect();
    (outline, links)
}

struct Resolver<'a> {
    pdf: &'a Pdf,
    catalog: Option<Dict<'a>>,
    pages: HashMap<ObjRef, usize>,
    named: std::cell::OnceCell<HashMap<Vec<u8>, Target>>,
}

impl<'a> Resolver<'a> {
    fn new(pdf: &'a Pdf) -> Self {
        let pages = pdf
            .pages()
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.raw().obj_id().map(|id| (ObjRef::from(id), i)))
            .collect();
        let xref = pdf.xref();
        Self { pdf, catalog: xref.get::<Dict<'_>>(xref.root_id()), pages, named: Default::default() }
    }

    fn outline(&self) -> Vec<OutlineItem> {
        let mut items = Vec::new();
        let Some(root) = self.catalog.as_ref().and_then(|c| c.get::<Dict<'_>>("Outlines")) else { return items };
        let mut seen = HashSet::new();
        self.walk(root.get::<Dict<'_>>("First"), 0, &mut items, &mut seen);
        items
    }

    fn walk(&self, mut node: Option<Dict<'a>>, depth: u16, items: &mut Vec<OutlineItem>, seen: &mut HashSet<ObjRef>) {
        while let Some(item) = node {
            if items.len() >= MAX_OUTLINE || depth > MAX_DEPTH {
                return;
            }
            // Outlines are linked lists; a malicious one may loop.
            if let Some(id) = item.obj_id()
                && !seen.insert(ObjRef::from(id))
            {
                return;
            }
            let title = item.get::<PdfString<'_>>("Title").map(|s| decode_text(s.as_bytes())).unwrap_or_default();
            let target = self.target_of(&item);
            items.push(OutlineItem { title: clean_title(&title), depth, target });
            self.walk(item.get::<Dict<'_>>("First"), depth + 1, items, seen);
            node = item.get::<Dict<'_>>("Next");
        }
    }

    fn links(&self, page: &Page<'_>) -> Vec<Link> {
        let Some(annots) = page.raw().get::<Array<'_>>("Annots") else { return Vec::new() };
        let to_view = page.initial_transform(true).to_kurbo();
        let mut links = Vec::new();
        for annot in annots.iter::<Dict<'_>>().take(MAX_LINKS_PER_PAGE * 4) {
            if annot.get::<Name<'_>>("Subtype").is_none_or(|s| s.as_str() != "Link") {
                continue;
            }
            let Some([x0, y0, x1, y1]) = annot.get::<[f32; 4]>("Rect") else { continue };
            let Some(target) = self.target_of(&annot) else { continue };
            let rect = to_view.transform_rect_bbox(Rect::new(x0 as f64, y0 as f64, x1 as f64, y1 as f64));
            if rect.width() > 0.5 && rect.height() > 0.5 {
                links.push(Link { rect, target });
            }
            if links.len() >= MAX_LINKS_PER_PAGE {
                break;
            }
        }
        links
    }

    /// The target of an outline item or link annotation: its /Dest, or its /A action.
    fn target_of(&self, dict: &Dict<'a>) -> Option<Target> {
        if let Some(dest) = dict.get::<Object<'_>>("Dest") {
            return self.dest(dest, 0);
        }
        let action = dict.get::<Dict<'_>>("A")?;
        match action.get::<Name<'_>>("S")?.as_str() {
            "GoTo" => self.dest(action.get::<Object<'_>>("D")?, 0),
            "URI" => {
                let uri = action.get::<PdfString<'_>>("URI")?;
                let uri = String::from_utf8_lossy(uri.as_bytes()).trim().to_string();
                is_safe_uri(&uri).then_some(Target::Uri(uri))
            }
            _ => None,
        }
    }

    fn dest(&self, dest: Object<'a>, depth: u8) -> Option<Target> {
        if depth > 4 {
            return None;
        }
        match dest {
            Object::Array(a) => self.explicit_dest(&a),
            Object::Name(n) => self.named().get(n.as_ref()).cloned(),
            Object::String(s) => self.named().get(s.as_bytes()).cloned(),
            Object::Dict(d) => self.dest(d.get::<Object<'_>>("D")?, depth + 1),
            _ => None,
        }
    }

    /// `[page /XYZ left top zoom]`, `[page /FitH top]` and friends.
    fn explicit_dest(&self, array: &Array<'a>) -> Option<Target> {
        let mut items = array.raw_iter();
        let page = match items.next()? {
            MaybeRef::Ref(r) => *self.pages.get(&r)?,
            // Some writers use a page number (that is meant for remote go-tos, but be lenient).
            MaybeRef::NotRef(Object::Number(n)) => usize::try_from(n.as_i64()).ok()?,
            _ => return None,
        };
        let page = page.min(self.pdf.pages().len().checked_sub(1)?);
        let kind = match items.next() {
            Some(MaybeRef::NotRef(Object::Name(n))) => n.as_str().to_string(),
            _ => String::new(),
        };
        let number = |o: Option<MaybeRef<Object<'_>>>| match o {
            Some(MaybeRef::NotRef(Object::Number(n))) => Some(n.as_f64()),
            _ => None,
        };
        let top = match kind.as_str() {
            "XYZ" => {
                let _left = items.next();
                number(items.next())
            }
            "FitH" | "FitBH" => number(items.next()),
            "FitR" => {
                let _ = (items.next(), items.next(), items.next());
                number(items.next())
            }
            _ => None,
        };
        let top = top.map(|y| {
            let to_view = self.pdf.pages()[page].initial_transform(true).to_kurbo();
            (to_view * Point::new(0.0, y)).y.max(0.0) as f32
        });
        Some(Target::Page { page, top })
    }

    /// Named destinations, from the catalog's /Dests dictionary and the /Names /Dests tree. Built on first use.
    fn named(&self) -> &HashMap<Vec<u8>, Target> {
        self.named.get_or_init(|| {
            let mut raw: Vec<(Vec<u8>, Object<'a>)> = Vec::new();
            if let Some(catalog) = &self.catalog {
                if let Some(dests) = catalog.get::<Dict<'_>>("Dests") {
                    for key in dests.keys() {
                        if let Some(value) = dests.get::<Object<'_>>(key.as_ref()) {
                            raw.push((key.as_ref().to_vec(), value));
                        }
                    }
                }
                if let Some(tree) = catalog.get::<Dict<'_>>("Names").and_then(|n| n.get::<Dict<'_>>("Dests")) {
                    let mut seen = HashSet::new();
                    self.name_tree(&tree, 0, &mut raw, &mut seen);
                }
            }
            let mut map = HashMap::with_capacity(raw.len());
            for (name, value) in raw {
                // Values are explicit destinations, or dictionaries holding one in /D.
                let target = match value {
                    Object::Array(a) => self.explicit_dest(&a),
                    Object::Dict(d) => match d.get::<Object<'_>>("D") {
                        Some(Object::Array(a)) => self.explicit_dest(&a),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(t) = target {
                    map.insert(name, t);
                }
            }
            map
        })
    }

    fn name_tree(&self, node: &Dict<'a>, depth: u8, out: &mut Vec<(Vec<u8>, Object<'a>)>, seen: &mut HashSet<ObjRef>) {
        if depth > 32 || out.len() > 200_000 {
            return;
        }
        if let Some(id) = node.obj_id()
            && !seen.insert(ObjRef::from(id))
        {
            return;
        }
        if let Some(names) = node.get::<Array<'_>>("Names") {
            let mut iter = names.raw_iter();
            while let (Some(key), Some(value)) = (iter.next(), iter.next()) {
                let key = match self.resolve(key) {
                    Some(Object::String(s)) => s.as_bytes().to_vec(),
                    Some(Object::Name(n)) => n.as_ref().to_vec(),
                    _ => continue,
                };
                if let Some(value) = self.resolve(value) {
                    out.push((key, value));
                }
            }
        }
        if let Some(kids) = node.get::<Array<'_>>("Kids") {
            for kid in kids.iter::<Dict<'_>>() {
                self.name_tree(&kid, depth + 1, out, seen);
            }
        }
    }

    fn resolve(&self, item: MaybeRef<Object<'a>>) -> Option<Object<'a>> {
        match item {
            MaybeRef::Ref(r) => self.pdf.xref().get::<Object<'_>>(r.into()),
            MaybeRef::NotRef(o) => Some(o),
        }
    }
}

/// Only addresses that are safe to hand to the desktop: no `file:`, no custom schemes that launch programs.
fn is_safe_uri(uri: &str) -> bool {
    let lower = uri.to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:"))
        && !uri.chars().any(char::is_control)
}

/// PDF text strings are UTF-16BE (with a BOM), UTF-8 (with a BOM), or PDFDocEncoding (close to Latin-1).
pub fn decode_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
        return String::from_utf16_lossy(&units);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    bytes.iter().map(|&b| pdf_doc_char(b)).collect()
}

fn pdf_doc_char(b: u8) -> char {
    // The bytes where PDFDocEncoding differs from Latin-1.
    match b {
        0x18 => '˘',
        0x19 => 'ˇ',
        0x1A => 'ˆ',
        0x1B => '˙',
        0x1C => '˝',
        0x1D => '˛',
        0x1E => '˚',
        0x1F => '˜',
        0x80 => '•',
        0x81 => '†',
        0x82 => '‡',
        0x83 => '…',
        0x84 => '—',
        0x85 => '–',
        0x86 => 'ƒ',
        0x87 => '⁄',
        0x88 => '‹',
        0x89 => '›',
        0x8A => '−',
        0x8B => '‰',
        0x8C => '„',
        0x8D => '“',
        0x8E => '”',
        0x8F => '‘',
        0x90 => '’',
        0x91 => '‚',
        0x92 => '™',
        0x93 => 'ﬁ',
        0x94 => 'ﬂ',
        0x95 => 'Ł',
        0x96 => 'Œ',
        0x97 => 'Š',
        0x98 => 'Ÿ',
        0x99 => 'Ž',
        0x9A => 'ı',
        0x9B => 'ł',
        0x9C => 'œ',
        0x9D => 'š',
        0x9E => 'ž',
        0xA0 => '€',
        b => b as char,
    }
}

fn clean_title(title: &str) -> String {
    let t: String = title.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.is_empty() { "Untitled".into() } else { t }
}

// ---------------------------------------------------------------- text

/// The text of one page, one entry per character, with the box each character covers.
#[derive(Debug, Default)]
pub struct PageText {
    /// Lower-cased characters; word and line breaks are a single space.
    chars: Vec<char>,
    /// Box of each character, in page points; `None` for inserted spaces.
    boxes: Vec<Option<Rect>>,
}

impl PageText {
    /// Every case-insensitive occurrence of `needle`, each as one box per line it covers.
    pub fn find(&self, needle: &str) -> Vec<Vec<Rect>> {
        let needle: Vec<char> = normalise(needle);
        if needle.is_empty() || needle.len() > self.chars.len() {
            return Vec::new();
        }
        let mut hits = Vec::new();
        let mut i = 0;
        while i + needle.len() <= self.chars.len() {
            if self.chars[i..i + needle.len()] == needle[..] {
                hits.push(merge_boxes(self.boxes[i..i + needle.len()].iter().flatten()));
                i += needle.len();
            } else {
                i += 1;
            }
        }
        hits
    }
}

fn normalise(s: &str) -> Vec<char> {
    let mut out = Vec::new();
    for c in s.chars().flat_map(char::to_lowercase) {
        if c.is_whitespace() {
            if out.last() != Some(&' ') {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
    while out.last() == Some(&' ') {
        out.pop();
    }
    if out.first() == Some(&' ') {
        out.remove(0);
    }
    out
}

/// Joins character boxes into one box per line.
fn merge_boxes<'a>(boxes: impl Iterator<Item = &'a Rect>) -> Vec<Rect> {
    let mut lines: Vec<Rect> = Vec::new();
    for b in boxes {
        match lines.last_mut() {
            Some(last) if (b.center().y - last.center().y).abs() < last.height().max(b.height()) * 0.5 => {
                *last = last.union(*b)
            }
            _ => lines.push(*b),
        }
    }
    lines
}

/// Extracts the text of `page` by running its content through a device that only records glyphs.
pub fn page_text<'a>(page: &Page<'a>, cache: &InterpreterCache<'a>) -> PageText {
    let (w, h) = page.render_dimensions();
    let transform = page.initial_transform(true).to_kurbo();
    let mut context = Context::new(
        transform,
        Rect::new(0.0, 0.0, w as f64, h as f64),
        cache,
        page.xref(),
        InterpreterSettings::default(),
    );
    let mut device = TextDevice::default();
    interpret_page(page, &mut context, &mut device);
    device.text
}

#[derive(Default)]
struct TextDevice {
    text: PageText,
    /// Box of the previous glyph, for spotting word and line breaks.
    last: Option<Rect>,
}

impl TextDevice {
    fn push(&mut self, s: &str, rect: Rect) {
        if let Some(last) = self.last {
            let size = last.height().max(rect.height()).max(1.0);
            let new_line = (rect.center().y - last.center().y).abs() > size * 0.5;
            let gap = rect.x0 - last.x1;
            let new_word = gap > size * 0.15 || gap < -size * 2.0;
            if (new_line || new_word) && self.text.chars.last().is_some_and(|c| *c != ' ') {
                self.text.chars.push(' ');
                self.text.boxes.push(None);
            }
        }
        for c in s.chars().flat_map(char::to_lowercase) {
            if c.is_whitespace() {
                if self.text.chars.last().is_some_and(|c| *c != ' ') {
                    self.text.chars.push(' ');
                    self.text.boxes.push(None);
                }
            } else if !c.is_control() {
                self.text.chars.push(c);
                self.text.boxes.push(Some(rect));
            }
        }
        self.last = Some(rect);
    }
}

impl<'a> Device<'a> for TextDevice {
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}

    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, _: &DrawMode) {
        // Invisible text (OCR layers) is drawn too, so scanned documents are searchable.
        for glyph in run.glyphs() {
            if self.text.chars.len() > 2_000_000 {
                return;
            }
            let Some(unicode) = glyph.as_unicode() else { continue };
            let advance = match &**glyph {
                Glyph::Outline(g) => g.advance_width(),
                Glyph::Type3(g) => g.advance_width(),
            }
            .filter(|a| a.is_finite() && *a > 0.0)
            .unwrap_or(500.0) as f64;
            // Glyph space is 1000 units per em; the em box spans descender to ascender.
            let full: Affine = props.transform * glyph.transform();
            let rect = full.transform_rect_bbox(Rect::new(0.0, -200.0, advance, 800.0));
            if !(rect.x0.is_finite() && rect.y0.is_finite() && rect.x1.is_finite() && rect.y1.is_finite()) {
                continue;
            }
            match unicode {
                BfString::Char(c) => self.push(c.encode_utf8(&mut [0; 4]), rect),
                BfString::String(s) => self.push(&s, rect),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_text_strings() {
        assert_eq!(decode_text(b"Chapter 1"), "Chapter 1");
        assert_eq!(decode_text(&[0xFE, 0xFF, 0x00, 0x43, 0x00, 0xE9]), "Cé");
        assert_eq!(decode_text(&[0x93, b'n']), "ﬁn");
    }

    #[test]
    fn finds_case_insensitively_across_spaces() {
        let mut device = TextDevice::default();
        let mut x = 0.0;
        for word in ["Hello", "World"] {
            for c in word.chars() {
                device.push(&c.to_string(), Rect::new(x, 0.0, x + 6.0, 12.0));
                x += 6.0;
            }
            x += 6.0; // a word gap
        }
        let hits = device.text.find("o  w");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].len(), 1, "one line, one box");
        assert!(device.text.find("hello world").len() == 1);
        assert!(device.text.find("xyz").is_empty());
    }

    #[test]
    fn rejects_unsafe_uris() {
        assert!(is_safe_uri("https://example.com"));
        assert!(is_safe_uri("mailto:a@b.c"));
        assert!(!is_safe_uri("file:///etc/passwd"));
        assert!(!is_safe_uri("javascript:alert(1)"));
    }
}
