//! Decoding pictures with glycin, which runs each format's decoder in its own sandboxed
//! process (bubblewrap, or `flatpak-spawn --sandbox` inside a Flatpak), so a malicious
//! file cannot do more than crash a throwaway process. Without glycin's loaders
//! installed, GTK's own (in-process) loaders are used instead.

use std::{path::Path, sync::OnceLock};

use anyhow::{Context, Result, anyhow};
use relm4::gtk::{gdk, gio, prelude::*};

use super::{Animation, Orientation};

/// An animation is only kept if all its frames fit in this much memory; otherwise
/// just its first frame is shown.
const MAX_ANIMATION_BYTES: usize = 512 * 1024 * 1024;
const MAX_ANIMATION_FRAMES: usize = 2000;

/// What a file decoded to.
pub struct Decoded {
    pub texture: gdk::Texture,
    /// From the file format itself (EXIF in HEIF and friends), if the decoder knows it.
    pub orientation: Option<Orientation>,
    pub animation: Option<Animation>,
}

/// Whether glycin has loaders to work with. Checked once; blocking.
pub fn sandboxed() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let available =
            !futures_lite::future::block_on(glycin::Loader::supported_mime_types()).is_empty();
        if !available {
            tracing::warn!(
                "no glycin loaders are installed; pictures are decoded without a sandbox"
            );
        }
        available
    })
}

/// Decode the picture at `path`. Blocking: call it off the main thread.
pub fn decode(path: &Path) -> Result<Decoded> {
    if !sandboxed() {
        let texture = gdk::Texture::from_filename(path)
            .with_context(|| format!("failed to decode {}", path.display()))?;
        return Ok(Decoded {
            texture,
            orientation: None,
            animation: None,
        });
    }
    futures_lite::future::block_on(decode_sandboxed(path))
}

async fn decode_sandboxed(path: &Path) -> Result<Decoded> {
    let mut loader = glycin::Loader::new(gio::File::for_path(path));
    // Lumen applies the orientation itself, so edits and lossless rotation can change it.
    loader.apply_transformations(false);
    let mut image = loader.load().await.map_err(short_error)?;
    let orientation = Orientation::from_exif(image.transformation_orientation() as u32);
    let first = image.next_frame().await.map_err(short_error)?;
    let texture = first.texture();

    let animation = match first.delay() {
        Some(delay) => {
            let start = first.details().n_frame();
            let mut frames = vec![(texture.clone(), delay)];
            let mut bytes = texture_bytes(&texture);
            loop {
                let frame = image.next_frame().await.map_err(short_error)?;
                // The decoder loops: back at the first frame means the animation is complete.
                let index = frame.details().n_frame();
                if index.is_none() || index == start {
                    break Some(Animation::new(frames));
                }
                let texture = frame.texture();
                bytes += texture_bytes(&texture);
                if bytes > MAX_ANIMATION_BYTES || frames.len() >= MAX_ANIMATION_FRAMES {
                    tracing::warn!(
                        path = %path.display(),
                        "animation too large to keep in memory; showing its first frame"
                    );
                    break None;
                }
                frames.push((texture, frame.delay().unwrap_or(delay)));
            }
        }
        None => None,
    };
    Ok(Decoded {
        texture,
        orientation: (orientation != Orientation::NORMAL).then_some(orientation),
        animation,
    })
}

/// Decode a picture to fit in `size` × `size` pixels, for a thumbnail. Blocking.
pub fn thumbnail(path: &Path, size: u32) -> Result<gdk::Texture> {
    if !sandboxed() {
        return Err(anyhow!("no sandboxed decoder"));
    }
    let texture = futures_lite::future::block_on(async {
        let loader = glycin::Loader::new(gio::File::for_path(path));
        let mut image = loader.load().await.map_err(short_error)?;
        let frame = image.next_frame().await.map_err(short_error)?;
        Ok::<_, anyhow::Error>(frame.texture())
    })?;
    // The loaders ignore requests to decode smaller, so shrink here and keep only the thumbnail.
    Ok(shrink(&texture, size))
}

/// `texture` scaled down to fit in `size` × `size` (never up).
fn shrink(texture: &gdk::Texture, size: u32) -> gdk::Texture {
    let (w, h) = (texture.width() as u32, texture.height() as u32);
    if w.max(h) <= size {
        return texture.clone();
    }
    let mut downloader = gdk::TextureDownloader::new(texture);
    downloader.set_format(gdk::MemoryFormat::R8g8b8a8);
    let (bytes, stride) = downloader.download_bytes();
    let mut pixels = Vec::with_capacity(w as usize * h as usize * 4);
    for row in bytes.chunks(stride).take(h as usize) {
        pixels.extend_from_slice(&row[..w as usize * 4]);
    }
    let Some(image) = image::RgbaImage::from_raw(w, h, pixels) else {
        return texture.clone();
    };
    let scale = size as f64 / w.max(h) as f64;
    let small = image::imageops::thumbnail(
        &image,
        ((w as f64 * scale).round() as u32).max(1),
        ((h as f64 * scale).round() as u32).max(1),
    );
    let (sw, sh) = small.dimensions();
    gdk::MemoryTexture::new(
        sw as i32,
        sh as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &relm4::gtk::glib::Bytes::from_owned(small.into_raw()),
        sw as usize * 4,
    )
    .upcast()
}

fn texture_bytes(texture: &gdk::Texture) -> usize {
    texture.width() as usize * texture.height() as usize * 4
}

/// glycin's errors can carry its whole configuration; keep the first line.
fn short_error(err: glycin::Error) -> anyhow::Error {
    let text = err.to_string();
    anyhow!(
        "{}",
        text.lines().next().unwrap_or("cannot decode the picture")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: i32, h: i32) -> gdk::Texture {
        let pixels: Vec<u8> = [200u8, 100, 50, 255].repeat((w * h) as usize);
        gdk::MemoryTexture::new(
            w,
            h,
            gdk::MemoryFormat::R8g8b8a8,
            &relm4::gtk::glib::Bytes::from_owned(pixels),
            w as usize * 4,
        )
        .upcast()
    }

    #[test]
    fn thumbnails_shrink_to_fit_and_keep_their_shape() {
        let small = shrink(&solid(400, 200), 144);
        assert_eq!((small.width(), small.height()), (144, 72));
        // Smaller than the box: left alone, never enlarged.
        let tiny = shrink(&solid(40, 20), 144);
        assert_eq!((tiny.width(), tiny.height()), (40, 20));
    }
}
