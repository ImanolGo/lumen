//! Form fields: reading what a PDF's form holds, and filling it in.
//!
//! Fields are read with the rest of the document's information. Filling one in writes
//! a new working copy with the value set, which Poppler gives a matching appearance,
//! so what is on screen is what will be saved.

use std::{collections::BTreeMap, path::Path};

use anyhow::{Context, Result};
use poppler::{Document, FormButtonType, FormChoiceType, FormFieldType, FormTextType, ffi};
use relm4::gtk::glib::translate::{FromGlibPtrNone, ToGlibPtr};

use super::{HitRect, PdfSource, annots, open_document, poppler_lock};

/// A field of the form, where it sits on its page and what it holds.
#[derive(Debug, Clone, PartialEq)]
pub struct FormField {
    /// Poppler's identifier, stable for the document.
    pub id: i32,
    /// In points from the page's top-left.
    pub area: HitRect,
    pub read_only: bool,
    pub kind: FieldKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FieldKind {
    Text {
        text: String,
        multiline: bool,
        password: bool,
        /// Most characters it takes, if limited.
        max_len: Option<usize>,
    },
    Check {
        on: bool,
    },
    /// One choice of a group: turning it on turns the others off.
    Radio {
        on: bool,
    },
    Choice {
        items: Vec<String>,
        selected: Option<usize>,
        /// A combo box that also takes typed text.
        editable: bool,
        text: String,
    },
    /// A place for a digital signature.
    Signature,
    /// A button that runs a script; Lumen runs none.
    Push,
}

impl FormField {
    /// Whether clicking the field does anything (a signature field is signed with ink).
    pub fn editable(&self) -> bool {
        !self.read_only && !matches!(self.kind, FieldKind::Push)
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        (self.area.x1..=self.area.x2).contains(&x) && (self.area.y1..=self.area.y2).contains(&y)
    }
}

/// A new value for a field.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    Text(String),
    On(bool),
    /// Pick the item at this index.
    Item(usize),
}

/// The fields of every page. The caller holds the Poppler lock.
pub(super) fn read_all(doc: &Document) -> BTreeMap<usize, Vec<FormField>> {
    (0..doc.n_pages().max(0) as usize)
        .filter_map(|index| {
            let fields = read_page(doc, index);
            (!fields.is_empty()).then_some((index, fields))
        })
        .collect()
}

fn read_page(doc: &Document, index: usize) -> Vec<FormField> {
    let Some(page) = doc.page(index as i32) else {
        return Vec::new();
    };
    let height = page.size().1;
    page.form_field_mapping()
        .iter()
        .filter_map(|mapping| {
            let raw: *const ffi::PopplerFormFieldMapping = mapping.to_glib_none().0;
            // SAFETY: `raw` points into `mapping`, which outlives this closure body; the
            // field is owned by the mapping and gets a reference of its own here.
            let (area, field) = unsafe {
                let field = (*raw).field;
                if field.is_null() {
                    return None;
                }
                ((*raw).area, poppler::FormField::from_glib_none(field))
            };
            Some(FormField {
                id: field.id(),
                // Poppler measures from the bottom of the page.
                area: HitRect {
                    x1: area.x1,
                    y1: height - area.y2,
                    x2: area.x2,
                    y2: height - area.y1,
                },
                read_only: field.is_read_only(),
                kind: kind_of(&field),
            })
        })
        .collect()
}

fn kind_of(field: &poppler::FormField) -> FieldKind {
    match field.field_type() {
        FormFieldType::Text => FieldKind::Text {
            text: field.text_get_text().map(String::from).unwrap_or_default(),
            multiline: field.text_get_text_type() == FormTextType::Multiline,
            password: field.text_is_password(),
            max_len: usize::try_from(field.text_get_max_len())
                .ok()
                .filter(|&n| n > 0),
        },
        FormFieldType::Button => match field.button_get_button_type() {
            FormButtonType::Check => FieldKind::Check {
                on: field.button_get_state(),
            },
            FormButtonType::Radio => FieldKind::Radio {
                on: field.button_get_state(),
            },
            _ => FieldKind::Push,
        },
        FormFieldType::Choice => {
            let items: Vec<String> = (0..field.choice_get_n_items())
                .map(|i| {
                    field
                        .choice_get_item(i)
                        .map(String::from)
                        .unwrap_or_default()
                })
                .collect();
            let selected = (0..items.len()).find(|&i| field.choice_is_item_selected(i as i32));
            FieldKind::Choice {
                selected,
                editable: field.choice_get_choice_type() == FormChoiceType::Combo
                    && field.choice_is_editable(),
                text: field
                    .choice_get_text()
                    .map(String::from)
                    .unwrap_or_default(),
                items,
            }
        }
        FormFieldType::Signature => FieldKind::Signature,
        _ => FieldKind::Push,
    }
}

