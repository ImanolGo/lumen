//! Turning and mirroring a JPEG without touching its pixels: the picture is kept byte for
//! byte and only the EXIF orientation tag changes, which every viewer honours. Anything
//! else (crop, resize, colours, markup) needs the pixels, so it is saved the usual way.

use image::metadata::Orientation;
use image::{DynamicImage, Rgba, RgbaImage};

use crate::edit::{Edits, apply};

/// Whether `edits` only turn and mirror.
pub fn only_turns(edits: &Edits) -> bool {
    !edits.is_none() && edits.crop.is_none() && edits.resize.is_none() && edits.adjust.is_neutral()
}

/// The orientation (EXIF values 1 to 8) a picture stored with orientation `old` has after
/// `edits` (turns, then a mirror) are applied to what is displayed.
pub fn combine(old: u8, edits: &Edits) -> u8 {
    // Six different pixels tell all eight orientations apart.
    let raw = RgbaImage::from_fn(3, 2, |x, y| Rgba([(y * 3 + x) as u8 * 40, 0, 0, 255]));
    let shown = |o: u8| {
        let mut img = DynamicImage::ImageRgba8(raw.clone());
        img.apply_orientation(Orientation::from_exif(o).unwrap_or(Orientation::NoTransforms));
        img.into_rgba8()
    };
    let wanted = apply(&shown(old.clamp(1, 8)), edits);
    (1..=8).find(|&o| shown(o) == wanted).unwrap_or(1)
}

/// The EXIF orientation of a JPEG (1 when it has none).
pub fn orientation(jpeg: &[u8]) -> u8 {
    exif::Reader::new()
        .read_from_container(&mut std::io::Cursor::new(jpeg))
        .ok()
        .and_then(|e| e.get_field(exif::Tag::Orientation, exif::In::PRIMARY).and_then(|f| f.value.get_uint(0)))
        .filter(|v| (1..=8).contains(v))
        .map_or(1, |v| v as u8)
}

/// The same JPEG with orientation `new`. A file with no EXIF gets a minimal block; one with
/// EXIF but no orientation tag gets the tag. Anything that is not a well-formed JPEG is not handled
/// (`None`), and the caller saves the usual way.
pub fn with_orientation(jpeg: &[u8], new: u8) -> Option<Vec<u8>> {
    if !(1..=8).contains(&new) || jpeg.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut out = jpeg.to_vec();
    // Walk the segments before the picture data.
    let mut at = 2;
    let mut after_app0 = 2;
    while at + 4 <= jpeg.len() && jpeg[at] == 0xFF {
        let marker = jpeg[at + 1];
        if marker == 0xDA || marker == 0xD9 {
            break; // the scan: no more headers
        }
        let len = u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]) as usize;
        let end = at.checked_add(2 + len).filter(|&e| e <= jpeg.len() && len >= 2)?;
        let data = &jpeg[at + 4..end];
        if marker == 0xE1 && data.starts_with(b"Exif\0\0") {
            let tiff = &data[6..];
            let (le, ifd) = match tiff.get(..4)? {
                b"II*\0" => (true, u32::from_le_bytes(tiff.get(4..8)?.try_into().ok()?) as usize),
                b"MM\0*" => (false, u32::from_be_bytes(tiff.get(4..8)?.try_into().ok()?) as usize),
                _ => return None,
            };
            let u16_at = |o: usize| -> Option<u16> {
                let b: [u8; 2] = tiff.get(o..o + 2)?.try_into().ok()?;
                Some(if le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
            };
            let count = u16_at(ifd)? as usize;
            for i in 0..count {
                let entry = ifd + 2 + i * 12;
                if u16_at(entry)? == 0x0112 {
                    // A SHORT of count one lives in the first two bytes of the value field.
                    if u16_at(entry + 2)? != 3 {
                        return None;
                    }
                    let value = if le { u16::from(new).to_le_bytes() } else { u16::from(new).to_be_bytes() };
                    let place = at + 4 + 6 + entry + 8;
                    out.get_mut(place..place + 2)?.copy_from_slice(&value);
                    return Some(out);
                }
            }
            let grown = with_entry(tiff, le, ifd, count, new)?;
            let mut segment = vec![0xFF, 0xE1];
            segment.extend(u16::try_from(grown.len() + 8).ok()?.to_be_bytes());
            segment.extend_from_slice(b"Exif\0\0");
            segment.extend(grown);
            out.splice(at..end, segment);
            return Some(out);
        }
        if marker == 0xE0 && at == after_app0 {
            after_app0 = end; // keep JFIF first
        }
        at = end;
    }
    // No EXIF at all: a block with just the orientation, after SOI (and a JFIF header).
    let mut block = Vec::with_capacity(36);
    block.extend_from_slice(&[0xFF, 0xE1, 0x00, 0x22]);
    block.extend_from_slice(b"Exif\0\0MM\0*\0\0\0\x08\0\x01");
    block.extend_from_slice(&[0x01, 0x12, 0x00, 0x03, 0, 0, 0, 1, 0, new, 0, 0]);
    block.extend_from_slice(&[0, 0, 0, 0]);
    out.splice(after_app0..after_app0, block);
    Some(out)
}

