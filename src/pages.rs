//! Rearranging a PDF's pages: a plan of which pages the new document is made of,
//! in which order and how far each is turned, and writing it (with `lopdf`).

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use lopdf::{Document, Object, ObjectId};

/// One page of the rearranged document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// 0 is the document being edited; 1 and up are documents pages were inserted from.
    pub source: usize,
    /// Page of that document, from 0.
    pub index: usize,
    /// Extra clockwise turn in degrees: 0, 90, 180 or 270.
    pub rotate: i32,
}

/// The document as it is: every page, in order, unturned.
pub fn identity(pages: usize) -> Vec<Entry> {
    (0..pages).map(|index| Entry { source: 0, index, rotate: 0 }).collect()
}

/// Turns the entries at `positions` by `degrees` (a multiple of 90).
pub fn rotate(plan: &mut [Entry], positions: &[usize], degrees: i32) {
    for &p in positions {
        if let Some(e) = plan.get_mut(p) {
            e.rotate = (e.rotate + degrees).rem_euclid(360);
        }
    }
}

/// Removes the entries at `positions`, unless that would leave none.
pub fn delete(plan: &mut Vec<Entry>, positions: &[usize]) -> bool {
    if positions.len() >= plan.len() {
        return false;
    }
    let mut i = 0;
    plan.retain(|_| {
        i += 1;
        !positions.contains(&(i - 1))
    });
    true
}

/// Moves the entries at `positions` (keeping their order) in front of the entry now at
/// `before` (`plan.len()` for the end). Returns where the moved entries now start.
pub fn move_to(plan: &mut Vec<Entry>, positions: &[usize], before: usize) -> usize {
    let moved: Vec<Entry> = positions.iter().filter_map(|&p| plan.get(p).copied()).collect();
    let mut sorted = positions.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let shift = sorted.iter().filter(|&&p| p < before).count();
    for &p in sorted.iter().rev() {
        if p < plan.len() {
            plan.remove(p);
        }
    }
    let at = before.saturating_sub(shift).min(plan.len());
    plan.splice(at..at, moved);
    at
}

/// A PDF to take pages from, and the password that opens it, if any.
pub struct Source {
    pub bytes: Vec<u8>,
    pub password: Option<String>,
}

/// Attributes a page may inherit from the page tree; they are copied onto each page,
/// because a page that moves to a new parent must not lose them.
const INHERITED: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

fn load(source: &Source) -> Result<Document> {
    let options = lopdf::LoadOptions { password: source.password.clone(), ..lopdf::LoadOptions::default() };
    let doc = Document::load_mem_with_options(&source.bytes, options).context("the PDF could not be read")?;
    if doc.is_encrypted() {
        bail!("the PDF could not be unlocked");
    }
    Ok(doc)
}

