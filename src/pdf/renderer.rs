//! A background thread that owns the Poppler document and renders pages on request.

use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
    thread,
};

use anyhow::Result;

use super::{PdfSource, RenderedPage, open_document, poppler_lock, render_page};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderJob {
    pub page: usize,
    pub scale: f64,
}

#[derive(Default)]
struct State {
    jobs: VecDeque<RenderJob>,
    in_flight: Option<RenderJob>,
    shutdown: bool,
}

type Shared = Arc<(Mutex<State>, Condvar)>;

pub struct PageRenderer {
    shared: Shared,
}

impl PageRenderer {
    /// `on_done` is called on the worker thread after every job.
    pub fn new(
        source: &PdfSource,
        on_done: impl Fn(RenderJob, Result<RenderedPage, String>) + Send + 'static,
    ) -> Self {
        let shared = Shared::default();
        let worker = Arc::clone(&shared);
        let source = source.clone();
        thread::spawn(move || {
            // Poppler documents are not `Send`, so the thread opens its own.
            let doc = {
                let _poppler = poppler_lock();
                open_document(&source).map_err(|err| format!("{err:#}"))
            };
            loop {
                let job = {
                    let (lock, condvar) = &*worker;
                    let mut state = lock.lock().unwrap();
                    loop {
                        if state.shutdown {
                            return;
                        }
                        if let Some(job) = state.jobs.pop_front() {
                            state.in_flight = Some(job);
                            break job;
                        }
                        state = condvar.wait(state).unwrap();
                    }
                };
                let result = match &doc {
                    Ok(doc) => {
                        let _poppler = poppler_lock();
                        render_page(doc, job.page, job.scale).map_err(|e| format!("{e:#}"))
                    }
                    Err(err) => Err(err.clone()),
                };
                worker.0.lock().unwrap().in_flight = None;
                on_done(job, result);
            }
        });
        Self { shared }
    }

    /// Replace everything still waiting with `jobs`, most important first.
    /// A job that is already being rendered is not queued again.
    pub fn request(&self, jobs: impl IntoIterator<Item = RenderJob>) {
        let (lock, condvar) = &*self.shared;
        let mut state = lock.lock().unwrap();
        let in_flight = state.in_flight;
        state.jobs = jobs.into_iter().filter(|j| Some(*j) != in_flight).collect();
        condvar.notify_all();
    }
}

impl Drop for PageRenderer {
    fn drop(&mut self) {
        let (lock, condvar) = &*self.shared;
        let mut state = lock.lock().unwrap();
        state.shutdown = true;
        state.jobs.clear();
        condvar.notify_all();
    }
}
