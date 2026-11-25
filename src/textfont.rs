//! Fonts for the text Lumen writes into a PDF (form values, text boxes): Helvetica, which every viewer
//! has, as long as the text fits its encoding, else a part of one of the system's fonts, embedded.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use lopdf::{Document, Object, ObjectId, Stream, dictionary};
use resvg::usvg::fontdb;

/// Widths of Helvetica's characters in WinAnsiEncoding, from 0x20, in thousandths of the size
/// (Adobe's AFM file). Codes without a character are 0.
#[rustfmt::skip]
const HELVETICA: [u16; 224] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278,
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556,
    1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778,
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556,
    333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556,
    556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584, 0,
    556, 0, 222, 556, 333, 1000, 556, 556, 333, 1000, 667, 333, 1000, 0, 611, 0,
    0, 222, 222, 333, 333, 350, 556, 1000, 333, 1000, 500, 333, 944, 0, 500, 667,
    278, 333, 556, 556, 556, 556, 260, 556, 333, 737, 370, 556, 584, 333, 737, 333,
    400, 584, 333, 333, 333, 556, 537, 278, 333, 333, 365, 556, 834, 834, 834, 611,
    667, 667, 667, 667, 667, 667, 1000, 722, 667, 667, 667, 667, 278, 278, 278, 278,
    722, 722, 778, 778, 778, 778, 778, 584, 778, 722, 722, 722, 722, 667, 667, 611,
    556, 556, 556, 556, 556, 556, 889, 500, 556, 556, 556, 556, 278, 278, 278, 278,
    556, 556, 556, 556, 556, 556, 556, 584, 611, 556, 556, 556, 556, 500, 556, 500,
];

/// The WinAnsiEncoding byte for `c` (the standard fonts' encoding), if it has one.
fn win_ansi(c: char) -> Option<u8> {
    Some(match c {
        ' '..='~' | '\u{A0}'..='\u{FF}' => c as u8,
        '\t' => b' ',
        '€' => 0x80,
        '‚' => 0x82,
        'ƒ' => 0x83,
        '„' => 0x84,
        '…' => 0x85,
        '†' => 0x86,
        '‡' => 0x87,
        'ˆ' => 0x88,
        '‰' => 0x89,
        'Š' => 0x8A,
        '‹' => 0x8B,
        'Œ' => 0x8C,
        'Ž' => 0x8E,
        '‘' => 0x91,
        '’' => 0x92,
        '“' => 0x93,
        '”' => 0x94,
        '•' => 0x95,
        '–' => 0x96,
        '—' => 0x97,
        '˜' => 0x98,
        '™' => 0x99,
        'š' => 0x9A,
        '›' => 0x9B,
        'œ' => 0x9C,
        'ž' => 0x9E,
        'Ÿ' => 0x9F,
        _ => return None,
    })
}

pub enum TextFont {
    Helvetica,
    Embedded {
        font: ObjectId,
        /// Each character's glyph in the embedded font, and its width in thousandths of the size.
        glyphs: HashMap<char, (u16, f64)>,
    },
}

impl TextFont {
    /// A font that can write `text`, with what it needs added to `doc`. If no font here can, it is
    /// Helvetica, which writes `?` for what it lacks.
    pub fn for_text(doc: &mut Document, text: &str) -> Self {
        if text.chars().filter(|c| !c.is_control()).all(|c| win_ansi(c).is_some()) {
            return Self::Helvetica;
        }
        embed(doc, text).unwrap_or_else(|| {
            tracing::warn!("no font here has all the characters of the text");
            Self::Helvetica
        })
    }

    /// The entry for a /Font resource dictionary.
    pub fn resource(&self) -> Object {
        match self {
            Self::Helvetica => Object::Dictionary(dictionary! {
                "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding",
            }),
            Self::Embedded { font, .. } => Object::Reference(*font),
        }
    }

    /// `line` as a hexadecimal string for `Tj`, without the brackets.
    pub fn encoded(&self, line: &str) -> String {
        let mut out = String::new();
        for c in line.chars() {
            let _ = match self {
                Self::Helvetica => write!(out, "{:02X}", win_ansi(c).unwrap_or(b'?')),
                Self::Embedded { glyphs, .. } => write!(out, "{:04X}", glyphs.get(&c).map_or(0, |g| g.0)),
            };
        }
        out
    }

