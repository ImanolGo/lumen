//! Memory-bounded cache of rendered pages.

use std::{collections::HashMap, ops::RangeInclusive};

use relm4::gtk::{gdk, prelude::*};

struct Entry {
    scale: f64,
    bytes: usize,
    last_used: u64,
}

/// Remembers at which scale each page was rendered and how much memory it
/// takes. The textures themselves live in the canvas; `insert` reports which
/// pages to drop from it.
pub struct PageCache {
    budget: usize,
    used: usize,
    clock: u64,
    entries: HashMap<usize, Entry>,
}

impl PageCache {
    pub fn new(budget: usize) -> Self {
        Self {
            budget,
            used: 0,
            clock: 0,
            entries: HashMap::new(),
        }
    }

    /// The scale `page` was rendered at, if it is cached.
    pub fn scale(&self, page: usize) -> Option<f64> {
        self.entries.get(&page).map(|e| e.scale)
    }

    /// Mark `page` as just used so it is evicted last.
    pub fn touch(&mut self, page: usize) {
        self.clock += 1;
        if let Some(entry) = self.entries.get_mut(&page) {
            entry.last_used = self.clock;
        }
    }

    /// Add (or replace) a page and return the pages evicted to stay within
    /// the budget. Pages inside `protect` are never evicted, even if that
    /// means exceeding the budget.
    pub fn insert(
        &mut self,
        page: usize,
        texture: &gdk::Texture,
        scale: f64,
        protect: RangeInclusive<usize>,
    ) -> Vec<usize> {
        let bytes = texture.width() as usize * texture.height() as usize * 4;
        self.clock += 1;
        if let Some(old) = self.entries.insert(
            page,
            Entry {
                scale,
                bytes,
                last_used: self.clock,
            },
        ) {
            self.used -= old.bytes;
        }
        self.used += bytes;
        self.evict(page, protect)
    }

    fn evict(&mut self, keep: usize, protect: RangeInclusive<usize>) -> Vec<usize> {
        let mut evicted = Vec::new();
        while self.used > self.budget {
            let victim = self
                .entries
                .iter()
                .filter(|(p, _)| **p != keep && !protect.contains(*p))
                .min_by_key(|(_, e)| e.last_used)
                .map(|(&p, _)| p);
            let Some(victim) = victim else { break };
            if let Some(entry) = self.entries.remove(&victim) {
                self.used -= entry.bytes;
            }
            evicted.push(victim);
        }
        evicted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texture(side: i32) -> gdk::Texture {
        let bytes = relm4::gtk::glib::Bytes::from_owned(vec![0u8; (side * side * 4) as usize]);
        gdk::MemoryTexture::new(
            side,
            side,
            gdk::MemoryFormat::R8g8b8a8,
            &bytes,
            (side * 4) as usize,
        )
        .upcast()
    }

    #[test]
    fn evicts_least_recently_used_first() {
        // Each 10×10 texture is 400 bytes; the budget holds two.
        let mut cache = PageCache::new(800);
        assert!(cache.insert(0, &texture(10), 1.0, 0..=0).is_empty());
        assert!(cache.insert(1, &texture(10), 1.0, 1..=1).is_empty());
        cache.touch(0);
        assert_eq!(cache.insert(2, &texture(10), 1.0, 2..=2), vec![1]);
        assert_eq!(cache.scale(1), None);
        assert_eq!(cache.scale(0), Some(1.0));
    }

    #[test]
    fn protected_pages_survive_even_over_budget() {
        let mut cache = PageCache::new(400);
        cache.insert(0, &texture(10), 1.0, 0..=1);
        let evicted = cache.insert(1, &texture(10), 1.0, 0..=1);
        assert!(evicted.is_empty());
        assert!(cache.scale(0).is_some() && cache.scale(1).is_some());
    }

    #[test]
    fn replacing_a_page_does_not_leak_budget() {
        let mut cache = PageCache::new(500);
        for _ in 0..10 {
            assert!(cache.insert(0, &texture(10), 2.0, 0..=0).is_empty());
        }
        assert_eq!(cache.scale(0), Some(2.0));
    }
}
