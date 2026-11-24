//! Copying a picture to the clipboard. iced's clipboard only carries text, so pictures go
//! through the Wayland data-control protocol (the `wl-clipboard-rs` crate, pure Rust). Compositors
//! that do not offer that protocol (GNOME's) and X11 sessions cannot take a picture this way;
//! the error says so.

use std::io::Write;

/// PNG bytes of `rgba` (straight alpha, `width` × `height`), written fast: the clipboard is for
/// pasting, not for keeping.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(rgba.len() / 3);
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())?;
    Ok(out)
}

/// Offers `png` as the clipboard's contents. Returns once the compositor has taken the offer
/// (or refused it); a thread keeps serving it until something else is copied.
pub fn copy_png(png: Vec<u8>) -> Result<(), String> {
    use wl_clipboard_rs::copy::{MimeType, Options, Source};
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("clipboard".into())
        .spawn(move || {
            // Foreground: this thread does the serving (the crate would otherwise fork).
            let mut options = Options::new();
            options.foreground(true);
            let prepared = options.prepare_copy(Source::Bytes(png.into()), MimeType::Specific("image/png".into()));
            match prepared {
                Ok(prepared) => {
                    let _ = tx.send(Ok(()));
                    // Blocks until another program takes the clipboard over.
                    if let Err(e) = prepared.serve() {
                        tracing::debug!("clipboard: {e}");
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                }
            }
        })
        .map_err(|e| e.to_string())?;
    match rx.recv() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(explain(&e)),
        Err(_) => Err("the clipboard thread stopped".into()),
    }
}

fn explain(e: &wl_clipboard_rs::copy::Error) -> String {
    use wl_clipboard_rs::copy::Error;
    match e {
        Error::MissingProtocol { .. } => {
            "this desktop does not let programs copy pictures to the clipboard (it lacks the data-control protocol)"
                .into()
        }
        Error::WaylandConnection(_) | Error::SocketOpenError(_) => "no Wayland session to copy into".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_becomes_a_png_that_reads_back_the_same() {
        let rgba: Vec<u8> = (0..8 * 5).flat_map(|i| [i as u8, (i * 3) as u8, 200, 255 - i as u8]).collect();
        let png = encode_png(8, 5, &rgba).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let mut reader = png::Decoder::new(std::io::Cursor::new(&png)).read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let frame = reader.next_frame(&mut buf).unwrap();
        assert_eq!((frame.width, frame.height), (8, 5));
        assert_eq!(&buf[..frame.buffer_size()], &rgba[..]);
    }

    #[test]
    fn a_size_that_does_not_match_the_pixels_is_an_error_not_a_crash() {
        assert!(encode_png(10, 10, &[0; 12]).is_err());
    }
}
