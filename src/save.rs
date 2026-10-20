//! Writing edited images back to disk.
//!
//! JPEGs that were only rotated or mirrored are not re-encoded: the file is
//! copied and just its EXIF orientation tag changes, so the picture data stays
//! byte for byte the same. Everything else is rendered and encoded again, with
//! the original's EXIF carried over.

use std::{
    fs::{self, File},
    io::BufWriter,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use image::{ImageFormat, codecs::jpeg::JpegEncoder};
use little_exif::{exif_tag::ExifTag, metadata::Metadata};

use crate::{
    edit::{self, EditStack},
    image::Orientation,
};

pub const DEFAULT_JPEG_QUALITY: u8 = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Jpeg {
        quality: u8,
    },
    Png,
    /// Lossless WebP.
    WebP,
}

/// Whether the pixels of the picture at `path` can be read back for editing. Viewing goes
/// through GTK's loaders and accepts more (SVG, AVIF, HEIF…), but those cannot be edited.
pub fn can_edit(path: &Path) -> bool {
    image::ImageFormat::from_path(path).is_ok_and(|format| format.reading_enabled())
}

impl Format {
    /// The format for a file name, from its extension.
    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "jpg" | "jpeg" | "jpe" => Some(Self::Jpeg {
                quality: DEFAULT_JPEG_QUALITY,
            }),
            "png" => Some(Self::Png),
            "webp" => Some(Self::WebP),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg { .. } => "jpg",
            Self::Png => "png",
            Self::WebP => "webp",
        }
    }

    fn image_format(self) -> ImageFormat {
        match self {
            Self::Jpeg { .. } => ImageFormat::Jpeg,
            Self::Png => ImageFormat::Png,
            Self::WebP => ImageFormat::WebP,
        }
    }
}

pub struct SaveRequest {
    pub source: PathBuf,
    pub dest: PathBuf,
    /// EXIF orientation of the source file.
    pub base_orientation: Orientation,
    pub edits: EditStack,
    pub format: Format,
}

/// Whether the request can be done by changing only the orientation tag.
fn is_lossless(request: &SaveRequest) -> bool {
    matches!(request.format, Format::Jpeg { .. })
        && Format::from_path(&request.source).is_some_and(|f| matches!(f, Format::Jpeg { .. }))
        && !request.edits.changes_pixels()
}

/// Write the edited image. Blocking: call it off the main thread.
pub fn save(request: &SaveRequest) -> Result<()> {
    let temp = temp_path(&request.dest);
    let result = if is_lossless(request) {
        write_lossless(request, &temp)
    } else {
        write_reencoded(request, &temp)
    };
    match result {
        Ok(()) => crate::fsutil::replace_file(&temp, &request.dest)
            .with_context(|| format!("cannot replace {}", request.dest.display())),
        Err(err) => {
            let _ = fs::remove_file(&temp);
            Err(err)
        }
    }
}

/// A sibling file that keeps the real extension (the EXIF writer goes by it).
fn temp_path(dest: &Path) -> PathBuf {
    let stem = dest
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = match dest.extension() {
        Some(ext) => format!(".{stem}.lumen-tmp.{}", ext.to_string_lossy()),
        None => format!(".{stem}.lumen-tmp"),
    };
    dest.with_file_name(name)
}

/// Copy the file and change only the orientation tag.
fn write_lossless(request: &SaveRequest, temp: &Path) -> Result<()> {
    let orientation = request
        .edits
        .orientation_only(request.base_orientation)
        .context("not an orientation-only edit")?;
    fs::copy(&request.source, temp).context("cannot copy the original")?;
    set_orientation(temp, orientation)
}

fn set_orientation(path: &Path, orientation: Orientation) -> Result<()> {
    let mut metadata = Metadata::new_from_path(path).unwrap_or_default();
    metadata.set_tag(ExifTag::Orientation(vec![orientation.exif_value() as u16]));
    metadata
        .write_to_file(path)
        .context("cannot write the EXIF orientation")
}

