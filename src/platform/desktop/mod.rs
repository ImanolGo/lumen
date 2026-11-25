//! macOS and Windows. What both do the same way is here, through crates that know each system's own
//! services; what only one does is in its own file.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as own;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as own;

pub use own::{CAPS, confine, fs, menu_bar, print, shell, trash};

pub mod dirs {
    use std::path::PathBuf;

    use directories::{ProjectDirs, UserDirs};

    use crate::i18n::tr;

    pub fn home() -> Option<PathBuf> {
        Some(UserDirs::new()?.home_dir().to_path_buf())
    }

    fn project() -> Option<ProjectDirs> {
        ProjectDirs::from("io.github", "imanolgo", "Lumen")
    }

    pub fn config() -> Option<PathBuf> {
        Some(project()?.config_dir().to_path_buf())
    }

    pub fn state() -> Option<PathBuf> {
        Some(project()?.data_local_dir().to_path_buf())
    }

    pub fn data() -> Option<PathBuf> {
        Some(project()?.data_dir().to_path_buf())
    }

    pub(super) fn cache() -> Option<PathBuf> {
        Some(project()?.cache_dir().to_path_buf())
    }

    /// The folders Lumen's own file chooser offers.
    pub fn places() -> Vec<(String, PathBuf)> {
        let mut out = Vec::new();
        if let Some(user) = UserDirs::new() {
            out.push((tr("Home"), user.home_dir().to_path_buf()));
            for dir in [user.desktop_dir(), user.document_dir(), user.download_dir(), user.picture_dir()] {
                if let Some(dir) = dir.filter(|d| d.is_dir()) {
                    out.push((dir.file_name().unwrap_or_default().to_string_lossy().into_owned(), dir.to_path_buf()));
                }
            }
        }
        out.extend(super::own::roots());
        out
    }
}

pub mod dialogs {
    use std::path::PathBuf;

    use anyhow::Result;
    use rfd::{AsyncFileDialog, FileHandle};

    use crate::i18n::tr;
    use crate::platform::NoDialog;

    /// `LUMEN_NO_PORTAL=1` shows Lumen's own chooser here too, for trying it.
    fn disabled() -> bool {
        std::env::var_os("LUMEN_NO_PORTAL").is_some_and(|v| !v.is_empty() && v != "0")
    }

    pub async fn choose(folder: bool) -> Result<Vec<PathBuf>> {
        if disabled() {
            return Err(NoDialog.into());
        }
        let paths = |files: Vec<FileHandle>| files.into_iter().map(|f| f.path().to_path_buf()).collect();
        if folder {
            let picked = AsyncFileDialog::new().set_title(tr("Open Folder")).pick_folder().await;
            return Ok(paths(picked.into_iter().collect()));
        }
        let picked = AsyncFileDialog::new()
            .set_title(tr("Open Files"))
            .add_filter(tr("Pictures and PDFs"), &crate::library::extensions())
            .add_filter(tr("All files"), &["*"])
            .pick_files()
            .await;
        Ok(paths(picked.unwrap_or_default()))
    }

    pub async fn save_as(suggested: PathBuf) -> Result<Option<PathBuf>> {
        if disabled() {
            return Err(NoDialog.into());
        }
        let mut dialog = AsyncFileDialog::new().set_title(tr("Save a Copy"));
        if let Some(dir) = suggested.parent().filter(|d| d.is_dir()) {
            dialog = dialog.set_directory(dir);
        }
        if let Some(name) = suggested.file_name() {
            dialog = dialog.set_file_name(name.to_string_lossy());
        }
        if let Some(ext) = suggested.extension().and_then(|e| e.to_str()) {
            // So that a name typed without an ending gets this one.
            dialog = dialog.add_filter(ext.to_uppercase(), &[ext]).add_filter(tr("All files"), &["*"]);
        }
        Ok(dialog.save_file().await.map(|f| f.path().to_path_buf()))
    }
}

pub mod clipboard {
    /// Puts a picture on the clipboard.
    pub fn copy_png(png: Vec<u8>) -> Result<(), String> {
        let picture = image::load_from_memory_with_format(&png, image::ImageFormat::Png).map_err(|e| e.to_string())?;
        let rgba = picture.into_rgba8();
        let (width, height) = (rgba.width() as usize, rgba.height() as usize);
        let data = arboard::ImageData { width, height, bytes: rgba.into_raw().into() };
        arboard::Clipboard::new().and_then(|mut c| c.set_image(data)).map_err(|e| e.to_string())
    }
}

pub mod watch {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use futures::SinkExt;
    use futures::channel::mpsc;
    use notify::{EventKind, RecursiveMode, Watcher};

