//! What the Properties dialog shows: the file, and for a PDF its document information,
//! for a picture its size, colour profile and EXIF summary. Everything here is blocking.

use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context, Result};
use exif::{In, Tag, Value};

use crate::library::Kind;

/// Labelled rows in titled groups.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sheet {
    pub sections: Vec<(String, Vec<(String, String)>)>,
}

impl Sheet {
    fn section(&mut self, title: &str, rows: Vec<(&str, Option<String>)>) {
        let rows: Vec<(String, String)> = rows
            .into_iter()
            .filter_map(|(k, v)| v.filter(|v| !v.trim().is_empty()).map(|v| (k.to_string(), v)))
            .collect();
        if !rows.is_empty() {
            self.sections.push((title.to_string(), rows));
        }
    }
}

/// Reads everything about `path`; a password-protected PDF needs its `password`.
pub fn read(path: &Path, password: Option<&str>) -> Result<Sheet> {
    let meta = std::fs::metadata(path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut sheet = Sheet::default();
    sheet.section(
        "File",
        vec![
            ("Name", path.file_name().map(|n| n.to_string_lossy().into_owned())),
            ("Folder", path.parent().map(|p| p.display().to_string())),
            ("Size", Some(file_size(meta.len()))),
            ("Modified", meta.modified().ok().map(format_time)),
            ("Created", meta.created().ok().map(format_time)),
        ],
    );
    match Kind::of(path) {
        Some(Kind::Pdf) => pdf_sections(&mut sheet, path, password)?,
        Some(Kind::Image) => picture_sections(&mut sheet, path),
        Some(Kind::Svg) => sheet.section("Picture", vec![("Format", Some("SVG (vector)".into()))]),
        None => {}
    }
    Ok(sheet)
}

// ------------------------------------------------------------------ PDF

fn pdf_sections(sheet: &mut Sheet, path: &Path, password: Option<&str>) -> Result<()> {
    use hayro::hayro_syntax::PdfVersion as V;
    let bytes = Arc::new(std::fs::read(path)?);
    let encrypted = lopdf::Document::load_mem(&bytes).map(|d| d.is_encrypted()).unwrap_or(password.is_some());
    let pdf = crate::pdf::parse(bytes, password)?;
    let m = pdf.metadata();
    let text = |b: &Option<Box<[u8]>>| b.as_deref().map(pdf_text);
    let date = |d: &Option<hayro::hayro_syntax::object::DateTime>| {
        d.map(|d| format!("{:04}-{:02}-{:02} {:02}:{:02}", d.year, d.month, d.day, d.hour, d.minute))
    };
    sheet.section(
        "Document",
        vec![
            ("Title", text(&m.title)),
            ("Author", text(&m.author)),
            ("Subject", text(&m.subject)),
            ("Keywords", text(&m.keywords)),
            ("Creator", text(&m.creator)),
            ("Producer", text(&m.producer)),
            ("Created", date(&m.creation_date)),
            ("Modified", date(&m.modification_date)),
        ],
    );
    let version = match pdf.version() {
        V::Pdf10 => "1.0",
        V::Pdf11 => "1.1",
        V::Pdf12 => "1.2",
        V::Pdf13 => "1.3",
        V::Pdf14 => "1.4",
        V::Pdf15 => "1.5",
        V::Pdf16 => "1.6",
        V::Pdf17 => "1.7",
        V::Pdf20 => "2.0",
    };
    let pages = pdf.pages();
    let size = pages.first().map(|p| {
        let (w, h) = p.render_dimensions();
        page_size_label(w, h)
    });
    let fields = crate::forms::read(&pdf).fields.len();
    sheet.section(
        "PDF",
        vec![
            ("Version", Some(version.to_string())),
            ("Pages", Some(pages.len().to_string())),
            ("Page size", size),
            ("Protected", Some(if encrypted { "Yes, with a password" } else { "No" }.to_string())),
            ("Form fields", (fields > 0).then(|| fields.to_string())),
        ],
    );
    Ok(())
}

/// A PDF text string: UTF-16 with a byte order mark, UTF-8 with one, or PDFDocEncoding
/// (which matches Latin-1 for everything that matters in a title).
pub fn pdf_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
        String::from_utf16_lossy(&units)
    } else if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(rest).into_owned()
    } else {
        bytes.iter().map(|&b| b as char).collect()
    }
}

