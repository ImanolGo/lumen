//! AVIF and HEIC through the system's libheif. This is the one place C code is reached,
//! and only when built with `--features heif`; the default build leaves it out.

use std::path::Path;

use anyhow::{Context, Result, bail};
use image::{DynamicImage, RgbaImage};
use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};

/// Decodes the primary picture of `path` to RGBA, with the file's own rotation and mirroring applied.
pub fn decode(path: &Path, max_bytes: u64) -> Result<DynamicImage> {
    let name = path.to_str().context("the file name is not valid text")?;
    let context = HeifContext::read_from_file(name).map_err(|e| anyhow::anyhow!("cannot read the picture ({e})"))?;
    let handle = context.primary_image_handle().map_err(|e| anyhow::anyhow!("the picture is damaged ({e})"))?;
    let (w, h) = (handle.width(), handle.height());
    if u64::from(w) * u64::from(h) * 4 > max_bytes {
        bail!("the picture is too large");
    }
    let image = LibHeif::new()
        .decode(&handle, ColorSpace::Rgb(RgbChroma::Rgba), None)
        .map_err(|e| anyhow::anyhow!("cannot decode the picture ({e})"))?;
    let plane = image.planes().interleaved.context("the decoder gave no pixels")?;
    let (w, h) = (plane.width as usize, plane.height as usize);
    let mut rgba = Vec::with_capacity(w * h * 4);
    for row in plane.data.chunks(plane.stride).take(h) {
        rgba.extend_from_slice(row.get(..w * 4).context("the decoder gave a short row")?);
    }
    Ok(DynamicImage::ImageRgba8(RgbaImage::from_raw(w as u32, h as u32, rgba).context("bad raster")?))
}
