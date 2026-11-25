//! Printing straight to CUPS, for desktops without a print portal.
//!
//! A small IPP client in plain Rust (no libcups): it speaks HTTP/1.1 over the local
//! CUPS socket (or `localhost:631`), lists the printers with their paper sizes and
//! two-sided support, and submits PDF jobs. Blocking; callers run it off the UI thread.

use std::io::{BufRead, BufReader, Read, Write};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::platform::print::{SOCKETS, connect as connect_socket};
use crate::print::Connection;

pub const TIMEOUT: Duration = Duration::from_secs(20);
/// Answers bigger than this are not believed (a printer list is a few kilobytes).
const MAX_RESPONSE: usize = 16 << 20;

// Operations.
const PRINT_JOB: u16 = 0x0002;
#[cfg(test)]
const VALIDATE_JOB: u16 = 0x0004;
const CUPS_GET_DEFAULT: u16 = 0x4001;
const CUPS_GET_PRINTERS: u16 = 0x4002;

// Delimiter and value tags.
const OPERATION: u8 = 0x01;
const JOB: u8 = 0x02;
const END: u8 = 0x03;
const PRINTER: u8 = 0x04;
const INTEGER: u8 = 0x21;
#[cfg(test)]
const BOOLEAN: u8 = 0x22;
#[cfg(test)]
const ENUM: u8 = 0x23;
#[cfg(test)]
const TEXT: u8 = 0x41;
const NAME: u8 = 0x42;
const KEYWORD: u8 = 0x44;
const URI: u8 = 0x45;
const CHARSET: u8 = 0x47;
const LANGUAGE: u8 = 0x48;
const MIME: u8 = 0x49;

/// A printer CUPS knows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Printer {
    pub name: String,
    /// What people call it ("Office printer"), if set.
    pub info: String,
    pub is_default: bool,
    /// Paper sizes as IPP media keywords (`iso_a4_210x297mm`…).
    pub media: Vec<String>,
    pub media_default: Option<String>,
    pub two_sided: bool,
}

impl Printer {
    pub fn label(&self) -> String {
        let name = if self.info.is_empty() { self.name.clone() } else { self.info.clone() };
        if self.is_default { format!("{name} (default)") } else { name }
    }
}

impl std::fmt::Display for Printer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label())
    }
}

/// How to print a job.
#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub copies: u32,
    pub media: Option<String>,
    pub two_sided: bool,
}

// ---------------------------------------------------------------- encoding

struct Request {
    buf: Vec<u8>,
}

impl Request {
    fn new(operation: u16, printer_uri: &str) -> Self {
        let mut r = Self { buf: Vec::with_capacity(512) };
        r.buf.extend_from_slice(&[2, 0]); // IPP 2.0
        r.buf.extend_from_slice(&operation.to_be_bytes());
        r.buf.extend_from_slice(&1u32.to_be_bytes()); // request id
        r.buf.push(OPERATION);
        r.attr(CHARSET, "attributes-charset", b"utf-8");
        r.attr(LANGUAGE, "attributes-natural-language", b"en");
        r.attr(URI, "printer-uri", printer_uri.as_bytes());
        r
    }

    fn attr(&mut self, tag: u8, name: &str, value: &[u8]) {
        self.buf.push(tag);
        self.buf.extend_from_slice(&(name.len() as u16).to_be_bytes());
        self.buf.extend_from_slice(name.as_bytes());
        self.buf.extend_from_slice(&(value.len() as u16).to_be_bytes());
        self.buf.extend_from_slice(value);
    }

    /// Another value of the attribute just added.
    fn also(&mut self, tag: u8, value: &[u8]) {
        self.attr(tag, "", value);
    }

    fn group(&mut self, tag: u8) {
        self.buf.push(tag);
    }

    fn finish(mut self) -> Vec<u8> {
        self.buf.push(END);
        self.buf
    }
}

fn user_name() -> String {
    std::env::var("USER").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| "lumen".into())
}