    /// How wide `line` is, at size 1.
    pub fn width(&self, line: &str) -> f64 {
        let thousandths: f64 = line
            .chars()
            .map(|c| match self {
                Self::Helvetica => f64::from(HELVETICA[usize::from(win_ansi(c).unwrap_or(b'?') - 0x20)]),
                Self::Embedded { glyphs, .. } => glyphs.get(&c).map_or(0.0, |g| g.1),
            })
            .sum();
        thousandths / 1000.0
    }
}

/// `text` in lines no wider than `width` (at size 1), broken between words, or inside one that is
/// too long by itself.
pub fn wrapped(text: &str, font: &TextFont, width: f64) -> Vec<String> {
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_inclusive(' ') {
            if !line.is_empty() && font.width(line.trim_end()) + gap(&line, font) + font.width(word.trim_end()) > width
            {
                out.push(std::mem::take(&mut line));
            }
            for c in word.chars() {
                line.push(c);
                if c != ' ' && line.chars().count() > 1 && font.width(&line) > width {
                    line.pop();
                    out.push(std::mem::replace(&mut line, c.to_string()));
                }
            }
        }
        out.push(line);
    }
    out
}

/// The spaces a line ends in, which only count once a word follows.
fn gap(line: &str, font: &TextFont) -> f64 {
    font.width(&line[line.trim_end().len()..])
}

/// A part of a system font with the characters of `text`.
fn embed(doc: &mut Document, text: &str) -> Option<TextFont> {
    let options = crate::raster::svg_options();
    let db = &options.fontdb;
    let mut chars: Vec<char> = text.chars().filter(|c| !c.is_control()).collect();
    chars.sort_unstable();
    chars.dedup();
    let usual = fontdb::Query { families: &[fontdb::Family::SansSerif], ..Default::default() };
    let plain =
        |f: &&fontdb::FaceInfo| f.style == fontdb::Style::Normal && f.weight == fontdb::Weight::NORMAL && !f.monospaced;
    let candidates = db.query(&usual).into_iter().chain(db.faces().filter(plain).map(|f| f.id));
    for id in candidates {
        let made = db.with_face_data(id, |data, index| subset(data, index, &chars)).flatten();
        if let Some(part) = made {
            return Some(part.add_to(doc));
        }
    }
    None
}

struct Part {
    name: String,
    /// The font file, and whether its outlines are PostScript ones.
    data: Vec<u8>,
    postscript: bool,
    glyphs: BTreeMap<char, (u16, f64)>,
    bbox: [f64; 4],
    ascent: f64,
    descent: f64,
}

/// The part of a font with `chars`, if it has them all and may be embedded.
fn subset(data: &[u8], index: u32, chars: &[char]) -> Option<Part> {
    let face = ttf_parser::Face::parse(data, index).ok()?;
    if face.permissions() == Some(ttf_parser::Permissions::Restricted) || face.is_variable() {
        return None;
    }
    let mut remapper = subsetter::GlyphRemapper::new();
    let scale = 1000.0 / f64::from(face.units_per_em());
    let mut glyphs = BTreeMap::new();
    for &c in chars {
        let glyph = face.glyph_index(c)?;
        let width = f64::from(face.glyph_hor_advance(glyph)?) * scale;
        glyphs.insert(c, (remapper.remap(glyph.0), width));
    }
    let font = subsetter::subset(data, index, &remapper).ok()?;
    let name = face.names().into_iter().find(|n| n.name_id == ttf_parser::name_id::POST_SCRIPT_NAME);
    let name: String = name.and_then(|n| n.to_string()).unwrap_or_else(|| "Font".into());
    let cff = face.raw_face().table(ttf_parser::Tag::from_bytes(b"CFF ")).is_some();
    let data = if cff {
        let raw = ttf_parser::RawFace::parse(&font, 0).ok()?;
        raw.table(ttf_parser::Tag::from_bytes(b"CFF "))?.to_vec()
    } else {
        font
    };
    let b = face.global_bounding_box();
    let em = |v: i16| f64::from(v) * scale;
    Some(Part {
        name: name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect(),
        data,
        postscript: cff,
        glyphs,
        bbox: [em(b.x_min), em(b.y_min), em(b.x_max), em(b.y_max)],
        ascent: em(face.ascender()),
        descent: em(face.descender()),
    })
}

