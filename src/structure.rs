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
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

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

/// The dictionaries in `array`, whatever else (a null, a reference to nothing) is among them:
/// `Array::iter` ends at the first thing that is something else.
pub fn dicts<'a>(pdf: &'a Pdf, array: &Array<'a>) -> impl Iterator<Item = Dict<'a>> + use<'a> {
    array.raw_iter().filter_map(|item| match item {
        MaybeRef::Ref(r) => pdf.xref().get::<Dict<'_>>(r.into()),
        MaybeRef::NotRef(Object::Dict(d)) => Some(d),
        MaybeRef::NotRef(_) => None,
    })
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
        for annot in dicts(self.pdf, &annots).take(MAX_LINKS_PER_PAGE * 4) {
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
        let mut next = || number(items.next());
        // A point, as far as the destination gives one: left, top.
        let (left, top) = match kind.as_str() {
            "XYZ" => (next(), next()),
            "FitH" | "FitBH" => (None, next()),
            "FitV" | "FitBV" => (next(), None),
            "FitR" => {
                let (l, b, r, t) = (next(), next(), next(), next());
                // On a turned page another side of the rectangle is the upper one.
                let to_view = self.pdf.pages()[page].initial_transform(true).to_kurbo().as_coeffs();
                (if to_view[1] < 0.0 { r } else { l }, if to_view[3] > 0.0 { b } else { t })
            }
            _ => (None, None),
        };
        let c = self.pdf.pages()[page].initial_transform(true).to_kurbo().as_coeffs();
        // Which of the two says how far down the page it is depends on how the page is turned.
        let part = |factor: f64, v: Option<f64>| if factor.abs() < 1e-6 { Some(0.0) } else { v.map(|v| v * factor) };
        let top = part(c[1], left).zip(part(c[3], top)).map(|(x, y)| (x + y + c[5]).max(0.0) as f32);
        // Without any number at all, it is the page as a whole.
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
            for kid in dicts(self.pdf, &kids) {
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

/// What separates a piece of text from the one before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gap {
    None,
    Space,
    Line,
}

/// Which way text runs on the page as shown: left to right normally, top to bottom
/// on a page turned a quarter clockwise, and so on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dir {
    #[default]
    Right,
    Down,
    Left,
    Up,
}

impl Dir {
    /// The direction closest to the vector (`dx`, `dy`) in page space (y down).
    fn of(dx: f64, dy: f64) -> Self {
        if dx.abs() >= dy.abs() {
            if dx >= 0.0 { Self::Right } else { Self::Left }
        } else if dy >= 0.0 {
            Self::Down
        } else {
            Self::Up
        }
    }

    /// `p` in line coordinates: how far along the line, and how far across (towards the next line).
    fn line(self, p: Point) -> (f64, f64) {
        match self {
            Self::Right => (p.x, p.y),
            // Turned clockwise, lines run down and the next line is to the left.
            Self::Down => (p.y, -p.x),
            Self::Left => (-p.x, -p.y),
            Self::Up => (-p.y, p.x),
        }
    }

    /// `r` in line coordinates: (start, end) along and (start, end) across.
    fn span(self, r: &Rect) -> ((f64, f64), (f64, f64)) {
        let (a0, c0) = self.line(Point::new(r.x0, r.y0));
        let (a1, c1) = self.line(Point::new(r.x1, r.y1));
        ((a0.min(a1), a0.max(a1)), (c0.min(c1), c0.max(c1)))
    }
}

/// One glyph's worth of text (a few characters for ligatures) and the box it covers.
#[derive(Debug, Clone)]
pub struct TextItem {
    pub text: String,
    pub rect: Rect,
    pub gap: Gap,
    pub dir: Dir,
}

/// The text of one page in content order: glyphs with their boxes for selection, and a
/// normalised copy for search.
#[derive(Debug, Default)]
pub struct PageText {
    items: Vec<TextItem>,
    /// Lower-cased characters; word and line breaks are a single space.
    chars: Vec<char>,
    /// Whether each of `chars` was an upper-case letter in the document (for matching case).
    upper: Vec<bool>,
    /// Box and writing direction of each character, in page points; `None` for inserted spaces.
    boxes: Vec<Option<(Rect, Dir)>>,
    /// Where in `chars` a hyphen ends a line in the middle of a word.
    soft: HashSet<usize>,
}

impl PageText {
    /// Number of glyphs; carets run from 0 to this.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Every case-insensitive occurrence of `needle`, each as one box per line it covers.
    #[cfg(test)]
    pub fn find(&self, needle: &str) -> Vec<Vec<Rect>> {
        self.find_with(needle, SearchOptions::default())
    }

