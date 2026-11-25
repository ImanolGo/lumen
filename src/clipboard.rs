//! Pictures for the clipboard. iced's clipboard only carries text; `platform::clipboard` carries these.

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
