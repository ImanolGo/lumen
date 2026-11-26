//! The sandboxed decoder, tried through the real binary: every format decodes inside it, and
//! what the sandbox must stop is stopped.

#![cfg(target_os = "linux")]

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const WORKER: &str = env!("CARGO_BIN_EXE_lumen");

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Runs the worker like Lumen does: the picture on standard input.
fn worker(mode: &str, ext: &str, input: Option<&Path>) -> Output {
    let stdin = match input {
        Some(p) => Stdio::from(std::fs::File::open(p).unwrap()),
        None => Stdio::null(),
    };
    Command::new(WORKER)
        .args(["--decode-worker", mode, ext])
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap()
}

/// What follows the byte a worker sends once it is shut in.
fn after_ready(out: &Output) -> &[u8] {
    match out.stdout.split_first() {
        Some((b'S', rest)) => rest,
        _ => &[],
    }
}

/// width, height of a successful answer.
fn answer(out: &Output) -> Result<(u32, u32), String> {
    match after_ready(out).split_first() {
        Some((0, rest)) => {
            let n = |i: usize| u32::from_le_bytes(rest[i..i + 4].try_into().unwrap());
            let (w, h) = (n(0), n(4));
            assert_eq!(rest.len(), 16 + (w * h * 4) as usize, "the pixels match the size");
            Ok((w, h))
        }
        Some((1, message)) => Err(String::from_utf8_lossy(message).into_owned()),
        _ => Err(format!("no answer: {:?} {}", out.status, String::from_utf8_lossy(&out.stderr))),
    }
}

fn sandbox_available() -> bool {
    // A kernel without seccomp filters would answer with the "no sandbox" exit status.
    worker("selftest-open", "x", None).status.code() != Some(3)
}

#[test]
fn every_format_decodes_inside_the_sandbox() {
    assert!(sandbox_available(), "this machine cannot sandbox: the test cannot say anything");
    let mut files: Vec<PathBuf> =
        std::fs::read_dir(fixtures().join("pictures")).unwrap().map(|e| e.unwrap().path()).collect();
    files.push(fixtures().join("gradient.jxl"));
    files.push(fixtures().join("gradient.jp2"));
    assert!(files.len() >= 16);
    for file in files {
        let ext = file.extension().unwrap().to_str().unwrap();
        for mode in ["load", "thumb", "print"] {
            let out = worker(mode, ext, Some(&file));
            assert_eq!(
                out.status.code(),
                Some(0),
                "{} {mode}: {:?} {}",
                file.display(),
                out.status,
                String::from_utf8_lossy(&out.stderr)
            );
            let (w, h) = answer(&out).unwrap_or_else(|e| panic!("{} {mode}: {e}", file.display()));
            assert!(w > 0 && h > 0);
            if mode == "load" && ext == "png" && file.ends_with("g.png") {
                assert_eq!((w, h), (400, 300));
            }
        }
    }
}

#[test]
fn a_damaged_picture_is_an_error_message_not_a_crash() {
    let garbage = std::env::temp_dir().join(format!("lumen-sandbox-garbage-{}.png", std::process::id()));
    std::fs::write(&garbage, b"\x89PNG\r\n\x1a\nthis is not a png at all").unwrap();
    let out = worker("load", "png", Some(&garbage));
    assert_eq!(out.status.code(), Some(0));
    assert!(answer(&out).is_err());
    // Random bytes with a JPEG start, too.
    std::fs::write(&garbage, [&[0xFF, 0xD8, 0xFF, 0xE0][..], &[0x55; 300][..]].concat()).unwrap();
    let out = worker("thumb", "jpg", Some(&garbage));
    assert_eq!(out.status.code(), Some(0));
    assert!(answer(&out).is_err());
    std::fs::remove_file(&garbage).unwrap();
}