/// The job request (without the document) for `printer`.
fn job_request(operation: u16, printer: &str, title: &str, job: &Job) -> Vec<u8> {
    let mut r = Request::new(operation, &format!("ipp://localhost/printers/{printer}"));
    r.attr(NAME, "requesting-user-name", user_name().as_bytes());
    r.attr(NAME, "job-name", title.as_bytes());
    r.attr(MIME, "document-format", b"application/pdf");
    r.group(JOB);
    r.attr(INTEGER, "copies", &(job.copies.clamp(1, 999) as i32).to_be_bytes());
    if let Some(media) = &job.media {
        r.attr(KEYWORD, "media", media.as_bytes());
    }
    let sides: &[u8] = if job.two_sided { b"two-sided-long-edge" } else { b"one-sided" };
    r.attr(KEYWORD, "sides", sides);
    // Pages are fitted to the paper, as a PDF viewer's print dialog does.
    r.attr(KEYWORD, "print-scaling", b"fit");
    r.finish()
}

// ---------------------------------------------------------------- decoding

/// One attribute group of a response: its tag and (name, tag, values) in order.
type Group = (u8, Vec<(String, u8, Vec<Vec<u8>>)>);

struct Response {
    status: u16,
    groups: Vec<Group>,
}

fn parse(bytes: &[u8]) -> Result<Response> {
    let mut at = 8;
    let take = |at: &mut usize, n: usize| -> Result<&[u8]> {
        let slice = bytes.get(*at..*at + n).context("the printing service sent a cut-off answer")?;
        *at += n;
        Ok(slice)
    };
    let head = bytes.get(..8).context("the printing service sent no answer")?;
    let status = u16::from_be_bytes([head[2], head[3]]);
    let mut groups: Vec<Group> = Vec::new();
    loop {
        let tag = take(&mut at, 1)?[0];
        if tag == END {
            break;
        }
        if tag < 0x10 {
            groups.push((tag, Vec::new()));
            continue;
        }
        let name_len = u16::from_be_bytes(take(&mut at, 2)?.try_into()?) as usize;
        let name = String::from_utf8_lossy(take(&mut at, name_len)?).into_owned();
        let value_len = u16::from_be_bytes(take(&mut at, 2)?.try_into()?) as usize;
        let value = take(&mut at, value_len)?.to_vec();
        let Some((_, attrs)) = groups.last_mut() else { bail!("the printing service sent a malformed answer") };
        match attrs.last_mut() {
            // No name: another value of the previous attribute.
            Some((_, _, values)) if name.is_empty() => values.push(value),
            _ => attrs.push((name, tag, vec![value])),
        }
    }
    Ok(Response { status, groups })
}

fn text_of(values: &[Vec<u8>]) -> String {
    values.first().map(|v| String::from_utf8_lossy(v).into_owned()).unwrap_or_default()
}

fn printers_from(response: &Response) -> Vec<Printer> {
    let mut printers = Vec::new();
    for (tag, attrs) in &response.groups {
        if *tag != PRINTER {
            continue;
        }
        let mut p = Printer::default();
        for (name, _, values) in attrs {
            match name.as_str() {
                "printer-name" => p.name = text_of(values),
                "printer-info" => p.info = text_of(values),
                "media-supported" => p.media = values.iter().map(|v| String::from_utf8_lossy(v).into_owned()).collect(),
                "media-default" => p.media_default = Some(text_of(values)).filter(|m| !m.is_empty()),
                "sides-supported" => p.two_sided = values.iter().any(|v| v.starts_with(b"two-sided")),
                _ => {}
            }
        }
        if !p.name.is_empty() {
            printers.push(p);
        }
    }
    printers
}

// ---------------------------------------------------------------- transport

/// Where `CUPS_SERVER` says the printing service is: a socket's path, or a host and port.
#[derive(Debug, PartialEq)]
enum Server {
    Socket(std::path::PathBuf),
    Host(String, u16),
}

