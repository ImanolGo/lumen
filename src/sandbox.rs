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
use std::process::{Child, ChildStdout, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};

use crate::animation::{self, Item};
use crate::platform::confine;
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
/// What a worker writes first, once it is shut in and before it reads the picture.
const READY: u8 = b'S';
/// The worker mode that streams the frames of an animation.
const FRAMES: &str = "frames";
/// The address space of a worker: enough for the biggest picture the limits in `raster` let
/// through (a JPEG XL decoder works in floating point), and an end to what a hostile one can take.
const MAX_MEMORY: u64 = 8 << 30;
/// The biggest file handed to a worker.
const MAX_INPUT: u64 = 2 << 30;

static ENABLED: AtomicBool = AtomicBool::new(true);
static WARNED: AtomicBool = AtomicBool::new(false);

/// Turns the sandbox on or off (a preference; `LUMEN_NO_SANDBOX=1` also turns it off).
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    // The unit test binary cannot be its own worker.
    if cfg!(test) {
        return false;
    }
    ENABLED.load(Ordering::Relaxed) && !std::env::var_os("LUMEN_NO_SANDBOX").is_some_and(|v| !v.is_empty() && v != "0")
}

// ------------------------------------------------------------------ parent

/// Runs `mode` on the picture at `path`, in the sandbox when that is on and possible.
pub fn run(mode: Mode, path: &Path) -> Result<Picture> {
    if !enabled() || !raster::sandboxable(path) {
        return raster::run_in_process(mode, path);
    }
    finish(worker_command(mode.name(), path).and_then(|c| run_worker(c, path)), mode, path)
}

/// Decodes in the app only when no worker could be shut in at all; never after one has seen the picture.
fn finish(result: Result<Picture, Failure>, mode: Mode, path: &Path) -> Result<Picture> {
    match result {
        Ok(picture) => Ok(picture),
        Err(Failure::Unavailable(why)) => {
            warn_unsandboxed(&why);
            raster::run_in_process(mode, path)
        }
        Err(Failure::Failed(e)) => Err(e),
    }
}

fn warn_unsandboxed(why: &str) {
    if !WARNED.swap(true, Ordering::Relaxed) {
        tracing::warn!("pictures are decoded without the sandbox: {why}");
    }
}

#[derive(Debug)]
enum Failure {
    /// The sandbox could not be started; nothing was decoded.
    Unavailable(String),
    /// The worker ran and the picture could not be decoded.
    Failed(anyhow::Error),
}

fn failed(why: &'static str) -> Failure {
    Failure::Failed(anyhow!(why))
}

fn worker_command(mode: &str, path: &Path) -> Result<std::process::Command, Failure> {
    let exe = confine::worker_exe().map_err(|e| Failure::Unavailable(format!("cannot find the decoder: {e}")))?;
    let mut command = std::process::Command::new(exe);
    confine::prepare(&mut command);
    command.arg("--decode-worker").arg(mode).arg(raster::extension(path));
    Ok(command)
}

/// A running worker. A watchdog thread ends it when a deadline passes; dropping this ends it too.
struct Worker {
    child: Arc<Mutex<Child>>,
    watch: Arc<(Mutex<Watch>, Condvar)>,
}

