//! The slideshow: fullscreen, one picture (or PDF page) after another every few seconds.
//! F5 starts and stops it, Space pauses, Esc leaves; it stops after the last file.

use std::time::{Duration, Instant};

use iced::{Subscription, Task};

use super::{App, Doc, Message};
use crate::i18n::tr;

/// How often the clock is looked at.
const TICK: Duration = Duration::from_millis(250);

/// A slideshow that is running.
pub struct Slideshow {
    pub paused: bool,
    /// Whether the window was already fullscreen, so leaving puts it back as it was.
    was_fullscreen: bool,
    /// What was on screen at the last look, and since when.
    shown: Option<((usize, usize), Instant)>,
    seconds: u64,
}

/// What comes after what is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    NextPage,
    NextFile,
    End,
}

/// `page` is (current, count) for a PDF; `index` is the file's place among `files`.
pub fn next_step(page: Option<(usize, usize)>, index: usize, files: usize) -> Step {
    match page {
        Some((current, count)) if current + 1 < count => Step::NextPage,
        _ if index + 1 < files => Step::NextFile,
        _ => Step::End,
    }
}

impl App {
    /// Whether there is anything to step through.
    pub(super) fn can_slideshow(&self) -> bool {
        let pages = matches!(&self.doc, Doc::Pdf(v) if v.doc.page_count() > 1);
        self.unsaved_files() == 0 && matches!(self.doc, Doc::Image(_) | Doc::Pdf(_)) && (self.files.len() > 1 || pages)
    }

    pub(super) fn slideshow_toggle(&mut self) -> Task<Message> {
        if self.slideshow.is_some() {
            return self.slideshow_stop();
        }
        self.menu = None;
        if !self.can_slideshow() {
            return self.toast(tr("A slideshow needs more than one picture or page, and no unsaved changes"));
        }
        self.slideshow = Some(Slideshow {
            paused: false,
            was_fullscreen: self.fullscreen,
            shown: None,
            seconds: u64::from(self.prefs.slideshow_seconds),
        });
        let fullscreen = if self.fullscreen { Task::none() } else { self.set_fullscreen(true) };
        let hint = self.notice(tr("Slideshow: Space pauses, Esc leaves"));
        Task::batch([fullscreen, hint])
    }

    pub(super) fn slideshow_stop(&mut self) -> Task<Message> {
        let Some(show) = self.slideshow.take() else { return Task::none() };
        if show.was_fullscreen { Task::none() } else { self.set_fullscreen(false) }
    }

    pub(super) fn slideshow_pause(&mut self) -> Task<Message> {
        let Some(show) = &mut self.slideshow else { return Task::none() };
        show.paused = !show.paused;
        // Counting starts afresh when it goes on.
        show.shown = None;
        let paused = show.paused;
        self.notice(if paused { tr("Slideshow paused") } else { tr("Slideshow resumed") })
    }

    /// Looks at the clock: when the same thing has been up long enough, goes on.
    pub(super) fn slideshow_tick(&mut self, now: Instant) -> Task<Message> {
        if self.unsaved_files() > 0 {
            return self.slideshow_stop();
        }
        let key = match &self.doc {
            Doc::Image(_) => (self.current, 0),
            Doc::Pdf(v) => (self.current, v.current_page),
            // Still loading (or failed): wait, counting from when something is there.
            _ => {
                if let Some(show) = &mut self.slideshow {
                    show.shown = None;
                }
                return Task::none();
            }
        };
        let Some(show) = &mut self.slideshow else { return Task::none() };
        if show.paused {
            return Task::none();
        }
        match show.shown {
            Some((seen, since)) if seen == key => {
                if now.saturating_duration_since(since) < Duration::from_secs(show.seconds) {
                    return Task::none();
                }
            }
            // Something new is up (by itself or by hand): start counting.
            _ => {
                show.shown = Some((key, now));
                return Task::none();
            }
        }
        show.shown = None;
        let page = match &self.doc {
            Doc::Pdf(v) => Some((v.current_page, v.doc.page_count())),
            _ => None,
        };
        match next_step(page, self.current, self.files.len()) {
            Step::NextPage => self.go_to_page(key.1 + 1),
            Step::NextFile => self.show(self.current + 1),
            Step::End => {
                let done = self.slideshow_stop();
                Task::batch([done, self.notice(tr("End of the slideshow"))])
            }
        }
    }

    pub(super) fn slideshow_subscription(&self) -> Subscription<Message> {
        match &self.slideshow {
            Some(show) if !show.paused => iced::time::every(TICK).map(Message::SlideshowTick),
            _ => Subscription::none(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_through_pages_then_files_then_ends() {
        assert_eq!(next_step(Some((0, 3)), 0, 2), Step::NextPage);
        assert_eq!(next_step(Some((2, 3)), 0, 2), Step::NextFile);
        assert_eq!(next_step(Some((2, 3)), 1, 2), Step::End);
        assert_eq!(next_step(None, 0, 3), Step::NextFile);
        assert_eq!(next_step(None, 2, 3), Step::End);
        // A one-page PDF in a folder just moves on.
        assert_eq!(next_step(Some((0, 1)), 0, 2), Step::NextFile);
        // A single multi-page PDF steps through its pages, then ends.
        assert_eq!(next_step(Some((0, 2)), 0, 1), Step::NextPage);
        assert_eq!(next_step(Some((1, 2)), 0, 1), Step::End);
    }
}
