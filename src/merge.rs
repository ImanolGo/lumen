//! Bringing along what pages inserted from another PDF need: their form fields, the places their
//! links name, and the part of the outline that leads to them.

use std::collections::{HashMap, HashSet};

use lopdf::{Dictionary, Document, Object, ObjectId};

const MAX_DEPTH: usize = 64;

/// Adds to the document whose catalog is `root` what the catalog `from` (of another document whose
/// objects are already here) says about `pages`. What turns out to belong to pages that are not
/// used is taken out again by `redact::remove_pages`.
pub fn adopt(doc: &mut Document, root: ObjectId, from: ObjectId, pages: &[ObjectId]) {
    let Ok(source) = doc.get_dictionary(from).cloned() else { return };
    let names = named_destinations(doc, &source);
    for page in pages {
        explicit_links(doc, *page, &names);
    }
    adopt_outline(doc, root, &source, &names);
    adopt_form(doc, root, &source);
}

fn resolved<'a>(doc: &'a Document, object: &'a Object) -> &'a Object {
    doc.dereference(object).map_or(object, |(_, o)| o)
}

fn dict_at(doc: &Document, dict: &Dictionary, key: &[u8]) -> Option<Dictionary> {
    resolved(doc, dict.get(key).ok()?).as_dict().ok().cloned()
}

fn list_at(doc: &Document, dict: &Dictionary, key: &[u8]) -> Vec<Object> {
    dict.get(key).ok().and_then(|o| resolved(doc, o).as_array().ok().cloned()).unwrap_or_default()
}

type Names = HashMap<Vec<u8>, Object>;

fn named_destinations(doc: &Document, catalog: &Dictionary) -> Names {
    let mut out = Names::new();
    for (name, dest) in dict_at(doc, catalog, b"Dests").iter().flat_map(|d| d.iter()) {
        out.extend(array_of(doc, dest).map(|d| (name.clone(), d)));
    }
    let tree = dict_at(doc, catalog, b"Names").and_then(|n| dict_at(doc, &n, b"Dests"));
    let mut waiting: Vec<(Dictionary, usize)> = tree.into_iter().map(|t| (t, 0)).collect();
    let mut seen = HashSet::new();
    while let Some((node, depth)) = waiting.pop() {
        for pair in list_at(doc, &node, b"Names").as_chunks::<2>().0 {
            if let (Object::String(name, _), Some(dest)) = (resolved(doc, &pair[0]), array_of(doc, &pair[1])) {
                out.insert(name.clone(), dest);
            }
        }
        for kid in list_at(doc, &node, b"Kids") {
            let fresh = kid.as_reference().is_ok_and(|id| seen.insert(id));
            if fresh
                && depth < MAX_DEPTH
                && let Ok(kid) = resolved(doc, &kid).as_dict()
            {
                waiting.push((kid.clone(), depth + 1));
            }
        }
    }
    out
}

/// A destination as `[page …]`.
fn array_of(doc: &Document, dest: &Object) -> Option<Object> {
    match resolved(doc, dest) {
        Object::Array(a) => Some(Object::Array(a.clone())),
        Object::Dictionary(d) => match resolved(doc, d.get(b"D").ok()?) {
            Object::Array(a) => Some(Object::Array(a.clone())),
            _ => None,
        },
        _ => None,
    }
}

/// Replaces a destination given by name with the place itself; names belong to one document.
fn explicit(doc: &Document, dict: &mut Dictionary, names: &Names) -> bool {
    let place = |o: &Object| match resolved(doc, o) {
        Object::Name(n) | Object::String(n, _) => Some(names.get(n).cloned()),
        _ => None,
    };
    let mut changed = false;
    if let Ok(dest) = dict.get(b"Dest")
        && let Some(found) = place(dest)
    {
        match found {
            Some(place) => dict.set("Dest", place),
            None => drop(dict.remove(b"Dest")),
        }
        changed = true;
    }
    if let Some(mut action) = dict_at(doc, dict, b"A")
        && action.get(b"S").and_then(Object::as_name).is_ok_and(|s| s == b"GoTo")
        && let Ok(d) = action.get(b"D")
        && let Some(found) = place(d)
    {
        match found {
            Some(place) => {
                action.set("D", place);
                dict.set("A", action);
            }
            None => drop(dict.remove(b"A")),
        }
        changed = true;
    }
    changed
}

fn explicit_links(doc: &mut Document, page: ObjectId, names: &Names) {
    let Ok(mut dict) = doc.get_dictionary(page).cloned() else { return };
    // Its place in the other document's structure tree, which does not come along.
    dict.remove(b"StructParents");
    let mut annots = list_at(doc, &dict, b"Annots");
    for annot in &mut annots {
        let Some(mut d) = resolved(doc, annot).as_dict().ok().cloned() else { continue };
        let tagged = d.remove(b"StructParent").is_some();
        if explicit(doc, &mut d, names) || tagged {
            match annot {
                Object::Reference(id) => drop(doc.objects.insert(*id, Object::Dictionary(d))),
                in_place => *in_place = Object::Dictionary(d),
            }
        }
    }
    if !annots.is_empty() {
        dict.set("Annots", annots);
    }
    doc.objects.insert(page, Object::Dictionary(dict));
}

