//! Playing an animated GIF, APNG or WebP: frames arrive from the decoder thread, one per
//! tick of their own delay. The first pass is kept (while it fits in memory) so later
//! passes replay from memory; a longer animation keeps streaming from the file.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use iced::widget::image;
use iced::{Subscription, Task};

use super::{App, Doc, Message, tr};
use crate::animation::{self, CACHE_BYTES, Item};

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
    /// The frame on screen: the current one once it is on the GPU.
    shown: Option<(image::Handle, Option<super::Gpu>)>,
    /// Its frames are too large to play.
    too_big: bool,
    /// The next frame was not there in time.
    waiting: bool,
    /// Passes played, how many there should be, and whether that is done.
    passes: u32,
    plays: Option<u32>,
    finished: bool,
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
            shown: None,
            too_big: false,
            waiting: false,
            passes: 0,
            plays: None,
            finished: false,
        })
    }

    /// The frame to show, if one has arrived.
    pub fn frame(&self) -> Option<&image::Handle> {
        self.shown.as_ref().map(|(h, _)| h)
    }

    /// How long until the next look; `None` once it has been played as often as it asks.
    pub fn period(&self) -> Option<Duration> {
        if self.finished {
            return None;
        }
        Some(match &self.current {
            Some((_, d)) if !self.waiting => (*d).clamp(Duration::from_millis(20), Duration::from_secs(10)),
            _ => POLL,
        })
    }

    /// One pass is over. Whether another one follows.
    fn again(&mut self, plays: Option<u32>) -> bool {
        self.passes = self.passes.saturating_add(1);
        self.plays = plays;
        self.finished = plays.is_some_and(|n| self.passes >= n);
        if self.finished {
            self.rx = None;
        }
        !self.finished
    }

    /// Goes on to the next frame. Returns false when the picture turns out not to be animated.
    pub fn advance(&mut self) -> bool {
        if self.finished {
            return true;
        }
        let Some(rx) = &self.rx else {
            // Replaying from memory.
            if self.cache.is_empty() {
                return false;
            }
            if self.index + 1 == self.cache.len() && !self.again(self.plays) {
                return true;
            }
            self.index = (self.index + 1) % self.cache.len();
            self.current = Some(self.cache[self.index].clone());
            return true;
        };
        match rx.try_recv() {
            Ok(Item::Frame(picture, delay)) => {
                let handle = image::Handle::from_rgba(picture.width, picture.height, picture.rgba);
                self.waiting = false;
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
            Ok(Item::LoopEnd(plays)) => {
                if self.seen <= 1 {
                    return false;
                }
                self.seen = 0;
                if !self.again(plays) {
                    return true;
                }
                if self.caching {
                    // The whole pass is in memory: the decoder can rest.
                    self.rx = None;
                    self.index = 0;
                    self.current = Some(self.cache[0].clone());
                    return true;
                }
                // The last frame has had its time: straight on to the first.
                self.advance()
            }
            Ok(Item::TooBig) => {
                self.too_big = true;
                false
            }
            Err(TryRecvError::Empty) => {
                // It is late: look again soon rather than after this frame's whole time.
                self.waiting = true;
                true
            }
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

    pub(super) fn anim_tick(&mut self, _now: Instant) -> Task<Message> {
        if !self.anim_visible() {
            return Task::none();
        }
        let Doc::Image(view) = &mut self.doc else { return Task::none() };
        if view.anim.as_mut().is_some_and(|a| !a.advance()) {
            let too_big = view.anim.take().is_some_and(|a| a.too_big);
            if too_big {
                return self.notice(tr("This animation is too large to play: only its first frame is shown"));
            }
        }
        let Doc::Image(view) = &self.doc else { return Task::none() };
        let Some(anim) = &view.anim else { return Task::none() };
        let Some((handle, _)) = anim.current.clone() else { return Task::none() };
        if anim.shown.as_ref().is_some_and(|(h, _)| h.id() == handle.id()) {
            return Task::none();
        }
        let generation = self.generation;
        self.uploaded(handle.clone(), move |gpu| Message::FrameReady(generation, handle, gpu))
    }

    pub(super) fn frame_ready(&mut self, generation: u64, handle: image::Handle, gpu: Option<super::Gpu>) {
        if generation == self.generation
            && let Doc::Image(view) = &mut self.doc
            && let Some(anim) = &mut view.anim
        {
            anim.shown = Some((handle, gpu));
        }
    }

    pub(super) fn anim_subscription(&self) -> Subscription<Message> {
        match &self.doc {
            Doc::Image(view) if self.anim_visible() => match view.anim.as_ref().and_then(Anim::period) {
                Some(period) => iced::time::every(period).map(Message::AnimTick),
                None => Subscription::none(),
            },
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
        Anim {
            rx: Some(rx),
            cache: Vec::new(),
            caching: true,
            cache_bytes: 0,
            seen: 0,
            index: 0,
            current: None,
            shown: None,
            too_big: false,
            waiting: false,
            passes: 0,
            plays: None,
            finished: false,
        }
    }

    fn shown(a: &Anim) -> u128 {
        a.period().unwrap().as_millis()
    }

    #[test]
    fn first_pass_is_cached_then_replayed_from_memory() {
        let mut a = anim(vec![frame(40), frame(50), frame(60), Item::LoopEnd(None)]);
        let mut seen = Vec::new();
        // Every frame once each time round: the last one is not shown twice as long.
        for _ in 0..8 {
            assert!(a.advance());
            seen.push(shown(&a));
        }
        assert!(a.rx.is_none(), "the decoder is let go");
        assert_eq!(seen, [40, 50, 60, 40, 50, 60, 40, 50]);
    }

    #[test]
    fn it_plays_as_often_as_it_asks() {
        let mut a = anim(vec![frame(40), frame(50), Item::LoopEnd(Some(2))]);
        let mut seen = Vec::new();
        while a.period().is_some() {
            assert!(a.advance());
            seen.push(a.period().map(|p| p.as_millis()));
        }
        // The last tick only ends it, with the last frame left on screen.
        assert_eq!(seen, [Some(40), Some(50), Some(40), Some(50), None]);
        assert!(a.advance() && a.current.is_some());
        let mut once = anim(vec![frame(40), frame(50), Item::LoopEnd(Some(1)), frame(40)]);
        for _ in 0..5 {
            assert!(once.advance());
        }
        assert_eq!(once.period(), None);
    }

    #[test]
    fn a_picture_with_one_frame_is_not_an_animation() {
        let mut a = anim(vec![frame(100), Item::LoopEnd(None)]);
        assert!(a.advance());
        assert!(!a.advance());
    }

    #[test]
    fn an_animation_that_is_too_large_is_given_up_and_said_so() {
        let mut a = anim(vec![Item::TooBig]);
        assert!(!a.advance());
        assert!(a.too_big);
    }

    #[test]
    fn waiting_for_the_decoder_keeps_the_current_frame() {
        let mut a = anim(vec![frame(80)]);
        assert!(a.advance());
        assert!(a.advance()); // nothing new yet
        assert_eq!(a.period(), Some(POLL), "looked for again soon");
        assert!(a.current.is_some());
    }

    #[test]
    fn too_long_animations_keep_streaming() {
        let mut a = anim(vec![frame(40), frame(40), Item::LoopEnd(None), frame(40), frame(40)]);
        a.cache_bytes = CACHE_BYTES; // already at the limit: the next frame tips it over
        assert!(a.advance());
        assert!(!a.caching && a.cache.is_empty());
        assert!(a.advance());
        assert!(a.advance()); // the end of the pass, and straight on: still streaming
        assert!(a.rx.is_some());
        assert_eq!(a.seen, 1);
    }
}