fn server(setting: &str) -> Option<Server> {
    // The IPP version may follow, which is no concern here.
    let setting = setting.split("/version=").next().unwrap_or(setting).trim();
    if setting.is_empty() {
        return None;
    }
    if setting.starts_with('/') {
        return Some(Server::Socket(setting.into()));
    }
    Some(match setting.rsplit_once(':') {
        // `[::1]` and `::1` hold colons without a port.
        Some((host, port)) if !host.is_empty() && (!host.contains(':') || host.ends_with(']')) => {
            Server::Host(host.trim_matches(['[', ']']).to_owned(), port.parse().ok()?)
        }
        _ => Server::Host(setting.trim_matches(['[', ']']).to_owned(), 631),
    })
}

fn connect() -> Result<Box<dyn Connection>> {
    let timed_tcp = |s: std::net::TcpStream| -> Result<Box<dyn Connection>> {
        s.set_read_timeout(Some(TIMEOUT))?;
        s.set_write_timeout(Some(TIMEOUT))?;
        Ok(Box::new(s))
    };
    match std::env::var("CUPS_SERVER").ok().as_deref().and_then(server) {
        Some(Server::Socket(path)) => {
            return connect_socket(&path);
        }
        Some(Server::Host(host, port)) => {
            use std::net::ToSocketAddrs;
            let mut addresses =
                (host.as_str(), port).to_socket_addrs().context("the printing service was not found")?;
            let s = addresses
                .find_map(|a| std::net::TcpStream::connect_timeout(&a, Duration::from_secs(5)).ok())
                .with_context(|| format!("no printing service at {host}"))?;
            return timed_tcp(s);
        }
        None => {}
    }
    if let Some(stream) = SOCKETS.iter().find_map(|path| connect_socket(std::path::Path::new(path)).ok()) {
        return Ok(stream);
    }
    let s = std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], 631).into(), Duration::from_secs(2))
        .context("no printing service (CUPS) is running on this computer")?;
    timed_tcp(s)
}

/// Posts an IPP request (and a document after it) to `path`, and returns the answer's body.
fn post(path: &str, ipp: &[u8], document: &[u8]) -> Result<Vec<u8>> {
    let mut stream = connect()?;
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/ipp\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        ipp.len() + document.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(ipp)?;
    stream.write_all(document)?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let code: u16 =
        line.split_whitespace().nth(1).and_then(|c| c.parse().ok()).context("the printing service did not answer")?;
    let (mut length, mut chunked) = (None, false);
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            length = v.trim().parse::<usize>().ok();
        } else if lower.starts_with("transfer-encoding:") && lower.contains("chunked") {
            chunked = true;
        }
    }
    if code != 200 {
        bail!("the printing service refused the request (HTTP {code})");
    }
    let mut body = Vec::new();
    if chunked {
        loop {
            line.clear();
            reader.read_line(&mut line)?;
            let size = usize::from_str_radix(line.trim().split(';').next().unwrap_or("0"), 16)?;
            if size == 0 || size > MAX_RESPONSE.saturating_sub(body.len()) {
                break;
            }
            let start = body.len();
            body.resize(start + size, 0);
            reader.read_exact(&mut body[start..])?;
            line.clear();
            reader.read_line(&mut line)?;
        }
    } else if let Some(n) = length {
        if n > MAX_RESPONSE {
            bail!("the printing service sent too much");
        }
        body.resize(n, 0);
        reader.read_exact(&mut body)?;
    } else {
        reader.take(MAX_RESPONSE as u64).read_to_end(&mut body)?;
    }
    Ok(body)
}

fn check(response: &Response) -> Result<()> {
    if response.status >= 0x0100 {
        let message = response
            .groups
            .iter()
            .flat_map(|(_, attrs)| attrs)
            .find(|(name, ..)| name == "status-message")
            .map(|(_, _, v)| text_of(v))
            .unwrap_or_default();
        bail!("the printer refused the job (IPP status {:#06x}) {message}", response.status);
    }
    Ok(())
}

// ---------------------------------------------------------------- API

