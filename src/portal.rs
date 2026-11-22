//! Native file and folder choosers through the XDG desktop portal.

use std::path::PathBuf;

use anyhow::{Context, Result};
use ashpd::desktop::ResponseError;
use ashpd::desktop::file_chooser::{FileFilter, SelectedFiles};

/// Asks the user for files (or one folder). `Ok(empty)` means cancelled.
pub async fn choose(folder: bool) -> Result<Vec<PathBuf>> {
    let mut request = SelectedFiles::open_file()
        .title(if folder { "Open Folder" } else { "Open Files" })
        .accept_label("Open")
        .modal(true)
        .multiple(!folder)
        .directory(folder);
    if !folder {
        let supported = FileFilter::new("Pictures and PDFs")
            .mimetype("application/pdf")
            .mimetype("image/png")
            .mimetype("image/jpeg")
            .mimetype("image/gif")
            .mimetype("image/webp")
            .mimetype("image/bmp")
            .mimetype("image/tiff")
            .mimetype("image/svg+xml")
            .mimetype("image/x-icon")
            .mimetype("image/vnd.microsoft.icon")
            .mimetype("image/qoi")
            .mimetype("image/x-tga")
            .mimetype("image/x-portable-anymap");
        request =
            request.filter(supported.clone()).filter(FileFilter::new("All files").glob("*")).current_filter(supported);
    }
    let files = match request.send().await.and_then(|r| r.response()) {
        Ok(files) => files,
        Err(ashpd::Error::Response(ResponseError::Cancelled)) => return Ok(Vec::new()),
        Err(e) => {
            return Err(e).context("no file chooser portal is available (install xdg-desktop-portal-gtk or -gnome)");
        }
    };
    Ok(files.uris().iter().filter_map(|uri| uri_to_path(uri.as_str())).collect())
}

/// Opens a web or mail address in the user's browser or mail app, through the portal.
pub async fn open_uri(uri: String) -> Result<()> {
    let parsed = ashpd::Uri::parse(&uri).context("not a valid address")?;
    ashpd::desktop::open_uri::OpenFileRequest::default()
        .ask(false)
        .send_uri(&parsed)
        .await
        .and_then(|r| r.response())
        .context("no desktop portal could open the link")
}

/// `file:///a%20b` → `/a b`. Non-file URIs are ignored.
fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.find('/').map(|i| &rest[i..])?; // drop an optional host
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = bytes.get(i + 1..i + 3)
            && let Some(b) = std::str::from_utf8(hex).ok().and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_vec(out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_file_uris() {
        assert_eq!(uri_to_path("file:///home/u/My%20Pics/a.png"), Some("/home/u/My Pics/a.png".into()));
        assert_eq!(uri_to_path("file://localhost/tmp/x%"), Some("/tmp/x%".into()));
        assert_eq!(uri_to_path("file:///caf%C3%A9.pdf"), Some("/café.pdf".into()));
        assert_eq!(uri_to_path("https://example.com/a.png"), None);
    }
}
