//! Animated GIF, APNG and WebP: a thread of its own decodes the frames one at a time and
//! hands them over a small channel, so a long animation never sits in memory whole.
//! The thread starts over at the end (the app stops it when it has the whole loop cached,
//! or when the picture is left and the channel is dropped).

use std::io::BufReader;
use std::path::Path;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::Duration;

use image::codecs::{gif::GifDecoder, png::PngDecoder, webp::WebPDecoder};
use image::{AnimationDecoder, Frames};

use crate::raster::Picture;

/// What the decoder thread sends.
pub enum Item {
    Frame(Picture, Duration),
    /// The last frame of one pass has been sent.
    LoopEnd,
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
    use std::io::Read;
    let mut head = Vec::new();
    let Ok(file) = std::fs::File::open(path) else { return false };
    if file.take(64 * 1024).read_to_end(&mut head).is_err() {
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

/// Starts decoding `path`'s frames. Pictures that are not animated send a single
/// `LoopEnd` (or one frame and then `LoopEnd`) and finish.
pub fn stream(path: &Path) -> Receiver<Item> {
    let (tx, rx) = sync_channel(AHEAD);
    let path = path.to_path_buf();
    let _ = std::thread::Builder::new().name("lumen-anim".into()).spawn(move || {
        loop {
            match pass(&path, &tx) {
                // Went through the frames: start again (the receiver ends the thread by dropping).
                Ok(frames) if frames > 1 => {}
                // A still picture, an error, or the receiver is gone: nothing to loop.
                _ => return,
            }
        }
    });
    rx
}

/// Sends one pass of frames; returns how many were sent, or an error when the receiver is gone.
fn pass(path: &Path, tx: &SyncSender<Item>) -> Result<usize, ()> {
    let Some(frames) = open(path) else {
        tx.send(Item::LoopEnd).map_err(|_| ())?;
        return Ok(0);
    };
    let mut count = 0;
    for frame in frames {
        let Ok(frame) = frame else { break };
        let delay = frame_delay(frame.delay());
        let buffer = frame.into_buffer();
        let (width, height) = buffer.dimensions();
        let picture = Picture { width, height, original: (width, height), rgba: buffer.into_raw().into() };
        tx.send(Item::Frame(picture, delay)).map_err(|_| ())?;
        count += 1;
    }
    tx.send(Item::LoopEnd).map_err(|_| ())?;
    Ok(count)
}

/// Browsers show frames that ask for no (or almost no) delay at a tenth of a second.
pub fn frame_delay(delay: image::Delay) -> Duration {
    let d = Duration::from(delay);
    if d <= Duration::from_millis(10) { Duration::from_millis(100) } else { d }
}

/// The frames of `path` if it is animated, else none.
fn open(path: &Path) -> Option<Frames<'static>> {
    let reader = BufReader::new(std::fs::File::open(path).ok()?);
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "gif" => Some(GifDecoder::new(reader).ok()?.into_frames()),
        "png" | "apng" => {
            let decoder = PngDecoder::new(reader).ok()?;
            if !decoder.is_apng().ok()? {
                return None;
            }
            Some(decoder.apng().ok()?.into_frames())
        }
        "webp" => {
            let decoder = WebPDecoder::new(reader).ok()?;
            if !decoder.has_animation() {
                return None;
            }
            Some(decoder.into_frames())
        }
        _ => None,
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
    fn tiny_delays_become_a_tenth_of_a_second() {
        assert_eq!(frame_delay(Delay::from_numer_denom_ms(0, 1)), Duration::from_millis(100));
        assert_eq!(frame_delay(Delay::from_numer_denom_ms(10, 1)), Duration::from_millis(100));
        assert_eq!(frame_delay(Delay::from_numer_denom_ms(40, 1)), Duration::from_millis(40));
        assert!(
            may_animate(Path::new("x.GIF")) && may_animate(Path::new("x.webp")) && !may_animate(Path::new("x.jpg"))
        );
    }
}
