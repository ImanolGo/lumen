//! Clickable links on PDF pages.
//!
//! Like the outline, link actions are only reachable through Poppler's C API;
//! that access is confined to `read_target` below. A worker thread owns its own
//! document and reads the links of the pages it is asked for.

use std::{
    collections::VecDeque,
    ffi::{CStr, c_char},
    sync::{Arc, Condvar, Mutex},
    thread,
};

use relm4::gtk::glib::translate::ToGlibPtr;

use poppler::{Document, ffi};

use super::{HitRect, PdfSource, open_document, outline::resolve, poppler_lock};

/// Where a link leads.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkTarget {
    /// A place in this document; `y` is points below the top of the page.
    Page {
        page: usize,
        y: Option<f64>,
    },
    Uri(String),
    /// A viewer command such as "NextPage".
    Named(NamedAction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedAction {
    Next,
    Prev,
    First,
    Last,
}

impl NamedAction {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "NextPage" => Some(Self::Next),
            "PrevPage" => Some(Self::Prev),
            "FirstPage" => Some(Self::First),
            "LastPage" => Some(Self::Last),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Link {
    /// Clickable area, in points from the page's top-left.
    pub area: HitRect,
    pub target: LinkTarget,
}

impl Link {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        (self.area.x1..=self.area.x2).contains(&x) && (self.area.y1..=self.area.y2).contains(&y)
    }
}

#[derive(Debug)]
pub struct LinksLoaded {
    pub page: usize,
    pub links: Vec<Link>,
}

#[derive(Default)]
struct State {
    pages: VecDeque<usize>,
    shutdown: bool,
}

type Shared = Arc<(Mutex<State>, Condvar)>;

pub struct LinkWorker {
    shared: Shared,
}

impl LinkWorker {
    /// `page_sizes` converts destinations to page positions; `on_page` runs on the worker thread.
    pub fn new(
        source: &PdfSource,
        page_sizes: Vec<(f64, f64)>,
        on_page: impl Fn(LinksLoaded) + Send + 'static,
    ) -> Self {
        let shared = Shared::default();
        let worker = Arc::clone(&shared);
        let source = source.clone();
        thread::spawn(move || {
            let doc = {
                let _poppler = poppler_lock();
                open_document(&source)
            };
            let Ok(doc) = doc else { return };
            loop {
                let index = {
                    let (lock, condvar) = &*worker;
                    let mut state = lock.lock().unwrap();
                    loop {
                        if state.shutdown {
                            return;
                        }
                        if let Some(index) = state.pages.pop_front() {
                            break index;
                        }
                        state = condvar.wait(state).unwrap();
                    }
                };
                let links = {
                    let _poppler = poppler_lock();
                    read_links(&doc, index, &page_sizes)
                };
                on_page(LinksLoaded { page: index, links });
            }
        });
        Self { shared }
    }

    /// Replace the pages still waiting with `pages`.
    pub fn request(&self, pages: impl IntoIterator<Item = usize>) {
        let (lock, condvar) = &*self.shared;
        lock.lock().unwrap().pages = pages.into_iter().collect();
        condvar.notify_all();
    }
}

impl Drop for LinkWorker {
    fn drop(&mut self) {
        let (lock, condvar) = &*self.shared;
        lock.lock().unwrap().shutdown = true;
        condvar.notify_all();
    }
}

fn read_links(doc: &Document, index: usize, page_sizes: &[(f64, f64)]) -> Vec<Link> {
    let Some(page) = doc.page(index as i32) else {
        return Vec::new();
    };
    let height = page.size().1;
    page.link_mapping()
        .iter()
        .filter_map(|mapping| {
            let raw: *const ffi::PopplerLinkMapping = mapping.to_glib_none().0;
            // SAFETY: `raw` points into `mapping`, which outlives this closure body;
            // the action pointer is owned by the mapping and only read here.
            let (area, target) = unsafe {
                let area = (*raw).area;
                (area, read_target((*raw).action, doc, page_sizes)?)
            };
            Some(Link {
                // Poppler measures from the bottom of the page.
                area: HitRect {
                    x1: area.x1,
                    y1: height - area.y2,
                    x2: area.x2,
                    y2: height - area.y1,
                },
                target,
            })
        })
        .collect()
}

/// SAFETY: `action` must be NULL or point to a live `PopplerAction`.
unsafe fn read_target(
    action: *const ffi::PopplerAction,
    doc: &Document,
    page_sizes: &[(f64, f64)],
) -> Option<LinkTarget> {
    unsafe {
        if action.is_null() {
            return None;
        }
        match (*action).type_ {
            ffi::POPPLER_ACTION_GOTO_DEST => {
                let target = resolve((*action).goto_dest.dest, doc, page_sizes)?;
                Some(LinkTarget::Page {
                    page: target.page,
                    y: target.y,
                })
            }
            ffi::POPPLER_ACTION_URI => c_string((*action).uri.uri).map(LinkTarget::Uri),
            ffi::POPPLER_ACTION_NAMED => c_string((*action).named.named_dest)
                .and_then(|name| NamedAction::parse(&name))
                .map(LinkTarget::Named),
            _ => None,
        }
    }
}

/// SAFETY: `ptr` must be NULL or a NUL-terminated string.
unsafe fn c_string(ptr: *const c_char) -> Option<String> {
    (!ptr.is_null()).then(|| {
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    })
}

/// Whether Lumen opens links with this scheme. Anything else (file:, javascript:,
/// custom handlers) is ignored: a PDF should not be able to launch programs.
pub fn is_safe_uri(uri: &str) -> bool {
    let scheme = uri
        .split_once(':')
        .map(|(scheme, _)| scheme.to_ascii_lowercase());
    matches!(scheme.as_deref(), Some("http" | "https" | "mailto"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_and_mail_links_are_safe() {
        assert!(is_safe_uri("https://example.com/a?b=c"));
        assert!(is_safe_uri("HTTP://example.com"));
        assert!(is_safe_uri("mailto:someone@example.com"));
        assert!(!is_safe_uri("file:///etc/passwd"));
        assert!(!is_safe_uri("javascript:alert(1)"));
        assert!(!is_safe_uri("ssh://host"));
        assert!(!is_safe_uri("no-scheme"));
        assert!(!is_safe_uri(""));
    }

    #[test]
    fn link_hit_testing_is_inclusive() {
        let link = Link {
            area: HitRect {
                x1: 10.0,
                y1: 20.0,
                x2: 50.0,
                y2: 40.0,
            },
            target: LinkTarget::Uri("https://example.com".into()),
        };
        assert!(
            link.contains(10.0, 20.0) && link.contains(50.0, 40.0) && link.contains(30.0, 30.0)
        );
        assert!(!link.contains(9.9, 30.0) && !link.contains(30.0, 40.1));
    }

    /// `tests/fixtures/links.pdf`: three links on page 1 (text baselines at 100, 150
    /// and 200 points from the top, starting at x = 72): a web link, a jump to
    /// page 2 at 120 points from the top, and a `file:` link.
    #[test]
    fn reads_links_with_top_left_areas() {
        let path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/links.pdf");
        let _poppler = poppler_lock();
        let doc = open_document(&PdfSource::new(&path, None)).unwrap();
        let links = read_links(&doc, 0, &[(400.0, 300.0), (400.0, 300.0)]);
        assert_eq!(links.len(), 3, "got {links:?}");

        // Poppler does not return them in document order.
        let web = links
            .iter()
            .find(|l| l.target == LinkTarget::Uri("https://example.com/".into()))
            .expect("the web link");
        // Around the first baseline, measured from the top of the page.
        assert!(
            web.area.y1 > 80.0 && web.area.y2 < 110.0,
            "area {:?}",
            web.area
        );
        assert!(web.contains(100.0, 95.0) && !web.contains(100.0, 150.0));

        let jump = links
            .iter()
            .find(|l| matches!(l.target, LinkTarget::Page { .. }))
            .unwrap();
        assert_eq!(
            jump.target,
            LinkTarget::Page {
                page: 1,
                y: Some(120.0)
            }
        );
        assert!(jump.contains(100.0, 145.0));

        let local = links
            .iter()
            .find(|l| matches!(&l.target, LinkTarget::Uri(u) if u.starts_with("file:")))
            .unwrap();
        assert!(!is_safe_uri(match &local.target {
            LinkTarget::Uri(u) => u,
            _ => unreachable!(),
        }));
    }
}
