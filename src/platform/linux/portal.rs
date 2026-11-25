//! Native file and folder choosers through the XDG desktop portal.

use std::path::PathBuf;

use crate::i18n::tr;
use crate::platform::NoDialog;
use anyhow::{Context, Result};
use ashpd::desktop::ResponseError;
use ashpd::desktop::file_chooser::{FileFilter, SelectedFiles};

/// Whether the portals are switched off (`LUMEN_NO_PORTAL=1`): Lumen's own file and
/// print dialogs are used then, as on desktops without a portal.
pub fn disabled() -> bool {
    std::env::var_os("LUMEN_NO_PORTAL").is_some_and(|v| !v.is_empty() && v != "0")
        || !crate::settings::portals_allowed()
}

/// Asks the user for files (or one folder). `Ok(empty)` means cancelled; a [`NoDialog`]
/// error means there is no portal to ask.
pub async fn choose(folder: bool) -> Result<Vec<PathBuf>> {
    if disabled() {
        return Err(NoDialog.into());
    }
    let title = if folder { tr("Open Folder") } else { tr("Open Files") };
    let open = tr("Open");
    let mut request = SelectedFiles::open_file()
        .title(title.as_str())
        .accept_label(open.as_str())
        .modal(true)
        .multiple(!folder)
        .directory(folder);
    if !folder {
        let supported = FileFilter::new(&tr("Pictures and PDFs"))
            .mimetype("application/pdf")
            .mimetype("image/png")
            .mimetype("image/jpeg")
            .mimetype("image/gif")
            .mimetype("image/webp")
            .mimetype("image/bmp")
            .mimetype("image/tiff")
            .mimetype("image/svg+xml")
            .mimetype("image/svg+xml-compressed")
            .mimetype("image/x-icon")
            .mimetype("image/vnd.microsoft.icon")
            .mimetype("image/qoi")
            .mimetype("image/x-tga")
            .mimetype("image/x-portable-anymap")
            .mimetype("image/jxl")
            .mimetype("image/jp2")
            .mimetype("image/jpx")
            .mimetype("image/x-jp2-codestream")
            // For desktops that do not know those names.
            .glob("*.svgz")
            .glob("*.j2k")
            .glob("*.j2c")
            .mimetype("image/x-exr");
        #[cfg(feature = "heif")]
        let supported = supported.mimetype("image/heic").mimetype("image/heif").mimetype("image/avif");
        request = request
            .filter(supported.clone())
            .filter(FileFilter::new(&tr("All files")).glob("*"))
            .current_filter(supported);
    }
    let files = match request.send().await.and_then(|r| r.response()) {
        Ok(files) => files,
        Err(ashpd::Error::Response(ResponseError::Cancelled)) => return Ok(Vec::new()),
        Err(e) => {
            tracing::info!("the file chooser portal failed ({e}); using Lumen's own");
            return Err(NoDialog.into());
        }
    };
    Ok(files.uris().iter().filter_map(|uri| uri_to_path(uri.as_str())).collect())
}

/// Asks where to save a file, suggesting `suggested` (its folder and name). `Ok(None)` means cancelled.
pub async fn save_as(suggested: PathBuf) -> Result<Option<PathBuf>> {
    if disabled() {
        return Err(NoDialog.into());
    }
    let name = suggested.file_name().map(|n| n.to_string_lossy().into_owned());
    let (title, save) = (tr("Save a Copy"), tr("Save"));
    let base = || {
        SelectedFiles::save_file()
            .title(title.as_str())
            .accept_label(save.as_str())
            .modal(true)
            .current_name(name.as_deref())
    };
    let request = match suggested.parent().filter(|d| d.is_dir()) {
        Some(dir) => base().current_folder(dir).unwrap_or_else(|_| base()),
        None => base(),
    };
    match request.send().await.and_then(|r| r.response()) {
        Ok(files) => Ok(files.uris().iter().find_map(|uri| uri_to_path(uri.as_str()))),
        Err(ashpd::Error::Response(ResponseError::Cancelled)) => Ok(None),
        Err(e) => {
            tracing::info!("the file chooser portal failed ({e}); using Lumen's own");
            Err(NoDialog.into())
        }
    }
}

/// Opens a web or mail address in the user's browser or mail app, through the portal, or
/// with `xdg-open` when there is none. Callers only pass addresses already checked to be
/// web or mail links.
pub async fn open_uri(uri: String) -> Result<()> {
    let parsed = ashpd::Uri::parse(&uri).context("not a valid address")?;
    if !disabled() {
        let sent = ashpd::desktop::open_uri::OpenFileRequest::default()
            .ask(false)
            .send_uri(&parsed)
            .await
            .and_then(|r| r.response());
        match sent {
            Ok(()) => return Ok(()),
            Err(ashpd::Error::Response(ResponseError::Cancelled)) => return Ok(()),
            Err(e) => tracing::info!("the open-uri portal failed ({e}); using xdg-open"),
        }
    }
    let status = tokio::process::Command::new("xdg-open")
        .arg(parsed.as_str())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .context("no program to open links with (xdg-open) was found")?;
    if !status.success() {
        anyhow::bail!("xdg-open could not open the link");
    }
    Ok(())
}

/// Shows `path` selected in the file manager (`org.freedesktop.FileManager1`), or else opens its
/// folder with `xdg-open`.
pub async fn show_in_folder(path: PathBuf) -> Result<()> {
    let uri = format!("file://{}", super::trash::encode(super::fs::bytes(&path)));
    let asked = async {
        let connection = ashpd::zbus::Connection::session().await?;
        connection
            .call_method(
                Some("org.freedesktop.FileManager1"),
                "/org/freedesktop/FileManager1",
                Some("org.freedesktop.FileManager1"),
                "ShowItems",
                &(vec![uri.as_str()], ""),
            )
            .await?;
        Ok::<(), ashpd::zbus::Error>(())
    };
    match asked.await {
        Ok(()) => Ok(()),
        Err(e) => {
            tracing::info!("no file manager answered ({e}); opening the folder with xdg-open");
            let dir = path.parent().context("that file has no folder")?;
            let status = tokio::process::Command::new("xdg-open")
                .arg(dir)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await
                .context("no file manager and no xdg-open were found")?;
            if !status.success() {
                anyhow::bail!("xdg-open could not open the folder");
            }
            Ok(())
        }
    }
}

/// Lets the user choose which app opens `path` (the portal's "Open With" dialog).
pub async fn open_with(path: PathBuf) -> Result<()> {
    if disabled() {
        anyhow::bail!("Open With needs the desktop portal (LUMEN_NO_PORTAL is set)");
    }
    let file = std::fs::File::open(&path).with_context(|| format!("cannot open {}", path.display()))?;
    match ashpd::desktop::open_uri::OpenFileRequest::default()
        .ask(true)
        .send_file(&file)
        .await
        .and_then(|r| r.response())
    {
        Ok(()) => Ok(()),
        Err(ashpd::Error::Response(ResponseError::Cancelled)) => Ok(()),
        Err(e) => Err(anyhow::anyhow!("the desktop could not offer apps to open it with ({e})")),
    }
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
