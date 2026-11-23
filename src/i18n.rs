//! Translations. English is the source language: every UI string is written in English and
//! passed through [`tr`], which looks it up in the catalogue of the user's language.
//!
//! Catalogues are `po/<language>.po` files embedded in the binary (no `msgfmt` step, nothing
//! to install). The language comes from `LC_ALL`, `LC_MESSAGES`, `LANGUAGE` or `LANG`, like
//! gettext's; a string with no translation shows in English.

use std::collections::HashMap;
use std::sync::OnceLock;

/// The catalogues Lumen ships: language code and its `.po` text.
const CATALOGUES: &[(&str, &str)] = &[("es", include_str!("../po/es.po"))];

static CATALOGUE: OnceLock<HashMap<String, String>> = OnceLock::new();

/// Translates `text` (English) into the user's language, or returns it as it is.
pub fn tr(text: &str) -> String {
    CATALOGUE.get_or_init(load).get(text).cloned().unwrap_or_else(|| text.to_owned())
}

/// `trf!("Saved “{}”", name)`: translates the template and fills its `{}` holes with the
/// arguments, shown the way `format!` shows them.
macro_rules! trf {
    ($template:literal $(, $arg:expr)* $(,)?) => {{
        let args: Vec<String> = vec![$(format!("{}", $arg)),*];
        $crate::i18n::trf_args($template, &args)
    }};
}

/// What [`trf!`] calls.
pub fn trf_args(template: &str, args: &[String]) -> String {
    fill(&tr(template), &args.iter().map(String::as_str).collect::<Vec<_>>())
}

/// Translates `template` and fills its `{}` holes, in order, with `args`.
#[allow(dead_code)]
pub fn trf(template: &str, args: &[&str]) -> String {
    fill(&tr(template), args)
}

fn fill(template: &str, args: &[&str]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut args = args.iter();
    let mut rest = template;
    while let Some(at) = rest.find("{}") {
        out.push_str(&rest[..at]);
        out.push_str(args.next().copied().unwrap_or(""));
        rest = &rest[at + 2..];
    }
    out.push_str(rest);
    out
}

fn load() -> HashMap<String, String> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let wanted = [env("LC_ALL"), env("LC_MESSAGES"), env("LANGUAGE"), env("LANG")];
    catalogue_for(&wanted.into_iter().flatten().collect::<Vec<_>>())
}

/// The catalogue for the first of `locales` (`"es_ES.UTF-8"`, `"fr:es"`, …) that has one.
fn catalogue_for(locales: &[String]) -> HashMap<String, String> {
    for locale in locales.iter().flat_map(|l| l.split(':')) {
        let code = locale.split(['_', '.', '@']).next().unwrap_or("");
        // "C" and "POSIX" mean English, whatever follows.
        if code.is_empty() || code == "C" || code == "POSIX" {
            return HashMap::new();
        }
        if let Some((_, po)) = CATALOGUES.iter().find(|(c, _)| *c == code) {
            return parse(po);
        }
    }
    HashMap::new()
}

/// Reads the messages of a `.po` file: `msgid`/`msgstr` pairs, with continued lines and the
/// usual escapes. Entries with a context or plural forms, and untranslated ones, are skipped.
pub fn parse(po: &str) -> HashMap<String, String> {
    #[derive(PartialEq)]
    enum Field {
        None,
        Id,
        Str,
        Other,
    }
    let mut map = HashMap::new();
    let (mut id, mut text, mut field, mut skip) = (String::new(), String::new(), Field::None, false);
    // A `msgctxt` line opens an entry that is skipped; its `msgid` must not end the entry before it.
    let mut after_context = false;
    let mut finish = |id: &mut String, text: &mut String, skip: &mut bool| {
        if !*skip && !id.is_empty() && !text.is_empty() {
            map.insert(std::mem::take(id), std::mem::take(text));
        }
        id.clear();
        text.clear();
        *skip = false;
    };
    for line in po.lines().map(str::trim) {
        if line.starts_with("msgid_plural ") {
            skip = true;
            field = Field::Other;
        } else if line.starts_with("msgctxt ") {
            finish(&mut id, &mut text, &mut skip);
            skip = true;
            after_context = true;
            field = Field::Other;
        } else if let Some(rest) = line.strip_prefix("msgid ") {
            if !std::mem::take(&mut after_context) {
                finish(&mut id, &mut text, &mut skip);
            }
            id = unquote(rest);
            field = Field::Id;
        } else if line.starts_with("msgstr[") {
            skip = true;
            field = Field::Other;
        } else if let Some(rest) = line.strip_prefix("msgstr ") {
            text = unquote(rest);
            field = Field::Str;
        } else if line.starts_with('"') {
            match field {
                Field::Id => id.push_str(&unquote(line)),
                Field::Str => text.push_str(&unquote(line)),
                _ => {}
            }
        } else if line.is_empty() {
            finish(&mut id, &mut text, &mut skip);
            field = Field::None;
        }
    }
    finish(&mut id, &mut text, &mut skip);
    map
}

