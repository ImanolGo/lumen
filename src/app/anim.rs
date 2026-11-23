//! Playing an animated GIF, APNG or WebP: frames arrive from the decoder thread, one per
//! tick of their own delay. The first pass is kept (while it fits in memory) so later
//! passes replay from memory; a longer animation keeps streaming from the file.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use iced::Subscription;
use iced::widget::image;

use super::{App, Doc, Message};
use crate::animation::{self, Item};

/// The most decoded frames kept for replaying.
const CACHE_BYTES: usize = 96 << 20;
/// Waiting for the decoder: look again this soon.
const POLL: Duration = Duration::from_millis(30);

pub struct Anim {
    rx: Option<Receiver<Item>>,
    cache: Vec<(image::Handle, Duration)>,
    caching: bool,
    cache_bytes: usize,
    /// Frames received in this pass.
    seen: usize,
    /// Which cached frame is up, when replaying.
    index: usize,
    current: Option<(image::Handle, Duration)>,
}

impl Anim {
    /// Starts decoding `path`'s frames, if its kind can be animated.
    pub fn start(path: &std::path::Path) -> Option<Self> {
        animation::may_animate(path).then(|| Self {
            rx: Some(animation::stream(path)),
            cache: Vec::new(),
            caching: true,
            cache_bytes: 0,
            seen: 0,
            index: 0,
            current: None,
        })
    }

    /// The frame to show, if one has arrived.
    pub fn frame(&self) -> Option<&image::Handle> {
        self.current.as_ref().map(|(h, _)| h)
    }

    /// How long until the next look.
    pub fn period(&self) -> Duration {
        self.current.as_ref().map_or(POLL, |(_, d)| (*d).clamp(Duration::from_millis(20), Duration::from_secs(10)))
    }

    /// Goes on to the next frame. Returns false when the picture turns out not to be animated.
    pub fn advance(&mut self) -> bool {
        let Some(rx) = &self.rx else {
            // Replaying from memory.
            if self.cache.is_empty() {
                return false;
            }
            self.index = (self.index + 1) % self.cache.len();
            self.current = Some(self.cache[self.index].clone());
            return true;
        };
        match rx.try_recv() {
            Ok(Item::Frame(picture, delay)) => {
                let handle = image::Handle::from_rgba(picture.width, picture.height, picture.rgba);
                self.seen += 1;
                if self.caching {
                    // Four bytes a pixel.
                    self.cache_bytes += picture.width as usize * picture.height as usize * 4;
                    if self.cache_bytes > CACHE_BYTES {
                        self.caching = false;
                        self.cache.clear();
                    } else {
                        self.cache.push((handle.clone(), delay));
                    }
                }
                self.current = Some((handle, delay));
                true
            }
            Ok(Item::LoopEnd) => {
                if self.seen <= 1 {
                    return false;
                }
                self.seen = 0;
                if self.caching {
                    // The whole pass is in memory: the decoder can rest.
                    self.rx = None;
                    self.index = self.cache.len().saturating_sub(1);
                }
                true
            }
            Err(TryRecvError::Empty) => true,
            Err(TryRecvError::Disconnected) => self.current.is_some(),
        }
    }
}

impl App {
    /// Whether the animation is what the picture area shows: not while editing or marking up,
    /// when the first frame is what the tools work on.
    pub(super) fn anim_visible(&self) -> bool {
        match &self.doc {
            Doc::Image(view) => {
                view.anim.is_some()
                    && !self.edit_open
                    && !self.markup_open
                    && !view.edit.is_dirty()
                    && self.crop.is_none()
            }
            _ => false,
        }
    }

    /// The frame to draw instead of the picture's first one, if the animation is showing.
    pub(super) fn anim_frame(&self) -> Option<&image::Handle> {
        match &self.doc {
            Doc::Image(view) if self.anim_visible() => view.anim.as_ref()?.frame(),
            _ => None,
        }
    }

    pub(super) fn anim_tick(&mut self, _now: Instant) {
        if !self.anim_visible() {
            return;
        }
        let Doc::Image(view) = &mut self.doc else { return };
        if view.anim.as_mut().is_some_and(|a| !a.advance()) {
            view.anim = None;
        }
    }

    pub(super) fn anim_subscription(&self) -> Subscription<Message> {
        match &self.doc {
            Doc::Image(view) if self.anim_visible() => {
                let period = view.anim.as_ref().map_or(POLL, Anim::period);
                iced::time::every(period).map(Message::AnimTick)
            }
            _ => Subscription::none(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::Picture;
    use std::sync::mpsc::sync_channel;

    fn frame(delay_ms: u64) -> Item {
        Item::Frame(
            Picture { width: 1, height: 1, original: (1, 1), rgba: vec![0, 0, 0, 255].into() },
            Duration::from_millis(delay_ms),
        )
    }

    fn anim(items: Vec<Item>) -> Anim {
        let (tx, rx) = sync_channel(16);
        for item in items {
            tx.send(item).unwrap();
        }
        // Keep the sender alive so an empty channel means "not yet", like the real thread.
        std::mem::forget(tx);
        Anim { rx: Some(rx), cache: Vec::new(), caching: true, cache_bytes: 0, seen: 0, index: 0, current: None }
    }

    #[test]
    fn first_pass_is_cached_then_replayed_from_memory() {
        let mut a = anim(vec![frame(40), frame(50), frame(60), Item::LoopEnd]);
        for want in [40, 50, 60] {
            assert!(a.advance());
            assert_eq!(a.period(), Duration::from_millis(want));
        }
        assert!(a.advance()); // the end of the pass: the decoder is let go
        assert!(a.rx.is_none());
        // Replay wraps around the cached frames.
        let mut seen = Vec::new();
        for _ in 0..4 {
            assert!(a.advance());
            seen.push(a.period().as_millis());
        }
        assert_eq!(seen, [40, 50, 60, 40]);
    }

    #[test]
    fn a_picture_with_one_frame_is_not_an_animation() {
        let mut a = anim(vec![frame(100), Item::LoopEnd]);
        assert!(a.advance());
        assert!(!a.advance());
    }

    #[test]
    fn waiting_for_the_decoder_keeps_the_current_frame() {
        let mut a = anim(vec![frame(80)]);
        assert!(a.advance());
        assert!(a.advance()); // nothing new yet
        assert_eq!(a.period(), Duration::from_millis(80));
        assert!(a.frame().is_some());
    }

    #[test]
    fn too_long_animations_keep_streaming() {
        let mut a = anim(vec![frame(40), frame(40), Item::LoopEnd, frame(40), frame(40)]);
        a.cache_bytes = CACHE_BYTES; // already at the limit: the next frame tips it over
        assert!(a.advance());
        assert!(!a.caching && a.cache.is_empty());
        assert!(a.advance());
        assert!(a.advance()); // the end of the pass: still streaming
        assert!(a.rx.is_some());
        assert!(a.advance());
        assert_eq!(a.seen, 1);
    }
}