    /// Like [`PageText::find`], optionally matching case and whole words only.
    pub fn find_with(&self, needle: &str, options: SearchOptions) -> Vec<Vec<Rect>> {
        let (needle, needle_upper) = normalise_cased(needle);
        if needle.is_empty() || needle.len() > self.chars.len() {
            return Vec::new();
        }
        // An accent written after its letter is part of the word too.
        let word = |c: char| c.is_alphanumeric() || c == '_' || is_combining_mark(c);
        // The neighbours, past a hyphen that only breaks the line.
        let before = |i: usize| {
            let i = i.checked_sub(1)?;
            let i = if self.soft.contains(&i) { i.checked_sub(1)? } else { i };
            self.chars.get(i).copied()
        };
        let after = |end: usize| self.chars.get(end + usize::from(self.soft.contains(&end))).copied();
        let mut hits = Vec::new();
        let mut i = 0;
        while i + needle.len() <= self.chars.len() {
            let found = self.matches_at(i, &needle, options.match_case.then_some(&needle_upper[..])).filter(|&end| {
                // A whole word is not touched by a letter or digit on either side (a side that
                // is itself punctuation, like the `+` of `C++`, has no edge to check).
                !options.whole_words
                    || ((!before(i).is_some_and(word) || !word(needle[0]))
                        && (!after(end).is_some_and(word) || !word(needle[needle.len() - 1])))
            });
            match found {
                Some(end) => {
                    hits.push(merge_boxes(self.boxes[i..end].iter().flatten().copied()));
                    i = end;
                }
                None => i += 1,
            }
        }
        hits
    }

    /// Where `needle` ends if it starts at `at`. A hyphen at the end of a line may be one the line
    /// break put there, so it matches a hyphen and nothing at all.
    fn matches_at(&self, at: usize, needle: &[char], upper: Option<&[bool]>) -> Option<usize> {
        let mut end = at;
        for (k, c) in needle.iter().enumerate() {
            if k > 0 && self.soft.contains(&end) && self.chars[end] != *c {
                end += 1;
            }
            if self.chars.get(end) != Some(c) || upper.is_some_and(|u| u[k] != self.upper[end]) {
                return None;
            }
            end += 1;
        }
        Some(end)
    }

    /// Whether `p` (page points) is on a glyph, give or take the space between words.
    pub fn is_over_text(&self, p: Point) -> bool {
        self.items.iter().any(|item| {
            // A little slack along the line (the gaps between words), hardly any across.
            let (along, across) = if matches!(item.dir, Dir::Right | Dir::Left) { (2.0, 0.5) } else { (0.5, 2.0) };
            item.rect.inflate(along, across).contains(p)
        })
    }

    /// The glyph nearest to `p`, preferring glyphs on the same line.
    fn nearest(&self, p: Point) -> Option<usize> {
        let distance = |item: &TextItem| {
            let ((a0, a1), (c0, c1)) = item.dir.span(&item.rect);
            let (a, c) = item.dir.line(p);
            let across = (c0 - c).max(c - c1).max(0.0);
            let along = (a0 - a).max(a - a1).max(0.0);
            across * 8.0 + along
        };
        (0..self.items.len()).min_by(|&a, &b| distance(&self.items[a]).total_cmp(&distance(&self.items[b])))
    }

    /// The caret (a position between glyphs, `0..=len`) nearest to `p`.
    pub fn caret_at(&self, p: Point) -> usize {
        match self.nearest(p) {
            Some(i) => {
                let item = &self.items[i];
                let past_middle = item.dir.line(p).0 > item.dir.line(item.rect.center()).0;
                if past_middle { i + 1 } else { i }
            }
            None => 0,
        }
    }

    /// The glyphs of the word at `p`, as a caret range.
    pub fn word_at(&self, p: Point) -> std::ops::Range<usize> {
        let Some(i) = self.nearest(p) else { return 0..0 };
        let mut start = i;
        while start > 0 && self.items[start].gap == Gap::None {
            start -= 1;
        }
        let mut end = i + 1;
        while end < self.items.len() && self.items[end].gap == Gap::None {
            end += 1;
        }
        start..end
    }

    /// Boxes covering the glyphs in `range`, one per line.
    pub fn boxes(&self, range: std::ops::Range<usize>) -> Vec<Rect> {
        let range = range.start.min(self.items.len())..range.end.min(self.items.len());
        merge_boxes(self.items[range].iter().map(|item| (item.rect, item.dir)))
    }

    /// The text of the glyphs in `range`, with spaces and line breaks where the page has them.
    pub fn text(&self, range: std::ops::Range<usize>) -> String {
        let range = range.start.min(self.items.len())..range.end.min(self.items.len());
        let mut out = String::new();
        for (k, item) in self.items[range].iter().enumerate() {
            if k > 0 {
                match item.gap {
                    Gap::None => {}
                    Gap::Space => out.push(' '),
                    Gap::Line => out.push('\n'),
                }
            }
            out.push_str(&item.text);
        }
        out
    }
}

/// What a search looks for beyond the letters themselves.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchOptions {
    pub match_case: bool,
    pub whole_words: bool,
}

/// `s` with every character written one way: ligatures as their letters, accents after their letter,
/// full-width forms as the usual ones; soft hyphens go.
fn folded(s: &str) -> impl Iterator<Item = char> {
    s.nfkd().filter(|c| *c != '\u{AD}')
}

