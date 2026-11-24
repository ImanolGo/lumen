//! The desktop's thumbnail cache (the freedesktop.org Thumbnail Managing Standard): a picture
//! another program already made a thumbnail of is shown at once, and the ones Lumen makes are
//! there for the file manager too. Thumbnails live in `~/.cache/thumbnails/large/` (256 px),
//! named after the MD5 of the file's `file://` address, as PNGs that record the address and the
//! file's modification time they were made from. A thumbnail that does not match the file (it
//! changed since) or that looks wrong is ignored, never trusted.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use crate::raster::{Picture, THUMB_SIDE};

/// The largest thumbnail read from the cache, so a hostile file in it costs little.
const MAX_SIDE: u32 = 512;
const MAX_BYTES: u64 = 4 << 20;

/// `~/.cache/thumbnails` (or under `$XDG_CACHE_HOME`).
pub fn base_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("thumbnails"))
}

/// A thumbnail found in the cache for `path`, if there is a good one.
pub fn read(base: &Path, path: &Path) -> Option<Picture> {
    let uri = file_uri(path)?;
    let modified = mtime(path)?;
    let file = base.join("large").join(format!("{}.png", md5_hex(uri.as_bytes())));
    if std::fs::metadata(&file).ok()?.len() > MAX_BYTES {
        return None;
    }
    let bytes = std::fs::read(&file).ok()?;
    decode(&bytes, &uri, modified)
}

/// Saves `picture` as the thumbnail of `path` (best effort: a cache that cannot be written is
/// just not there).
pub fn write(base: &Path, path: &Path, picture: &Picture) {
    let _ = try_write(base, path, picture);
}

fn try_write(base: &Path, path: &Path, picture: &Picture) -> Option<()> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let uri = file_uri(path)?;
    let modified = mtime(path)?;
    let dir = base.join("large");
    // The standard asks for these folders to be private.
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).ok()?;
    let name = format!("{}.png", md5_hex(uri.as_bytes()));
    let png = encode(picture, &uri, modified)?;
    // Written under another name first, so nobody ever reads half a file.
    let temp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let mut out = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temp).ok()?;
    if out.write_all(&png).is_err() || std::fs::rename(&temp, dir.join(name)).is_err() {
        let _ = std::fs::remove_file(&temp);
        return None;
    }
    Some(())
}

fn mtime(path: &Path) -> Option<u64> {
    let m = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(m.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs())
}

fn encode(picture: &Picture, uri: &str, modified: u64) -> Option<Vec<u8>> {
    // Whatever the picture's size, the thumbnail is at most 256 px (the caller already did that).
    if picture.width == 0 || picture.height == 0 || picture.width.max(picture.height) > THUMB_SIDE {
        return None;
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, picture.width, picture.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    for (key, value) in [
        ("Thumb::URI", uri.to_string()),
        ("Thumb::MTime", modified.to_string()),
        ("Thumb::Image::Width", picture.original.0.to_string()),
        ("Thumb::Image::Height", picture.original.1.to_string()),
        ("Software", "Lumen".to_string()),
    ] {
        encoder.add_text_chunk(key.to_string(), value).ok()?;
    }
    let mut writer = encoder.write_header().ok()?;
    writer.write_image_data(&picture.rgba).ok()?;
    writer.finish().ok()?;
    Some(out)
}

fn decode(bytes: &[u8], uri: &str, modified: u64) -> Option<Picture> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let info = reader.info();
    if info.width == 0 || info.height == 0 || info.width > MAX_SIDE || info.height > MAX_SIDE {
        return None;
    }
    // It must be the thumbnail of this very file, as it is now.
    let text = |key: &str| info.uncompressed_latin1_text.iter().find(|t| t.keyword == key).map(|t| t.text.clone());
    if text("Thumb::URI")? != uri || text("Thumb::MTime")?.parse::<u64>().ok()? != modified {
        return None;
    }
    let original = (
        text("Thumb::Image::Width").and_then(|v| v.parse().ok()),
        text("Thumb::Image::Height").and_then(|v| v.parse().ok()),
    );
    let mut buf = vec![0; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut buf).ok()?;
    let data = &buf[..frame.buffer_size()];
    let rgba: Vec<u8> = match frame.color_type {
        png::ColorType::Rgba => data.to_vec(),
        png::ColorType::Rgb => data.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => {
            data.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect()
        }
        png::ColorType::Grayscale => data.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    if rgba.len() != frame.width as usize * frame.height as usize * 4 {
        return None;
    }
    let original = match original {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w, h),
        _ => (frame.width, frame.height),
    };
    Some(Picture { width: frame.width, height: frame.height, original, rgba: rgba.into() })
}