#[derive(Default)]
struct Watch {
    deadline: Option<Instant>,
    /// The deadline passed and the worker was killed for it.
    timed_out: bool,
    /// Nothing left to watch.
    over: bool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Worker {
    fn start(mut command: std::process::Command, path: &Path) -> Result<(Self, ChildStdout), Failure> {
        let file =
            std::fs::File::open(path).map_err(|e| Failure::Failed(anyhow!("cannot open {}: {e}", path.display())))?;
        let mut child = command
            .stdin(Stdio::from(file))
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| Failure::Unavailable(format!("cannot start the decoder process: {e}")))?;
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Failure::Unavailable("no pipe".into()));
        };
        let worker = Self {
            child: Arc::new(Mutex::new(child)),
            watch: Arc::new((Mutex::new(Watch::default()), Condvar::new())),
        };
        worker.set_deadline(Some(TIMEOUT));
        let (child, watch) = (worker.child.clone(), worker.watch.clone());
        std::thread::spawn(move || {
            let (state, changed) = &*watch;
            let mut w = lock(state);
            while !w.over {
                let left = w.deadline.map(|d| d.saturating_duration_since(Instant::now()));
                match left {
                    None => w = changed.wait(w).unwrap_or_else(|e| e.into_inner()),
                    Some(Duration::ZERO) => {
                        w.timed_out = true;
                        let _ = lock(&child).kill();
                        return;
                    }
                    Some(left) => w = changed.wait_timeout(w, left).unwrap_or_else(|e| e.into_inner()).0,
                }
            }
        });
        Ok((worker, stdout))
    }

    /// The worker is killed unless the deadline is moved within `left`; `None`: it may take its time.
    fn set_deadline(&self, left: Option<Duration>) {
        let (state, changed) = &*self.watch;
        lock(state).deadline = left.map(|l| Instant::now() + l);
        changed.notify_all();
    }

    /// Why the worker stopped without finishing its answer. `ready`: it had shut itself in.
    fn why_it_stopped(&self, ready: bool) -> Failure {
        // It normally is already exiting. One that closed its output and stays is killed (on drop).
        let patience = Instant::now() + Duration::from_secs(2);
        let status = loop {
            match lock(&self.child).try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < patience => {}
                _ => return failed("the decoder gave no answer"),
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        if lock(&self.watch.0).timed_out {
            return failed("the picture took too long to decode");
        }
        // Stopped by the sandbox: either way the picture is not decoded here.
        if let Some(why) = confine::explain(&status) {
            return failed(why);
        }
        // Only a worker that never got as far as the picture can say that there is no sandbox.
        if status.code() == Some(EXIT_NO_SANDBOX) && !ready {
            return Failure::Unavailable("the kernel refused the sandbox".into());
        }
        failed("the decoder crashed on this picture")
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let (state, changed) = &*self.watch;
        lock(state).over = true;
        changed.notify_all();
        // A worker that still writes would wait for ever for a reader: end it. The kernel takes a
        // while to take its memory back, so it is collected in the background.
        let child = self.child.clone();
        let _ = lock(&child).kill();
        std::thread::spawn(move || lock(&child).wait());
    }
}

/// Reads the byte a worker sends once it is shut in, before it looks at the picture.
fn read_ready(from: &mut impl Read) -> bool {
    let mut byte = [0u8; 1];
    from.read_exact(&mut byte).is_ok() && byte[0] == READY
}

fn run_worker(command: std::process::Command, path: &Path) -> Result<Picture, Failure> {
    let (worker, mut stdout) = Worker::start(command, path)?;
    let ready = read_ready(&mut stdout);
    if !ready {
        return Err(worker.why_it_stopped(false));
    }
    match read_reply(&mut stdout) {
        Ok(picture) => Ok(picture),
        Err(ReplyError::Cut(_)) => Err(worker.why_it_stopped(true)),
        Err(ReplyError::Said(e)) => Err(Failure::Failed(e)),
    }
}

/// Decodes the frames of the animation at `path` in the sandbox, over and over, until `emit`
/// refuses one. False: there is no sandbox here and nothing was decoded.
pub fn run_frames(path: &Path, emit: &mut dyn FnMut(Item) -> Result<(), ()>) -> bool {
    match worker_command(FRAMES, path).and_then(|c| frames_worker(c, path, emit)) {
        Ok(()) => true,
        Err(Failure::Failed(e)) => {
            tracing::info!("the animation stopped: {e}");
            true
        }
        Err(Failure::Unavailable(why)) => {
            warn_unsandboxed(&why);
            false
        }
    }
}

fn frames_worker(
    command: std::process::Command,
    path: &Path,
    emit: &mut dyn FnMut(Item) -> Result<(), ()>,
) -> Result<(), Failure> {
    let (worker, mut stdout) = Worker::start(command, path)?;
    if !read_ready(&mut stdout) {
        return Err(worker.why_it_stopped(false));
    }
    loop {
        worker.set_deadline(Some(TIMEOUT));
        let item = match read_item(&mut stdout) {
            Ok(Some(item)) => item,
            Ok(None) => return Ok(()),
            Err(ReplyError::Cut(_)) => return Err(worker.why_it_stopped(true)),
            Err(ReplyError::Said(e)) => return Err(Failure::Failed(e)),
        };
        // The frames may wait here for as long as the animation is not on screen.
        worker.set_deadline(None);
        if emit(item).is_err() {
            return Ok(());
        }
    }
}

const ITEM_FRAME: u8 = 0;
const ITEM_LOOP_END: u8 = 2;
const ITEM_TOO_BIG: u8 = 3;
const ITEM_LAST: u8 = 4;

