//! Decoding pictures in a child process with almost no rights.
//!
//! Picture decoders parse files that may be hostile. Lumen runs them in `lumen-rs
//! --decode-worker`, a fresh process per picture that is given the open file as its standard
//! input, shuts itself in (Landlock: no file system access; seccomp: a short list of
//! allowed system calls, everything else kills it) and only then reads and decodes. The
//! pixels come back over its standard output. A decoder that crashes, is stopped by the
//! sandbox or hangs only costs one picture and an error message.
//!
//! If the sandbox cannot start (an old kernel, say) Lumen decodes in-process as before and
//! says so in the log; if a decoder is stopped *by* the sandbox it does not: that picture
//! simply fails to open.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

use crate::raster::{self, Picture};

/// What the worker is asked to do with the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Decode for display.
    Load,
    /// A small preview.
    Thumbnail,
    /// Full resolution, for printing.
    Print,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Load => "load",
            Mode::Thumbnail => "thumb",
            Mode::Print => "print",
        }
    }
}

/// The worker gives up on a picture after this long.
const TIMEOUT: Duration = Duration::from_secs(120);
/// Exit status of a worker that could not shut itself in.
const EXIT_NO_SANDBOX: i32 = 3;
/// The biggest file handed to a worker.
const MAX_INPUT: u64 = 2 << 30;

static ENABLED: AtomicBool = AtomicBool::new(true);
static WARNED: AtomicBool = AtomicBool::new(false);

/// Turns the sandbox on or off (a preference; `LUMEN_NO_SANDBOX=1` also turns it off).
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed) && !std::env::var_os("LUMEN_NO_SANDBOX").is_some_and(|v| !v.is_empty() && v != "0")
}

// ------------------------------------------------------------------ parent

/// Runs `mode` on the picture at `path`, in the sandbox when that is on and possible.
pub fn run(mode: Mode, path: &Path) -> Result<Picture> {
    if !enabled() || !raster::sandboxable(path) {
        return raster::run_in_process(mode, path);
    }
    match run_worker(mode, path) {
        Ok(picture) => Ok(picture),
        Err(Failure::Unavailable(why)) => {
            if !WARNED.swap(true, Ordering::Relaxed) {
                tracing::warn!("pictures are decoded without the sandbox: {why}");
            }
            raster::run_in_process(mode, path)
        }
        Err(Failure::Failed(e)) => Err(e),
    }
}

enum Failure {
    /// The sandbox could not be started; nothing was decoded.
    Unavailable(String),
    /// The worker ran and the picture could not be decoded.
    Failed(anyhow::Error),
}

fn run_worker(mode: Mode, path: &Path) -> Result<Picture, Failure> {
    use std::process::{Command, Stdio};
    let unavailable = |what: &str, e: &dyn std::fmt::Display| Failure::Unavailable(format!("{what}: {e}"));
    let file =
        std::fs::File::open(path).map_err(|e| Failure::Failed(anyhow!("cannot open {}: {e}", path.display())))?;
    let exe = std::env::current_exe().map_err(|e| unavailable("cannot find itself", &e))?;
    let mut child = Command::new(exe)
        .arg("--decode-worker")
        .arg(mode.name())
        .arg(raster::extension(path))
        .stdin(Stdio::from(file))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| unavailable("cannot start the decoder process", &e))?;
    let mut stdout = child.stdout.take().ok_or_else(|| Failure::Unavailable("no pipe".into()))?;

    // A watchdog ends a worker that hangs.
    let child = std::sync::Arc::new(std::sync::Mutex::new(child));
    let finished = std::sync::Arc::new(AtomicBool::new(false));
    let watchdog = {
        let (child, finished) = (child.clone(), finished.clone());
        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            while start.elapsed() < TIMEOUT {
                if finished.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            if let Ok(mut c) = child.lock() {
                let _ = c.kill();
            }
        })
    };

    let mut reply = Vec::new();
    let read = stdout.read_to_end(&mut reply);
    finished.store(true, Ordering::Relaxed);
    let status = child.lock().map_err(|_| Failure::Unavailable("poisoned".into()))?.wait();
    let _ = watchdog.join();
    let status = status.map_err(|e| unavailable("cannot wait for the decoder process", &e))?;

    use std::os::unix::process::ExitStatusExt;
    if let Some(signal) = status.signal() {
        // SIGSYS: a system call the sandbox does not allow. That is the sandbox doing its job
        // (or a bug); either way the picture is not decoded here.
        return Err(Failure::Failed(match signal {
            libc::SIGSYS => {
                anyhow!("the picture tried something the decoder's sandbox does not allow, so it was not opened")
            }
            libc::SIGKILL => anyhow!("the picture took too long to decode"),
            _ => anyhow!("the decoder crashed on this picture"),
        }));
    }
    if status.code() == Some(EXIT_NO_SANDBOX) {
        return Err(Failure::Unavailable("the kernel refused the sandbox".into()));
    }
    if read.is_err() || status.code() != Some(0) {
        // Not our worker at all (a test binary, say): no sandbox to speak of.
        return Err(Failure::Unavailable(format!("the decoder process gave no usable answer ({status})")));
    }
    parse_reply(&reply).map_err(Failure::Failed)
}