/// `file:///home/me/a%20b.png`, written the way GLib writes it (so other programs, which
/// name the thumbnail after the MD5 of this text, find the same file as we do).
pub fn file_uri(path: &Path) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;
    if !path.is_absolute() {
        return None;
    }
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => uri.push(b as char),
            b'/' | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'=' | b':' | b'@' => {
                uri.push(b as char)
            }
            _ => uri.push_str(&format!("%{b:02X}")),
        }
    }
    Some(uri)
}

/// MD5 (RFC 1321) as lower-case hex. It only names files here; nothing depends on it being hard to forge.
pub fn md5_hex(data: &[u8]) -> String {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14,
        20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6,
        10, 15, 21,
    ];
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4_294_967_296.0) as u32).collect();
    let (mut a0, mut b0, mut c0, mut d0) = (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&((data.len() as u64).wrapping_mul(8)).to_le_bytes());
    for chunk in message.as_chunks::<64>().0 {
        let m: Vec<u32> = chunk.as_chunks::<4>().0.iter().map(|w| u32::from_le_bytes(*w)).collect();
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    [a0, b0, c0, d0].iter().flat_map(|v| v.to_le_bytes()).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_matches_the_rfc_test_suite() {
        for (text, hash) in [
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a", "0cc175b9c0f1b6a831c399e269772661"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            ("abcdefghijklmnopqrstuvwxyz", "c3fcd3d76192e4007dfb496cca67e13b"),
            (
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ] {
            assert_eq!(md5_hex(text.as_bytes()), hash, "{text:?}");
        }
    }

    #[test]
    fn a_thumbnail_is_named_after_the_md5_of_its_address() {
        // Checked with `md5sum`, and against the 616 thumbnails other programs had made.
        assert_eq!(md5_hex(b"file:///home/jens/photo.png"), "3a295334ea4f1f388246178ae903e902");
    }

    #[test]
    fn addresses_are_written_like_glib_writes_them() {
        assert_eq!(
            file_uri(Path::new("/home/me/My Photos/ñ #1.png")).unwrap(),
            "file:///home/me/My%20Photos/%C3%B1%20%231.png"
        );
        assert_eq!(file_uri(Path::new("/a/b+c(d)@e:f.png")).unwrap(), "file:///a/b+c(d)@e:f.png");
        assert!(file_uri(Path::new("relative.png")).is_none());
    }

    fn picture(w: u32, h: u32) -> Picture {
        let rgba: Vec<u8> =
            (0..w * h).flat_map(|i| [(i % 251) as u8, (i / 7 % 251) as u8, 99, 255 - (i % 3) as u8]).collect();
        Picture { width: w, height: h, original: (4000, 3000), rgba: rgba.into() }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lumen-thumbcache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_thumbnail_written_can_be_read_back_until_the_file_changes() {
        let dir = temp("roundtrip");
        let file = dir.join("photo.png");
        std::fs::write(&file, b"not really a picture").unwrap();
        let base = dir.join("cache");
        assert!(read(&base, &file).is_none());
        let pic = picture(256, 171);
        write(&base, &file, &pic);
        let back = read(&base, &file).expect("a thumbnail was cached");
        assert_eq!((back.width, back.height, back.original), (256, 171, (4000, 3000)));
        assert_eq!(&back.rgba[..], &pic.rgba[..], "pixels survive");
        // The file is private to its owner.
        let mode =
            std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(base.join("large")).unwrap().permissions());
        assert_eq!(mode & 0o077, 0, "the folder is private");
        // A file changed since (its modification time moved) no longer matches.
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(later).unwrap();
        assert!(read(&base, &file).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn nonsense_in_the_cache_is_ignored() {
        let dir = temp("nonsense");
        let file = dir.join("photo.png");
        std::fs::write(&file, b"x").unwrap();
        let base = dir.join("cache");
        let name = format!("{}.png", md5_hex(file_uri(&file).unwrap().as_bytes()));
        std::fs::create_dir_all(base.join("large")).unwrap();
        std::fs::write(base.join("large").join(&name), b"\x89PNG garbage").unwrap();
        assert!(read(&base, &file).is_none());
        // A real thumbnail of some other file under this name is not ours either.
        let other = dir.join("other.png");
        std::fs::write(&other, b"y").unwrap();
        write(&base, &other, &picture(10, 10));
        let theirs = base.join("large").join(format!("{}.png", md5_hex(file_uri(&other).unwrap().as_bytes())));
        std::fs::copy(theirs, base.join("large").join(&name)).unwrap();
        assert!(read(&base, &file).is_none(), "the address inside does not match");
        // Too big to be a thumbnail: not written, and not read.
        assert!(encode(&picture(300, 10), "u", 0).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