/// One message of a worker that decodes frames; `None` when it has nothing more.
fn read_item(from: &mut impl Read) -> Result<Option<Item>, ReplyError> {
    let cut = |e: std::io::Error| ReplyError::Cut(e.to_string());
    let mut kind = [0u8; 1];
    from.read_exact(&mut kind).map_err(cut)?;
    match kind[0] {
        ITEM_FRAME => {
            let mut head = [0u8; 12];
            from.read_exact(&mut head).map_err(cut)?;
            let n = |i: usize| u32::from_le_bytes([head[i], head[i + 1], head[i + 2], head[i + 3]]);
            let (width, height) = (n(0), n(4));
            if width == 0 || height == 0 || !animation::fits(width, height) {
                return Err(ReplyError::Said(anyhow!("the decoder's answer was damaged")));
            }
            let mut pixels = vec![0u8; width as usize * height as usize * 4];
            from.read_exact(&mut pixels).map_err(cut)?;
            let picture = Picture { width, height, original: (width, height), rgba: pixels.into() };
            Ok(Some(Item::Frame(picture, Duration::from_micros(u64::from(n(8))))))
        }
        ITEM_LOOP_END => {
            let mut plays = [0u8; 4];
            from.read_exact(&mut plays).map_err(cut)?;
            let plays = u32::from_le_bytes(plays);
            Ok(Some(Item::LoopEnd((plays > 0).then_some(plays))))
        }
        ITEM_TOO_BIG => Ok(Some(Item::TooBig)),
        ITEM_LAST => Ok(None),
        _ => Err(ReplyError::Said(anyhow!("the decoder's answer was damaged"))),
    }
}

fn write_item(out: &mut impl Write, item: &Item) -> std::io::Result<()> {
    match item {
        Item::Frame(p, delay) => {
            let mut head = [ITEM_FRAME; 13];
            let micros = u32::try_from(delay.as_micros()).unwrap_or(u32::MAX);
            for (i, v) in [p.width, p.height, micros].into_iter().enumerate() {
                head[1 + i * 4..5 + i * 4].copy_from_slice(&v.to_le_bytes());
            }
            out.write_all(&head)?;
            out.write_all(&p.rgba)?;
        }
        Item::LoopEnd(plays) => {
            out.write_all(&[ITEM_LOOP_END])?;
            out.write_all(&plays.unwrap_or(0).to_le_bytes())?;
        }
        Item::TooBig => out.write_all(&[ITEM_TOO_BIG])?,
    }
    out.flush()
}

