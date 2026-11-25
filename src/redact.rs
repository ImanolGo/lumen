//! Taking removed pages out of a PDF for good: whatever else in the document still leads to them (the
//! outline, links, named destinations, form fields, the structure tree) would otherwise keep their
//! content in the file, where anyone could get it back.

use std::collections::HashSet;

use lopdf::{Dictionary, Document, Object, ObjectId};

/// Trees (outlines, names, fields, structure) deeper than this are cut off.
const MAX_DEPTH: usize = 200;

/// Removes `pages` from `doc`, and everything that leads to them. They must already be gone from the
/// page tree; `kept` are the pages that stay.
pub fn remove_pages(doc: &mut Document, pages: &HashSet<ObjectId>, kept: &[ObjectId]) {
    if pages.is_empty() {
        return;
    }
    // What is drawn on the removed pages only.
    let staying: HashSet<ObjectId> = kept.iter().flat_map(|p| annots(doc, *p)).collect();
    let mut gone = pages.clone();
    gone.extend(pages.iter().flat_map(|p| annots(doc, *p)).filter(|a| !staying.contains(a)));
    for id in &gone {
        doc.objects.remove(id);
    }

    let Some(root) = doc.trailer.get(b"Root").and_then(Object::as_reference).ok() else { return };
    let mut cleaner = Cleaner { doc, gone, dead_names: HashSet::new(), seen: HashSet::new() };
    cleaner.named_destinations(root);
    cleaner.catalog(root);
    for page in kept {
        cleaner.links(*page);
    }
    cleaner.fields(root);
    cleaner.structure(root);
}

fn annots(doc: &Document, page: ObjectId) -> Vec<ObjectId> {
    let Ok(dict) = doc.get_dictionary(page) else { return Vec::new() };
    list(doc, dict, b"Annots").iter().filter_map(|o| o.as_reference().ok()).collect()
}

/// The array under `key`, whether it is written in place or referred to.
fn list(doc: &Document, dict: &Dictionary, key: &[u8]) -> Vec<Object> {
    match dict.get(key).map(|o| resolved(doc, o)) {
        Ok(Object::Array(a)) => a.clone(),
        _ => Vec::new(),
    }
}

fn resolved<'a>(doc: &'a Document, object: &'a Object) -> &'a Object {
    doc.dereference(object).map_or(object, |(_, o)| o)
}

fn name_of(object: &Object) -> Option<&[u8]> {
    match object {
        Object::Name(n) => Some(n),
        Object::String(s, _) => Some(s),
        _ => None,
    }
}

struct Cleaner<'a> {
    doc: &'a mut Document,
    /// Objects that were removed.
    gone: HashSet<ObjectId>,
    /// Destinations that led to a removed page.
    dead_names: HashSet<Vec<u8>>,
    seen: HashSet<ObjectId>,
}

