//! Copying a picture through the Wayland data-control protocol (the `wl-clipboard-rs` crate, pure
//! Rust). Compositors that do not offer that protocol (GNOME's) and X11 sessions cannot take a
//! picture this way; the error says so.

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
