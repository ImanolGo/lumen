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

use relm4::gtk::{gdk, gdk_pixbuf::Pixbuf, glib, prelude::*};

pub const THUMB_SIZE: i32 = 128;
const WORKERS: usize = 2;

/// Pixels of a finished thumbnail, safe to move to the main thread.
#[derive(Debug)]
pub struct ThumbData {
    width: i32,
    height: i32,
    rowstride: usize,
    has_alpha: bool,
    bytes: glib::Bytes,
}

impl ThumbData {
    pub fn into_texture(self) -> gdk::Texture {
        let format = if self.has_alpha {
            gdk::MemoryFormat::R8g8b8a8
        } else {
            gdk::MemoryFormat::R8g8b8
        };
        gdk::MemoryTexture::new(self.width, self.height, format, &self.bytes, self.rowstride)
            .upcast()
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

fn render(path: &std::path::Path) -> Option<ThumbData> {
    let pixbuf = Pixbuf::from_file_at_scale(path, THUMB_SIZE, THUMB_SIZE, true).ok()?;
    let pixbuf = pixbuf.apply_embedded_orientation().unwrap_or(pixbuf);
    Some(ThumbData {
        width: pixbuf.width(),
        height: pixbuf.height(),
        rowstride: pixbuf.rowstride() as usize,
        has_alpha: pixbuf.has_alpha(),
        bytes: pixbuf.read_pixel_bytes(),
    })
}