/// Lower-cases `s` and turns every run of whitespace into one space (none at the ends), also
/// saying which of the characters were upper case in `s`.
fn normalise_cased(s: &str) -> (Vec<char>, Vec<bool>) {
    let mut chars = Vec::new();
    let mut upper = Vec::new();
    for original in folded(s) {
        for c in original.to_lowercase() {
            if c.is_whitespace() {
                if chars.last() != Some(&' ') {
                    chars.push(' ');
                    upper.push(false);
                }
            } else {
                chars.push(c);
                upper.push(original.is_uppercase());
            }
        }
    }
    while chars.last() == Some(&' ') {
        chars.pop();
        upper.pop();
    }
    if chars.first() == Some(&' ') {
        chars.remove(0);
        upper.remove(0);
    }
    (chars, upper)
}

/// Whether `b` continues the line of `last`, both running in `dir`.
fn same_line(last: &Rect, b: &Rect, dir: Dir) -> bool {
    let (_, (l0, l1)) = dir.span(last);
    let (_, (b0, b1)) = dir.span(b);
    ((b0 + b1) / 2.0 - (l0 + l1) / 2.0).abs() < (l1 - l0).max(b1 - b0) * 0.5
}

/// Joins character boxes into one box per line.
fn merge_boxes(boxes: impl Iterator<Item = (Rect, Dir)>) -> Vec<Rect> {
    let mut lines: Vec<(Rect, Dir)> = Vec::new();
    for (b, dir) in boxes {
        match lines.last_mut() {
            Some((last, d)) if *d == dir && same_line(last, &b, dir) => *last = last.union(b),
            _ => lines.push((b, dir)),
        }
    }
    lines.into_iter().map(|(r, _)| r).collect()
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
    device.finish()
}

#[derive(Default)]
struct TextDevice {
    /// Glyphs in content order; [`TextDevice::finish`] puts them in reading order.
    items: Vec<TextItem>,
    /// Per item, whether a fragment (a run of text on one line) starts there.
    breaks: Vec<bool>,
    chars: usize,
    /// Box and direction of the previous glyph, for spotting word and line breaks.
    last: Option<(Rect, Dir)>,
    /// A space glyph was drawn since the last visible one.
    space: bool,
}

impl TextDevice {
    fn push(&mut self, s: &str, rect: Rect, dir: Dir) {
        let s: String = s.chars().filter(|c| !c.is_control() || c.is_whitespace()).collect();
        if s.trim().is_empty() {
            self.space |= !s.is_empty();
            return;
        }
        let gap = match self.last {
            None => Gap::None,
            // Text turning another way starts a new line.
            Some((_, last_dir)) if last_dir != dir => Gap::Line,
            Some((last, _)) => {
                // Measured along and across the line, so turned text reads like upright text.
                let ((_, last_end), (l0, l1)) = dir.span(&last);
                let ((start, _), (r0, r1)) = dir.span(&rect);
                let size = (l1 - l0).max(r1 - r0).max(1.0);
                let new_line = ((r0 + r1) / 2.0 - (l0 + l1) / 2.0).abs() > size * 0.5;
                let gap = start - last_end;
                if new_line || gap < -size * 2.0 {
                    Gap::Line
                } else if self.space || gap > size * 0.15 {
                    Gap::Space
                } else {
                    Gap::None
                }
            }
        };
        self.space = false;
        self.chars += s.chars().count();
        // A jump far along the line (to the next column, say) also starts a fragment.
        let jump = self.last.is_some_and(|(last, d)| {
            let ((_, end), (c0, c1)) = d.span(&last);
            d == dir && dir.span(&rect).0.0 - end > (c1 - c0).max(1.0) * 1.2
        });
        self.breaks.push(gap == Gap::Line || jump);
        self.items.push(TextItem { text: s, rect, gap, dir });
        self.last = Some((rect, dir));
    }

    /// The page's text, in reading order.
    fn finish(self) -> PageText {
        PageText::new(reading_order(self.items, &self.breaks))
    }
}

impl PageText {
    /// Builds the search copy (lower-cased, one space per break) from `items`.
    fn new(items: Vec<TextItem>) -> Self {
        let mut chars = Vec::new();
        let mut upper = Vec::new();
        let mut boxes = Vec::new();
        let mut soft = HashSet::new();
        for item in &items {
            let broken_word = item.gap == Gap::Line
                && chars.last() == Some(&'-')
                && chars.len() > 1
                && chars[chars.len() - 2].is_alphabetic()
                && item.text.chars().next().is_some_and(char::is_lowercase);
            if broken_word {
                soft.insert(chars.len() - 1);
            } else if item.gap != Gap::None && chars.last().is_some_and(|c| *c != ' ') {
                chars.push(' ');
                upper.push(false);
                boxes.push(None);
            }
            for original in folded(&item.text) {
                for c in original.to_lowercase() {
                    if c.is_whitespace() {
                        if chars.last().is_some_and(|c| *c != ' ') {
                            chars.push(' ');
                            upper.push(false);
                            boxes.push(None);
                        }
                    } else {
                        chars.push(c);
                        upper.push(original.is_uppercase());
                        boxes.push(Some((item.rect, item.dir)));
                    }
                }
            }
        }
        Self { items, chars, upper, boxes, soft }
    }
}