/// Puts the other document's outline after this one's.
fn adopt_outline(doc: &mut Document, root: ObjectId, source: &Dictionary, names: &Names) {
    let Some(theirs) = dict_at(doc, source, b"Outlines") else { return };
    // Every entry, with its destination made explicit.
    let first = theirs.get(b"First").and_then(Object::as_reference).ok();
    let mut waiting: Vec<ObjectId> = first.into_iter().collect();
    let mut seen = HashSet::new();
    while let Some(id) = waiting.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Ok(mut item) = doc.get_dictionary(id).cloned() else { continue };
        for key in [&b"First"[..], b"Next"] {
            waiting.extend(item.get(key).and_then(Object::as_reference).ok());
        }
        item.remove(b"SE");
        explicit(doc, &mut item, names);
        doc.objects.insert(id, Object::Dictionary(item));
    }
    let Some(first) = first else { return };
    let mut top = Vec::new();
    let mut next = Some(first);
    while let Some(id) = next.filter(|id| !top.contains(id) && top.len() < 20_000) {
        let Ok(item) = doc.get_dictionary(id) else { break };
        next = item.get(b"Next").and_then(Object::as_reference).ok();
        top.push(id);
    }
    let (Some(&head), Some(&tail)) = (top.first(), top.last()) else { return };

    let Ok(catalog) = doc.get_dictionary(root).cloned() else { return };
    let outline = match catalog.get(b"Outlines").and_then(Object::as_reference) {
        Ok(id) if doc.get_dictionary(id).is_ok() => id,
        _ => {
            let id = doc.add_object(Dictionary::new());
            if let Ok(catalog) = doc.get_dictionary_mut(root) {
                catalog.set("Outlines", id);
            }
            id
        }
    };
    let set = |doc: &mut Document, id: ObjectId, key: &str, value: Object| {
        if let Ok(d) = doc.get_dictionary_mut(id) {
            d.set(key, value);
        }
    };
    for id in &top {
        set(doc, *id, "Parent", outline.into());
    }
    let mine = doc.get_dictionary(outline).cloned().unwrap_or_default();
    match mine.get(b"Last").and_then(Object::as_reference) {
        Ok(last) => {
            set(doc, last, "Next", head.into());
            set(doc, head, "Prev", last.into());
        }
        Err(_) => set(doc, outline, "First", head.into()),
    }
    set(doc, outline, "Last", tail.into());
    let count = |d: &Dictionary| d.get(b"Count").and_then(Object::as_i64).unwrap_or(0).max(0);
    set(doc, outline, "Count", (count(&mine) + count(&theirs)).into());
}

/// Adds the other document's form fields to this one's form.
fn adopt_form(doc: &mut Document, root: ObjectId, source: &Dictionary) {
    let Some(theirs) = dict_at(doc, source, b"AcroForm") else { return };
    let fields = list_at(doc, &theirs, b"Fields");
    if fields.is_empty() {
        return;
    }
    let Ok(mut catalog) = doc.get_dictionary(root).cloned() else { return };
    let mut form = dict_at(doc, &catalog, b"AcroForm").unwrap_or_default();
    let mut all = list_at(doc, &form, b"Fields");
    let name = |doc: &Document, field: &Object| -> Option<Vec<u8>> {
        resolved(doc, field)
            .as_dict()
            .ok()?
            .get(b"T")
            .ok()
            .and_then(|t| resolved(doc, t).as_str().ok())
            .map(<[u8]>::to_vec)
    };
    let mut taken: HashSet<Vec<u8>> = all.iter().filter_map(|f| name(doc, f)).collect();
    for field in fields {
        let Ok(id) = field.as_reference() else { continue };
        let own = name(doc, &field);
        let Ok(dict) = doc.get_dictionary_mut(id) else { continue };
        // Two fields of one name would be one field.
        if let Some(own) = own {
            let mut unique = own.clone();
            let mut n = 2;
            while !taken.insert(unique.clone()) {
                unique = own.clone();
                let suffix = format!(" {n}");
                if own.starts_with(&[0xFE, 0xFF]) {
                    unique.extend(suffix.encode_utf16().flat_map(u16::to_be_bytes));
                } else {
                    unique.extend(suffix.bytes());
                }
                n += 1;
            }
            if unique != own {
                dict.set("T", Object::String(unique, lopdf::StringFormat::Literal));
            }
        }
        // What the whole form said about its fields.
        for key in [&b"DA"[..], b"Q"] {
            if let (false, Ok(value)) = (dict.has(key), theirs.get(key)) {
                dict.set(key, value.clone());
            }
        }
        all.push(field);
    }
    form.set("Fields", all);
    // Resources, kind by kind (fonts, encodings); what is here already stays.
    let mut resources = dict_at(doc, &form, b"DR").unwrap_or_default();
    for (kind, value) in dict_at(doc, &theirs, b"DR").unwrap_or_default().iter() {
        let Ok(value) = resolved(doc, value).as_dict().cloned() else { continue };
        let mut mine = dict_at(doc, &resources, kind).unwrap_or_default();
        for (key, value) in value.iter() {
            if !mine.has(key) {
                mine.set(key.clone(), value.clone());
            }
        }
        resources.set(kind.clone(), mine);
    }
    if !resources.is_empty() {
        form.set("DR", resources);
    }
    if theirs.get(b"NeedAppearances").and_then(Object::as_bool).unwrap_or(false) {
        form.set("NeedAppearances", true);
    }
    catalog.set("AcroForm", form);
    doc.objects.insert(root, Object::Dictionary(catalog));
}
