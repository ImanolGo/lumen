//! Printing through the desktop's portal, and the local CUPS socket.

use std::fs::File;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ashpd::desktop::ResponseError;
use ashpd::desktop::print::{Orientation, PageSetup, PreparePrintOptions, PrintOptions, PrintProxy, Settings};

use crate::library::Kind;
use crate::print::{Connection, Job, MARGIN, MM_TO_PT, Outcome, Paper, picture_to_pdf, title_of};

/// Where CUPS listens.
pub const SOCKETS: [&str; 2] = ["/run/cups/cups.sock", "/var/run/cups/cups.sock"];

/// Connects to a printing service's socket.
pub fn connect(socket: &Path) -> Result<Box<dyn Connection>> {
    let s = UnixStream::connect(socket).with_context(|| format!("no printing service at {}", socket.display()))?;
    s.set_read_timeout(Some(crate::cups::TIMEOUT))?;
    s.set_write_timeout(Some(crate::cups::TIMEOUT))?;
    Ok(Box::new(s))
}

fn cancelled(e: &ashpd::Error) -> bool {
    matches!(e, ashpd::Error::Response(ResponseError::Cancelled))
}

/// Shows the desktop's print dialog for `path` and, if confirmed, prints it.
pub async fn print(path: PathBuf) -> Result<Outcome> {
    if super::portal::disabled() {
        return Ok(Outcome::NoPortal);
    }
    let title = title_of(&path);
    let Ok(proxy) = PrintProxy::new().await else { return Ok(Outcome::NoPortal) };
    let has_pages = Kind::of(&path) == Some(Kind::Pdf);
    let request = proxy
        .prepare_print(
            None,
            &title,
            Settings::default(),
            PageSetup::default(),
            PreparePrintOptions::default().set_modal(true).set_accept_label("Print").set_has_current_page(has_pages),
        )
        .await;
    let prepared = match request.and_then(|r| r.response()) {
        Ok(p) => p,
        Err(e) if cancelled(&e) => return Ok(Outcome::Cancelled),
        // The portal is there but cannot print (no backend for it): print without it.
        Err(e) => {
            tracing::info!("the print portal failed ({e}); printing directly");
            return Ok(Outcome::NoPortal);
        }
    };

    let token = prepared.token;
    let file = if has_pages {
        File::open(&path).with_context(|| format!("cannot read {}", path.display()))?
    } else {
        let paper = paper(&prepared.page_setup);
        let src = path.clone();
        tokio::task::spawn_blocking(move || picture_to_pdf_file(&src, &paper))
            .await
            .context("print preparation stopped")??
    };

    let sent = proxy
        .print(None, &title, &file.as_fd(), PrintOptions::default().set_token(token).set_modal(true))
        .await
        .and_then(|r| r.response());
    match sent {
        Ok(()) => Ok(Outcome::Sent),
        Err(e) if cancelled(&e) => Ok(Outcome::Cancelled),
        Err(e) => Err(e).context("the print job failed"),
    }
}

/// The paper chosen in the portal's dialog, with orientation applied. Defaults to A4.
fn paper(setup: &PageSetup) -> Paper {
    let mm = |v: Option<f64>, d: f32| v.map(|v| v as f32).filter(|v| *v > 0.0).unwrap_or(d) * MM_TO_PT;
    let (mut w, mut h) = (mm(setup.width, 210.0), mm(setup.height, 297.0));
    let landscape = matches!(setup.orientation, Some(Orientation::Landscape) | Some(Orientation::ReverseLandscape));
    if landscape && w < h || !landscape && w > h && setup.orientation.is_some() {
        std::mem::swap(&mut w, &mut h);
    }
    let margin = |v: Option<f64>| v.map(|v| v as f32 * MM_TO_PT).filter(|v| *v >= 0.0).unwrap_or(MARGIN);
    let margins =
        [margin(setup.margin_top), margin(setup.margin_right), margin(setup.margin_bottom), margin(setup.margin_left)];
    Paper { width: w, height: h, margins }
}

/// Writes the picture's PDF to a private temporary file and returns it open,
/// already unlinked, so nothing is left behind whatever happens next.
fn picture_to_pdf_file(path: &Path, paper: &Paper) -> Result<File> {
    let bytes = picture_to_pdf(path, paper)?;
    let copy = Job::copy("picture.pdf", |path| Ok(crate::fsio::write_new(path, &bytes)?))?;
    Ok(File::open(&copy.path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_paper_is_a4() {
        let a4 = paper(&PageSetup::default());
        assert!((a4.width - 595.3).abs() < 1.0 && (a4.height - 841.9).abs() < 1.0, "{a4:?}");
    }
}