fn write_reencoded(request: &SaveRequest, temp: &Path) -> Result<()> {
    let stored = image::open(&request.source)
        .with_context(|| format!("cannot read {}", request.source.display()))?;
    let mut rendered = edit::render(&stored, request.base_orientation, request.edits.active());
    // Annotations are part of the picture once saved.
    if let Some(layer) = request.edits.current_layer() {
        rendered = crate::annotate::render::flatten(&rendered, &layer)
            .context("cannot draw the annotations")?;
    }

    match request.format {
        Format::Jpeg { quality } => {
            let file = File::create(temp).context("cannot create the output file")?;
            let mut encoder = JpegEncoder::new_with_quality(BufWriter::new(file), quality);
            encoder
                .encode_image(&rendered.to_rgb8())
                .context("cannot encode the JPEG")?;
        }
        other => rendered
            .save_with_format(temp, other.image_format())
            .context("cannot encode the image")?,
    }

    // The pixels are upright now, so the carried-over EXIF must say so.
    let mut metadata = Metadata::new_from_path(&request.source).unwrap_or_default();
    metadata.set_tag(ExifTag::Orientation(vec![1]));
    metadata.set_tag(ExifTag::ExifImageWidth(vec![rendered.width()]));
    metadata.set_tag(ExifTag::ExifImageHeight(vec![rendered.height()]));
    if let Err(err) = metadata.write_to_file(temp) {
        // The picture itself is fine; losing the metadata is not worth failing the save.
        tracing::warn!("could not copy the EXIF data: {err}");
    }
    Ok(())
}

/// A name for "Save As" next to the original: `photo.jpg` -> `photo-edited.png`.
pub fn suggested_name(source: &Path, format: Format) -> String {
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("{stem}-edited.{}", format.extension())
}