    /// A burst of events (a save is often several) is taken as one.
    const SETTLE: Duration = Duration::from_millis(150);

    /// `Some(())` each time something happened to `path`; `None` once, if watching is not possible
    /// here (the caller then looks now and then). It never ends by itself: drop it to stop.
    pub fn changes(path: PathBuf) -> impl futures::Stream<Item = Option<()>> {
        iced::stream::channel(4, async move |mut out: mpsc::Sender<Option<()>>| {
            if let Err(e) = watch(&path, &mut out).await {
                tracing::debug!("cannot watch {}: {e}", path.display());
                let _ = out.send(None).await;
            }
            std::future::pending::<()>().await;
        })
    }

    async fn watch(path: &Path, out: &mut mpsc::Sender<Option<()>>) -> notify::Result<()> {
        let name = path.file_name().ok_or_else(|| notify::Error::generic("no file name"))?.to_owned();
        // Its folder is watched: editors often save by renaming a new file over the old one. The
        // system may know the folder by another path (a link), so only names are compared.
        let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let (tell, mut heard) = tokio::sync::mpsc::unbounded_channel();
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let about_it = match &event {
                Ok(e) => {
                    e.need_rescan()
                        || (!matches!(e.kind, EventKind::Access(_))
                            && e.paths.iter().any(|p| p.file_name() == Some(name.as_os_str())))
                }
                Err(_) => false,
            };
            if about_it || event.is_err() {
                let _ = tell.send(event.map(drop));
            }
        })?;
        watcher.watch(dir, RecursiveMode::NonRecursive)?;
        while let Some(event) = heard.recv().await {
            event?;
            tokio::time::sleep(SETTLE).await;
            while let Ok(more) = heard.try_recv() {
                more?;
            }
            if out.send(Some(())).await.is_err() {
                break;
            }
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use futures::StreamExt;

        #[tokio::test]
        async fn writing_replacing_and_deleting_the_file_are_noticed() {
            let dir = std::env::temp_dir().join(format!("lumen-watch-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("book.pdf");
            std::fs::write(&file, b"one").unwrap();
            // The system may still be telling of that.
            tokio::time::sleep(Duration::from_secs(1)).await;
            let mut stream = Box::pin(changes(file.clone()));
            let wait = Duration::from_secs(10);
            let quiet = Duration::from_millis(1500);
            while tokio::time::timeout(quiet, stream.next()).await.is_ok() {}
            std::fs::write(dir.join("other.txt"), b"x").unwrap();
            assert!(tokio::time::timeout(quiet, stream.next()).await.is_err(), "another file");
            std::fs::write(&file, b"two").unwrap();
            assert_eq!(tokio::time::timeout(wait, stream.next()).await.unwrap(), Some(Some(())));
            while tokio::time::timeout(quiet, stream.next()).await.is_ok() {}
            std::fs::write(dir.join("new"), b"three").unwrap();
            std::fs::rename(dir.join("new"), &file).unwrap();
            assert_eq!(tokio::time::timeout(wait, stream.next()).await.unwrap(), Some(Some(())));
            while tokio::time::timeout(quiet, stream.next()).await.is_ok() {}
            std::fs::remove_file(&file).unwrap();
            assert_eq!(tokio::time::timeout(wait, stream.next()).await.unwrap(), Some(Some(())));
            drop(stream);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

pub mod startup {
    pub fn before_window() {}

    #[cfg(target_os = "macos")]
    pub fn window() -> iced::window::settings::PlatformSpecific {
        // Lumen's header is drawn inside the title bar, beside the traffic lights.
        iced::window::settings::PlatformSpecific {
            title_hidden: true,
            titlebar_transparent: true,
            fullsize_content_view: true,
        }
    }

    #[cfg(windows)]
    pub fn window() -> iced::window::settings::PlatformSpecific {
        Default::default()
    }

    /// The language of the system: a program started from its desktop has no `LANG`.
    pub fn language() -> Option<String> {
        sys_locale::get_locale().map(|l| l.replace('-', "_"))
    }
}

pub mod thumbs {
    use std::path::{Path, PathBuf};

    /// Lumen's own cache, in the format of the freedesktop one.
    pub fn shared_cache() -> Option<PathBuf> {
        Some(super::dirs::cache()?.join("thumbnails"))
    }

    /// What names `path` in it.
    pub fn file_uri(path: &Path) -> Option<String> {
        path.is_absolute().then(|| format!("file:///{}", path.display()))
    }
}

pub mod control {
    use crate::control::Shared;

    pub fn requests() -> impl futures::Stream<Item = Shared> {
        futures::stream::pending()
    }
}