/// The printers CUPS knows, the default one first.
pub fn printers() -> Result<Vec<Printer>> {
    let mut r = Request::new(CUPS_GET_PRINTERS, "ipp://localhost/");
    r.attr(NAME, "requesting-user-name", user_name().as_bytes());
    r.attr(KEYWORD, "requested-attributes", b"printer-name");
    for a in ["printer-info", "media-supported", "media-default", "sides-supported"] {
        r.also(KEYWORD, a.as_bytes());
    }
    let response = parse(&post("/", &r.finish(), &[])?)?;
    check(&response)?;
    let mut printers = printers_from(&response);

    let default = Request::new(CUPS_GET_DEFAULT, "ipp://localhost/").finish();
    if let Ok(response) = post("/", &default, &[]).and_then(|b| parse(&b)) {
        let name = printers_from(&response).into_iter().next().map(|p| p.name);
        for p in &mut printers {
            p.is_default = Some(&p.name) == name.as_ref();
        }
    }
    printers.sort_by_key(|p| !p.is_default);
    Ok(printers)
}

/// Prints the PDF `document` on `printer`.
pub fn print(printer: &str, title: &str, job: &Job, document: &[u8]) -> Result<()> {
    let request = job_request(PRINT_JOB, printer, title, job);
    let response = parse(&post(&format!("/printers/{printer}"), &request, document)?)?;
    check(&response)
}

/// Asks `printer` whether it would take the job, without printing anything.
#[cfg(test)]
pub fn validate(printer: &str, title: &str, job: &Job) -> Result<()> {
    let request = job_request(VALIDATE_JOB, printer, title, job);
    let response = parse(&post(&format!("/printers/{printer}"), &request, &[])?)?;
    check(&response)
}

/// A friendly name for a media keyword: `iso_a4_210x297mm` → `A4`.
pub fn media_label(keyword: &str) -> String {
    let known = [
        ("iso_a4_", "A4"),
        ("iso_a3_", "A3"),
        ("iso_a5_", "A5"),
        ("na_letter_", "Letter"),
        ("na_legal_", "Legal"),
        ("iso_b5_", "B5"),
        ("na_executive_", "Executive"),
        ("na_index-4x6_", "4 × 6 in photo"),
        ("na_5x7_", "5 × 7 in photo"),
        ("om_small-photo_", "10 × 15 cm photo"),
        ("iso_dl_", "DL envelope"),
        ("na_number-10_", "No. 10 envelope"),
    ];
    for (prefix, label) in known {
        if keyword.starts_with(prefix) {
            return label.to_string();
        }
    }
    keyword.to_string()
}