/// "595 × 842 pt (210 × 297 mm, A4)".
pub fn page_size_label(w: f32, h: f32) -> String {
    let mm = |pt: f32| pt * 25.4 / 72.0;
    let (short, long) = (w.min(h), w.max(h));
    let name = [
        ("A5", 420.0, 595.0),
        ("A4", 595.0, 842.0),
        ("A3", 842.0, 1191.0),
        ("Letter", 612.0, 792.0),
        ("Legal", 612.0, 1008.0),
    ]
    .iter()
    .find(|(_, s, l)| (short - s).abs() < 3.0 && (long - l).abs() < 3.0)
    .map(|(n, ..)| format!(", {n}"))
    .unwrap_or_default();
    format!("{:.0} × {:.0} pt ({:.0} × {:.0} mm{name})", w, h, mm(w), mm(h))
}

// -------------------------------------------------------------- picture

fn picture_sections(sheet: &mut Sheet, path: &Path) {
    let reader = image::ImageReader::open(path).ok().and_then(|r| r.with_guessed_format().ok());
    let format = reader.as_ref().and_then(|r| r.format()).map(|f| format!("{f:?}").to_uppercase());
    let dims = reader.and_then(|r| r.into_dimensions().ok());
    let icc = crate::metadata::icc_profile(path);
    sheet.section(
        "Picture",
        vec![
            ("Format", format),
            ("Size", dims.map(|(w, h)| format!("{w} × {h} px ({:.1} megapixels)", w as f64 * h as f64 / 1e6))),
            (
                "Colour profile",
                Some(match icc {
                    Some(p) => format!(
                        "Embedded ICC profile, {}",
                        file_size(p.len() as u64).split(" (").next().unwrap_or_default()
                    ),
                    None => "None (sRGB assumed)".to_string(),
                }),
            ),
        ],
    );
    let Some(exif) = std::fs::File::open(path)
        .ok()
        .and_then(|f| exif::Reader::new().read_from_container(&mut std::io::BufReader::new(f)).ok())
    else {
        return;
    };
    let get = |tag: Tag| {
        exif.get_field(tag, In::PRIMARY)
            .map(|f| f.display_value().with_unit(&exif).to_string().trim_matches('"').trim().to_string())
    };
    let camera = match (get(Tag::Make), get(Tag::Model)) {
        (Some(make), Some(model)) if model.starts_with(&make) => Some(model),
        (Some(make), Some(model)) => Some(format!("{make} {model}")),
        (a, b) => a.or(b),
    };
    let exposure: Vec<String> = [Tag::ExposureTime, Tag::FNumber, Tag::PhotographicSensitivity, Tag::FocalLength]
        .into_iter()
        .filter_map(get)
        .collect();
    sheet.section(
        "Camera",
        vec![
            ("Camera", camera),
            ("Lens", get(Tag::LensModel)),
            ("Exposure", (!exposure.is_empty()).then(|| exposure.join("  ·  "))),
            ("Taken", get(Tag::DateTimeOriginal).or_else(|| get(Tag::DateTime))),
            ("Location", gps(&exif)),
        ],
    );
}

/// "48.8584° N, 2.2945° E" from the GPS tags.
fn gps(exif: &exif::Exif) -> Option<String> {
    let coordinate = |tag: Tag, reference: Tag| -> Option<(f64, String)> {
        let Value::Rational(parts) = &exif.get_field(tag, In::PRIMARY)?.value else { return None };
        let [d, m, s] = parts.get(..3)? else { return None };
        let degrees = d.to_f64() + m.to_f64() / 60.0 + s.to_f64() / 3600.0;
        let r = exif.get_field(reference, In::PRIMARY)?.display_value().to_string();
        Some((degrees, r.trim_matches('"').to_string()))
    };
    let (lat, ns) = coordinate(Tag::GPSLatitude, Tag::GPSLatitudeRef)?;
    let (lon, ew) = coordinate(Tag::GPSLongitude, Tag::GPSLongitudeRef)?;
    Some(format!("{lat:.4}° {ns}, {lon:.4}° {ew}"))
}

