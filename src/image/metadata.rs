//! EXIF metadata shown in the properties panel.

use std::{fs::File, io::BufReader, path::Path};

use exif::{Exif, In, Rational, Tag, Value};

use super::Orientation;

#[derive(Debug, Clone, Default)]
pub struct Metadata {
    pub orientation: Orientation,
    /// Size of the file on disk, in bytes.
    pub file_size: Option<u64>,
    pub camera: Option<String>,
    pub date_taken: Option<String>,
    /// Aperture, shutter speed, ISO and focal length on one line.
    pub exposure: Option<String>,
    /// Decimal degrees, e.g. "47.37690° N, 8.54170° E".
    pub location: Option<String>,
}

impl Metadata {
    /// Never fails: missing or broken EXIF just leaves the fields empty.
    pub fn read(path: &Path) -> Self {
        let mut meta = Self {
            file_size: std::fs::metadata(path).ok().map(|m| m.len()),
            ..Self::default()
        };
        let Some(exif) = File::open(path)
            .ok()
            .and_then(|f| exif::Reader::new().read_from_container(&mut BufReader::new(f)).ok())
        else {
            return meta;
        };

        if let Some(value) = exif
            .get_field(Tag::Orientation, In::PRIMARY)
            .and_then(|f| f.value.get_uint(0))
        {
            meta.orientation = Orientation::from_exif(value);
        }

        meta.camera = camera(&exif);
        meta.date_taken = ascii(&exif, Tag::DateTimeOriginal).map(|d| reformat_date(&d));
        meta.exposure = exposure(&exif);
        meta.location = location(&exif);
        meta
    }
}

fn ascii(exif: &Exif, tag: Tag) -> Option<String> {
    match &exif.get_field(tag, In::PRIMARY)?.value {
        Value::Ascii(parts) => {
            let text = String::from_utf8_lossy(parts.first()?).trim().to_owned();
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

/// "Canon" + "Canon EOS R5" -> "Canon EOS R5" (many models repeat the make).
fn camera(exif: &Exif) -> Option<String> {
    let make = ascii(exif, Tag::Make);
    let model = ascii(exif, Tag::Model);
    match (make, model) {
        (Some(make), Some(model)) if model.starts_with(&make) => Some(model),
        (Some(make), Some(model)) => Some(format!("{make} {model}")),
        (None, model) => model,
        (make, None) => make,
    }
}

/// EXIF writes "2024:05:01 12:30:00"; show "2024-05-01 12:30:00".
fn reformat_date(raw: &str) -> String {
    match raw.split_once(' ') {
        Some((date, time)) => format!("{} {time}", date.replace(':', "-")),
        None => raw.to_owned(),
    }
}

fn exposure(exif: &Exif) -> Option<String> {
    let shown = |tag| {
        let field = exif.get_field(tag, In::PRIMARY)?;
        Some(field.display_value().with_unit(exif).to_string())
    };
    let iso = exif
        .get_field(Tag::PhotographicSensitivity, In::PRIMARY)
        .and_then(|f| f.value.get_uint(0))
        .map(|iso| format!("ISO {iso}"));
    let parts: Vec<String> = [shown(Tag::FNumber), shutter(exif), iso, shown(Tag::FocalLength)]
        .into_iter()
        .flatten()
        .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// "1/120 s" for fast shutters, "2.5 s" for slow ones.
fn shutter(exif: &Exif) -> Option<String> {
    let Value::Rational(times) = &exif.get_field(Tag::ExposureTime, In::PRIMARY)?.value else {
        return None;
    };
    let seconds = times.first()?.to_f64();
    if seconds <= 0.0 {
        None
    } else if seconds < 0.5 {
        Some(format!("1/{} s", (1.0 / seconds).round()))
    } else {
        Some(format!("{seconds} s"))
    }
}

fn location(exif: &Exif) -> Option<String> {
    let lat = coordinate(exif, Tag::GPSLatitude, Tag::GPSLatitudeRef, ('N', 'S'))?;
    let lon = coordinate(exif, Tag::GPSLongitude, Tag::GPSLongitudeRef, ('E', 'W'))?;
    Some(format!("{lat}, {lon}"))
}

fn coordinate(exif: &Exif, tag: Tag, ref_tag: Tag, (pos, neg): (char, char)) -> Option<String> {
    let Value::Rational(dms) = &exif.get_field(tag, In::PRIMARY)?.value else {
        return None;
    };
    let [d, m, s] = dms.as_slice() else { return None };
    let degrees = to_f64(d) + to_f64(m) / 60.0 + to_f64(s) / 3600.0;
    let hemisphere = ascii(exif, ref_tag)?.chars().next()?.to_ascii_uppercase();
    let hemisphere = if hemisphere == neg { neg } else { pos };
    Some(format!("{degrees:.5}° {hemisphere}"))
}

fn to_f64(r: &Rational) -> f64 {
    r.to_f64()
}