// ---------------------------------------------------------------- reading order
//
// Content order is how the file happens to draw the text. It is usually right, and
// for tables and code it is better than anything worked out from positions; but
// for pages in columns it often goes across them, line by line. So the order is
// changed only where the page clearly has columns of running text: glyphs are
// grouped into fragments (runs on one line), fragments into rows, and consecutive
// rows that share a narrow vertical gap (a gutter) into a band. A band whose sides
// are both set flush and in long lines is read column by column (recursively, so
// three columns work too); everything else keeps the file's order.

/// A run of glyphs on one line, in content order, and its extent along and across the line.
#[derive(Debug, Clone, Copy)]
struct Fragment {
    items: (usize, usize),
    along: (f64, f64),
    across: (f64, f64),
    dir: Dir,
}

impl Fragment {
    fn height(&self) -> f64 {
        self.across.1 - self.across.0
    }
}

/// Consecutive rows of fragments, and the gutters they all share.
type Band = (Vec<Vec<usize>>, Vec<(f64, f64)>);

/// Deepest nesting of columns looked for.
const MAX_COLUMN_DEPTH: u8 = 6;

fn reading_order(items: Vec<TextItem>, breaks: &[bool]) -> Vec<TextItem> {
    if items.len() < 2 {
        return items;
    }
    let mut fragments: Vec<Fragment> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let ((a0, a1), (c0, c1)) = item.dir.span(&item.rect);
        match fragments.last_mut() {
            Some(f) if !breaks.get(i).copied().unwrap_or(true) => {
                f.items.1 = i + 1;
                f.along = (f.along.0.min(a0), f.along.1.max(a1));
                f.across = (f.across.0.min(c0), f.across.1.max(c1));
            }
            _ => fragments.push(Fragment { items: (i, i + 1), along: (a0, a1), across: (c0, c1), dir: item.dir }),
        }
    }
    // Each writing direction is laid out on its own, in the order it first appears.
    let mut dirs: Vec<Dir> = Vec::new();
    for f in &fragments {
        if !dirs.contains(&f.dir) {
            dirs.push(f.dir);
        }
    }
    let mut order = Vec::with_capacity(fragments.len());
    for dir in dirs {
        let group: Vec<usize> = (0..fragments.len()).filter(|&i| fragments[i].dir == dir).collect();
        layout(&fragments, group, 0, &mut order);
    }

    if order.iter().enumerate().all(|(i, &k)| i == k) {
        return items;
    }
    let mut slots: Vec<Option<TextItem>> = items.into_iter().map(Some).collect();
    let mut out = Vec::with_capacity(slots.len());
    let mut previous: Option<(usize, &Fragment)> = None;
    for &k in &order {
        let f = &fragments[k];
        for (n, slot) in slots[f.items.0..f.items.1].iter_mut().enumerate() {
            let Some(mut item) = slot.take() else { continue };
            if n == 0 {
                item.gap = match previous {
                    None => Gap::None,
                    // Still in the file's order: keep its spacing.
                    Some((p, _)) if p + 1 == k => item.gap,
                    // Next to the previous fragment on the same line: a word gap.
                    Some((_, p)) if p.dir == f.dir && same_row(p, f) && f.along.0 >= p.along.1 - p.height() => {
                        Gap::Space
                    }
                    Some(_) => Gap::Line,
                };
            }
            out.push(item);
        }
        previous = Some((k, f));
    }
    out
}

/// Whether two fragments sit on the same line.
fn same_row(a: &Fragment, b: &Fragment) -> bool {
    let overlap = a.across.1.min(b.across.1) - a.across.0.max(b.across.0);
    overlap > a.height().min(b.height()) * 0.5
}

/// The gaps of at least `min` between `spans` (sorted, possibly overlapping) inside `lo..hi`.
fn free_space(spans: &[(f64, f64)], lo: f64, hi: f64, min: f64) -> Vec<(f64, f64)> {
    let mut gaps = Vec::new();
    let mut at = lo;
    for &(s, e) in spans {
        if s - at >= min {
            gaps.push((at, s));
        }
        at = at.max(e);
    }
    if hi - at >= min {
        gaps.push((at, hi));
    }
    gaps
}

fn intersect(a: &[(f64, f64)], b: &[(f64, f64)], min: f64) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for &(a0, a1) in a {
        for &(b0, b1) in b {
            let (s, e) = (a0.max(b0), a1.min(b1));
            if e - s >= min {
                out.push((s, e));
            }
        }
    }
    out
}