fn inherited(doc: &Document, page: ObjectId, key: &[u8]) -> Option<Object> {
    let mut node = page;
    // The tree is shallow; the bound guards against a loop in a damaged file.
    for _ in 0..64 {
        let dict = doc.get_object(node).ok()?.as_dict().ok()?;
        if let Ok(value) = dict.get(key) {
            return Some(value.clone());
        }
        node = dict.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

fn rotation_of(doc: &Document, page: ObjectId) -> i64 {
    match inherited(doc, page, b"Rotate") {
        Some(Object::Integer(r)) => r,
        Some(Object::Reference(id)) => doc.get_object(id).ok().and_then(|o| o.as_i64().ok()).unwrap_or(0),
        _ => 0,
    }
}

/// A document made of `plan`: entry source 0 is `base`, source n is `extras[n - 1]`.
/// A password-protected `base` stays protected the same way.
pub fn compose(base: &Source, extras: &[Source], plan: &[Entry]) -> Result<Vec<u8>> {
    if plan.is_empty() {
        bail!("a document needs at least one page");
    }
    let mut doc = load(base)?;
    let encryption = doc.encryption_state.take();

    // Pull the other documents' objects in, renumbered so they do not clash.
    let mut pages: Vec<BTreeMap<u32, ObjectId>> = vec![doc.get_pages()];
    for extra in extras {
        let mut other = load(extra)?;
        other.renumber_objects_with(doc.max_id + 1);
        pages.push(other.get_pages());
        doc.max_id = doc.max_id.max(other.max_id);
        doc.objects.extend(other.objects);
    }

    // Make every page self-contained, and note how far each one is turned now.
    let mut turn: BTreeMap<ObjectId, i64> = BTreeMap::new();
    for id in pages.iter().flat_map(|p| p.values().copied()).collect::<Vec<_>>() {
        let missing: Vec<(&[u8], Object)> = INHERITED
            .iter()
            .filter_map(|key| {
                let has = doc.get_object(id).ok()?.as_dict().ok()?.has(key);
                (!has).then(|| inherited(&doc, id, key).map(|value| (*key, value))).flatten()
            })
            .collect();
        turn.insert(id, rotation_of(&doc, id));
        let dict = doc.get_object_mut(id)?.as_dict_mut()?;
        for (key, value) in missing {
            dict.set(key, value);
        }
    }

    let root: ObjectId =
        doc.catalog()?.get(b"Pages").and_then(Object::as_reference).context("the document has no page tree")?;
    let mut kids = Vec::with_capacity(plan.len());
    for entry in plan {
        let id = *pages
            .get(entry.source)
            .and_then(|p| p.get(&(entry.index as u32 + 1)))
            .with_context(|| format!("page {} is missing", entry.index + 1))?;
        if kids.contains(&Object::Reference(id)) {
            bail!("a page cannot be in the document twice");
        }
        let total = (turn[&id] + entry.rotate as i64).rem_euclid(360);
        let dict = doc.get_object_mut(id)?.as_dict_mut()?;
        dict.set("Parent", Object::Reference(root));
        if total != 0 || dict.has(b"Rotate") {
            dict.set("Rotate", Object::Integer(total));
        }
        kids.push(Object::Reference(id));
    }
    let tree = doc.get_object_mut(root)?.as_dict_mut()?;
    tree.set("Kids", Object::Array(kids));
    tree.set("Count", Object::Integer(plan.len() as i64));
    for key in INHERITED {
        tree.remove(key);
    }

    doc.prune_objects();
    if let Some(state) = encryption {
        doc.encrypt(&state).context("the PDF could not be protected again")?;
    }
    let mut out = Vec::with_capacity(base.bytes.len());
    doc.save_to(&mut out).context("the PDF could not be written")?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hayro::hayro_syntax::Pdf;

    fn e(index: usize) -> Entry {
        Entry { source: 0, index, rotate: 0 }
    }

    #[test]
    fn plans_rotate_delete_and_move() {
        let mut plan = identity(5);
        rotate(&mut plan, &[1, 3], 90);
        rotate(&mut plan, &[1], -180);
        assert_eq!((plan[1].rotate, plan[3].rotate), (270, 90));
        assert!(!delete(&mut plan.clone(), &[0, 1, 2, 3, 4]), "a document keeps one page");
        assert!(delete(&mut plan, &[0, 2]));
        assert_eq!(plan.iter().map(|e| e.index).collect::<Vec<_>>(), [1, 3, 4]);

        let mut plan = identity(5);
        // Pages 1 and 3 in front of page 0.
        assert_eq!(move_to(&mut plan, &[1, 3], 0), 0);
        assert_eq!(plan, [e(1), e(3), e(0), e(2), e(4)]);
        // Page 0 to the end.
        let mut plan = identity(3);
        assert_eq!(move_to(&mut plan, &[0], 3), 2);
        assert_eq!(plan, [e(1), e(2), e(0)]);
        // Moving down past itself.
        let mut plan = identity(4);
        assert_eq!(move_to(&mut plan, &[0, 1], 3), 1);
        assert_eq!(plan, [e(2), e(0), e(1), e(3)]);
    }

    fn texts(bytes: Vec<u8>, password: &str) -> Vec<(String, u32)> {
        let pdf = Pdf::new_with_password(bytes, password).unwrap();
        let cache = hayro::hayro_interpret::InterpreterCache::new();
        pdf.pages()
            .iter()
            .map(|p| {
                let t = crate::structure::page_text(p, &cache);
                (t.text(0..t.len()), {
                    use hayro::hayro_syntax::page::Rotation;
                    match p.rotation() {
                        Rotation::None => 0,
                        Rotation::Horizontal => 90,
                        Rotation::Flipped => 180,
                        Rotation::FlippedHorizontal => 270,
                    }
                })
            })
            .collect()
    }

    #[test]
    fn writes_reordered_turned_and_inserted_pages() {
        let base = crate::pdf::tests::structured_bytes();
        let extra = crate::pdf::tests::structured_bytes();
        let plan = [
            Entry { source: 0, index: 1, rotate: 90 },
            Entry { source: 1, index: 0, rotate: 0 },
            Entry { source: 0, index: 0, rotate: 0 },
        ];
        let out = compose(&Source { bytes: base, password: None }, &[Source { bytes: extra, password: None }], &plan)
            .unwrap();
        let pages = texts(out, "");
        assert_eq!(pages.len(), 3);
        assert!(pages[0].0 == "Second page" && pages[0].1 == 90, "{pages:?}");
        assert!(pages[1].0.contains("Hello") && pages[2].0.contains("Hello"), "{pages:?}");
    }

    #[test]
    fn protected_documents_stay_protected() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/encrypted.pdf");
        let bytes = std::fs::read(path).unwrap();
        let n = Pdf::new_with_password(bytes.clone(), "secret").unwrap().pages().len();
        let mut plan = identity(n);
        rotate(&mut plan, &[0], 180);
        let out = compose(&Source { bytes, password: Some("secret".into()) }, &[], &plan).unwrap();
        assert!(Pdf::new(out.clone()).is_err(), "still needs the password");
        let pages = texts(out, "secret");
        assert_eq!(pages[0].1, 180);
        assert!(!pages[0].0.trim().is_empty(), "the text is still readable");
    }
}
