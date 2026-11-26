//! Writing pages and pictures out: PNG files at a chosen resolution, and PDFs.
//! Everything here is blocking; callers run it on a worker thread.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use image::RgbaImage;

use crate::pdf;
use crate::raster::Picture;

/// The resolutions offered for PNG export.
pub const DPIS: [u32; 3] = [72, 150, 300];

/// Pixels per PDF point at `dpi` (PDF points are 1/72 inch).
pub fn scale_for(dpi: u32) -> f32 {
    dpi as f32 / 72.0
}

/// Turns a rendered page clockwise by `degrees` (0, 90, 180 or 270).
pub fn turned(pic: &Picture, degrees: i32) -> Result<RgbaImage> {
    let img = RgbaImage::from_raw(pic.width, pic.height, pic.rgba.to_vec()).context("bad page bitmap")?;
    Ok(match degrees.rem_euclid(360) {
        90 => image::imageops::rotate90(&img),
        180 => image::imageops::rotate180(&img),
        270 => image::imageops::rotate270(&img),
        _ => img,
    })
}

/// Encodes `img` as a PNG that records its resolution (`dpi`), so other apps show it at the right size.
pub fn png_bytes(img: &RgbaImage, dpi: u32) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, img.width(), img.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let per_metre = (dpi as f64 / 0.0254).round() as u32;
    encoder.set_pixel_dims(Some(png::PixelDimensions { xppu: per_metre, yppu: per_metre, unit: png::Unit::Meter }));
    let mut writer = encoder.write_header().context("cannot write the PNG")?;
    writer.write_image_data(img.as_raw()).context("cannot write the PNG")?;
    writer.finish().context("cannot write the PNG")?;
    Ok(out)
}

/// One page of `doc` as a PNG at `dpi`, turned by `rotate` degrees, written to `target`. Returns the
/// resolution it really has: a very large page is drawn at a lower one.
pub fn page_to_png(doc: &pdf::Document, page: usize, rotate: i32, dpi: u32, target: &Path) -> Result<u32> {
    let scale = doc.real_scale(page, scale_for(dpi)).context("the page is too large")?;
    let real = ((scale * 72.0).round() as u32).min(dpi);
    let pic = futures_lite_block_on(doc.render(page, scale))?;
    let bytes = png_bytes(&turned(&pic, rotate)?, real)?;
    crate::fsio::write_atomic(target, &bytes)?;
    Ok(real)
}

/// File names for several pages of `stem` in a folder: `stem-3.png`, padded so they sort.
pub fn numbered_names(stem: &str, pages: &[usize], ext: &str) -> Vec<String> {
    let width = pages.iter().max().map_or(1, |m| (m + 1).to_string().len());
    pages.iter().map(|p| format!("{stem}-{:0width$}.{ext}", p + 1)).collect()
}

/// `dir/name`, or the first of `name (2)`, `name (3)`… that does not exist yet.
pub fn free_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s, format!(".{e}")),
        None => (name, String::new()),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .expect("an unbounded range always finds a free name")
}

/// Runs a future that only waits on a render thread, from a blocking worker.
fn futures_lite_block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => handle.block_on(future),
        Err(_) => tokio::runtime::Builder::new_current_thread().build().expect("a runtime").block_on(future),
    }
}