/// Write the PDF in `source` to `dest` with field `id` set to `value`. Blocking.
pub fn set_value(source: &PdfSource, id: i32, value: &FieldValue, dest: &Path) -> Result<()> {
    let _poppler = poppler_lock();
    let doc = open_document(source)?;
    let field = doc
        .form_field(id)
        .with_context(|| format!("the form has no field {id}"))?;
    match (field.field_type(), value) {
        (FormFieldType::Text, FieldValue::Text(text)) => field.text_set_text(text),
        (FormFieldType::Button, FieldValue::On(on)) => field.button_set_state(*on),
        (FormFieldType::Choice, FieldValue::Item(index)) => {
            field.choice_unselect_all();
            field.choice_select_item(*index as i32);
        }
        (FormFieldType::Choice, FieldValue::Text(text)) => field.choice_set_text(text),
        (kind, value) => anyhow::bail!("cannot set a {kind:?} field to {value:?}"),
    }
    annots::save(&doc, dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture() -> PdfSource {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/form.pdf");
        PdfSource::new(&path, None)
    }

    fn fields(source: &PdfSource) -> Vec<FormField> {
        let _poppler = poppler_lock();
        let doc = open_document(source).unwrap();
        read_all(&doc).into_values().flatten().collect()
    }

    fn find(fields: &[FormField], pred: impl Fn(&FieldKind) -> bool) -> &FormField {
        fields
            .iter()
            .find(|f| pred(&f.kind))
            .expect("the fixture has that field")
    }

    #[test]
    fn reads_every_kind_of_field() {
        let fields = fields(&fixture());
        assert!(matches!(
            find(&fields, |k| matches!(k, FieldKind::Text { .. })).kind,
            FieldKind::Text { ref text, .. } if text.is_empty()
        ));
        assert!(matches!(
            find(&fields, |k| matches!(k, FieldKind::Check { .. })).kind,
            FieldKind::Check { on: false }
        ));
        let choice = find(&fields, |k| matches!(k, FieldKind::Choice { .. }));
        assert!(matches!(&choice.kind, FieldKind::Choice { items, .. } if items.len() == 3));
        // Areas are on the page, measured from the top.
        assert!(
            fields
                .iter()
                .all(|f| f.area.x1 < f.area.x2 && f.area.y1 < f.area.y2)
        );
    }

    #[test]
    fn values_are_written_to_the_copy() {
        let source = fixture();
        let before = fields(&source);
        let text = find(&before, |k| matches!(k, FieldKind::Text { .. })).id;
        let check = find(&before, |k| matches!(k, FieldKind::Check { .. })).id;
        let choice = find(&before, |k| matches!(k, FieldKind::Choice { .. })).id;

        let dir = std::env::temp_dir().join(format!("lumen-forms-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b, c) = (dir.join("a.pdf"), dir.join("b.pdf"), dir.join("c.pdf"));
        set_value(&source, text, &FieldValue::Text("Ada Lovelace".into()), &a).unwrap();
        set_value(&PdfSource::new(&a, None), check, &FieldValue::On(true), &b).unwrap();
        set_value(&PdfSource::new(&b, None), choice, &FieldValue::Item(2), &c).unwrap();

        let after = fields(&PdfSource::new(&c, None));
        let by_id = |id| after.iter().find(|f| f.id == id).unwrap().kind.clone();
        assert!(matches!(by_id(text), FieldKind::Text { text, .. } if text == "Ada Lovelace"));
        assert_eq!(by_id(check), FieldKind::Check { on: true });
        assert!(matches!(
            by_id(choice),
            FieldKind::Choice {
                selected: Some(2),
                ..
            }
        ));
        // The original is untouched.
        assert!(matches!(
            find(&fields(&source), |k| matches!(k, FieldKind::Text { .. })).kind,
            FieldKind::Text { ref text, .. } if text.is_empty()
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
