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

impl Source {
    /// An open document's pages, as it was opened.
    pub fn of(doc: &crate::pdf::Document) -> Self {
        Self { bytes: doc.original().to_vec(), password: doc.password().map(str::to_owned) }
    }
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
    let Some(turn) = inherited(doc, page, b"Rotate") else { return 0 };
    // Any kind of number: some programs write 90.0.
    doc.dereference(&turn).ok().and_then(|(_, o)| o.as_float().ok()).map_or(0, |r| r.round() as i64)
}

fn kids_of(doc: &Document, node: ObjectId) -> Vec<ObjectId> {
    let kids = doc.get_dictionary(node).and_then(|d| d.get(b"Kids")).and_then(Object::as_array);
    kids.map(|k| k.iter().filter_map(|o| o.as_reference().ok()).collect()).unwrap_or_default()
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
    let root = doc.trailer.get(b"Root").and_then(Object::as_reference).context("the document has no catalog")?;
    for extra in extras {
        let mut other = load(extra)?;
        other.renumber_objects_with(doc.max_id + 1);
        let their_pages = other.get_pages();
        let their_root = other.trailer.get(b"Root").and_then(Object::as_reference);
        doc.max_id = doc.max_id.max(other.max_id);
        doc.objects.extend(other.objects);
        if let Ok(from) = their_root {
            let all: Vec<ObjectId> = their_pages.values().copied().collect();
            crate::merge::adopt(&mut doc, root, from, &all);
        }
        pages.push(their_pages);
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

    let tree_id: ObjectId =
        doc.catalog()?.get(b"Pages").and_then(Object::as_reference).context("the document has no page tree")?;
    let mut kids = Vec::with_capacity(plan.len());
    let mut used = std::collections::HashSet::new();
    for entry in plan {
        let id = *pages
            .get(entry.source)
            .and_then(|p| p.get(&(entry.index as u32 + 1)))
            .with_context(|| format!("page {} is missing", entry.index + 1))?;
        if !used.insert(id) {
            bail!("a page cannot be in the document twice");
        }
        let total = (turn[&id] + entry.rotate as i64).rem_euclid(360);
        let dict = doc.get_object_mut(id)?.as_dict_mut()?;
        dict.set("Parent", Object::Reference(tree_id));
        if total != 0 || dict.has(b"Rotate") {
            dict.set("Rotate", Object::Integer(total));
        }
        kids.push(Object::Reference(id));
    }
    let tree = doc.get_object_mut(tree_id)?.as_dict_mut()?;
    tree.set("Kids", Object::Array(kids));
    tree.set("Count", Object::Integer(plan.len() as i64));
    for key in INHERITED {
        tree.remove(key);
    }

    let kept: Vec<ObjectId> = kids_of(&doc, tree_id);
    let removed = pages.iter().flat_map(|p| p.values().copied()).filter(|id| !used.contains(id)).collect();
    crate::redact::remove_pages(&mut doc, &removed, &kept);
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

    /// Three pages that everything in a document can lead to: an outline, named destinations of both
    /// kinds, links, a form with fields on each page, and a structure tree that repeats the text.
    fn connected() -> Vec<u8> {
        use lopdf::{Stream, dictionary};
        let mut doc = Document::with_version("1.7");
        let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
        let (tree, pages): (ObjectId, Vec<ObjectId>) =
            (doc.new_object_id(), (0..3).map(|_| doc.new_object_id()).collect());
        let (form_field, struct_root, struct_doc) = (doc.new_object_id(), doc.new_object_id(), doc.new_object_id());
        let dest = |i: usize| vec![Object::Reference(pages[i]), "Fit".into()];
        let rect = || -> Vec<Object> { vec![0.into(), 0.into(), 50.into(), 20.into()] };
        let (mut widgets, mut elements) = (Vec::new(), Vec::new());
        for (i, page) in pages.iter().enumerate() {
            let n = i + 1;
            let content = format!("/P <</MCID 0>> BDC BT /F1 12 Tf 72 700 Td (secret{n}) Tj ET EMC");
            let content = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
            let look = format!("BT /F1 9 Tf (widget{n}) Tj ET").into_bytes();
            let look = doc.add_object(Stream::new(dictionary! { "Subtype" => "Form", "BBox" => rect() }, look));
            let widget = doc.add_object(dictionary! {
                "Subtype" => "Widget", "Parent" => form_field, "P" => *page, "Rect" => rect(),
                "AP" => dictionary! { "N" => look },
            });
            let own = doc.add_object(dictionary! {
                "Subtype" => "Widget", "FT" => "Tx", "T" => Object::string_literal(format!("own{n}")), "P" => *page,
                "V" => Object::string_literal(format!("value{n}")), "Rect" => rect(),
            });
            let link = doc.add_object(dictionary! { "Subtype" => "Link", "Rect" => rect(), "Dest" => dest(2) });
            let jump = doc.add_object(dictionary! {
                "Subtype" => "Link", "Rect" => rect(),
                "A" => dictionary! { "S" => "GoTo", "D" => Object::string_literal("third") },
            });
            let element = doc.add_object(dictionary! {
                "Type" => "StructElem", "S" => "P", "P" => struct_doc, "Pg" => *page, "K" => 0,
                "ActualText" => Object::string_literal(format!("spoken{n}")),
            });
            widgets.push((Object::Reference(widget), Object::Reference(own)));
            elements.push(Object::Reference(element));
            doc.objects.insert(
                *page,
                dictionary! {
                    "Type" => "Page", "Parent" => tree, "MediaBox" => vec![0.into(), 0.into(), 600.into(), 800.into()],
                    "Contents" => content, "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
                    "Annots" => vec![widget.into(), own.into(), link.into(), jump.into()],
                }
                .into(),
            );
        }
        let kids: Vec<Object> = pages.iter().map(|p| Object::Reference(*p)).collect();
        doc.objects.insert(tree, dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => 3 }.into());
        doc.objects.insert(
            form_field,
            dictionary! {
                "FT" => "Tx", "T" => Object::string_literal("shared"),
                "Kids" => widgets.iter().map(|w| w.0.clone()).collect::<Vec<_>>(),
            }
            .into(),
        );
        let mut fields: Vec<Object> = vec![form_field.into()];
        fields.extend(widgets.iter().map(|w| w.1.clone()));
        doc.objects.insert(
            struct_doc,
            dictionary! {
                "Type" => "StructElem", "S" => "Document", "P" => struct_root, "K" => elements.clone(),
                "Alt" => Object::string_literal("spoken3 as well"),
            }
            .into(),
        );
        let parents = doc.add_object(dictionary! { "Nums" => vec![0.into(), Object::Array(elements)] });
        doc.objects.insert(
            struct_root,
            dictionary! { "Type" => "StructTreeRoot", "K" => struct_doc, "ParentTree" => parents }.into(),
        );
        // First, second (holding third), and a third one by name.
        let (outline, items) = (doc.new_object_id(), (0..4).map(|_| doc.new_object_id()).collect::<Vec<_>>());
        let title = |t: &str| Object::string_literal(t);
        doc.objects.insert(outline, dictionary! { "First" => items[0], "Last" => items[3], "Count" => 4 }.into());
        doc.objects.insert(
            items[0],
            dictionary! {
                "Title" => title("title1"), "Parent" => outline, "Next" => items[1], "Dest" => dest(0),
            }
            .into(),
        );
        doc.objects.insert(items[1], dictionary! {
            "Title" => title("title2"), "Parent" => outline, "Prev" => items[0], "Next" => items[3], "Dest" => dest(1),
            "First" => items[2], "Last" => items[2], "Count" => 1,
        }.into());
        doc.objects.insert(
            items[2],
            dictionary! { "Title" => title("title3"), "Parent" => items[1], "Dest" => dest(2) }.into(),
        );
        doc.objects.insert(
            items[3],
            dictionary! {
                "Title" => title("title3 by name"), "Parent" => outline, "Prev" => items[1], "Dest" => title("third"),
            }
            .into(),
        );
        let names: Vec<Object> = vec![title("first"), dest(0).into(), title("third"), dest(2).into()];
        let names = doc.add_object(dictionary! { "Names" => names });
        let names = doc.add_object(dictionary! { "Kids" => vec![names.into()] });
        let catalog = doc.add_object(dictionary! {
            "Type" => "Catalog", "Pages" => tree, "Outlines" => outline, "StructTreeRoot" => struct_root,
            "OpenAction" => dest(2), "Names" => dictionary! { "Dests" => names },
            "Dests" => dictionary! { "old3" => dest(2), "old1" => dest(0) },
            "AcroForm" => dictionary! { "Fields" => fields },
        });
        doc.trailer.set("Root", catalog);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    /// Every stream unpacked, so that text anywhere in the file can be found.
    fn unpacked(bytes: &[u8]) -> String {
        let mut doc = Document::load_mem(bytes).unwrap();
        doc.decompress();
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    fn nothing_of_a_removed_page_is_left_in_the_file() {
        let source = || Source { bytes: connected(), password: None };
        let all = unpacked(&compose(&source(), &[], &identity(3)).unwrap());
        let words = ["secret", "widget", "value", "own", "spoken", "title"];
        for word in words {
            for n in 1..=3 {
                assert!(all.contains(&format!("{word}{n}")), "{word}{n} is there to begin with");
            }
        }
        assert!(all.contains("old3") && all.contains("OpenAction"), "nothing is dropped when no page is");

        let some = compose(&source(), &[], &[e(0), e(1)]).unwrap();
        let text = unpacked(&some);
        for word in words {
            assert!(!text.contains(&format!("{word}3")), "{word}3 is still in the file");
            assert!(text.contains(&format!("{word}1")) && text.contains(&format!("{word}2")), "{word} of kept pages");
        }
        assert!(!text.contains("third") && !text.contains("old3") && !text.contains("OpenAction"));
        assert!(text.contains("first") && text.contains("old1"));
        // What is left is sound: the outline, and the form.
        let pdf = Pdf::new(some).unwrap();
        let (outline, links) = crate::structure::read(&pdf);
        assert_eq!(outline.iter().map(|o| o.title.as_str()).collect::<Vec<_>>(), ["title1", "title2"]);
        assert!(links.iter().all(Vec::is_empty), "{links:?}");
        let form = crate::forms::read(&pdf);
        assert_eq!(form.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["shared", "own1", "own2"]);
        assert_eq!(form.fields[0].widgets.len(), 2);
        // Inserted into another document, pages bring their fields, their links and their entries along.
        let plan = [e(0), Entry { source: 1, index: 2, rotate: 0 }, Entry { source: 1, index: 0, rotate: 0 }];
        let base = Source { bytes: connected(), password: None };
        let merged = compose(&base, &[source()], &plan).unwrap();
        assert!(!unpacked(&merged).contains("secret2"));
        let pdf = Pdf::new(merged).unwrap();
        let (outline, links) = crate::structure::read(&pdf);
        let target = |n: usize| Some(crate::structure::Target::Page { page: n, top: None });
        let entries: Vec<_> = outline.iter().map(|o| (o.title.as_str(), o.target.clone())).collect();
        assert_eq!(
            entries,
            [
                ("title1", target(0)),
                ("title1", target(2)),
                ("title2", None),
                ("title3", target(1)),
                ("title3 by name", target(1))
            ]
        );
        assert_eq!(links.iter().map(Vec::len).collect::<Vec<_>>(), [0, 2, 2]);
        assert!(links[2].iter().all(|l| Some(l.target.clone()) == target(1)), "{links:?}");
        let form = crate::forms::read(&pdf);
        let names: Vec<&str> = form.fields.iter().map(|f| f.name.as_str()).collect();
        // Told apart from the fields the document had before its other pages went, too.
        assert_eq!(names, ["shared", "own1", "shared 2", "own1 2", "own3 2"]);
        // Taking one page of it into another document brings nothing else along.
        let base = Source { bytes: crate::pdf::tests::structured_bytes(), password: None };
        let plan = [e(0), Entry { source: 1, index: 0, rotate: 0 }];
        let text = unpacked(&compose(&base, &[source()], &plan).unwrap());
        assert!(text.contains("secret1") && !text.contains("secret2") && !text.contains("widget3"), "{text}");
    }

    #[test]
    fn a_turn_written_as_a_real_number_counts() {
        let mut doc = Document::load_mem(&connected()).unwrap();
        for (i, turn) in [(1, Object::Real(90.0)), (2, Object::Integer(180))] {
            let id = doc.get_pages()[&i];
            doc.get_dictionary_mut(id).unwrap().set("Rotate", turn);
        }
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        let out = compose(&Source { bytes, password: None }, &[], &[e(1), e(0)]).unwrap();
        assert_eq!(texts(out, "").iter().map(|t| t.1).collect::<Vec<_>>(), [180, 90]);
    }

    #[test]
    fn an_exported_page_does_not_bring_the_others_along() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pages.pdf");
        let source = Source { bytes: std::fs::read(path).unwrap(), password: None };
        let text = unpacked(&compose(&source, &[], &[e(0)]).unwrap());
        assert!(text.contains("Page 1") && !text.contains("needle"));
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