impl Cleaner<'_> {
    fn dict(&self, object: &Object) -> Option<Dictionary> {
        resolved(self.doc, object).as_dict().ok().cloned()
    }

    fn get(&self, id: ObjectId) -> Option<Dictionary> {
        self.doc.get_dictionary(id).ok().cloned()
    }

    fn put(&mut self, id: ObjectId, dict: Dictionary) {
        self.doc.objects.insert(id, Object::Dictionary(dict));
    }

    /// Stores a changed value where `dict[key]` keeps it: in place, or in the object it refers to.
    fn store(&mut self, dict: &mut Dictionary, key: &[u8], value: Object) {
        match dict.get(key) {
            Ok(Object::Reference(id)) => {
                self.doc.objects.insert(*id, value);
            }
            _ => dict.set(key, value),
        }
    }

    fn is_gone(&self, object: &Object) -> bool {
        object.as_reference().is_ok_and(|id| self.gone.contains(&id))
    }

    /// Whether a destination (or a name for one) leads to a removed page.
    fn leads_away(&self, dest: &Object) -> bool {
        match resolved(self.doc, dest) {
            Object::Array(a) => a.first().is_some_and(|page| self.is_gone(page)),
            Object::Dictionary(d) => d.get(b"D").is_ok_and(|d| self.leads_away_shallow(d)),
            other => name_of(other).is_some_and(|n| self.dead_names.contains(n)),
        }
    }

    fn leads_away_shallow(&self, dest: &Object) -> bool {
        match resolved(self.doc, dest) {
            Object::Array(a) => a.first().is_some_and(|page| self.is_gone(page)),
            other => name_of(other).is_some_and(|n| self.dead_names.contains(n)),
        }
    }

    /// Whether a link or an outline entry leads to a removed page.
    fn points_away(&self, dict: &Dictionary) -> bool {
        if dict.get(b"Dest").is_ok_and(|d| self.leads_away(d)) {
            return true;
        }
        let Some(action) = dict.get(b"A").ok().and_then(|a| self.dict(a)) else { return false };
        // Only jumps within this document: a page number of another file is not a page of this one.
        action.get(b"S").and_then(Object::as_name).is_ok_and(|s| s == b"GoTo")
            && (action.get(b"D").is_ok_and(|d| self.leads_away(d)) || action.get(b"SD").is_ok_and(|d| self.is_gone(d)))
    }

    fn named_destinations(&mut self, root: ObjectId) {
        let Some(mut catalog) = self.get(root) else { return };
        // The old way: a dictionary.
        if let Some(mut dests) = catalog.get(b"Dests").ok().and_then(|d| self.dict(d)) {
            let dead: Vec<Vec<u8>> = dests.iter().filter(|(_, v)| self.leads_away(v)).map(|(k, _)| k.clone()).collect();
            for name in dead {
                dests.remove(&name);
                self.dead_names.insert(name);
            }
            self.store(&mut catalog, b"Dests", Object::Dictionary(dests));
            self.put(root, catalog.clone());
        }
        // The new way: a tree of names.
        if let Some(mut names) = catalog.get(b"Names").ok().and_then(|n| self.dict(n)) {
            if let Ok(tree) = names.get(b"Dests").cloned() {
                let tree = self.name_tree(tree, 0);
                names.set("Dests", tree);
            }
            self.store(&mut catalog, b"Names", Object::Dictionary(names));
            self.put(root, catalog);
        }
    }

    fn name_tree(&mut self, node: Object, depth: usize) -> Object {
        let Some(mut dict) = self.dict(&node) else { return node };
        if depth > MAX_DEPTH {
            return Object::Null;
        }
        if dict.has(b"Names") {
            let mut kept = Vec::new();
            for pair in list(self.doc, &dict, b"Names").as_chunks::<2>().0 {
                if self.leads_away(&pair[1]) {
                    if let Some(name) = name_of(resolved(self.doc, &pair[0])) {
                        self.dead_names.insert(name.to_vec());
                    }
                } else {
                    kept.extend_from_slice(pair);
                }
            }
            dict.set("Names", kept);
        }
        if dict.has(b"Kids") {
            let mut kids = Vec::new();
            for kid in list(self.doc, &dict, b"Kids") {
                let fresh = kid.as_reference().map_or(true, |id| self.seen.insert(id));
                if fresh {
                    kids.push(self.name_tree(kid, depth + 1));
                }
            }
            dict.set("Kids", kids);
        }
        match node {
            Object::Reference(id) => {
                self.put(id, dict);
                node
            }
            _ => Object::Dictionary(dict),
        }
    }

    /// What the catalog itself says: where to open, article threads, the outline.
    fn catalog(&mut self, root: ObjectId) {
        let Some(mut catalog) = self.get(root) else { return };
        let opens_away = catalog
            .get(b"OpenAction")
            .is_ok_and(|o| self.leads_away(o) || self.dict(o).is_some_and(|d| self.points_away(&d)));
        if opens_away {
            catalog.remove(b"OpenAction");
        }
        // Threads run through pages by way of beads that know their page; they are rare, and go whole.
        catalog.remove(b"Threads");
        if let Ok(outline) = catalog.get(b"Outlines").and_then(Object::as_reference)
            && self.outline(outline, 0).is_none()
        {
            catalog.remove(b"Outlines");
        }
        self.put(root, catalog);
    }

    /// Takes the dead entries out of the outline under `parent`; how many entries are left to
    /// show, or `None` if there are none. An entry whose own target is gone stays (without it) only to hold others.
    fn outline(&mut self, parent: ObjectId, depth: usize) -> Option<usize> {
        let mut dict = self.get(parent)?;
        let mut kept: Vec<ObjectId> = Vec::new();
        let mut next = dict.get(b"First").and_then(Object::as_reference).ok();
        let mut shown = 0;
        while let Some(id) = next {
            let Some(mut item) = self.get(id) else { break };
            if depth > MAX_DEPTH || !self.seen.insert(id) {
                break;
            }
            next = item.get(b"Next").and_then(Object::as_reference).ok();
            let below = if item.has(b"First") { self.outline(id, depth + 1) } else { None };
            // `outline` wrote the entry's own children.
            item = self.get(id).unwrap_or(item);
            let away = self.points_away(&item);
            if away {
                item.remove(b"Dest");
                item.remove(b"A");
            }
            // Where the entry came from in the structure tree.
            item.remove(b"SE");
            if away && below.is_none() {
                self.doc.objects.remove(&id);
                continue;
            }
            self.put(id, item);
            kept.push(id);
            shown += 1 + below.unwrap_or(0);
        }
        for (i, id) in kept.iter().enumerate() {
            let Some(mut item) = self.get(*id) else { continue };
            for (key, neighbour) in [(&b"Prev"[..], i.checked_sub(1)), (&b"Next"[..], Some(i + 1))] {
                match neighbour.and_then(|n| kept.get(n)) {
                    Some(n) => item.set(key, Object::Reference(*n)),
                    None => drop(item.remove(key)),
                }
            }
            self.put(*id, item);
        }
        match (kept.first(), kept.last()) {
            (Some(first), Some(last)) => {
                dict.set("First", Object::Reference(*first));
                dict.set("Last", Object::Reference(*last));
                // A negative count means closed.
                let closed = dict.get(b"Count").and_then(Object::as_i64).is_ok_and(|n| n < 0);
                dict.set("Count", Object::Integer(if closed { -(shown as i64) } else { shown as i64 }));
            }
            _ => {
                for key in [&b"First"[..], b"Last", b"Count"] {
                    dict.remove(key);
                }
            }
        }
        self.put(parent, dict);
        (shown > 0).then_some(shown)
    }

    /// The links on a page that stays.
    fn links(&mut self, page: ObjectId) {
        let Some(mut dict) = self.get(page) else { return };
        if !dict.has(b"Annots") {
            return;
        }
        let mut kept = Vec::new();
        for annot in list(self.doc, &dict, b"Annots") {
            let Some(d) = self.dict(&annot) else { continue };
            let link = d.get(b"Subtype").and_then(Object::as_name).is_ok_and(|s| s == b"Link");
            let away = self.points_away(&d);
            if link && away {
                if let Ok(id) = annot.as_reference() {
                    self.doc.objects.remove(&id);
                    self.gone.insert(id);
                }
                continue;
            }
            if away && let Ok(id) = annot.as_reference() {
                // A button that jumps there: the button stays.
                let mut d = d;
                d.remove(b"A");
                self.put(id, d);
            }
            kept.push(annot);
        }
        self.store(&mut dict, b"Annots", Object::Array(kept));
        self.put(page, dict);
    }

    /// Form fields that were only on removed pages.
    fn fields(&mut self, root: ObjectId) {
        let Some(mut catalog) = self.get(root) else { return };
        let Some(mut form) = catalog.get(b"AcroForm").ok().and_then(|f| self.dict(f)) else { return };
        self.seen.clear();
        for key in [&b"Fields"[..], b"CO"] {
            if form.has(key) {
                let kept = list(self.doc, &form, key).into_iter().filter(|f| self.field(f, 0)).collect();
                self.store(&mut form, key, Object::Array(kept));
            }
            self.seen.clear();
        }
        self.store(&mut catalog, b"AcroForm", Object::Dictionary(form));
        self.put(root, catalog);
    }

    /// Whether anything is left of a field.
    fn field(&mut self, field: &Object, depth: usize) -> bool {
        let Ok(id) = field.as_reference() else { return true };
        if self.gone.contains(&id) || depth > MAX_DEPTH || !self.seen.insert(id) {
            return false;
        }
        let Some(mut dict) = self.get(id) else { return false };
        if !dict.has(b"Kids") {
            return true;
        }
        let before = list(self.doc, &dict, b"Kids");
        let count = before.len();
        let kept: Vec<Object> = before.into_iter().filter(|k| self.field(k, depth + 1)).collect();
        if count > 0 && kept.is_empty() {
            self.doc.objects.remove(&id);
            self.gone.insert(id);
            return false;
        }
        self.store(&mut dict, b"Kids", Object::Array(kept));
        self.put(id, dict);
        true
    }

    /// The structure tree: what was tagged on removed pages (which may hold their text).
    fn structure(&mut self, root: ObjectId) {
        let Some(catalog) = self.get(root) else { return };
        let Ok(tree) = catalog.get(b"StructTreeRoot").and_then(Object::as_reference) else { return };
        let Some(mut dict) = self.get(tree) else { return };
        self.seen.clear();
        if dict.has(b"K") {
            let (kids, _) = self.element_kids(&dict, None, 0);
            dict.set("K", kids);
        }
        self.put(tree, dict);
    }

    /// The children of a structure element that stay, and whether any went.
    fn element_kids(&mut self, dict: &Dictionary, page: Option<ObjectId>, depth: usize) -> (Vec<Object>, bool) {
        let kids = match dict.get(b"K") {
            Ok(k) => match resolved(self.doc, k) {
                Object::Array(a) => a.clone(),
                _ => vec![k.clone()],
            },
            Err(_) => Vec::new(),
        };
        let on_gone_page = page.is_some_and(|p| self.gone.contains(&p));
        let (mut kept, mut lost) = (Vec::new(), false);
        for kid in kids {
            let stays = match resolved(self.doc, &kid) {
                Object::Integer(_) => !on_gone_page,
                Object::Dictionary(d) => {
                    let d = d.clone();
                    let page = d.get(b"Pg").and_then(Object::as_reference).ok().or(page);
                    match d.get(b"Type").and_then(Object::as_name) {
                        // Marked content, or an annotation.
                        Ok(b"MCR") => !page.is_some_and(|p| self.gone.contains(&p)),
                        Ok(b"OBJR") => {
                            !page.is_some_and(|p| self.gone.contains(&p))
                                && !d.get(b"Obj").is_ok_and(|o| self.is_gone(o))
                        }
                        _ => match kid.as_reference() {
                            Ok(id) => {
                                let (stays, partly) = self.element(id, d, page, depth + 1);
                                lost |= partly;
                                stays
                            }
                            Err(_) => !page.is_some_and(|p| self.gone.contains(&p)),
                        },
                    }
                }
                _ => false,
            };
            if stays {
                kept.push(kid);
            } else {
                lost = true;
            }
        }
        (kept, lost)
    }

    /// Whether a structure element stays, and whether something under it went.
    fn element(&mut self, id: ObjectId, mut dict: Dictionary, page: Option<ObjectId>, depth: usize) -> (bool, bool) {
        if depth > MAX_DEPTH || !self.seen.insert(id) {
            self.doc.objects.remove(&id);
            return (false, true);
        }
        let had = dict.has(b"K");
        let (kids, lost) = self.element_kids(&dict, page, depth);
        let on_gone_page = page.is_some_and(|p| self.gone.contains(&p));
        if (had && kids.is_empty() && lost) || (!had && on_gone_page) {
            self.doc.objects.remove(&id);
            return (false, true);
        }
        if had {
            dict.set("K", kids);
        }
        if on_gone_page {
            dict.remove(b"Pg");
        }
        if lost {
            // Text given for the whole element, some of which is no longer there.
            for key in [&b"ActualText"[..], b"Alt", b"E", b"T"] {
                dict.remove(key);
            }
        }
        self.put(id, dict);
        (true, lost)
    }
}
