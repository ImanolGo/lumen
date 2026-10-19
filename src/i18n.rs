//! Translations with gettext.
//!
//! Every text the user sees goes through `tr()` (or `tr_fmt()` when it has
//! values in it). Without a translation, or for English, the text comes back
//! unchanged.

use std::path::PathBuf;

use gettextrs::{bind_textdomain_codeset, bindtextdomain, gettext, textdomain};
use relm4::adw::{self, prelude::*};

const DOMAIN: &str = "lumen";

/// Where the compiled translations (`<lang>/LC_MESSAGES/lumen.mo`) are looked for:
/// `$LUMEN_LOCALEDIR`, else `../share/locale` next to the program (an install under
/// any prefix, including a Flatpak's `/app`), else the system folder.
fn locale_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("LUMEN_LOCALEDIR") {
        return PathBuf::from(dir);
    }
    let installed = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|bin| bin.join("../share/locale")))
        .filter(|dir| dir.is_dir());
    installed.unwrap_or_else(|| PathBuf::from("/usr/share/locale"))
}

/// Call once at startup, before any text is shown.
pub fn init() {
    // GTK sets the locale itself when it starts; only the text domain is needed here.
    if let Err(err) = bindtextdomain(DOMAIN, locale_dir()) {
        tracing::warn!("translations are unavailable: {err}");
        return;
    }
    let _ = bind_textdomain_codeset(DOMAIN, "UTF-8");
    let _ = textdomain(DOMAIN);
}

/// Marks a text for translation without translating it yet, for tables that are
/// built at compile time. Pass the text through `tr()` when it is shown.
pub const fn n_(text: &'static str) -> &'static str {
    text
}

/// The translation of `text`.
pub fn tr(text: &str) -> String {
    gettext(text)
}

/// The translation of `text`, with each `{name}` replaced by its value. Translators
/// may move the placeholders around to suit their language.
pub fn tr_fmt(text: &str, values: &[(&str, &str)]) -> String {
    let mut out = gettext(text);
    for (name, value) in values {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

/// Add buttons to an alert dialog, with translated labels.
pub fn add_responses(dialog: &adw::AlertDialog, responses: &[(&str, &str)]) {
    for (id, label) in responses {
        dialog.add_response(id, &tr(label));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untranslated_text_comes_back_unchanged() {
        assert_eq!(tr("Open"), "Open");
    }

    #[test]
    fn placeholders_are_filled_in_wherever_the_translation_puts_them() {
        assert_eq!(
            tr_fmt("Page {page} of {total}", &[("page", "3"), ("total", "40")]),
            "Page 3 of 40"
        );
        // A translation may reorder them; the same values still land in the right places.
        let swapped = "{total} pages, now on {page}"
            .replace("{page}", "3")
            .replace("{total}", "40");
        assert_eq!(swapped, "40 pages, now on 3");
    }
}
