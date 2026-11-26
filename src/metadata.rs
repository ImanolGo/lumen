//! Carrying a picture's metadata into an edited copy: the EXIF data (camera, lens,
//! date, location…) and the colour profile.
//!
//! The copy's pixels are already upright and may have a new size, so the EXIF
//! orientation becomes "normal", the pixel dimensions are updated, and the embedded
//! thumbnail (which would show the unedited picture) is left out.

use std::io::{BufReader, Cursor};
use std::path::Path;

use exif::{Field, In, Tag, Value};
use image::ImageDecoder;

/// What a copy of a picture keeps from the original.
#[derive(Debug, Default, Clone)]
pub struct Metadata {
    /// EXIF as a TIFF structure, the way image encoders take it.
    pub exif: Option<Vec<u8>>,
    pub icc: Option<Vec<u8>>,
}

impl Metadata {
    /// The metadata of the picture at `source`, fixed up for an upright copy of `width`×`height`.
    pub fn for_copy(source: &Path, width: u32, height: u32) -> Self {
        Self { exif: exif_for_copy(source, width, height), icc: icc_profile(source).filter(|icc| is_rgb(icc)) }
    }
}

/// Tags that describe the original's layout rather than the picture, and must not be copied.
fn dropped(tag: Tag) -> bool {
    [
        Tag::Orientation,
        Tag::PixelXDimension,
        Tag::PixelYDimension,
        Tag::ImageWidth,
        Tag::ImageLength,
        Tag::StripOffsets,
        Tag::StripByteCounts,
        Tag::RowsPerStrip,
        Tag::TileOffsets,
        Tag::TileByteCounts,
        Tag::JPEGInterchangeFormat,
        Tag::JPEGInterchangeFormatLength,
        Tag::Compression,
        Tag::BitsPerSample,
        Tag::SamplesPerPixel,
        Tag::PhotometricInterpretation,
        Tag::PlanarConfiguration,
        // Offsets inside maker notes point into the original file; copied, they would point nowhere.
        Tag::MakerNote,
    ]
    .contains(&tag)
}

fn exif_for_copy(source: &Path, width: u32, height: u32) -> Option<Vec<u8>> {
    let file = std::fs::File::open(source).ok()?;
    let exif = exif::Reader::new().read_from_container(&mut BufReader::new(file)).ok()?;
    let kept: Vec<&Field> = exif.fields().filter(|f| f.ifd_num == In::PRIMARY && !dropped(f.tag)).collect();
    if kept.is_empty() {
        return None;
    }
    let fixed = [
        Field { tag: Tag::Orientation, ifd_num: In::PRIMARY, value: Value::Short(vec![1]) },
        Field { tag: Tag::PixelXDimension, ifd_num: In::PRIMARY, value: Value::Long(vec![width]) },
        Field { tag: Tag::PixelYDimension, ifd_num: In::PRIMARY, value: Value::Long(vec![height]) },
    ];
    let mut writer = exif::experimental::Writer::new();
    for field in kept.iter().copied().chain(fixed.iter()) {
        writer.push_field(field);
    }
    let mut out = Cursor::new(Vec::new());
    match writer.write(&mut out, exif.little_endian()) {
        Ok(()) => Some(out.into_inner()),
        Err(e) => {
            tracing::warn!("the EXIF data of {} could not be kept: {e}", source.display());
            None
        }
    }
}

/// Whether a colour profile is for RGB pixels, which is what a copy has (its colour space field).
fn is_rgb(icc: &[u8]) -> bool {
    icc.get(16..20) == Some(b"RGB ")
}