/// The worker's answer: a status byte, then either the picture or a message.
fn parse_reply(reply: &[u8]) -> Result<Picture> {
    match reply.split_first() {
        Some((0, rest)) => {
            let header = rest.get(..16).context("the decoder's answer was cut short")?;
            let n = |i: usize| u32::from_le_bytes([header[i], header[i + 1], header[i + 2], header[i + 3]]);
            let (width, height, original) = (n(0), n(4), (n(8), n(12)));
            let pixels = &rest[16..];
            if u64::from(width) * u64::from(height) * 4 != pixels.len() as u64 {
                bail!("the decoder's answer was damaged");
            }
            Ok(Picture { width, height, original, rgba: bytes::Bytes::copy_from_slice(pixels) })
        }
        Some((1, message)) => bail!("{}", String::from_utf8_lossy(message)),
        _ => bail!("the decoder gave no answer"),
    }
}

fn encode_reply(result: Result<Picture>) -> Vec<u8> {
    match result {
        Ok(p) => {
            let mut out = Vec::with_capacity(17 + p.rgba.len());
            out.push(0);
            for v in [p.width, p.height, p.original.0, p.original.1] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend_from_slice(&p.rgba);
            out
        }
        Err(e) => {
            let mut out = vec![1];
            out.extend_from_slice(format!("{e:#}").as_bytes());
            out
        }
    }
}

// ------------------------------------------------------------------ worker

/// `lumen-rs --decode-worker MODE EXT`, with the picture on standard input. Never returns.
pub fn worker_main(args: &[String]) -> ! {
    let (mode, ext) = (args.first().map(String::as_str).unwrap_or(""), args.get(1).cloned().unwrap_or_default());
    // Only for looking at what the worker does with `gdb`/`strace`.
    let unsandboxed = args.iter().any(|a| a == "--unsandboxed");

    if !unsandboxed && let Err(e) = enter() {
        let _ = writeln!(std::io::stderr(), "lumen-rs worker: {e}");
        std::process::exit(EXIT_NO_SANDBOX);
    }
    // The self-tests try things the sandbox must stop; they are only reached once it is on.
    if let Some(test) = mode.strip_prefix("selftest-") {
        self_test(test);
    }

    let mode = match mode {
        "load" => Mode::Load,
        "thumb" => Mode::Thumbnail,
        "print" => Mode::Print,
        _ => std::process::exit(2),
    };
    let mut data = Vec::new();
    let read = std::io::stdin().take(MAX_INPUT).read_to_end(&mut data);
    let result = read.context("cannot read the picture").and_then(|_| match mode {
        Mode::Load => raster::load_bytes(&data, &ext),
        Mode::Thumbnail => raster::thumbnail_bytes(&data, &ext),
        Mode::Print => raster::print_bytes(&data, &ext),
    });
    // A decoder that panics is an error like any other (the parent sees a crash otherwise).
    let reply = encode_reply(result);
    let mut out = std::io::stdout().lock();
    let written = out.write_all(&reply).and_then(|()| out.flush());
    std::process::exit(if written.is_ok() { 0 } else { 1 });
}

/// What the sandbox must stop; each one ends in the process being killed, so reaching
/// the last line means the sandbox failed.
fn self_test(test: &str) -> ! {
    match test {
        "open" => drop(std::fs::File::open("/etc/hostname")),
        "list" => drop(std::fs::read_dir("/")),
        "socket" => drop(std::net::TcpStream::connect("127.0.0.1:9")),
        "exec" => drop(std::process::Command::new("/bin/true").status()),
        "write-file" => drop(std::fs::write("/tmp/lumen-rs-sandbox-escape", b"x")),
        _ => std::process::exit(2),
    }
    // Still alive: report that, loudly.
    let _ = writeln!(std::io::stderr(), "NOT SANDBOXED");
    std::process::exit(42)
}