/// A TIFF block whose first directory (of `count` entries at `ifd`) gets an orientation. The directory
/// is written again at the end, with the new entry; everything else stays where it is, so every
/// offset in the block still holds.
fn with_entry(tiff: &[u8], le: bool, ifd: usize, count: usize, new: u8) -> Option<Vec<u8>> {
    let two = |v: u16| if le { v.to_le_bytes() } else { v.to_be_bytes() };
    let four = |v: u32| if le { v.to_le_bytes() } else { v.to_be_bytes() };
    let entries = tiff.get(ifd + 2..ifd + 2 + count * 12)?;
    let next = tiff.get(ifd + 2 + count * 12..ifd + 6 + count * 12)?;
    let tag = |entry: &[u8; 12]| {
        let b = [entry[0], entry[1]];
        if le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) }
    };
    let mut entry = [0u8; 12];
    entry[..2].copy_from_slice(&two(0x0112));
    entry[2..4].copy_from_slice(&two(3));
    entry[4..8].copy_from_slice(&four(1));
    entry[8..10].copy_from_slice(&two(u16::from(new)));
    let mut all: Vec<[u8; 12]> = entries.as_chunks::<12>().0.to_vec();
    // Entries are in the order of their tags.
    let place = all.iter().position(|e| tag(e) > 0x0112).unwrap_or(all.len());
    all.insert(place, entry);
    let mut out = tiff.to_vec();
    if out.len() % 2 == 1 {
        out.push(0);
    }
    let start = u32::try_from(out.len()).ok()?;
    out.get_mut(4..8)?.copy_from_slice(&four(start));
    out.extend(two(u16::try_from(all.len()).ok()?));
    out.extend(all.concat());
    out.extend_from_slice(next);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::jpeg::JpegEncoder;
    use image::{ExtendedColorType, ImageEncoder};

    /// A 48x32 JPEG with a gradient, with the given EXIF (a TIFF block) if any.
    fn jpeg(exif_tiff: Option<Vec<u8>>) -> Vec<u8> {
        let img = image::RgbImage::from_fn(48, 32, |x, y| image::Rgb([(x * 5) as u8, (y * 7) as u8, 90]));
        let mut out = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut out, 90);
        if let Some(e) = exif_tiff {
            encoder.set_exif_metadata(e).unwrap();
        }
        encoder.write_image(&img, 48, 32, ExtendedColorType::Rgb8).unwrap();
        out
    }

    fn exif_with(fields: Vec<exif::Field>, little_endian: bool) -> Vec<u8> {
        let mut writer = exif::experimental::Writer::new();
        for f in &fields {
            writer.push_field(f);
        }
        let mut out = std::io::Cursor::new(Vec::new());
        writer.write(&mut out, little_endian).unwrap();
        out.into_inner()
    }

    fn orient_field(v: u16) -> exif::Field {
        exif::Field { tag: exif::Tag::Orientation, ifd_num: exif::In::PRIMARY, value: exif::Value::Short(vec![v]) }
    }

    /// The bytes from the scan on: the picture data itself.
    fn scan(jpeg: &[u8]) -> &[u8] {
        let at = jpeg.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
        &jpeg[at..]
    }

    fn edits(turns: u8, flip: bool) -> Edits {
        Edits { turns, flip, ..Edits::default() }
    }

    #[test]
    fn composes_orientations_like_the_pixels_do() {
        // Turning an upright picture a quarter clockwise is orientation 6; a mirror is 2.
        assert_eq!(combine(1, &edits(1, false)), 6);
        assert_eq!(combine(1, &edits(2, false)), 3);
        assert_eq!(combine(1, &edits(3, false)), 8);
        assert_eq!(combine(1, &edits(0, true)), 2);
        // Turning back undoes it.
        assert_eq!(combine(6, &edits(3, false)), 1);
        assert_eq!(combine(6, &edits(2, false)), 8);
        // Every stored orientation and every edit lands on a valid one, and four turns change nothing.
        for old in 1..=8u8 {
            assert_eq!(combine(old, &edits(4, false)), old);
            for turns in 0..4 {
                for flip in [false, true] {
                    assert!((1..=8).contains(&combine(old, &edits(turns, flip))));
                }
            }
        }
        assert!(only_turns(&edits(1, false)) && !only_turns(&Edits::default()));
    }

    #[test]
    fn rewrites_an_existing_orientation_in_place_for_both_byte_orders() {
        for little in [false, true] {
            let original = jpeg(Some(exif_with(vec![orient_field(1)], little)));
            assert_eq!(orientation(&original), 1);
            let turned = with_orientation(&original, 6).unwrap();
            assert_eq!(orientation(&turned), 6);
            assert_eq!(turned.len(), original.len());
            assert_eq!(scan(&turned), scan(&original), "the picture data is untouched");
        }
    }

    #[test]
    fn adds_a_block_when_there_is_no_exif() {
        let original = jpeg(None);
        assert_eq!(orientation(&original), 1);
        let turned = with_orientation(&original, 8).unwrap();
        assert_eq!(orientation(&turned), 8);
        assert_eq!(scan(&turned), scan(&original));
        assert_eq!(&turned[..2], [0xFF, 0xD8]);
        // And it can be turned again.
        let again = with_orientation(&turned, 3).unwrap();
        assert_eq!(orientation(&again), 3);
        assert_eq!(again.len(), turned.len());
    }

    #[test]
    fn adds_the_tag_to_exif_that_lacks_it() {
        for le in [true, false] {
            let text = |tag, t: &str| exif::Field {
                tag,
                ifd_num: exif::In::PRIMARY,
                value: exif::Value::Ascii(vec![t.as_bytes().to_vec()]),
            };
            let fields = vec![
                text(exif::Tag::Make, "Lumen Camera Company"),
                text(exif::Tag::DateTimeOriginal, "2024:05:06 07:08:09"),
                text(exif::Tag::Software, "something longer than four bytes"),
            ];
            let original = jpeg(Some(exif_with(fields, le)));
            let turned = with_orientation(&original, 6).unwrap();
            assert_eq!((orientation(&turned), scan(&turned)), (6, scan(&original)));
            let exif = exif::Reader::new().read_from_container(&mut std::io::Cursor::new(&turned)).unwrap();
            let shown = |tag| exif.get_field(tag, exif::In::PRIMARY).unwrap().display_value().to_string();
            assert_eq!(shown(exif::Tag::Make), "\"Lumen Camera Company\"");
            assert_eq!(shown(exif::Tag::Software), "\"something longer than four bytes\"");
            assert_eq!(shown(exif::Tag::DateTimeOriginal), "2024-05-06 07:08:09");
            // And now it has one to change.
            assert_eq!(with_orientation(&turned, 3).unwrap().len(), turned.len());
        }
    }

    #[test]
    fn leaves_what_it_cannot_do_alone() {
        assert!(with_orientation(b"not a jpeg", 6).is_none());
        assert!(with_orientation(&jpeg(None), 9).is_none());
    }

    #[test]
    fn the_turned_file_shows_what_turning_the_pixels_would() {
        let dir = std::env::temp_dir().join(format!("lumen-rs-lossless-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.jpg"), dir.join("b.jpg"));
        let original = jpeg(Some(exif_with(vec![orient_field(1)], false)));
        std::fs::write(&a, &original).unwrap();
        for e in [edits(1, false), edits(2, true), edits(3, true)] {
            let turned = with_orientation(&original, combine(1, &e)).unwrap();
            std::fs::write(&b, &turned).unwrap();
            let want = apply(
                &image::RgbaImage::from_raw(48, 32, Vec::from(crate::raster::load(&a).unwrap().rgba)).unwrap(),
                &e,
            );
            let got = crate::raster::load(&b).unwrap();
            assert_eq!((got.width, got.height), want.dimensions());
            assert_eq!(&got.rgba[..], want.as_raw().as_slice(), "{e:?}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
