//! Building a PDF from a page plan: reordering, deleting, turning and inserting
//! pages. Poppler cannot restructure documents, so this uses `lopdf`.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use lopdf::{Document, Object, ObjectId};
use relm4::gtk::cairo;

use super::pageops::Entry;
use crate::image::Metadata;

/// A PDF to take pages from, and the password that opens it if it has one.
#[derive(Debug, Clone)]
pub struct Source {
    pub path: PathBuf,
    pub password: Option<String>,
}

/// Attributes a page may inherit from the page tree; they are copied onto each
/// page, because a page that moves to a new parent must not lose them.
const INHERITED: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

fn load(source: &Source) -> Result<Document> {
    let doc = match &source.password {
        Some(password) => Document::load_with_password(&source.path, password),
        None => Document::load(&source.path),
    }
    .with_context(|| format!("cannot read {}", source.path.display()))?;
    if doc.is_encrypted() {
        bail!("{} is password protected", source.path.display());
    }
    Ok(doc)
}

/// The value of an inheritable attribute, looked up the page tree.
fn inherited(doc: &Document, page: ObjectId, key: &[u8]) -> Option<Object> {
    let mut node = page;
    // The tree is shallow; the bound guards against a loop in a damaged file.
    for _ in 0..64 {
        let dict = doc.get_object(node).ok()?.as_dict().ok()?;
        if let Ok(value) = dict.get(key) {
            return Some(value.clone());
        }
        node = dict.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

fn rotation_of(doc: &Document, page: ObjectId) -> i64 {
    match inherited(doc, page, b"Rotate") {
        Some(Object::Integer(r)) => r,
        Some(Object::Reference(id)) => doc
            .get_object(id)
            .ok()
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0),
        _ => 0,
    }
}

/// Write a document made of `plan` to `out`. Entry source 0 is `base`; source n is
/// `extras[n - 1]`.
pub fn compose(base: &Source, extras: &[Source], plan: &[Entry], out: &Path) -> Result<()> {
    if plan.is_empty() {
        bail!("a document needs at least one page");
    }
    let mut doc = load(base)?;

    // Pull every other document's objects into this one, renumbered to avoid clashes.
    let mut pages: Vec<BTreeMap<u32, ObjectId>> = vec![doc.get_pages()];
    for extra in extras {
        let mut other = load(extra)?;
        other.renumber_objects_with(doc.max_id + 1);
        pages.push(other.get_pages());
        doc.max_id = doc.max_id.max(other.max_id);
        doc.objects.extend(other.objects);
    }

    // Make every page self-contained and work out each one's current turn.
    let mut turn: BTreeMap<ObjectId, i64> = BTreeMap::new();
    for id in pages
        .iter()
        .flat_map(|p| p.values().copied())
        .collect::<Vec<_>>()
    {
        let missing: Vec<(&[u8], Object)> = INHERITED
            .iter()
            .filter_map(|key| {
                let has = doc.get_object(id).ok()?.as_dict().ok()?.has(key);
                (!has)
                    .then(|| inherited(&doc, id, key).map(|value| (*key, value)))
                    .flatten()
            })
            .collect();
        turn.insert(id, rotation_of(&doc, id));
        let dict = doc.get_object_mut(id)?.as_dict_mut()?;
        for (key, value) in missing {
            dict.set(key, value);
        }
    }

    let root: ObjectId = doc
        .catalog()?
        .get(b"Pages")
        .and_then(Object::as_reference)
        .context("the document has no page tree")?;

    let mut kids = Vec::with_capacity(plan.len());
    for entry in plan {
        let id = *pages
            .get(entry.source)
            .and_then(|p| p.get(&(entry.index as u32 + 1)))
            .with_context(|| {
                format!(
                    "page {} of document {} is missing",
                    entry.index + 1,
                    entry.source
                )
            })?;
        let total = (turn[&id] + entry.rotate as i64).rem_euclid(360);
        let dict = doc.get_object_mut(id)?.as_dict_mut()?;
        dict.set("Parent", Object::Reference(root));
        if total != 0 || dict.has(b"Rotate") {
            dict.set("Rotate", Object::Integer(total));
        }
        kids.push(Object::Reference(id));
    }

    let tree = doc.get_object_mut(root)?.as_dict_mut()?;
    tree.set("Kids", Object::Array(kids));
    tree.set("Count", Object::Integer(plan.len() as i64));
    // Anything the tree used to inherit now lives on the pages.
    for key in INHERITED {
        tree.remove(key);
    }

    doc.trailer.remove(b"Encrypt");
    doc.prune_objects();
    doc.save(out)
        .with_context(|| format!("cannot write {}", out.display()))?;
    Ok(())
}

/// A one-page PDF showing the picture at `source`, upright, one pixel per 0.75 point
/// (the size it has on screen).
pub fn image_to_pdf(source: &Path, dest: &Path) -> Result<()> {
    let mut picture = crate::image::decode::pixels(source)?;
    let mut upright = true;
    if let Some(orientation) =
        image::metadata::Orientation::from_exif(Metadata::read(source).orientation.exif_value())
    {
        upright = orientation == image::metadata::Orientation::NoTransforms;
        picture.apply_orientation(orientation);
    }
    let rgba = picture.to_rgba8();
    let (width, height) = (rgba.width() as i32, rgba.height() as i32);
    let stride = width as usize * 4;

    // Cairo wants premultiplied BGRA.
    let mut data = vec![0u8; stride * height as usize];
    for (src, dst) in rgba.pixels().zip(data.as_chunks_mut::<4>().0.iter_mut()) {
        let [r, g, b, a] = src.0;
        let premultiply = |c: u8| ((c as u16 * a as u16 + 127) / 255) as u8;
        *dst = [premultiply(b), premultiply(g), premultiply(r), a];
    }
    let surface = cairo::ImageSurface::create_for_data(
        data,
        cairo::Format::ARgb32,
        width,
        height,
        stride as i32,
    )?;

    // An upright JPEG goes into the PDF as it is, instead of as pixels that are compressed again.
    if upright
        && let Ok(bytes) = std::fs::read(source)
        && bytes.starts_with(&[0xFF, 0xD8])
    {
        surface.set_mime_data(cairo::MIME_TYPE_JPEG, bytes)?;
    }

    const SCALE: f64 = 0.75;
    let pdf = cairo::PdfSurface::new(width as f64 * SCALE, height as f64 * SCALE, dest)?;
    {
        let cr = cairo::Context::new(&pdf)?;
        cr.scale(SCALE, SCALE);
        cr.set_source_surface(&surface, 0.0, 0.0)?;
        cr.paint()?;
        cr.show_page()?;
    }
    pdf.finish();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::{
        PdfSource, load_info, open_document,
        pageops::{PageOp, plan},
        poppler_lock,
    };

    fn fixture(name: &str) -> Source {
        Source {
            path: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(name),
            password: None,
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lumen-compose-{name}-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join("out.pdf")
    }

    #[test]
    fn a_jpeg_page_keeps_the_jpeg_instead_of_raw_pixels() {
        use image::{Rgb, RgbImage};
        let dest = scratch("jpeg");
        let jpeg = dest.with_file_name("photo.jpg");
        let mut img = RgbImage::new(640, 480);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = Rgb([(x % 251) as u8, (y % 241) as u8, ((x * y) % 239) as u8]);
        }
        img.save(&jpeg).unwrap();
        let jpeg_size = std::fs::metadata(&jpeg).unwrap().len();
        image_to_pdf(&jpeg, &dest).unwrap();
        let pdf_size = std::fs::metadata(&dest).unwrap().len();
        // Raw pixels would be about 1.2 MB, however well they deflate.
        assert!(pdf_size < jpeg_size * 2, "{pdf_size} vs {jpeg_size}");
        let _ = std::fs::remove_dir_all(dest.parent().unwrap());
    }

    /// The text of each page and its displayed size, as Poppler sees the file.
    fn describe(path: &Path) -> Vec<(String, (f64, f64))> {
        let _poppler = poppler_lock();
        let doc = open_document(&PdfSource::new(path, None)).expect("Poppler opens the result");
        (0..doc.n_pages())
            .map(|i| {
                let page = doc.page(i).unwrap();
                (
                    page.text().map(|t| t.trim().to_owned()).unwrap_or_default(),
                    page.size(),
                )
            })
            .collect()
    }

    fn texts(path: &Path) -> Vec<String> {
        describe(path).into_iter().map(|(t, _)| t).collect()
    }

    fn run(op: PageOp, n: usize, extras: &[Source], name: &str) -> PathBuf {
        let entries = plan(&op, n).unwrap();
        let out = scratch(name);
        compose(&fixture("pages.pdf"), extras, &entries, &out).expect("composes");
        out
    }

    #[test]
    fn reordering_changes_the_page_order() {
        let out = run(
            PageOp::Move {
                pages: vec![0, 1],
                before: 5,
            },
            5,
            &[],
            "move",
        );
        assert_eq!(texts(&out), ["P3", "P4", "P5", "P1", "P2"]);
        let _ = std::fs::remove_dir_all(out.parent().unwrap());
    }

    #[test]
    fn deleting_removes_pages() {
        let out = run(PageOp::Delete(vec![1, 3]), 5, &[], "delete");
        assert_eq!(texts(&out), ["P1", "P3", "P5"]);
        assert_eq!(load_info(&PdfSource::new(&out, None)).unwrap().n_pages, 3);
        let _ = std::fs::remove_dir_all(out.parent().unwrap());
    }

    #[test]
    fn rotating_turns_the_chosen_pages() {
        let out = run(
            PageOp::Rotate {
                pages: vec![2],
                degrees: 90,
            },
            5,
            &[],
            "rotate",
        );
        let pages = describe(&out);
        assert_eq!(
            pages[2].1,
            (200.0, 300.0),
            "a landscape page turned a quarter is portrait"
        );
        assert_eq!(pages[1].1, (300.0, 200.0));
        assert_eq!(pages[2].0, "P3", "the text is still there");

        // Turning it back (as a second operation on the result) restores the size.
        let back = scratch("rotate-back");
        let entries = plan(
            &PageOp::Rotate {
                pages: vec![2],
                degrees: -90,
            },
            5,
        )
        .unwrap();
        compose(
            &Source {
                path: out.clone(),
                password: None,
            },
            &[],
            &entries,
            &back,
        )
        .unwrap();
        assert_eq!(describe(&back)[2].1, (300.0, 200.0));
        for p in [out, back] {
            let _ = std::fs::remove_dir_all(p.parent().unwrap());
        }
    }

    #[test]
    fn inserting_brings_pages_from_another_file() {
        // Two pages of "selection.pdf" (400×300) go between pages 2 and 3.
        let out = run(
            PageOp::Insert {
                at: 2,
                source: 1,
                count: 2,
            },
            5,
            &[fixture("selection.pdf")],
            "insert",
        );
        let pages = describe(&out);
        assert_eq!(pages.len(), 7);
        assert_eq!(pages[0].0, "P1");
        assert!(
            pages[2].0.contains("Hello brave"),
            "inserted page: {:?}",
            pages[2].0
        );
        assert!(
            pages[3].0.contains("Page two"),
            "inserted page: {:?}",
            pages[3].0
        );
        assert_eq!(pages[2].1, (400.0, 300.0));
        assert_eq!(pages[4].0, "P3");
        let _ = std::fs::remove_dir_all(out.parent().unwrap());
    }

    #[test]
    fn operations_chain_on_their_own_output() {
        // Delete, then move, then rotate: each works on the previous file, as in the app.
        let first = run(PageOp::Delete(vec![0]), 5, &[], "chain1");
        let second = scratch("chain2");
        compose(
            &Source {
                path: first.clone(),
                password: None,
            },
            &[],
            &plan(
                &PageOp::Move {
                    pages: vec![3],
                    before: 0,
                },
                4,
            )
            .unwrap(),
            &second,
        )
        .unwrap();
        assert_eq!(texts(&second), ["P5", "P2", "P3", "P4"]);
        let third = scratch("chain3");
        compose(
            &Source {
                path: second.clone(),
                password: None,
            },
            &[],
            &plan(
                &PageOp::Rotate {
                    pages: vec![0],
                    degrees: 180,
                },
                4,
            )
            .unwrap(),
            &third,
        )
        .unwrap();
        assert_eq!(describe(&third)[0].1, (300.0, 200.0));
        assert_eq!(texts(&third), ["P5", "P2", "P3", "P4"]);
        for p in [first, second, third] {
            let _ = std::fs::remove_dir_all(p.parent().unwrap());
        }
    }

    #[test]
    fn an_empty_plan_is_refused() {
        assert!(compose(&fixture("pages.pdf"), &[], &[], &scratch("empty")).is_err());
    }

    #[test]
    fn a_picture_becomes_a_page() {
        let dir = scratch("image");
        let dir = dir.parent().unwrap().to_owned();
        let png = dir.join("pic.png");
        image::RgbaImage::from_pixel(80, 40, image::Rgba([200, 30, 30, 255]))
            .save(&png)
            .unwrap();
        let pdf = dir.join("pic.pdf");
        image_to_pdf(&png, &pdf).unwrap();
        let pages = describe(&pdf);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].1, (60.0, 30.0), "80 × 40 px at 0.75 pt each");
        let _ = std::fs::remove_dir_all(dir);
    }
}
