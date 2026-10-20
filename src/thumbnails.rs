//! Background thumbnail generation for the file strip.
//!
//! A small fixed pool of threads works through the most recent request, so
//! opening a folder of thousands of photos never spawns thousands of decoders.

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    thread,
};

use relm4::gtk::{gdk, gdk_pixbuf::Pixbuf, gio, glib, prelude::*};

pub const THUMB_SIZE: i32 = 144;
const WORKERS: usize = 2;

/// Pixels of a finished thumbnail, safe to move to the main thread.
#[derive(Debug)]
pub enum ThumbData {
    Picture {
        width: i32,
        height: i32,
        rowstride: usize,
        has_alpha: bool,
        bytes: glib::Bytes,
    },
    /// The first page of a PDF.
    Page(crate::pdf::RenderedPage),
    /// Decoded in glycin's sandbox.
    Texture(gdk::Texture),
}

impl ThumbData {
    pub fn into_texture(self) -> gdk::Texture {
        match self {
            Self::Picture {
                width,
                height,
                rowstride,
                has_alpha,
                bytes,
            } => {
                let format = if has_alpha {
                    gdk::MemoryFormat::R8g8b8a8
                } else {
                    gdk::MemoryFormat::R8g8b8
                };
                gdk::MemoryTexture::new(width, height, format, &bytes, rowstride).upcast()
            }
            Self::Page(page) => page.into_texture(),
            Self::Texture(texture) => texture,
        }
    }
}

type Queue = Arc<(Mutex<VecDeque<PathBuf>>, Condvar)>;

pub struct ThumbnailLoader {
    queue: Queue,
}

impl ThumbnailLoader {
    /// `on_ready` is called on a worker thread for every finished thumbnail.
    pub fn new(on_ready: impl Fn(PathBuf, ThumbData) + Send + Sync + 'static) -> Self {
        let queue: Queue = Arc::default();
        let on_ready = Arc::new(on_ready);
        for _ in 0..WORKERS {
            let queue = Arc::clone(&queue);
            let on_ready = Arc::clone(&on_ready);
            thread::spawn(move || {
                loop {
                    let path = {
                        let (lock, condvar) = &*queue;
                        let mut pending = lock.lock().unwrap();
                        loop {
                            if let Some(path) = pending.pop_front() {
                                break path;
                            }
                            pending = condvar.wait(pending).unwrap();
                        }
                    };
                    if let Some(data) = render(&path) {
                        on_ready(path, data);
                    }
                }
            });
        }
        Self { queue }
    }

    /// Replace the pending work with `paths`, in priority order.
    pub fn request(&self, paths: impl IntoIterator<Item = PathBuf>) {
        let (lock, condvar) = &*self.queue;
        let mut pending = lock.lock().unwrap();
        pending.clear();
        pending.extend(paths);
        condvar.notify_all();
    }
}

/// The thumbnail the file manager (or an earlier run) already made, if it is still valid.
fn cached(path: &std::path::Path) -> Option<gdk::Texture> {
    let info = gio::File::for_path(path)
        .query_info(
            "thumbnail::path,thumbnail::is-valid",
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )
        .ok()?;
    if !info.boolean("thumbnail::is-valid") {
        return None;
    }
    let thumbnail = info.attribute_byte_string("thumbnail::path")?;
    gdk::Texture::from_filename(std::path::Path::new(thumbnail.as_str())).ok()
}

/// Save `texture` in the desktop's thumbnail cache, as the freedesktop thumbnail
/// specification says, so the next run (and the file manager) can use it. Best effort.
fn store(path: &std::path::Path, texture: &gdk::Texture) {
    let _ = try_store(path, texture);
}

fn try_store(path: &std::path::Path, texture: &gdk::Texture) -> Option<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

    let uri = gio::File::for_path(path).uri();
    let hash = glib::compute_checksum_for_string(glib::ChecksumType::Md5, uri.as_str())?;
    let dir = glib::user_cache_dir().join("thumbnails").join("large");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .ok()?;
    let mtime = std::fs::metadata(path).ok()?.mtime();

    let mut downloader = gdk::TextureDownloader::new(texture);
    downloader.set_format(gdk::MemoryFormat::R8g8b8a8);
    let (bytes, stride) = downloader.download_bytes();
    let pixbuf = Pixbuf::from_bytes(
        &bytes,
        relm4::gtk::gdk_pixbuf::Colorspace::Rgb,
        true,
        8,
        texture.width(),
        texture.height(),
        stride as i32,
    );
    let temp = dir.join(format!(".{hash}.{}.tmp", std::process::id()));
    pixbuf
        .savev(
            &temp,
            "png",
            &[
                ("tEXt::Thumb::URI", uri.as_str()),
                ("tEXt::Thumb::MTime", &mtime.to_string()),
            ],
        )
        .ok()?;
    std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600)).ok()?;
    std::fs::rename(&temp, dir.join(format!("{hash}.png"))).ok()
}

fn render(path: &std::path::Path) -> Option<ThumbData> {
    if let Some(texture) = cached(path) {
        return Some(ThumbData::Texture(texture));
    }
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
    {
        return crate::pdf::render_thumbnail(path, THUMB_SIZE)
            .ok()
            .map(ThumbData::Page);
    }
    // Pictures from anywhere are decoded in glycin's sandbox when it is installed.
    if crate::image::decode::sandboxed() {
        return match crate::image::decode::thumbnail(path, THUMB_SIZE as u32) {
            Ok(texture) => {
                store(path, &texture);
                Some(ThumbData::Texture(texture))
            }
            Err(err) => {
                tracing::debug!(path = %path.display(), "no thumbnail: {err:#}");
                None
            }
        };
    }
    let pixbuf = Pixbuf::from_file_at_scale(path, THUMB_SIZE, THUMB_SIZE, true).ok()?;
    let pixbuf = pixbuf.apply_embedded_orientation().unwrap_or(pixbuf);
    Some(ThumbData::Picture {
        width: pixbuf.width(),
        height: pixbuf.height(),
        rowstride: pixbuf.rowstride() as usize,
        has_alpha: pixbuf.has_alpha(),
        bytes: pixbuf.read_pixel_bytes(),
    })
}