impl Part {
    fn add_to(self, doc: &mut Document) -> TextFont {
        let real = |v: f64| Object::Real(v as f32);
        let name = format!("LUMENA+{}", self.name);
        let mut file = if self.postscript {
            Stream::new(dictionary! { "Subtype" => "CIDFontType0C" }, self.data)
        } else {
            Stream::new(dictionary! { "Length1" => self.data.len() as i64 }, self.data)
        };
        let _ = file.compress();
        let file = doc.add_object(file);
        let mut descriptor = dictionary! {
            "Type" => "FontDescriptor", "FontName" => Object::Name(name.clone().into_bytes()), "Flags" => 4,
            "FontBBox" => self.bbox.iter().map(|v| real(*v)).collect::<Vec<_>>(), "ItalicAngle" => 0,
            "Ascent" => real(self.ascent), "Descent" => real(self.descent), "CapHeight" => real(self.ascent),
            "StemV" => 80,
        };
        descriptor.set(if self.postscript { "FontFile3" } else { "FontFile2" }, file);
        let descriptor = doc.add_object(descriptor);
        let mut widths: Vec<Object> = Vec::new();
        for (glyph, width) in self.glyphs.values() {
            widths.push(i64::from(*glyph).into());
            widths.push(Object::Array(vec![real(*width)]));
        }
        let mut cid = dictionary! {
            "Type" => "Font", "Subtype" => if self.postscript { "CIDFontType0" } else { "CIDFontType2" },
            "BaseFont" => Object::Name(name.clone().into_bytes()), "FontDescriptor" => descriptor, "W" => widths,
            "CIDSystemInfo" => dictionary! {
                "Registry" => Object::string_literal("Adobe"), "Ordering" => Object::string_literal("Identity"),
                "Supplement" => 0,
            },
        };
        if !self.postscript {
            cid.set("CIDToGIDMap", "Identity");
        }
        let cid = doc.add_object(cid);
        let mut to_unicode = String::from(
            "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /Lumen def /CMapType 2 def\n\
             1 begincodespacerange <0000> <FFFF> endcodespacerange\n",
        );
        let pairs: Vec<_> = self.glyphs.iter().collect();
        for some in pairs.chunks(100) {
            let _ = writeln!(to_unicode, "{} beginbfchar", some.len());
            for (c, (glyph, _)) in some {
                let mut units = [0u16; 2];
                let hex: String = c.encode_utf16(&mut units).iter().map(|u| format!("{u:04X}")).collect();
                let _ = writeln!(to_unicode, "<{glyph:04X}> <{hex}>");
            }
            to_unicode.push_str("endbfchar\n");
        }
        to_unicode.push_str("endcmap end end\n");
        let to_unicode = doc.add_object(Stream::new(dictionary! {}, to_unicode.into_bytes()));
        let font = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type0", "BaseFont" => Object::Name(name.into_bytes()),
            "Encoding" => "Identity-H", "DescendantFonts" => vec![cid.into()], "ToUnicode" => to_unicode,
        });
        TextFont::Embedded { font, glyphs: self.glyphs.into_iter().collect() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helvetica_writes_its_whole_encoding() {
        let f = TextFont::Helvetica;
        assert_eq!(f.encoded("Škoda œuvre ž €"), "8A6B6F6461209C75767265209E2080");
        assert_eq!(f.encoded("я"), "3F");
        assert!((f.width("Hi") - 0.944).abs() < 1e-9 && (f.width("œ") - 0.944).abs() < 1e-9);
        assert!(matches!(TextFont::for_text(&mut Document::new(), "Žižek\nŸ"), TextFont::Helvetica));
    }

    #[test]
    fn lines_break_between_words_and_inside_long_ones() {
        let f = TextFont::Helvetica;
        assert_eq!(wrapped("one two three", &f, f.width("one two")), ["one two ", "three"]);
        assert_eq!(wrapped("a\n\nb", &f, 5.0), ["a", "", "b"]);
        assert_eq!(wrapped("iiiiii", &f, f.width("iii")), ["iii", "iii"]);
        // A box narrower than a letter still ends.
        assert_eq!(wrapped("ab c", &f, 0.01).concat().replace(' ', ""), "abc");
    }

    #[test]
    fn other_scripts_use_an_embedded_part_of_a_system_font() {
        let mut doc = Document::with_version("1.7");
        // Not every machine has a font for it; then it is question marks, not a failure.
        let font = TextFont::for_text(&mut doc, "Привет, Ω");
        if let TextFont::Embedded { glyphs, .. } = &font {
            assert_eq!(glyphs.len(), 9);
            assert_eq!(font.encoded("Пр").len(), 8);
            assert!(font.width("Привет") > 2.0 && font.width("Привет") < 5.0);
        }
    }
}
