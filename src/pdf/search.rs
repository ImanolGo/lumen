//! Text search in a background thread.
//!
//! The worker owns its own Poppler document and walks the pages one by one,
//! reporting hits as it goes, so results appear while the search is still
//! running. Starting a new search abandons the old one between pages.

use std::{
    sync::{Arc, Condvar, Mutex},
    thread,
};

use poppler::FindFlags;

use super::{PdfSource, open_document, poppler_lock};

/// A match, in points from the top-left corner of its page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitRect {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

#[derive(Debug)]
pub enum SearchEvent {
    /// Hits on one page (never empty).
    Page {
        id: u64,
        page: usize,
        hits: Vec<HitRect>,
    },
    /// Every page has been searched.
    Done { id: u64 },
}

struct Query {
    id: u64,
    text: String,
    start_page: usize,
}

#[derive(Default)]
struct State {
    query: Option<Query>,
    /// Id of the newest request; older searches stop when they notice it changed.
    latest: u64,
    shutdown: bool,
}

type Shared = Arc<(Mutex<State>, Condvar)>;

pub struct SearchWorker {
    shared: Shared,
}

impl SearchWorker {
    /// `on_event` is called on the worker thread.
    pub fn new(source: &PdfSource, on_event: impl Fn(SearchEvent) + Send + 'static) -> Self {
        let shared = Shared::default();
        let worker = Arc::clone(&shared);
        let source = source.clone();
        thread::spawn(move || {
            // Poppler documents are not `Send`, so the thread opens its own.
            let doc = {
                let _poppler = poppler_lock();
                open_document(&source)
            };
            let Ok(doc) = doc else { return };
            let n_pages = doc.n_pages().max(0) as usize;
            loop {
                let query = {
                    let (lock, condvar) = &*worker;
                    let mut state = lock.lock().unwrap();
                    loop {
                        if state.shutdown {
                            return;
                        }
                        if let Some(query) = state.query.take() {
                            break query;
                        }
                        state = condvar.wait(state).unwrap();
                    }
                };

                // Start at the page being viewed, then wrap around.
                let start = query.start_page.min(n_pages.saturating_sub(1));
                let mut finished = true;
                for page_index in (start..n_pages).chain(0..start) {
                    if worker.0.lock().unwrap().latest != query.id {
                        finished = false;
                        break;
                    }
                    let _poppler = poppler_lock();
                    let Some(page) = doc.page(page_index as i32) else {
                        continue;
                    };
                    let height = page.size().1;
                    let hits: Vec<HitRect> = page
                        .find_text_with_options(
                            &query.text,
                            FindFlags::DEFAULT | FindFlags::IGNORE_DIACRITICS,
                        )
                        .iter()
                        // Poppler measures from the bottom of the page.
                        .map(|r| HitRect {
                            x1: r.x1(),
                            y1: height - r.y2(),
                            x2: r.x2(),
                            y2: height - r.y1(),
                        })
                        .collect();
                    if !hits.is_empty() {
                        on_event(SearchEvent::Page {
                            id: query.id,
                            page: page_index,
                            hits,
                        });
                    }
                }
                if finished {
                    on_event(SearchEvent::Done { id: query.id });
                }
            }
        });
        Self { shared }
    }

    /// Abandon any running search and look for `text`, beginning at `start_page`.
    pub fn start(&self, id: u64, text: String, start_page: usize) {
        let (lock, condvar) = &*self.shared;
        let mut state = lock.lock().unwrap();
        state.latest = id;
        state.query = Some(Query {
            id,
            text,
            start_page,
        });
        condvar.notify_all();
    }

    /// Abandon any running search.
    pub fn stop(&self, id: u64) {
        let (lock, _) = &*self.shared;
        let mut state = lock.lock().unwrap();
        state.latest = id;
        state.query = None;
    }
}

impl Drop for SearchWorker {
    fn drop(&mut self) {
        let (lock, condvar) = &*self.shared;
        let mut state = lock.lock().unwrap();
        state.shutdown = true;
        condvar.notify_all();
    }
}