/// Makes sure a PDF export has something to export.
pub fn need_pages(count: usize) -> Result<()> {
    if count == 0 {
        bail!("no pages selected");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_follow_the_resolution() {
        assert_eq!(scale_for(72), 1.0);
        assert_eq!(scale_for(150), 150.0 / 72.0);
        assert!((scale_for(300) - 4.1667).abs() < 1e-3);
    }

    #[test]
    fn a_large_page_says_the_resolution_it_really_has() {
        let mut pdf = lopdf::Document::with_version("1.7");
        let pages = pdf.new_object_id();
        let media: Vec<lopdf::Object> = vec![0.into(), 0.into(), 4096.into(), 100.into()];
        let page = pdf.add_object(lopdf::Dictionary::from_iter([
            ("Type", "Page".into()),
            ("Parent", pages.into()),
            ("MediaBox", media.into()),
        ]));
        let kids: Vec<lopdf::Object> = vec![page.into()];
        let tree = [("Type", "Pages".into()), ("Kids", kids.into()), ("Count", 1.into())];
        pdf.objects.insert(pages, lopdf::Dictionary::from_iter(tree).into());
        let catalog = [("Type", "Catalog".into()), ("Pages", pages.into())];
        let root = pdf.add_object(lopdf::Dictionary::from_iter(catalog));
        pdf.trailer.set("Root", root);
        let dir = std::env::temp_dir().join(format!("lumen-dpi-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        pdf.save(dir.join("big.pdf")).unwrap();
        let doc = pdf::Document::open(&dir.join("big.pdf")).unwrap();
        let out = dir.join("out.png");
        assert_eq!(page_to_png(&doc, 0, 0, 72, &out).unwrap(), 72);
        assert_eq!(page_to_png(&doc, 0, 0, 300, &out).unwrap(), 144);
        let reader =
            png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&out).unwrap())).read_info().unwrap();
        assert_eq!((reader.info().width, reader.info().pixel_dims.unwrap().xppu), (8192, 5669));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn turning_swaps_the_sides() {
        let pic = Picture { width: 4, height: 2, original: (4, 2), rgba: vec![255; 4 * 2 * 4].into() };
        assert_eq!(turned(&pic, 0).unwrap().dimensions(), (4, 2));
        assert_eq!(turned(&pic, 90).unwrap().dimensions(), (2, 4));
        assert_eq!(turned(&pic, 180).unwrap().dimensions(), (4, 2));
        assert_eq!(turned(&pic, -90).unwrap().dimensions(), (2, 4));
    }

    #[test]
    fn png_records_its_resolution_and_round_trips() {
        let mut img = RgbaImage::from_pixel(3, 2, image::Rgba([10, 20, 30, 255]));
        img.put_pixel(1, 1, image::Rgba([200, 100, 50, 255]));
        let bytes = png_bytes(&img, 300).unwrap();
        let decoder = png::Decoder::new(std::io::Cursor::new(&bytes));
        let reader = decoder.read_info().unwrap();
        let dims = reader.info().pixel_dims.unwrap();
        assert_eq!(dims.unit, png::Unit::Meter);
        assert_eq!(dims.xppu, 11811);
        let back = image::load_from_memory(&bytes).unwrap().into_rgba8();
        assert_eq!(back, img);
    }

    #[test]
    fn names_are_numbered_and_never_clash() {
        assert_eq!(numbered_names("doc", &[0, 8, 11], "png"), ["doc-01.png", "doc-09.png", "doc-12.png"]);
        assert_eq!(numbered_names("doc", &[0, 1], "png"), ["doc-1.png", "doc-2.png"]);
        let dir = std::env::temp_dir().join(format!("lumen-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(free_path(&dir, "a.png"), dir.join("a.png"));
        std::fs::write(dir.join("a.png"), b"x").unwrap();
        assert_eq!(free_path(&dir, "a.png"), dir.join("a (2).png"));
        std::fs::write(dir.join("a (2).png"), b"x").unwrap();
        assert_eq!(free_path(&dir, "a.png"), dir.join("a (3).png"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn exports_a_page_as_png_at_two_resolutions() {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/form.pdf");
        let doc = pdf::Document::open(&source).unwrap();
        let (w, h) = doc.page_size(0);
        let dir = std::env::temp_dir().join(format!("lumen-export-png-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for dpi in [72u32, 150] {
            let target = dir.join(format!("p{dpi}.png"));
            let (doc, t) = (doc.clone(), target.clone());
            tokio::task::spawn_blocking(move || page_to_png(&doc, 0, 0, dpi, &t)).await.unwrap().unwrap();
            let (pw, ph) = image::image_dimensions(&target).unwrap();
            let k = scale_for(dpi);
            assert_eq!((pw, ph), ((w * k) as u32, (h * k) as u32), "at {dpi} dpi");
        }
        // Turned a quarter, the sides swap.
        let target = dir.join("turned.png");
        let (d, t) = (doc.clone(), target.clone());
        tokio::task::spawn_blocking(move || page_to_png(&d, 0, 90, 72, &t)).await.unwrap().unwrap();
        let (pw, ph) = image::image_dimensions(&target).unwrap();
        assert_eq!((pw, ph), (h as u32, w as u32));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_picture_becomes_a_pdf_as_big_as_itself() {
        let dir = std::env::temp_dir().join(format!("lumen-export-pic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.png");
        image::RgbaImage::from_pixel(300, 150, image::Rgba([1, 2, 3, 255])).save(&path).unwrap();
        let bytes = crate::print::picture_to_pdf_native(&path, 150.0).unwrap();
        let out = dir.join("p.pdf");
        std::fs::write(&out, &bytes).unwrap();
        let doc = pdf::Document::open(&out).unwrap();
        assert_eq!(doc.page_count(), 1);
        let (w, h) = doc.page_size(0);
        assert!((w - 144.0).abs() < 0.5 && (h - 72.0).abs() < 0.5, "{w}x{h}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