/// The text inside one quoted `.po` string, with its escapes undone.
fn unquote(quoted: &str) -> String {
    let inner = quoted.trim().strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or("");
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"# comment
msgid ""
msgstr ""
"Language: es\n"

#: src/app.rs:1
msgid "Save"
msgstr "Guardar"

msgid "Saved “{}”"
msgstr "Guardado «{}»"

msgid "A long one"
msgstr ""
"Una muy "
"larga"

msgid "Quote \" and \\ and newline\n"
msgstr "Comilla \" y \\ y salto\n"

msgid "Untranslated"
msgstr ""

msgctxt "menu"
msgid "Open"
msgstr "Abrir (menú)"

msgid "One file"
msgid_plural "Many files"
msgstr[0] "Un archivo"
msgstr[1] "Muchos archivos"

msgid "After"
msgstr "Después"
"#;

    #[test]
    fn reads_messages_continued_lines_and_escapes() {
        let m = parse(SAMPLE);
        assert_eq!(m["Save"], "Guardar");
        assert_eq!(m["Saved “{}”"], "Guardado «{}»");
        assert_eq!(m["A long one"], "Una muy larga");
        assert_eq!(m["Quote \" and \\ and newline\n"], "Comilla \" y \\ y salto\n");
        assert_eq!(m["After"], "Después", "an entry after skipped ones is still read");
    }

    #[test]
    fn skips_the_header_untranslated_contextual_and_plural_entries() {
        let m = parse(SAMPLE);
        assert!(!m.contains_key(""));
        assert!(!m.contains_key("Untranslated"));
        assert!(!m.contains_key("Open"));
        assert!(!m.contains_key("One file") && !m.contains_key("Many files"));
    }

    #[test]
    fn holes_are_filled_in_order() {
        assert_eq!(fill("{} of {}", &["3", "10"]), "3 of 10");
        assert_eq!(fill("no holes", &["x"]), "no holes");
        assert_eq!(fill("{} and {}", &["a"]), "a and ");
    }

    #[test]
    fn the_language_comes_from_the_locale_variables() {
        let es = |l: &str| !catalogue_for(&[l.to_string()]).is_empty();
        assert!(es("es_ES.UTF-8") && es("es") && es("es_MX@euro") && es("fr:es"));
        assert!(!es("en_US.UTF-8") && !es("C") && !es("POSIX") && !es("de_DE"));
        // The first language that has a catalogue wins; C stops the search.
        assert!(!es("C:es"));
        assert!(catalogue_for(&[]).is_empty());
    }

    /// Every string passed to `tr` or `trf!` in the source, and the few the code picks by name.
    fn strings_in_the_source() -> Vec<String> {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let mut files = Vec::new();
        walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
        let mut found = Vec::new();
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap();
            // The module's own tests and docs use tr/trf as examples.
            if file.ends_with("i18n.rs") {
                continue;
            }
            let mut from = 0;
            while let Some(at) = ["tr(", "trf!("]
                .iter()
                .filter_map(|p| text[from..].find(p).map(|i| (i + from, *p)))
                .min_by_key(|(i, _)| *i)
            {
                let (at, opener) = at;
                from = at + opener.len();
                // Only the functions themselves: not `str(`, `Str(` and the like.
                if text[..at].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                // Everything up to the matching parenthesis; every string inside is a message
                // (`tr(if a { "One" } else { "Two" })` has two).
                let bytes = text.as_bytes();
                let (mut depth, mut i, mut literals) = (1, from, Vec::new());
                while depth > 0 && i < bytes.len() {
                    match bytes[i] {
                        b'(' => depth += 1,
                        b')' => depth -= 1,
                        b'"' => {
                            let start = i + 1;
                            i += 1;
                            while bytes[i] != b'"' {
                                i += if bytes[i] == b'\\' { 2 } else { 1 };
                            }
                            literals.push(unquote(&format!("\"{}\"", &text[start..i])));
                        }
                        _ => {}
                    }
                    i += 1;
                }
                if opener == "trf!(" {
                    literals.truncate(1);
                }
                found.extend(literals);
            }
        }
        // Chosen by name rather than written inside the call.
        found.extend(["Files", "Contents", "Pages"].map(String::from));
        found.retain(|s| !s.is_empty());
        found
    }

    #[test]
    fn every_string_the_code_translates_has_a_spanish_entry() {
        let es = catalogue_for(&["es".to_string()]);
        // Names, codes and examples that read the same in every language.
        let same = ["Aa", "Lumen", "px", "GPL-3.0-or-later · github.com/ImanolGo/lumen-rs", "#rrggbb"];
        let missing: Vec<String> = strings_in_the_source()
            .into_iter()
            .filter(|s| !es.contains_key(s) && !same.contains(&s.as_str()))
            .collect();
        assert!(missing.is_empty(), "no Spanish for: {missing:#?}");
    }

    #[test]
    fn the_shipped_spanish_catalogue_reads_and_keeps_its_holes() {
        let es = catalogue_for(&["es".to_string()]);
        assert!(es.len() > 100, "only {} messages", es.len());
        for (english, spanish) in &es {
            assert_eq!(
                english.matches("{}").count(),
                spanish.matches("{}").count(),
                "{english:?} and {spanish:?} must have the same number of {{}}"
            );
        }
    }
}
