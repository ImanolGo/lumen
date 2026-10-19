//! The document outline (table of contents).
//!
//! The safe Poppler bindings cannot read an outline entry's action, so this
//! module calls the C API directly. Every pointer is checked and freed here;
//! nothing raw leaves this file. The result is plain data that can cross threads.

use std::{
    ffi::{CStr, c_char},
    sync::Arc,
};

use relm4::gtk::glib::translate::{from_glib_full, ToGlibPtr, ToGlibPtrMut};

use poppler::{Document, IndexIter, ffi};

/// Outlines nested deeper than this are cut off (guards against malformed files).
const MAX_DEPTH: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutlineTarget {
    /// 0-based page index.
    pub page: usize,
    /// Distance from the top of the page in points, when the entry names a position.
    pub y: Option<f64>,
}

#[derive(Debug)]
pub struct OutlineItem {
    pub title: String,
    /// `None` for entries that do not point into this document (links, scripts…).
    pub target: Option<OutlineTarget>,
    pub children: Vec<Arc<OutlineItem>>,
}

/// Read the whole outline. `page_sizes` converts PDF coordinates (origin at the
/// bottom) to distances from the top. Empty if the document has no outline.
pub fn read_outline(doc: &Document, page_sizes: &[(f64, f64)]) -> Vec<Arc<OutlineItem>> {
    // SAFETY: `poppler_index_iter_new` returns an owned iterator or NULL when
    // the document has no outline; the bindings' `IndexIter::new` would
    // wrap NULL, so the pointer is checked first.
    let iter = unsafe {
        let ptr = ffi::poppler_index_iter_new(doc.to_glib_none().0);
        if ptr.is_null() {
            return Vec::new();
        }
        from_glib_full::<_, IndexIter>(ptr)
    };
    read_level(iter, doc, page_sizes, 0)
}

fn read_level(
    mut iter: IndexIter,
    doc: &Document,
    page_sizes: &[(f64, f64)],
    depth: usize,
) -> Vec<Arc<OutlineItem>> {
    let mut items = Vec::new();
    loop {
        let (title, target) = read_action(&mut iter, doc, page_sizes);
        let children = match iter.child() {
            Some(child) if depth < MAX_DEPTH => read_level(child, doc, page_sizes, depth + 1),
            _ => Vec::new(),
        };
        items.push(Arc::new(OutlineItem {
            title,
            target,
            children,
        }));
        if !iter.next() {
            return items;
        }
    }
}

fn read_action(
    iter: &mut IndexIter,
    doc: &Document,
    page_sizes: &[(f64, f64)],
) -> (String, Option<OutlineTarget>) {
    // SAFETY: the action is owned by us until `poppler_action_free`. Which union
    // member is valid depends on `type_`, which is checked before reading
    // `goto_dest`; `title` is shared by all members through `any`.
    unsafe {
        let action = ffi::poppler_index_iter_get_action(iter.to_glib_none_mut().0);
        if action.is_null() {
            return (String::new(), None);
        }
        let title = c_string((*action).any.title).unwrap_or_default();
        let target = if (*action).type_ == ffi::POPPLER_ACTION_GOTO_DEST {
            resolve((*action).goto_dest.dest, doc, page_sizes)
        } else {
            None
        };
        ffi::poppler_action_free(action);
        (title, target)
    }
}

/// Turn a destination into a page and position, looking named ones up.
///
/// SAFETY: `dest` must be NULL or point to a live `PopplerDest`.
pub(super) unsafe fn resolve(
    dest: *mut ffi::PopplerDest,
    doc: &Document,
    page_sizes: &[(f64, f64)],
) -> Option<OutlineTarget> {
    unsafe {
        if dest.is_null() {
            return None;
        }
        if (*dest).type_ == ffi::POPPLER_DEST_NAMED {
            let name = c_string((*dest).named_dest)?;
            let found = doc.find_dest(&name)?;
            let ptr: *mut ffi::PopplerDest = found.to_glib_none().0;
            // A named destination that resolves to another name would loop forever.
            if ptr.is_null() || (*ptr).type_ == ffi::POPPLER_DEST_NAMED {
                return None;
            }
            return target_of(ptr, page_sizes);
        }
        target_of(dest, page_sizes)
    }
}

/// SAFETY: `dest` must point to a live, non-named `PopplerDest`.
unsafe fn target_of(dest: *const ffi::PopplerDest, page_sizes: &[(f64, f64)]) -> Option<OutlineTarget> {
    let (page_num, kind, top) = unsafe { ((*dest).page_num, (*dest).type_, (*dest).top) };
    let page = usize::try_from(page_num.checked_sub(1)?).ok()?;
    let height = page_sizes.get(page)?.1;
    let y = (kind == ffi::POPPLER_DEST_XYZ && top > 0.0).then(|| (height - top).clamp(0.0, height));
    Some(OutlineTarget { page, y })
}

/// SAFETY: `ptr` must be NULL or a NUL-terminated string.
unsafe fn c_string(ptr: *const c_char) -> Option<String> {
    (!ptr.is_null()).then(|| unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned())
}
