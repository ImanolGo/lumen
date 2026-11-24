//! Waiting for another program to change the open file, with inotify: nothing is polled, so a
//! window left open costs no CPU at all. The file's folder is watched (editors often save by
//! renaming a new file over the old one, which a watch on the file itself would miss), and only
//! events about this file's name count.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::SinkExt;
use futures::channel::mpsc;
use rustix::fs::inotify::{self, CreateFlags, WatchFlags};
use rustix::io::Errno;

/// A burst of events (a save is often several) is taken as one.
const SETTLE: Duration = Duration::from_millis(150);

/// `Some(())` each time something happened to `path` (its contents were written, a file was moved
/// onto it, it was touched); `None` once, if watching is not possible here (the caller then falls
/// back to looking now and then). It never ends by itself: drop it to stop.
pub fn changes(path: PathBuf) -> impl futures::Stream<Item = Option<()>> {
    iced::stream::channel(4, async move |mut out: mpsc::Sender<Option<()>>| {
        if let Err(e) = watch(&path, &mut out).await {
            tracing::debug!("cannot watch {}: {e}", path.display());
            let _ = out.send(None).await;
        }
        std::future::pending::<()>().await;
    })
}

async fn watch(path: &Path, out: &mut mpsc::Sender<Option<()>>) -> std::io::Result<()> {
    let name = path.file_name().ok_or_else(|| std::io::Error::other("no file name"))?;
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let fd = inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK)?;
    inotify::add_watch(
        &fd,
        dir,
        WatchFlags::CLOSE_WRITE | WatchFlags::MOVED_TO | WatchFlags::CREATE | WatchFlags::ATTRIB,
    )?;
    // `new` is deprecated because it cannot tell who owns the descriptor; `register` is `unsafe`,
    // which this program forbids. Here the `OwnedFd` is the owner, the case the note is not about.
    #[allow(deprecated)]
    let ready = tokio::io::unix::AsyncFd::new(fd)?;
    loop {
        let mut guard = ready.readable().await?;
        let mut hit = drain(ready.get_ref(), name)?;
        guard.clear_ready();
        if hit {
            // Let the rest of the burst arrive, then take it all in one go.
            tokio::time::sleep(SETTLE).await;
            hit = drain(ready.get_ref(), name)? || hit;
            let _ = hit;
            if out.send(Some(())).await.is_err() {
                return Ok(());
            }
        }
    }
}

/// Reads every event waiting; whether any was about `name`.
fn drain(fd: &impl std::os::fd::AsFd, name: &OsStr) -> std::io::Result<bool> {
    let mut buffer = [std::mem::MaybeUninit::<u8>::uninit(); 4096];
    let mut reader = inotify::Reader::new(fd, &mut buffer);
    let mut hit = false;
    loop {
        match reader.next() {
            Ok(event) => hit |= event.file_name().is_some_and(|n| n.to_bytes() == name.as_bytes()),
            Err(Errno::AGAIN) => return Ok(hit),
            Err(e) => return Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lumen-watch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The next thing the stream says within `wait`, or none.
    async fn next(
        stream: &mut (impl futures::Stream<Item = Option<()>> + Unpin),
        wait: Duration,
    ) -> Option<Option<()>> {
        tokio::time::timeout(wait, stream.next()).await.ok().flatten()
    }

    #[tokio::test]
    async fn writing_replacing_and_touching_the_file_are_noticed_other_files_are_not() {
        let dir = temp("events");
        let file = dir.join("book.pdf");
        std::fs::write(&file, b"one").unwrap();
        let mut stream = Box::pin(changes(file.clone()));
        // The watch is set up when the stream is first polled; let it get there.
        assert!(next(&mut stream, Duration::from_millis(300)).await.is_none(), "nothing has happened yet");

        std::fs::write(dir.join("other.txt"), b"x").unwrap();
        assert!(next(&mut stream, Duration::from_millis(500)).await.is_none(), "another file is not this one");

        std::fs::write(&file, b"two").unwrap();
        assert_eq!(next(&mut stream, Duration::from_secs(3)).await, Some(Some(())), "written in place");

        std::fs::write(dir.join("book.tmp"), b"three").unwrap();
        std::fs::rename(dir.join("book.tmp"), &file).unwrap();
        assert_eq!(next(&mut stream, Duration::from_secs(3)).await, Some(Some(())), "replaced by a rename");

        std::fs::File::options().write(true).open(&file).unwrap().set_modified(std::time::SystemTime::now()).unwrap();
        assert_eq!(next(&mut stream, Duration::from_secs(3)).await, Some(Some(())), "touched");

        // Several writes close together make one notice.
        for i in 0..5 {
            std::fs::write(&file, vec![b'x'; i + 1]).unwrap();
        }
        assert_eq!(next(&mut stream, Duration::from_secs(3)).await, Some(Some(())));
        assert!(next(&mut stream, Duration::from_millis(500)).await.is_none(), "one notice for the burst");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn a_folder_that_cannot_be_watched_is_reported_once() {
        let mut stream = Box::pin(changes(PathBuf::from("/no/such/folder/file.pdf")));
        assert_eq!(next(&mut stream, Duration::from_secs(2)).await, Some(None));
        assert!(next(&mut stream, Duration::from_millis(300)).await.is_none(), "and then it is quiet");
    }
}