/// Shuts the process in. Landlock removes every file system right (best effort: older
/// kernels lack it); seccomp then allows only what decoding needs. Seccomp is required.
fn enter() -> Result<()> {
    use landlock::{ABI, Access, AccessFs, CompatLevel, Compatible, Ruleset, RulesetAttr};
    // No file system rights at all, whatever the kernel's ABI knows about.
    let landlock = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(ABI::V5))
        .and_then(|r| r.create())
        .and_then(|r| r.restrict_self());
    if let Err(e) = landlock {
        // Seccomp below still stops file access; Landlock is a second wall.
        let _ = writeln!(std::io::stderr(), "lumen-rs worker: no Landlock ({e})");
    }
    seccomp()
}

/// The system calls a decoder needs, found by watching real decodes (see `tests/sandbox.rs`).
fn allowed_syscalls() -> Vec<i64> {
    use libc::*;
    vec![
        // memory
        SYS_mmap,
        SYS_munmap,
        SYS_mprotect,
        SYS_mremap,
        SYS_madvise,
        SYS_brk,
        // reading the picture, writing the answer (restricted below to their descriptors)
        SYS_read,
        SYS_close,
        SYS_fstat,
        SYS_newfstatat,
        SYS_lseek,
        // threads and waiting
        SYS_futex,
        SYS_clone3,
        SYS_clone,
        SYS_set_robust_list,
        SYS_rseq,
        SYS_sched_yield,
        SYS_sched_getaffinity,
        SYS_nanosleep,
        SYS_clock_nanosleep,
        SYS_gettid,
        SYS_getpid,
        SYS_tgkill,
        SYS_set_tid_address,
        // signals (Rust's runtime, panics)
        SYS_rt_sigaction,
        SYS_rt_sigprocmask,
        SYS_rt_sigreturn,
        SYS_sigaltstack,
        // time, randomness, naming threads
        SYS_clock_gettime,
        SYS_getrandom,
        SYS_prctl,
        // leaving
        SYS_exit,
        SYS_exit_group,
    ]
}

fn seccomp() -> Result<()> {
    use seccompiler::{
        BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter, SeccompRule,
    };
    use std::collections::BTreeMap;
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = allowed_syscalls().into_iter().map(|n| (n, vec![])).collect();
    // Writing is only for the answer (and a last word on standard error).
    let fd = |n: u64| -> Result<SeccompRule> {
        Ok(SeccompRule::new(vec![SeccompCondition::new(0, SeccompCmpArgLen::Dword, SeccompCmpOp::Eq, n)?])?)
    };
    rules.insert(libc::SYS_write, vec![fd(1)?, fd(2)?]);
    // Threads only: no new processes.
    let filter: BpfProgram = SeccompFilter::new(
        rules,
        SeccompAction::KillProcess,
        SeccompAction::Allow,
        std::env::consts::ARCH.try_into().map_err(|_| anyhow!("no sandbox for this processor"))?,
    )?
    .try_into()?;
    seccompiler::apply_filter(&filter)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(w: u32, h: u32) -> Picture {
        Picture { width: w, height: h, original: (w * 2, h * 2), rgba: vec![7; (w * h * 4) as usize].into() }
    }

    #[test]
    fn the_answer_round_trips() {
        let back = parse_reply(&encode_reply(Ok(picture(3, 2)))).unwrap();
        assert_eq!((back.width, back.height, back.original), (3, 2, (6, 4)));
        assert_eq!(back.rgba.len(), 24);
        assert!(back.rgba.iter().all(|&b| b == 7));
    }

    #[test]
    fn errors_travel_as_text() {
        let e = parse_reply(&encode_reply(Err(anyhow!("the picture is damaged")))).unwrap_err();
        assert_eq!(e.to_string(), "the picture is damaged");
    }

    #[test]
    fn a_cut_short_or_wrong_answer_is_refused() {
        assert!(parse_reply(&[]).is_err());
        assert!(parse_reply(&[0, 1, 2]).is_err());
        let mut cut = encode_reply(Ok(picture(4, 4)));
        cut.truncate(cut.len() - 5);
        assert!(parse_reply(&cut).is_err());
        let mut long = encode_reply(Ok(picture(2, 2)));
        long.push(0);
        assert!(parse_reply(&long).is_err());
        assert!(parse_reply(&[9, 1]).is_err());
    }

    #[test]
    fn what_the_sandbox_cannot_take_is_decoded_in_process() {
        // This test binary is not the app, so a worker cannot be started from here: the
        // answer must still come, from the in-process path.
        let dir = std::env::temp_dir().join(format!("lumen-rs-sandbox-fallback-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.png");
        image::RgbaImage::from_pixel(5, 3, image::Rgba([1, 2, 3, 255])).save(&path).unwrap();
        let pic = run(Mode::Load, &path).unwrap();
        assert_eq!((pic.width, pic.height), (5, 3));
        set_enabled(false);
        let pic = run(Mode::Thumbnail, &path).unwrap();
        set_enabled(true);
        assert_eq!((pic.width, pic.height), (5, 3));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