/// Appends the fragments of `group` to `order` in reading order.
fn layout(fragments: &[Fragment], mut group: Vec<usize>, depth: u8, order: &mut Vec<usize>) {
    if group.len() < 2 {
        order.extend(group);
        return;
    }
    let mut heights: Vec<f64> = group.iter().map(|&i| fragments[i].height()).collect();
    heights.sort_by(f64::total_cmp);
    let size = heights[heights.len() / 2].max(1.0);
    let gutter = size;
    let lo = group.iter().map(|&i| fragments[i].along.0).fold(f64::INFINITY, f64::min);
    let hi = group.iter().map(|&i| fragments[i].along.1).fold(f64::NEG_INFINITY, f64::max);

    // Rows: fragments whose extents across overlap, top to bottom.
    group.sort_by(|&a, &b| fragments[a].across.0.total_cmp(&fragments[b].across.0));
    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut row_end = f64::NEG_INFINITY;
    for &i in &group {
        let f = &fragments[i];
        match rows.last_mut() {
            Some(row) if f.across.0 < row_end - f.height().min(size) * 0.3 => {
                row.push(i);
                row_end = row_end.max(f.across.1);
            }
            _ => {
                rows.push(vec![i]);
                row_end = f.across.1;
            }
        }
    }
    let spans = |row: &[usize]| {
        let mut v: Vec<(f64, f64)> = row.iter().map(|&i| fragments[i].along).collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        v
    };
    // Gaps between a row's own pieces (not its margins).
    let inner = |row: &[usize]| {
        let s = spans(row);
        let (first, last) = (s.first().map_or(lo, |x| x.0), s.iter().map(|x| x.1).fold(lo, f64::max));
        free_space(&s, first, last, gutter)
    };

    // Bands of consecutive rows that share a gutter. A clearly wider gap than between
    // lines (before a footer, say) also ends a band of columns.
    let top = |row: &[usize]| row.iter().map(|&i| fragments[i].across.0).fold(f64::INFINITY, f64::min);
    let bottom = |row: &[usize]| row.iter().map(|&i| fragments[i].across.1).fold(f64::NEG_INFINITY, f64::max);
    let mut bands: Vec<Band> = Vec::new();
    for row in rows {
        let own = inner(&row);
        let free = free_space(&spans(&row), lo, hi, gutter);
        let apart = bands.last().and_then(|(r, _)| r.last()).is_some_and(|last| top(&row) - bottom(last) > 1.8 * size);
        match bands.last_mut() {
            Some((_, gaps)) if !gaps.is_empty() && apart => bands.push((vec![row], own)),
            Some((band_rows, gaps)) if !gaps.is_empty() => {
                let kept = intersect(gaps, &free, gutter);
                if kept.is_empty() {
                    bands.push((vec![row], own));
                } else {
                    *gaps = kept;
                    band_rows.push(row);
                }
            }
            Some((band_rows, _)) if own.is_empty() => band_rows.push(row),
            _ => bands.push((vec![row], own)),
        }
    }

    for (band_rows, gaps) in bands {
        let members: Vec<usize> = band_rows.iter().flatten().copied().collect();
        // The first gutter splits off the first column (the rest is split again), if both
        // sides read like running text.
        let split = gaps.iter().copied().min_by(|a, b| a.0.total_cmp(&b.0)).and_then(|(g0, g1)| {
            let (left, right): (Vec<usize>, Vec<usize>) =
                members.iter().partition(|&&i| fragments[i].along.1 <= (g0 + g1) / 2.0);
            // Running text: lines of at least ten ems.
            let prose = |side: &[usize]| {
                let mut widths: Vec<f64> = side.iter().map(|&i| fragments[i].along.1 - fragments[i].along.0).collect();
                widths.sort_by(f64::total_cmp);
                !widths.is_empty() && widths[widths.len() / 2] >= 10.0 * size
            };
            // A column of text is set flush: most of its lines start, and end, at the same
            // place. Code with comments beside it, or a table, is ragged somewhere.
            let flush = |side: &[usize]| {
                let start = side.iter().map(|&i| fragments[i].along.0).fold(f64::INFINITY, f64::min);
                let end = side.iter().map(|&i| fragments[i].along.1).fold(f64::NEG_INFINITY, f64::max);
                let starts = side.iter().filter(|&&i| fragments[i].along.0 - start <= 0.6 * size).count();
                let ends = side.iter().filter(|&&i| end - fragments[i].along.1 <= 0.6 * size).count();
                starts * 10 >= side.len() * 6 && ends * 10 >= side.len() * 6
            };
            let narrow = g1 - g0 <= 5.0 * size;
            let columns = band_rows.len() >= 3 && depth < MAX_COLUMN_DEPTH && narrow;
            // The right side may hold more columns; judge the one next to the gutter.
            let next: Vec<usize> = right.iter().copied().filter(|&i| fragments[i].along.0 - g1 <= 0.6 * size).collect();
            (columns && next.len() >= 3 && prose(&left) && prose(&next) && flush(&left) && flush(&next))
                .then_some((left, right))
        });
        match split {
            Some((left, right)) => {
                layout(fragments, left, depth + 1, order);
                layout(fragments, right, depth + 1, order);
            }
            // No columns here: the file's order.
            None => {
                let mut members = members;
                members.sort_unstable();
                order.extend(members);
            }
        }
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
            if self.chars > 2_000_000 {
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
            // The way the glyph advances on the page, turned with the page and the text.
            let along = full * Point::new(advance, 0.0) - full * Point::ORIGIN;
            let dir = Dir::of(along.x, along.y);
            match unicode {
                BfString::Char(c) => self.push(c.encode_utf8(&mut [0; 4]), rect, dir),
                BfString::String(s) => self.push(&s, rect, dir),
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
    fn matches_case_and_whole_words_when_asked() {
        let mut device = TextDevice::default();
        let mut x = 0.0;
        for word in ["Cat", "concatenate", "cat", "C++", "C+++"] {
            for c in word.chars() {
                device.push(&c.to_string(), Rect::new(x, 0.0, x + 6.0, 12.0), Dir::Right);
                x += 6.0;
            }
            x += 6.0;
        }
        let text = device.finish();
        let any = SearchOptions::default();
        let case = SearchOptions { match_case: true, ..any };
        let whole = SearchOptions { whole_words: true, ..any };
        assert_eq!(text.find_with("cat", any).len(), 3, "Cat, concatenate, cat");
        assert_eq!(text.find_with("cat", case).len(), 2, "concatenate, cat");
        assert_eq!(text.find_with("Cat", case).len(), 1);
        assert_eq!(text.find_with("cat", whole).len(), 2, "Cat and cat, not concatenate");
        assert_eq!(text.find_with("cat", SearchOptions { match_case: true, whole_words: true }).len(), 1);
        // A word ending in punctuation still matches as a whole word.
        assert_eq!(text.find_with("c++", whole).len(), 2, "C++ and the start of C+++");
    }

    /// Lines of words, each word its glyphs, given as the strings the PDF maps them to.
    fn lines(lines: &[&[&str]]) -> PageText {
        let mut device = TextDevice::default();
        for (row, glyphs) in lines.iter().enumerate() {
            let (mut x, y) = (0.0, row as f64 * 20.0);
            for glyph in *glyphs {
                if *glyph != " " {
                    device.push(glyph, Rect::new(x, y, x + 6.0, y + 12.0), Dir::Right);
                }
                x += 6.0;
            }
        }
        device.finish()
    }

    #[test]
    fn finds_ligatures_accents_and_words_broken_across_lines() {
        let any = SearchOptions::default();
        let text = lines(&[&["e", "ﬃ", "c", "i", "e", "n", "t"], &["r", "e\u{301}", "s", "u", "m", "é"]]);
        assert_eq!(text.find("efficient").len(), 1, "a ligature");
        assert_eq!(text.find("eﬃcient").len(), 1, "also when it is typed");
        assert_eq!(text.find("résumé").len(), 1, "accents written apart or not");
        assert_eq!(text.find("RE\u{301}SUME\u{301}").len(), 1);
        let text =
            lines(&[&["i", "n", "f", "o", "r", "-"], &["m", "a", "t", "i", "o", "n", " ", "x", "-"], &["Y", "z"]]);
        assert_eq!(text.find("information").len(), 1, "a word the line break cut");
        assert_eq!(text.find("information")[0].len(), 2, "a box on each line");
        assert_eq!(text.find("infor-mation").len(), 1, "or one with a hyphen of its own");
        assert_eq!(text.find_with("information", SearchOptions { whole_words: true, ..any }).len(), 1);
        assert_eq!(text.find_with("mation", SearchOptions { whole_words: true, ..any }).len(), 0);
        assert_eq!(text.find("xyz").len(), 0, "a capital does not continue a word");
        assert_eq!(lines(&[&["a", "b", "\u{AD}"], &["c", "d"]]).find("ab cd").len(), 1);
    }

    #[test]
    fn finds_case_insensitively_across_spaces() {
        let mut device = TextDevice::default();
        let mut x = 0.0;
        for word in ["Hello", "World"] {
            for c in word.chars() {
                device.push(&c.to_string(), Rect::new(x, 0.0, x + 6.0, 12.0), Dir::Right);
                x += 6.0;
            }
            x += 6.0; // a word gap
        }
        let text = device.finish();
        let hits = text.find("o  w");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].len(), 1, "one line, one box");
        assert!(text.find("hello world").len() == 1);
        assert!(text.find("xyz").is_empty());
    }

    #[test]
    fn selects_and_copies() {
        let mut device = TextDevice::default();
        for (line, words) in [["Hello", "World"], ["Second", "line"]].iter().enumerate() {
            let y = line as f64 * 20.0;
            let mut x = 0.0;
            for word in words {
                for c in word.chars() {
                    device.push(&c.to_string(), Rect::new(x, y, x + 6.0, y + 12.0), Dir::Right);
                    x += 6.0;
                }
                device.push(" ", Rect::new(x, y, x + 3.0, y + 12.0), Dir::Right);
                x += 3.0;
            }
        }
        let t = &device.finish();
        assert_eq!(t.len(), 20);
        assert_eq!(t.text(0..t.len()), "Hello World\nSecond line");
        // From the right half of the "o" of "Hello" to the right half of the "S" of "Second".
        let from = t.caret_at(Point::new(28.0, 6.0));
        let to = t.caret_at(Point::new(5.0, 26.0));
        assert_eq!((from, to), (5, 11));
        assert_eq!(t.text(from..to), "World\nS");
        assert_eq!(t.text(t.word_at(Point::new(60.0, 6.0))), "World");
        assert_eq!(t.boxes(0..t.len()).len(), 2, "one box per line");
        assert!(t.is_over_text(Point::new(3.0, 6.0)));
        assert!(!t.is_over_text(Point::new(3.0, 16.0)));
    }

    /// The page of `selects_and_copies`, turned a quarter clockwise: lines run down, the
    /// next line is to the left. A point (x, y) on the upright page is at (H - y, x) here.
    #[test]
    fn turned_pages_read_like_upright_ones() {
        const H: f64 = 40.0;
        let turn = |r: Rect| Rect::new(H - r.y1, r.x0, H - r.y0, r.x1);
        let at = |x: f64, y: f64| Point::new(H - y, x);
        let mut device = TextDevice::default();
        for (line, words) in [["Hello", "World"], ["Second", "line"]].iter().enumerate() {
            let y = line as f64 * 20.0;
            let mut x = 0.0;
            for word in words {
                for c in word.chars() {
                    device.push(&c.to_string(), turn(Rect::new(x, y, x + 6.0, y + 12.0)), Dir::Down);
                    x += 6.0;
                }
                device.push(" ", turn(Rect::new(x, y, x + 3.0, y + 12.0)), Dir::Down);
                x += 3.0;
            }
        }
        let t = &device.finish();
        assert_eq!(t.text(0..t.len()), "Hello World\nSecond line");
        let (from, to) = (t.caret_at(at(28.0, 6.0)), t.caret_at(at(5.0, 26.0)));
        assert_eq!(t.text(from..to), "World\nS");
        assert_eq!(t.text(t.word_at(at(60.0, 6.0))), "World");
        let lines = t.boxes(0..t.len());
        assert_eq!(lines.len(), 2, "one box per line");
        assert!(lines[0].height() > lines[0].width(), "a turned line is tall: {lines:?}");
        assert_eq!(t.find("hello world").len(), 1);
        assert_eq!(t.find("world second")[0].len(), 2, "a match over two lines has two boxes");
        assert!(t.is_over_text(at(3.0, 6.0)) && !t.is_over_text(at(3.0, 16.0)));

        // Text turning from one line to the next still breaks the line.
        let mut mixed = TextDevice::default();
        mixed.push("a", Rect::new(0.0, 0.0, 6.0, 12.0), Dir::Right);
        mixed.push("b", Rect::new(6.0, 0.0, 12.0, 12.0), Dir::Down);
        assert_eq!(mixed.finish().text(0..2), "a\nb");
    }

    /// A real page with /Rotate 90: its text comes out in words and lines, not a letter a line.
    #[test]
    fn text_of_a_rotated_page() {
        let bytes = crate::pdf::tests::structured_bytes();
        let plan = [crate::pages::Entry { source: 0, index: 0, rotate: 90 }];
        let turned = crate::pages::compose(&crate::pages::Source { bytes, password: None }, &[], &plan).unwrap();
        let pdf = Pdf::new(turned).unwrap();
        let cache = InterpreterCache::new();
        let text = page_text(&pdf.pages()[0], &cache);
        assert_eq!(text.text(0..text.len()), "Hello Lumen World");
        assert_eq!(text.find("lumen").len(), 1);
    }

    /// Draws `text` as 6×12 glyphs from (`x`, `y`), mapped through `place` (for turned pages).
    fn draw(device: &mut TextDevice, text: &str, x: f64, y: f64, place: &dyn Fn(Rect) -> Rect, dir: Dir) {
        let mut x = x;
        for c in text.chars() {
            let w = if c == ' ' { 3.0 } else { 6.0 };
            device.push(&c.to_string(), place(Rect::new(x, y, x + w, y + 12.0)), dir);
            x += w;
        }
    }

    /// A title, two columns of three lines drawn across (left line, right line, …), and a footer.
    fn two_columns(place: &dyn Fn(Rect) -> Rect, dir: Dir) -> String {
        let mut d = TextDevice::default();
        draw(&mut d, "A Title Across Both Columns", 0.0, 0.0, place, dir);
        // Justified columns: every line the same width, a 24 pt gutter between them.
        let left =
            ["left column first line of text", "left column second line of it", "left column third and last one"];
        let right = ["right column first line of it", "right column second line here", "right column third and last o"];
        for (k, (l, r)) in left.iter().zip(right).enumerate() {
            let y = 30.0 + k as f64 * 16.0;
            draw(&mut d, l, 0.0, y, place, dir);
            draw(&mut d, r, 180.0, y, place, dir);
        }
        draw(&mut d, "Page footer", 0.0, 100.0, place, dir);
        let t = d.finish();
        t.text(0..t.len())
    }

    const TWO_COLUMNS: &str = "A Title Across Both Columns\nleft column first line of text\nleft column second line of it\n\
         left column third and last one\nright column first line of it\nright column second line here\n\
         right column third and last o\nPage footer";

    #[test]
    fn columns_are_read_one_after_the_other() {
        assert_eq!(two_columns(&|r| r, Dir::Right), TWO_COLUMNS);
        // The same page turned a quarter clockwise reads the same.
        const H: f64 = 200.0;
        assert_eq!(two_columns(&|r: Rect| Rect::new(H - r.y1, r.x0, H - r.y0, r.x1), Dir::Down), TWO_COLUMNS);

        let mut d = TextDevice::default();
        for k in 0..4 {
            let y = k as f64 * 16.0;
            for (c, x) in ["a", "b", "c"].iter().zip([0.0, 190.0, 380.0]) {
                draw(&mut d, &format!("column {c} line {k} has some words"), x, y, &|r| r, Dir::Right);
            }
        }
        let t = d.finish();
        let text = t.text(0..t.len());
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "column a line 0 has some words");
        assert_eq!(lines[3], "column a line 3 has some words");
        assert_eq!(lines[4], "column b line 0 has some words");
        assert_eq!(lines[11], "column c line 3 has some words");
        // Search follows reading order: from the end of one column into the next.
        assert_eq!(t.find("line 3 has some words column b line 0").len(), 1);
    }

    #[test]
    fn code_with_comments_beside_it_stays_in_rows() {
        let mut d = TextDevice::default();
        for (k, (code, note)) in [
            ("profile *device_named;", "/* The named color profile */"),
            ("profile *gray;", "/* Default gray profile for gray */"),
            ("profile *default_rgb;", "/* Default RGB profile for RGB */"),
            ("bool override;", "/* Whether to override the profiles */"),
        ]
        .iter()
        .enumerate()
        {
            let y = k as f64 * 16.0;
            draw(&mut d, code, 0.0, y, &|r| r, Dir::Right);
            draw(&mut d, note, 220.0, y, &|r| r, Dir::Right);
        }
        let t = d.finish();
        assert!(t.text(0..t.len()).starts_with("profile *device_named; /* The named color profile */\nprofile *gray;"));
    }

    #[test]
    fn tables_of_contents_and_short_columns_stay_in_rows() {
        let mut d = TextDevice::default();
        for (k, (title, page)) in
            [("Introduction", "1"), ("Getting started", "4"), ("Advanced topics", "12"), ("Index", "40")]
                .iter()
                .enumerate()
        {
            let y = k as f64 * 16.0;
            draw(&mut d, title, 0.0, y, &|r| r, Dir::Right);
            draw(&mut d, page, 300.0, y, &|r| r, Dir::Right);
        }
        let t = d.finish();
        assert_eq!(t.text(0..t.len()), "Introduction 1\nGetting started 4\nAdvanced topics 12\nIndex 40");

        // Two short side-by-side lines (a heading and a date, say) are not columns.
        let mut d = TextDevice::default();
        draw(&mut d, "Letter heading", 0.0, 0.0, &|r| r, Dir::Right);
        draw(&mut d, "3 May", 300.0, 0.0, &|r| r, Dir::Right);
        draw(&mut d, "Dear reader, this is one column of text.", 0.0, 20.0, &|r| r, Dir::Right);
        let t = d.finish();
        assert_eq!(t.text(0..t.len()), "Letter heading 3 May\nDear reader, this is one column of text.");
    }

    /// A real two-column page whose content is drawn across the columns.
    #[test]
    fn a_two_column_pdf_reads_in_columns() {
        use pdf_writer::{Content, Finish, Name, Rect as PRect, Ref, Str};
        let r = Ref::new;
        let mut w = pdf_writer::Pdf::new();
        w.catalog(r(1)).pages(r(2));
        w.pages(r(2)).kids([r(3)]).count(1);
        let mut page = w.page(r(3));
        page.parent(r(2)).media_box(PRect::new(0.0, 0.0, 600.0, 800.0)).contents(r(4));
        page.resources().fonts().pair(Name(b"F1"), r(5));
        page.finish();
        let mut c = Content::new();
        c.begin_text().set_font(Name(b"F1"), 10.0);
        for k in 0..5 {
            let y = 700.0 - k as f32 * 14.0;
            c.set_text_matrix([1.0, 0.0, 0.0, 1.0, 50.0, y])
                .show(Str(format!("Left column line {k} of prose").as_bytes()));
            c.set_text_matrix([1.0, 0.0, 0.0, 1.0, 200.0, y])
                .show(Str(format!("Right column line {k} of prose").as_bytes()));
        }
        c.end_text();
        w.stream(r(4), &c.finish());
        w.type1_font(r(5)).base_font(Name(b"Helvetica"));
        let pdf = Pdf::new(w.finish()).unwrap();
        let text = page_text(&pdf.pages()[0], &InterpreterCache::new());
        let all = text.text(0..text.len());
        let lines: Vec<&str> = all.lines().collect();
        assert_eq!(lines.len(), 10, "{all}");
        assert!(
            lines[..5].iter().all(|l| l.starts_with("Left")) && lines[5..].iter().all(|l| l.starts_with("Right")),
            "{all}"
        );
    }

    #[test]
    fn rejects_unsafe_uris() {
        assert!(is_safe_uri("https://example.com"));
        assert!(is_safe_uri("mailto:a@b.c"));
        assert!(!is_safe_uri("file:///etc/passwd"));
        assert!(!is_safe_uri("javascript:alert(1)"));
    }
}