// ------------------------------------------------------------- numbers

/// "1.2 MB (1,234,567 bytes)".
pub fn file_size(bytes: u64) -> String {
    let units = ["bytes", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < units.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    let digits: String = bytes.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    if unit == 0 { format!("{bytes} bytes") } else { format!("{value:.1} {} ({grouped} bytes)", units[unit]) }
}

/// "2026-11-23 15:09 UTC" (the standard library has no time zones).
pub fn format_time(t: SystemTime) -> String {
    let secs = t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02} UTC", rem / 3600, rem % 3600 / 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn decodes_pdf_text_strings() {
        assert_eq!(pdf_text(b"Plain"), "Plain");
        assert_eq!(pdf_text(&[0xFE, 0xFF, 0x00, b'H', 0x00, 0xE9]), "Hé");
        assert_eq!(pdf_text(&[0xEF, 0xBB, 0xBF, b'o', b'k']), "ok");
        assert_eq!(pdf_text(&[0x63, 0x61, 0x66, 0xE9]), "café");
    }

    #[test]
    fn names_common_paper_sizes() {
        assert_eq!(page_size_label(595.0, 842.0), "595 × 842 pt (210 × 297 mm, A4)");
        assert!(page_size_label(792.0, 612.0).ends_with("Letter)"));
        assert!(!page_size_label(500.0, 500.0).contains(", A"));
    }

    #[test]
    fn formats_sizes_and_dates() {
        assert_eq!(file_size(512), "512 bytes");
        assert_eq!(file_size(1_234_567), "1.2 MB (1,234,567 bytes)");
        assert_eq!(format_time(SystemTime::UNIX_EPOCH), "1970-01-01 00:00 UTC");
        // 2026-11-23 15:09:00 UTC
        assert_eq!(format_time(SystemTime::UNIX_EPOCH + Duration::from_secs(1_795_446_540)), "2026-11-23 15:09 UTC");
        // A leap day.
        assert_eq!(format_time(SystemTime::UNIX_EPOCH + Duration::from_secs(1_709_164_800)), "2024-02-29 00:00 UTC");
    }

    #[test]
    fn reads_a_pdf_with_form_fields() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/form.pdf");
        let sheet = read(&path, None).unwrap();
        let rows: Vec<&(String, String)> = sheet.sections.iter().flat_map(|(_, r)| r).collect();
        let find = |k: &str| rows.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        assert_eq!(find("Name"), Some("form.pdf"));
        assert!(find("Version").is_some());
        assert!(find("Pages").is_some_and(|p| p.parse::<usize>().unwrap() >= 1));
        assert!(find("Form fields").is_some());
        assert_eq!(find("Protected"), Some("No"));
    }

    #[test]
    fn reads_the_password_protected_fixture() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/encrypted.pdf");
        assert!(read(&path, None).is_err());
        let sheet = read(&path, Some("secret")).unwrap();
        let protected = sheet.sections.iter().flat_map(|(_, r)| r).find(|(k, _)| k == "Protected");
        assert_eq!(protected.map(|(_, v)| v.as_str()), Some("Yes, with a password"));
    }

    #[test]
    fn reads_a_picture() {
        let dir = std::env::temp_dir().join(format!("lumen-rs-props-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.png");
        image::RgbaImage::from_pixel(300, 200, image::Rgba([1, 2, 3, 255])).save(&path).unwrap();
        let sheet = read(&path, None).unwrap();
        let rows: Vec<&(String, String)> = sheet.sections.iter().flat_map(|(_, r)| r).collect();
        let find = |k: &str| rows.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        assert_eq!(find("Format"), Some("PNG"));
        assert!(find("Colour profile").is_some_and(|c| c.starts_with("None")));
        assert!(rows.iter().any(|(k, v)| k == "Size" && v.starts_with("300 × 200 px")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
