//! Animated GIF, APNG and WebP: a thread of its own decodes the frames one at a time and
//! hands them over a small channel, so a long animation never sits in memory whole.
//! The thread starts over at the end (the app stops it when it has the whole loop cached,
//! or when the picture is left and the channel is dropped).

use std::io::Cursor;
use std::path::Path;
use std::sync::mpsc::{Receiver, sync_channel};
use std::time::Duration;

use image::codecs::{gif::GifDecoder, png::PngDecoder, webp::WebPDecoder};
use image::{AnimationDecoder, Frames, ImageDecoder};

use crate::raster::Picture;

/// What the decoder thread sends.
pub enum Item {
    Frame(Picture, Duration),
    /// The last frame of one pass has been sent.
    LoopEnd,
    /// The frames are too large to play; nothing else follows.
    TooBig,
}

/// Frames at most this far ahead are decoded (more would only fill memory).
const AHEAD: usize = 3;

/// Whether a file might be animated, judged by its extension.
pub fn may_animate(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(),
        Some("gif" | "png" | "apng" | "webp")
    )
}

/// Whether the file's own header says it holds several frames: an APNG's `acTL` chunk, or the
/// animation flag of a WebP. A plain look at the first few kilobytes, decided at once (unlike
/// playing it, which only finds out when it reaches the end). A GIF is never judged here.
pub fn looks_animated(path: &Path) -> bool {
    header_says_animated(path, 64 * 1024)
}

fn header_says_animated(path: &Path, limit: u64) -> bool {
    use std::io::Read;
    let mut head = Vec::new();
    let Ok(file) = std::fs::File::open(path) else { return false };
    if file.take(limit).read_to_end(&mut head).is_err() {
        return false;
    }
    animated_header(&head)
}

fn animated_header(head: &[u8]) -> bool {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if head.starts_with(PNG) {
        let mut at = PNG.len();
        while let Some(chunk) = head.get(at..at + 8) {
            let size = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as usize;
            match &chunk[4..8] {
                b"acTL" => return true,
                // The animation control chunk comes before the picture data.
                b"IDAT" | b"IEND" => return false,
                _ => {}
            }
            at = at.saturating_add(12).saturating_add(size);
        }
        return false;
    }
    if head.len() >= 21 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" && &head[12..16] == b"VP8X" {
        return head[20] & 0x02 != 0;
    }
    false
}

/// The most decoded frames kept for replaying.
pub const CACHE_BYTES: usize = 96 << 20;

/// Whether frames of this size can be played: the one on screen, the next, and those waiting
/// must fit in the memory meant for animations.
pub fn fits(width: u32, height: u32) -> bool {
    u64::from(width) * u64::from(height) <= CACHE_BYTES as u64 / (4 * (AHEAD as u64 + 2))
}

/// Starts decoding `path`'s frames, in the sandbox when that is on. Pictures that are not
/// animated send a single `LoopEnd` (or one frame and then `LoopEnd`) and finish.
pub fn stream(path: &Path) -> Receiver<Item> {
    let (tx, rx) = sync_channel(AHEAD);
    let path = path.to_path_buf();
    let _ = std::thread::Builder::new().name("lumen-anim".into()).spawn(move || {
        // Still pictures are not worth a decoder: their header says so.
        let ext = crate::raster::extension(&path);
        if ext != "gif" && !header_says_animated(&path, 4 << 20) {
            let _ = tx.send(Item::LoopEnd);
            return;
        }
        let mut emit = |item| tx.send(item).map_err(drop);
        if crate::sandbox::enabled() && crate::sandbox::run_frames(&path, &mut emit) {
            return;
        }
        match std::fs::read(&path) {
            Ok(data) => play(&data, &ext, &mut emit),
            Err(_) => drop(emit(Item::LoopEnd)),
        }
    });
    rx
}

/// Decodes the frames of `data` (a file with extension `ext`) over and over until `emit`
/// refuses one. What is not animated is sent once.
pub fn play(data: &[u8], ext: &str, emit: &mut dyn FnMut(Item) -> Result<(), ()>) {
    // Anything else is a still picture, an error, or nobody listening: nothing to loop.
    while matches!(pass(data, ext, emit), Ok(frames) if frames > 1) {}
}

/// Sends one pass of frames; returns how many were sent, or an error when nobody wants them.
fn pass(data: &[u8], ext: &str, emit: &mut dyn FnMut(Item) -> Result<(), ()>) -> Result<usize, ()> {
    let mut count = 0;
    match open(data, ext) {
        Opened::Still => {}
        Opened::TooBig => {
            emit(Item::TooBig)?;
            return Ok(0);
        }
        Opened::Frames(frames) => {
            for frame in frames {
                let Ok(frame) = frame else { break };
                let delay = frame_delay(frame.delay());
                let buffer = frame.into_buffer();
                let (width, height) = buffer.dimensions();
                let picture = Picture { width, height, original: (width, height), rgba: buffer.into_raw().into() };
                emit(Item::Frame(picture, delay))?;
                count += 1;
            }
        }
    }
    emit(Item::LoopEnd)?;
    Ok(count)
}

/// Browsers show frames that ask for no (or almost no) delay at a tenth of a second.
pub fn frame_delay(delay: image::Delay) -> Duration {
    let d = Duration::from(delay);
    if d <= Duration::from_millis(10) { Duration::from_millis(100) } else { d }
}

enum Opened<'a> {
    Still,
    /// Its frames are too large to play.
    TooBig,
    Frames(Frames<'a>),
}

