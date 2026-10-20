//! The keyboard shortcuts window.

use relm4::adw::{self, prelude::*};

use crate::i18n::{n_, tr};

/// Sections of (title, accelerator) pairs, in the order they are shown.
const SECTIONS: &[(&str, &[(&str, &str)])] = &[
    (
        n_("General"),
        &[
            (n_("Open"), "<Control>o"),
            (n_("Print"), "<Control>p"),
            (n_("Toggle markup toolbar"), "<Control><Shift>a"),
            (n_("Preferences"), "<Control>comma"),
            (n_("Keyboard shortcuts"), "<Control>question"),
            (n_("Fullscreen"), "F11"),
            (n_("Slideshow"), "F5"),
            (n_("Leave fullscreen or the slideshow"), "Escape"),
            (n_("Toggle sidebar"), "F9"),
            (n_("Properties"), "<Alt>Return"),
            (n_("Close window"), "<Control>w"),
        ],
    ),
    (
        n_("Pictures and zoom"),
        &[
            (n_("Next file"), "Right space"),
            (n_("Previous file"), "Left BackSpace"),
            (n_("Zoom in"), "<Control>plus"),
            (n_("Zoom out"), "<Control>minus"),
            (n_("Fit to window"), "<Control>0"),
            (n_("Actual size"), "<Control>1"),
        ],
    ),
    (
        n_("PDF documents"),
        &[
            (n_("Next page"), "Page_Down"),
            (n_("Previous page"), "Page_Up"),
            (n_("First page"), "Home"),
            (n_("Last page"), "End"),
            (n_("Go to page"), "<Control>g"),
            (n_("Find"), "<Control>f"),
            (n_("Next match"), "F3"),
            (n_("Previous match"), "<Shift>F3"),
            (n_("Copy selected text"), "<Control>c"),
            (n_("Delete selected pages (in the sidebar)"), "Delete"),
        ],
    ),
    (
        n_("Editing"),
        &[
            (n_("Undo"), "<Control>z"),
            (n_("Redo"), "<Control><Shift>z"),
            (n_("Rotate right"), "<Control>r"),
            (n_("Rotate left"), "<Control><Shift>r"),
            (n_("Crop"), "<Control>k"),
            (n_("Resize"), "<Control><Alt>r"),
            (n_("Adjust colour"), "<Control><Alt>c"),
            (n_("Save"), "<Control>s"),
            (n_("Save as"), "<Control><Shift>s"),
        ],
    ),
    (
        n_("Markup"),
        &[
            (n_("Delete the selected annotation"), "Delete"),
            (n_("Move the selected annotation"), "Left Right Up Down"),
            (n_("Move in bigger steps"), "<Shift>Up <Shift>Down"),
            (n_("Finish editing text"), "Return"),
            (n_("New line in text"), "<Shift>Return"),
            (n_("Apply the open tool"), "Return"),
            (n_("Cancel the open tool"), "Escape"),
        ],
    ),
];

pub fn show() {
    let dialog = adw::ShortcutsDialog::new();
    for (title, items) in SECTIONS {
        let section = adw::ShortcutsSection::new(Some(&tr(title)));
        for (name, accel) in *items {
            section.add(adw::ShortcutsItem::new(&tr(name), accel));
        }
        dialog.add(section);
    }
    dialog.present(relm4::main_application().active_window().as_ref());
}