pub(crate) fn icc_profile(source: &Path) -> Option<Vec<u8>> {
    let reader = image::ImageReader::open(source).ok()?.with_guessed_format().ok()?;
    reader.into_decoder().ok()?.icc_profile().ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;

    /// A JPEG with EXIF: a camera make, a date, a GPS position, orientation 6 and a 999×999 size.
    fn photo(path: &Path) {
        let fields = [
            Field { tag: Tag::Make, ifd_num: In::PRIMARY, value: Value::Ascii(vec![b"Lumen Camera".to_vec()]) },
            Field { tag: Tag::Orientation, ifd_num: In::PRIMARY, value: Value::Short(vec![6]) },
            Field {
                tag: Tag::DateTimeOriginal,
                ifd_num: In::PRIMARY,
                value: Value::Ascii(vec![b"2024:05:06 07:08:09".to_vec()]),
            },
            Field { tag: Tag::PixelXDimension, ifd_num: In::PRIMARY, value: Value::Long(vec![999]) },
            Field { tag: Tag::GPSLatitudeRef, ifd_num: In::PRIMARY, value: Value::Ascii(vec![b"N".to_vec()]) },
        ];
        let mut writer = exif::experimental::Writer::new();
        fields.iter().for_each(|f| writer.push_field(f));
        let mut exif = Cursor::new(Vec::new());
        writer.write(&mut exif, false).unwrap();
        let mut file = std::fs::File::create(path).unwrap();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new(&mut file);
        encoder.set_exif_metadata(exif.into_inner()).unwrap();
        encoder.write_image(&[128; 8 * 4 * 3], 8, 4, image::ExtendedColorType::Rgb8).unwrap();
    }

    fn read(path: &Path) -> exif::Exif {
        exif::Reader::new().read_from_container(&mut BufReader::new(std::fs::File::open(path).unwrap())).unwrap()
    }

    #[test]
    fn only_a_profile_for_rgb_goes_into_an_rgb_copy() {
        let dir = std::env::temp_dir().join(format!("lumen-icc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (space, kept) in [(b"RGB ", true), (b"CMYK", false), (b"GRAY", false)] {
            let mut icc = vec![0u8; 128];
            icc[16..20].copy_from_slice(space);
            let path = dir.join("p.png");
            let mut encoder = image::codecs::png::PngEncoder::new(std::fs::File::create(&path).unwrap());
            encoder.set_icc_profile(icc).unwrap();
            encoder.write_image(&[9; 12], 2, 2, image::ExtendedColorType::Rgb8).unwrap();
            assert!(icc_profile(&path).is_some());
            assert_eq!(Metadata::for_copy(&path, 2, 2).icc.is_some(), kept, "{}", String::from_utf8_lossy(space));
        }
        assert!(!is_rgb(b"short"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copies_keep_the_exif_but_not_the_old_layout() {
        let dir = std::env::temp_dir().join(format!("lumen-exif-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("photo.jpg");
        photo(&source);
        let meta = Metadata::for_copy(&source, 40, 30);
        assert!(meta.exif.is_some());
        let mut layer = crate::annotate::AnnotationLayer::default();
        layer.add(crate::annotate::Shape::Rect(crate::annotate::Rect::new(1.0, 1.0, 5.0, 5.0)), Default::default());
        for name in ["copy.jpg", "copy.png", "copy.webp"] {
            let out = dir.join(name);
            crate::annotate::flatten::save(image::RgbaImage::new(40, 30), &layer, &out, &meta).unwrap();
            let exif = read(&out);
            let text = |tag| exif.get_field(tag, In::PRIMARY).map(|f| f.display_value().to_string());
            assert_eq!(text(Tag::Make).as_deref(), Some("\"Lumen Camera\""), "{name}");
            assert_eq!(text(Tag::DateTimeOriginal).as_deref(), Some("2024-05-06 07:08:09"), "{name}");
            assert!(text(Tag::GPSLatitudeRef).is_some(), "{name}");
            assert_eq!(exif.get_field(Tag::Orientation, In::PRIMARY).and_then(|f| f.value.get_uint(0)), Some(1));
            assert_eq!(exif.get_field(Tag::PixelXDimension, In::PRIMARY).and_then(|f| f.value.get_uint(0)), Some(40));
        }
        // A picture without EXIF has nothing to keep.
        let plain = dir.join("plain.png");
        image::RgbaImage::new(2, 2).save(&plain).unwrap();
        assert!(Metadata::for_copy(&plain, 2, 2).exif.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