#[test]
fn the_sandbox_stops_file_access_sockets_and_programs() {
    assert!(sandbox_available(), "this machine cannot sandbox: the test cannot say anything");
    for test in ["open", "list", "socket", "exec", "write-file", "stat", "read-other", "dumpable"] {
        let out = worker(&format!("selftest-{test}"), "x", None);
        assert_eq!(
            out.status.signal(),
            Some(31),
            "selftest-{test} was not stopped by seccomp: {:?} {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert!(!Path::new("/tmp/lumen-sandbox-escape").exists());
}

#[test]
fn threads_and_the_memory_limit_work_inside() {
    for test in ["thread", "memory"] {
        let out = worker(&format!("selftest-{test}"), "x", None);
        assert_eq!(out.status.code(), Some(0), "{test}: {:?} {}", out.status, String::from_utf8_lossy(&out.stderr));
    }
}

#[test]
fn a_panic_is_not_taken_for_a_missing_sandbox() {
    let out = worker("selftest-panic", "x", None);
    assert_eq!(out.status.code(), Some(101), "{:?} {}", out.status, String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.stdout, b"S", "it had shut itself in");
}

#[test]
fn a_picture_that_claims_to_be_huge_is_refused() {
    let path = std::env::temp_dir().join(format!("lumen-sandbox-huge-{}.png", std::process::id()));
    let mut encoder = png::Encoder::new(std::fs::File::create(&path).unwrap(), 50_000, 50_000);
    encoder.set_color(png::ColorType::Rgba);
    let mut writer = encoder.write_header().unwrap();
    writer.write_chunk(png::chunk::IDAT, &[0x78, 0x9c]).unwrap();
    drop(writer);
    for mode in ["load", "print"] {
        let out = worker(mode, "png", Some(&path));
        assert!(answer(&out).unwrap_err().contains("too large"), "{mode}");
    }
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn animations_are_decoded_inside_too() {
    use std::io::Read;
    for (name, ext) in [("anim.png", "png")] {
        let mut child = Command::new(WORKER)
            .args(["--decode-worker", "frames", ext])
            .stdin(std::fs::File::open(fixtures().join("pictures").join(name)).unwrap())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut out = child.stdout.take().unwrap();
        let mut byte = [0u8; 1];
        out.read_exact(&mut byte).unwrap();
        assert_eq!(byte, [b'S']);
        // Frames, then the end of the pass, then frames again.
        let (mut frames, mut passes) = (0, 0);
        while passes < 2 {
            out.read_exact(&mut byte).unwrap();
            match byte[0] {
                0 => {
                    let mut head = [0u8; 12];
                    out.read_exact(&mut head).unwrap();
                    let n = |i: usize| u32::from_le_bytes(head[i..i + 4].try_into().unwrap()) as usize;
                    out.read_exact(&mut vec![0u8; n(0) * n(4) * 4]).unwrap();
                    frames += 1;
                }
                2 => {
                    out.read_exact(&mut [0u8; 4]).unwrap();
                    passes += 1;
                }
                other => panic!("{name}: unexpected {other}"),
            }
        }
        assert!(frames >= 4);
        // Nobody listens any more: it ends by itself.
        drop(out);
        assert_ne!(child.wait().unwrap().signal(), Some(31));
    }
}

/// A picture that says its colours are Display P3 is shown in sRGB, also inside the sandbox.
#[test]
fn colours_with_a_profile_are_turned_into_srgb_inside_too() {
    use image::ImageEncoder;
    assert!(sandbox_available(), "this machine cannot sandbox: the test cannot say anything");
    let path = std::env::temp_dir().join(format!("lumen-p3-{}.png", std::process::id()));
    let pixels: Vec<u8> = [100u8, 180, 100, 255].repeat(4);
    let write = |icc: Option<Vec<u8>>| {
        let mut file = std::fs::File::create(&path).unwrap();
        let mut png = image::codecs::png::PngEncoder::new(&mut file);
        if let Some(icc) = icc {
            png.set_icc_profile(icc).unwrap();
        }
        png.write_image(&pixels, 2, 2, image::ExtendedColorType::Rgba8).unwrap();
    };
    let first_pixel = || {
        let out = worker("load", "png", Some(&path));
        assert_eq!(answer(&out), Ok((2, 2)), "{}", String::from_utf8_lossy(&out.stderr));
        let rest = &after_ready(&out)[1..];
        rest[16..20].to_vec()
    };
    write(None);
    assert_eq!(first_pixel(), [100, 180, 100, 255], "without a profile, as it is");
    write(Some(moxcms::ColorProfile::new_display_p3().encode().unwrap()));
    let shown = first_pixel();
    assert_ne!(shown, [100, 180, 100, 255], "with one, turned into sRGB");
    assert!(shown[1] > 180, "a P3 green is a stronger green in sRGB terms: {shown:?}");
    std::fs::remove_file(&path).unwrap();
}