/// The frames of `data` if it is animated, decoded under the same limits as still pictures.
fn open<'a>(data: &'a [u8], ext: &str) -> Opened<'a> {
    fn frames<'a, D: ImageDecoder + AnimationDecoder<'a>>(mut decoder: D) -> Opened<'a> {
        let (width, height) = decoder.dimensions();
        if !fits(width, height) {
            return Opened::TooBig;
        }
        if decoder.set_limits(crate::raster::limits()).is_err() {
            return Opened::Still;
        }
        Opened::Frames(decoder.into_frames())
    }
    let reader = Cursor::new(data);
    match ext {
        "gif" => GifDecoder::new(reader).map_or(Opened::Still, frames),
        "png" | "apng" => {
            let Ok(decoder) = PngDecoder::with_limits(reader, crate::raster::limits()) else { return Opened::Still };
            if !decoder.is_apng().unwrap_or(false) {
                return Opened::Still;
            }
            let (width, height) = decoder.dimensions();
            if !fits(width, height) {
                return Opened::TooBig;
            }
            decoder.apng().map_or(Opened::Still, |d| Opened::Frames(d.into_frames()))
        }
        "webp" => match WebPDecoder::new(reader) {
            Ok(decoder) if decoder.has_animation() => frames(decoder),
            _ => Opened::Still,
        },
        _ => Opened::Still,
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn headers_tell_still_pictures_from_animated_ones() {
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pictures");
        assert!(looks_animated(&fixtures.join("anim.png")), "an APNG");
        assert!(!looks_animated(&fixtures.join("g.png")));
        assert!(!looks_animated(&fixtures.join("gi.png")), "an interlaced one");
        assert!(!looks_animated(&fixtures.join("g.webp")));
        assert!(!looks_animated(&fixtures.join("nothing-here.png")));
        // A WebP's animation flag, and a file too short to say.
        let mut webp = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0".to_vec();
        webp.extend_from_slice(&[0x02, 0, 0, 0]);
        assert!(animated_header(&webp));
        webp[20] = 0x10; // only an alpha channel
        assert!(!animated_header(&webp));
        assert!(!animated_header(b"RIFF"));
        assert!(!animated_header(b""));
    }
    use super::*;
    use image::{Delay, Frame, Rgba, RgbaImage, codecs::gif::GifEncoder};

    /// A GIF of `n` frames, each a flat colour, 40 ms apart.
    pub fn gif(path: &Path, n: u8) {
        let mut encoder = GifEncoder::new(std::fs::File::create(path).unwrap());
        for i in 0..n {
            let img = RgbaImage::from_pixel(6, 4, Rgba([i * 80, 0, 255 - i * 80, 255]));
            encoder.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(40, 1))).unwrap();
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lumen-rs-anim-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn streams_the_frames_of_a_gif_and_loops() {
        let dir = scratch("gif");
        let path = dir.join("a.gif");
        gif(&path, 3);
        let rx = stream(&path);
        let mut seen = Vec::new();
        // Two passes: frames, an end marker, frames again.
        while seen.iter().filter(|s| **s == "end").count() < 2 {
            match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
                Item::Frame(p, d) => {
                    assert_eq!((p.width, p.height), (6, 4));
                    assert_eq!(d, Duration::from_millis(40));
                    seen.push("frame");
                }
                Item::LoopEnd => seen.push("end"),
                Item::TooBig => panic!("not a large one"),
            }
        }
        assert_eq!(seen, ["frame", "frame", "frame", "end", "frame", "frame", "frame", "end"]);
        drop(rx);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_still_picture_is_not_looped() {
        let dir = scratch("still");
        let path = dir.join("s.gif");
        gif(&path, 1);
        let rx = stream(&path);
        let mut frames = 0;
        while let Item::Frame(..) = rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            frames += 1;
        }
        assert_eq!(frames, 1);
        // The thread has finished: the channel closes.
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_err());
        // A plain PNG is not animated at all.
        let png = dir.join("p.png");
        RgbaImage::from_pixel(3, 3, Rgba([1, 2, 3, 255])).save(&png).unwrap();
        let rx = stream(&png);
        assert!(matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Item::LoopEnd));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn frames_too_large_for_the_memory_meant_for_them_are_not_decoded() {
        assert!(fits(1920, 1080));
        assert!(!fits(16_000, 16_000) && !fits(u32::MAX, u32::MAX));
        // A GIF of one tiny frame on a canvas of 16 000 × 16 000.
        let mut gif = b"GIF89a".to_vec();
        gif.extend([0x80, 0x3E, 0x80, 0x3E, 0x80, 0, 0, 0, 0, 0, 255, 255, 255]);
        gif.extend([0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 0x44, 1, 0, 0x3B]);
        let mut items = Vec::new();
        play(&gif, "gif", &mut |item| {
            items.push(item);
            Ok(())
        });
        assert!(matches!(items[..], [Item::TooBig]));
    }

    #[test]
    fn tiny_delays_become_a_tenth_of_a_second() {
        assert_eq!(frame_delay(Delay::from_numer_denom_ms(0, 1)), Duration::from_millis(100));
        assert_eq!(frame_delay(Delay::from_numer_denom_ms(10, 1)), Duration::from_millis(100));
        assert_eq!(frame_delay(Delay::from_numer_denom_ms(40, 1)), Duration::from_millis(40));
        assert!(
            may_animate(Path::new("x.GIF")) && may_animate(Path::new("x.webp")) && !may_animate(Path::new("x.jpg"))
        );
    }
}