/// Used by the EXIF-only test to describe an edit.
#[cfg(test)]
fn rotate_stack(ops: &[crate::edit::EditOp]) -> EditStack {
    let mut stack = EditStack::default();
    for op in ops {
        stack.push(*op);
    }
    stack
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    #[test]
    fn only_formats_the_image_crate_reads_are_editable() {
        for name in ["a.jpg", "a.PNG", "a.webp", "a.gif", "a.bmp", "a.tiff"] {
            assert!(can_edit(Path::new(name)), "{name}");
        }
        for name in ["a.svg", "a.avif", "a.heic", "a.pdf", "a"] {
            assert!(!can_edit(Path::new(name)), "{name}");
        }
    }

    use image::{DynamicImage, Rgb, RgbImage};

    use super::*;
    use crate::edit::{CropRect, EditOp};

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lumen-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A noisy 64×48 JPEG, so a re-encode could not possibly be byte-identical.
    fn make_jpeg(path: &Path) {
        let mut img = RgbImage::new(64, 48);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = Rgb([(x * 4) as u8, (y * 5) as u8, ((x * y) % 251) as u8]);
        }
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(img)
            .write_to(&mut bytes, ImageFormat::Jpeg)
            .unwrap();
        fs::write(path, bytes.into_inner()).unwrap();
    }

    /// The JPEG without its APP1 (EXIF/XMP) segments: what must never change on a lossless rotate.
    fn jpeg_body(bytes: &[u8]) -> Vec<u8> {
        let mut body = vec![0xFF, 0xD8];
        let mut i = 2;
        while i + 4 <= bytes.len() && bytes[i] == 0xFF {
            let marker = bytes[i + 1];
            let length = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
            if marker == 0xDA {
                // Start of scan: everything after this is entropy-coded picture data.
                body.extend_from_slice(&bytes[i..]);
                return body;
            }
            if marker != 0xE1 {
                body.extend_from_slice(&bytes[i..i + 2 + length]);
            }
            i += 2 + length;
        }
        body
    }

    fn request(source: &Path, dest: &Path, ops: &[EditOp], format: Format) -> SaveRequest {
        SaveRequest {
            source: source.to_owned(),
            dest: dest.to_owned(),
            base_orientation: Orientation::NORMAL,
            edits: rotate_stack(ops),
            format,
        }
    }

    fn orientation_of(path: &Path) -> Option<u8> {
        let file = File::open(path).ok()?;
        let exif = exif::Reader::new()
            .read_from_container(&mut std::io::BufReader::new(file))
            .ok()?;
        let value = exif
            .get_field(exif::Tag::Orientation, exif::In::PRIMARY)?
            .value
            .get_uint(0)?;
        Some(value as u8)
    }

    #[test]
    fn rotating_a_jpeg_changes_only_the_tag() {
        let dir = scratch_dir("lossless");
        let path = dir.join("photo.jpg");
        make_jpeg(&path);
        let before = fs::read(&path).unwrap();

        let req = request(
            &path,
            &path,
            &[EditOp::RotateCw],
            Format::Jpeg { quality: 90 },
        );
        assert!(is_lossless(&req));
        save(&req).unwrap();

        let after = fs::read(&path).unwrap();
        assert_ne!(before, after, "the tag was written");
        assert_eq!(
            jpeg_body(&before),
            jpeg_body(&after),
            "picture data is untouched"
        );
        assert_eq!(orientation_of(&path), Some(6));

        // Rotating back (as one more edit on top of the new file) restores orientation 1.
        let mut back = request(
            &path,
            &path,
            &[EditOp::RotateCcw],
            Format::Jpeg { quality: 90 },
        );
        back.base_orientation = Orientation::from_exif(6);
        save(&back).unwrap();
        assert_eq!(orientation_of(&path), Some(1));
        assert_eq!(jpeg_body(&before), jpeg_body(&fs::read(&path).unwrap()));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn save_as_copy_leaves_the_original_alone() {
        let dir = scratch_dir("copy");
        let (source, dest) = (dir.join("a.jpg"), dir.join("b.jpg"));
        make_jpeg(&source);
        let original = fs::read(&source).unwrap();

        save(&request(
            &source,
            &dest,
            &[EditOp::FlipH],
            Format::Jpeg { quality: 90 },
        ))
        .unwrap();
        assert_eq!(fs::read(&source).unwrap(), original);
        assert_eq!(orientation_of(&dest), Some(2));
        assert!(fs::read_dir(&dir).unwrap().all(|e| {
            !e.unwrap()
                .file_name()
                .to_string_lossy()
                .contains("lumen-tmp")
        }));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cropping_reencodes_and_resets_the_orientation() {
        let dir = scratch_dir("crop");
        let path = dir.join("photo.jpg");
        make_jpeg(&path);
        let crop = EditOp::Crop(CropRect {
            x: 8,
            y: 8,
            w: 32,
            h: 16,
        });

        let req = request(
            &path,
            &path,
            &[EditOp::RotateCw, crop],
            Format::Jpeg { quality: 85 },
        );
        assert!(!is_lossless(&req));
        save(&req).unwrap();

        let saved = image::open(&path).unwrap();
        // Rotated first (64×48 -> 48×64), then cropped to 32×16.
        assert_eq!((saved.width(), saved.height()), (32, 16));
        assert_ne!(orientation_of(&path), Some(6));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn converting_to_png_keeps_the_pixels_of_a_rotation() {
        let dir = scratch_dir("png");
        let (source, dest) = (dir.join("a.jpg"), dir.join("a.png"));
        make_jpeg(&source);

        save(&request(&source, &dest, &[EditOp::RotateCw], Format::Png)).unwrap();
        let expected = image::open(&source).unwrap().rotate90().to_rgb8();
        assert_eq!(image::open(&dest).unwrap().to_rgb8(), expected);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn saving_burns_annotations_into_the_pixels() {
        use crate::annotate::{AnnotationLayer, Color, Rect, Shape, Style};
        let dir = scratch_dir("annotated");
        let (source, dest) = (dir.join("a.jpg"), dir.join("a.png"));
        make_jpeg(&source);

        let mut layer = AnnotationLayer::default();
        let red = Color::rgba(1.0, 0.0, 0.0, 1.0);
        layer.add(
            Shape::Rect(Rect::new(10.0, 10.0, 20.0, 20.0)),
            Style {
                stroke: red,
                fill: Some(red),
                width: 0.0,
                font_size: 12.0,
            },
        );
        let mut edits = EditStack::default();
        edits.push_with_layer(EditOp::Annotate, Some(std::sync::Arc::new(layer)));

        // Even a JPEG-to-JPEG save has to re-encode now.
        let jpeg = SaveRequest {
            source: source.clone(),
            dest: dir.join("b.jpg"),
            base_orientation: Orientation::NORMAL,
            edits: edits.clone(),
            format: Format::Jpeg { quality: 95 },
        };
        assert!(!is_lossless(&jpeg));

        save(&SaveRequest {
            source,
            dest: dest.clone(),
            base_orientation: Orientation::NORMAL,
            edits,
            format: Format::Png,
        })
        .unwrap();
        let saved = image::open(&dest).unwrap().to_rgb8();
        assert_eq!(
            saved.get_pixel(20, 20).0,
            [255, 0, 0],
            "the rectangle is in the picture"
        );
        assert_ne!(saved.get_pixel(50, 40).0, [255, 0, 0]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn formats_follow_the_extension() {
        assert_eq!(Format::from_path(Path::new("x.PNG")), Some(Format::Png));
        assert!(matches!(
            Format::from_path(Path::new("x.jpeg")),
            Some(Format::Jpeg { .. })
        ));
        assert_eq!(Format::from_path(Path::new("x.webp")), Some(Format::WebP));
        assert_eq!(Format::from_path(Path::new("x.gif")), None);
        assert_eq!(
            suggested_name(Path::new("/a/photo.jpg"), Format::Png),
            "photo-edited.png"
        );
    }
}