/// The paper size in points of a media keyword such as `iso_a4_210x297mm` or `na_letter_8.5x11in`.
pub fn media_size(keyword: &str) -> Option<(f32, f32)> {
    let dims = keyword.rsplit('_').next()?;
    let (unit, scale) = match dims.strip_suffix("mm") {
        Some(d) => (d, 72.0 / 25.4),
        None => (dims.strip_suffix("in")?, 72.0),
    };
    let (w, h) = unit.split_once('x')?;
    Some((w.parse::<f32>().ok()? * scale, h.parse::<f32>().ok()? * scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cups_server_names_a_socket_or_a_host() {
        assert_eq!(server("/run/my/cups.sock"), Some(Server::Socket("/run/my/cups.sock".into())));
        assert_eq!(server("printer.home"), Some(Server::Host("printer.home".into(), 631)));
        assert_eq!(server("printer.home:8631/version=1.1"), Some(Server::Host("printer.home".into(), 8631)));
        assert_eq!(server("[::1]:632"), Some(Server::Host("::1".into(), 632)));
        assert_eq!(server("::1"), Some(Server::Host("::1".into(), 631)));
        assert_eq!((server(""), server("host:nonsense")), (None, None));
    }

    #[test]
    fn encodes_a_print_job() {
        let job = Job { copies: 2, media: Some("iso_a4_210x297mm".into()), two_sided: true };
        let r = job_request(PRINT_JOB, "Office", "Report", &job);
        assert_eq!(&r[..4], &[2, 0, 0, 2], "IPP 2.0, Print-Job");
        let parsed = parse(&r).unwrap();
        let attrs: Vec<(&str, u8, String)> = parsed
            .groups
            .iter()
            .flat_map(|(_, a)| a)
            .map(|(n, t, v)| (n.as_str(), *t, String::from_utf8_lossy(&v[0]).into_owned()))
            .collect();
        assert!(attrs.contains(&("printer-uri", URI, "ipp://localhost/printers/Office".into())));
        assert!(attrs.contains(&("document-format", MIME, "application/pdf".into())));
        assert!(attrs.contains(&("media", KEYWORD, "iso_a4_210x297mm".into())));
        assert!(attrs.contains(&("sides", KEYWORD, "two-sided-long-edge".into())));
        let copies = parsed.groups[1].1.iter().find(|(n, ..)| n == "copies").unwrap();
        assert_eq!(copies.2[0], 2i32.to_be_bytes());
        assert_eq!(parsed.groups.iter().map(|g| g.0).collect::<Vec<_>>(), [OPERATION, JOB]);
    }

    #[test]
    fn reads_a_printer_list() {
        // An answer as CUPS sends it: status, operation attributes, then one group per printer.
        let mut r = Request { buf: vec![2, 0, 0, 0, 0, 0, 0, 1] };
        r.group(OPERATION);
        r.attr(CHARSET, "attributes-charset", b"utf-8");
        for (name, info, two) in [("Office", "Office printer", true), ("Photo", "", false)] {
            r.group(PRINTER);
            r.attr(NAME, "printer-name", name.as_bytes());
            r.attr(TEXT, "printer-info", info.as_bytes());
            r.attr(KEYWORD, "media-supported", b"iso_a4_210x297mm");
            r.also(KEYWORD, b"na_letter_8.5x11in");
            r.attr(KEYWORD, "media-default", b"iso_a4_210x297mm");
            r.attr(KEYWORD, "sides-supported", b"one-sided");
            if two {
                r.also(KEYWORD, b"two-sided-long-edge");
            }
            r.attr(BOOLEAN, "printer-is-accepting-jobs", &[1]);
            r.attr(ENUM, "printer-state", &3i32.to_be_bytes());
        }
        let printers = printers_from(&parse(&r.finish()).unwrap());
        assert_eq!(printers.len(), 2);
        assert_eq!(printers[0].label(), "Office printer");
        assert_eq!(printers[0].media, ["iso_a4_210x297mm", "na_letter_8.5x11in"]);
        assert!(printers[0].two_sided && !printers[1].two_sided);
        assert_eq!(printers[1].label(), "Photo");
    }

    #[test]
    fn cut_off_answers_are_errors() {
        let full = job_request(PRINT_JOB, "P", "T", &Job { copies: 1, media: None, two_sided: false });
        for n in [0, 3, 9, full.len() / 2] {
            assert!(parse(&full[..n]).is_err(), "{n}");
        }
    }

    #[test]
    fn paper_names_and_sizes() {
        assert_eq!(media_label("iso_a4_210x297mm"), "A4");
        assert_eq!(media_label("na_letter_8.5x11in"), "Letter");
        assert_eq!(media_label("custom_thing"), "custom_thing");
        let (w, h) = media_size("iso_a4_210x297mm").unwrap();
        assert!((w - 595.3).abs() < 0.1 && (h - 841.9).abs() < 0.1);
        assert_eq!(media_size("na_letter_8.5x11in"), Some((612.0, 792.0)));
        assert_eq!(media_size("oddball"), None);
    }

    /// Talks to the CUPS on this computer: lists the printers and validates a job on the
    /// first one, which prints nothing. `cargo test cups_live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn cups_live() {
        let printers = printers().unwrap();
        for p in &printers {
            println!(
                "{} [{}] media: {:?} default: {:?} two-sided: {}",
                p.label(),
                p.name,
                p.media.len(),
                p.media_default,
                p.two_sided
            );
        }
        let first = printers.first().expect("a printer");
        let job = Job { copies: 1, media: first.media_default.clone(), two_sided: false };
        validate(&first.name, "Lumen test", &job).unwrap();
        println!("validate-job: ok");
    }
}