#[derive(Debug)]
enum ReplyError {
    /// The answer stopped short or never came.
    Cut(#[expect(dead_code, reason = "for debugging")] String),
    /// The worker said the picture cannot be decoded.
    Said(anyhow::Error),
}

/// Reads the worker's answer without growing buffers or copying the pixels more than needed:
/// the status byte and the four sizes first, then the pixels into a buffer of exactly their size.
fn read_reply(from: &mut impl Read) -> Result<Picture, ReplyError> {
    let cut = |e: std::io::Error| ReplyError::Cut(e.to_string());
    let mut status = [0u8; 1];
    from.read_exact(&mut status).map_err(cut)?;
    match status[0] {
        0 => {
            let mut header = [0u8; 16];
            from.read_exact(&mut header).map_err(cut)?;
            let n = |i: usize| u32::from_le_bytes([header[i], header[i + 1], header[i + 2], header[i + 3]]);
            let (width, height, original) = (n(0), n(4), (n(8), n(12)));
            let size = (u64::from(width) * u64::from(height)).saturating_mul(4);
            // Nothing a decoder under this limit can make is bigger.
            if size == 0 || size > raster::MAX_BYTES {
                return Err(ReplyError::Said(anyhow!("the decoder's answer was damaged")));
            }
            let mut pixels = vec![0u8; size as usize];
            from.read_exact(&mut pixels).map_err(cut)?;
            // Nothing more may follow.
            if from.read(&mut [0u8; 1]).map_err(cut)? != 0 {
                return Err(ReplyError::Said(anyhow!("the decoder's answer was damaged")));
            }
            Ok(Picture { width, height, original, rgba: bytes::Bytes::from(pixels) })
        }
        1 => {
            let mut message = Vec::new();
            from.take(1 << 16).read_to_end(&mut message).map_err(cut)?;
            Err(ReplyError::Said(anyhow!("{}", String::from_utf8_lossy(&message))))
        }
        _ => Err(ReplyError::Said(anyhow!("the decoder gave no answer"))),
    }
}

/// Writes the worker's answer: a status byte, then the picture (sizes, then pixels, written
/// straight from where they are) or a message.
fn write_reply(out: &mut impl Write, result: Result<Picture>) -> std::io::Result<()> {
    match result {
        Ok(p) => {
            let mut head = [0u8; 17];
            for (i, v) in [p.width, p.height, p.original.0, p.original.1].into_iter().enumerate() {
                head[1 + i * 4..5 + i * 4].copy_from_slice(&v.to_le_bytes());
            }
            out.write_all(&head)?;
            out.write_all(&p.rgba)?;
        }
        Err(e) => {
            out.write_all(&[1])?;
            out.write_all(format!("{e:#}").as_bytes())?;
        }
    }
    out.flush()
}

// ------------------------------------------------------------------ worker

/// `lumen-rs --decode-worker MODE EXT`, with the picture on standard input. Never returns.
pub fn worker_main(args: &[String]) -> ! {
    let (mode, ext) = (args.first().map(String::as_str).unwrap_or(""), args.get(1).cloned().unwrap_or_default());
    // Only for looking at what the worker does with `gdb`/`strace`.
    let unsandboxed = cfg!(debug_assertions) && args.iter().any(|a| a == "--unsandboxed");

    if !unsandboxed && let Err(e) = confine::enter(MAX_MEMORY) {
        let _ = writeln!(std::io::stderr(), "lumen-rs worker: {e}");
        std::process::exit(EXIT_NO_SANDBOX);
    }
    let mut out = std::io::stdout().lock();
    if out.write_all(&[READY]).and_then(|()| out.flush()).is_err() {
        std::process::exit(1);
    }
    // The self-tests try things the sandbox must stop; they are only reached once it is on.
    if let Some(test) = mode.strip_prefix("selftest-") {
        self_test(test);
    }

    if mode == FRAMES {
        let mut data = Vec::new();
        if std::io::stdin().take(MAX_INPUT).read_to_end(&mut data).is_err() {
            std::process::exit(1);
        }
        animation::play(&data, &ext, &mut |item| write_item(&mut out, &item).map_err(drop));
        let _ = out.write_all(&[ITEM_LAST]).and_then(|()| out.flush());
        std::process::exit(0);
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
    let written = write_reply(&mut out, result);
    std::process::exit(if written.is_ok() { 0 } else { 1 });
}

/// What the sandbox must stop; each of those ends in the process being killed, so reaching
/// the last line means the sandbox failed.
fn self_test(test: &str) -> ! {
    match test {
        "open" => drop(std::fs::File::open("/etc/hostname")),
        "list" => drop(std::fs::read_dir("/")),
        "socket" => drop(std::net::TcpStream::connect("127.0.0.1:9")),
        // Not for the sandbox to stop: a decoder that panics, hangs, or uses threads.
        "panic" => panic!("selftest"),
        "thread" => {
            let sum = std::thread::Builder::new().name("selftest".into()).spawn(|| 2 + 2).map(|t| t.join());
            std::process::exit(if matches!(sum, Ok(Ok(4))) { 0 } else { 1 });
        }
        "memory" => {
            let big: Result<(), _> = Vec::<u8>::new().try_reserve_exact((MAX_MEMORY + 1) as usize);
            std::process::exit(if big.is_err() { 0 } else { 1 });
        }
        "write-file" => drop(std::fs::write("/tmp/lumen-rs-sandbox-escape", b"x")),
        other if confine::escape(other) => {}
        _ => std::process::exit(2),
    }
    // Still alive: report that, loudly.
    let _ = writeln!(std::io::stderr(), "NOT SANDBOXED");
    std::process::exit(42)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(w: u32, h: u32) -> Picture {
        Picture { width: w, height: h, original: (w * 2, h * 2), rgba: vec![7; (w * h * 4) as usize].into() }
    }

    fn wire(result: Result<Picture>) -> Vec<u8> {
        let mut out = Vec::new();
        write_reply(&mut out, result).unwrap();
        out
    }

    fn read(bytes: &[u8]) -> Result<Picture, ReplyError> {
        read_reply(&mut &bytes[..])
    }

    #[test]
    fn the_answer_round_trips() {
        let back = read(&wire(Ok(picture(3, 2)))).ok().unwrap();
        assert_eq!((back.width, back.height, back.original), (3, 2, (6, 4)));
        assert_eq!(back.rgba.len(), 24);
        assert!(back.rgba.iter().all(|&b| b == 7));
    }

    #[test]
    fn errors_travel_as_text() {
        match read(&wire(Err(anyhow!("the picture is damaged")))) {
            Err(ReplyError::Said(e)) => assert_eq!(e.to_string(), "the picture is damaged"),
            _ => panic!("expected the worker's message"),
        }
    }

    #[test]
    fn a_cut_short_or_wrong_answer_is_refused() {
        assert!(matches!(read(&[]), Err(ReplyError::Cut(_))));
        assert!(matches!(read(&[0, 1, 2]), Err(ReplyError::Cut(_))));
        let mut cut = wire(Ok(picture(4, 4)));
        cut.truncate(cut.len() - 5);
        assert!(matches!(read(&cut), Err(ReplyError::Cut(_))));
        // More bytes than the sizes promise, and sizes no decoder could make.
        let mut long = wire(Ok(picture(2, 2)));
        long.push(0);
        assert!(matches!(read(&long), Err(ReplyError::Said(_))));
        let mut huge = wire(Ok(picture(2, 2)));
        huge[1..5].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(read(&huge), Err(ReplyError::Said(_))));
        assert!(matches!(read(&[9, 1]), Err(ReplyError::Said(_))));
    }

    /// A picture that decodes fine, and what comes of it when the worker behaves like `script`.
    #[cfg(unix)]
    fn with_fake_worker(name: &str, script: &str) -> Result<Picture> {
        let dir = std::env::temp_dir().join(format!("lumen-rs-sandbox-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.png");
        image::RgbaImage::from_pixel(5, 3, image::Rgba([1, 2, 3, 255])).save(&path).unwrap();
        let mut command = std::process::Command::new("sh");
        command.arg("-c").arg(script);
        let result = finish(run_worker(command, &path), Mode::Load, &path);
        std::fs::remove_dir_all(&dir).unwrap();
        result
    }

    #[test]
    #[cfg(unix)]
    fn only_a_worker_that_could_not_be_shut_in_means_decoding_here() {
        assert_eq!(with_fake_worker("none", "exit 3").unwrap().width, 5);
        // A panic, a cut-off answer, a lie after the sandbox was on, a signal, nonsense: all errors.
        for (name, script) in [
            ("panic", "printf S; exit 101"),
            ("early-panic", "exit 101"),
            ("cut", "printf 'S\\000\\001'; exit 0"),
            ("silent", "exit 0"),
            ("liar", "printf S; exit 3"),
            ("sigsys", "printf S; kill -SYS $$"),
            ("nonsense", "printf 'Sx'"),
            ("not-ready", "printf 'x'"),
        ] {
            assert!(with_fake_worker(name, script).is_err(), "{name} was decoded in the app");
        }
    }

    #[test]
    #[cfg(unix)]
    fn a_worker_whose_answer_is_refused_does_not_hold_anyone_up() {
        // Sizes over the limit, and then more bytes than the pipe holds.
        let start = Instant::now();
        let e = with_fake_worker("oversized", "printf 'S\\000'; exec cat /dev/zero | tr '\\000' '\\377'").unwrap_err();
        assert!(e.to_string().contains("damaged"), "{e}");
        // One that closes its output and stays.
        assert!(with_fake_worker("stays", "printf S; exec >&-; sleep 60").is_err());
        assert!(start.elapsed() < Duration::from_secs(30));
    }

    #[test]
    fn frames_round_trip() {
        let mut wire = Vec::new();
        write_item(&mut wire, &Item::Frame(picture(3, 2), Duration::from_millis(40))).unwrap();
        write_item(&mut wire, &Item::LoopEnd(Some(3))).unwrap();
        write_item(&mut wire, &Item::TooBig).unwrap();
        wire.push(ITEM_LAST);
        let mut from = &wire[..];
        match read_item(&mut from).unwrap() {
            Some(Item::Frame(p, d)) => assert_eq!((p.width, p.height, d), (3, 2, Duration::from_millis(40))),
            _ => panic!("expected a frame"),
        }
        assert!(matches!(read_item(&mut from), Ok(Some(Item::LoopEnd(Some(3))))));
        assert!(matches!(read_item(&mut from), Ok(Some(Item::TooBig))));
        assert!(matches!(read_item(&mut from), Ok(None)));
        assert!(matches!(read_item(&mut from), Err(ReplyError::Cut(_))));
        // A frame bigger than any the worker may send.
        let mut huge = vec![ITEM_FRAME];
        huge.extend([0xFF; 12]);
        assert!(matches!(read_item(&mut &huge[..]), Err(ReplyError::Said(_))));
    }
}
