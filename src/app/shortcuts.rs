//! The keyboard shortcuts window.

use relm4::adw::{self, prelude::*};

/// Sections of (title, accelerator) pairs, in the order they are shown.
const SECTIONS: &[(&str, &[(&str, &str)])] = &[
    (
        "General",
        &[
            ("Open", "<Control>o"),
            ("Print", "<Control>p"),
            ("Toggle markup toolbar", "<Control><Shift>a"),
            ("Preferences", "<Control>comma"),
            ("Keyboard shortcuts", "<Control>question"),
            ("Fullscreen", "F11"),
            ("Slideshow", "F5"),
            ("Toggle sidebar", "F9"),
            ("Properties", "<Alt>Return"),
            ("Close window", "<Control>w"),
        ],
    ),
    (
        "Pictures and zoom",
        &[
            ("Next file", "Right space"),
            ("Previous file", "Left BackSpace"),
            ("Zoom in", "<Control>plus"),
            ("Zoom out", "<Control>minus"),
            ("Fit to window", "<Control>0"),
            ("Actual size", "<Control>1"),
        ],
    ),
    (
        "PDF documents",
        &[
            ("Next page", "Page_Down"),
            ("Previous page", "Page_Up"),
            ("First page", "Home"),
            ("Last page", "End"),
            ("Go to page", "<Control>g"),
            ("Find", "<Control>f"),
            ("Next match", "F3"),
            ("Previous match", "<Shift>F3"),
            ("Copy selected text", "<Control>c"),
            ("Delete selected pages (in the sidebar)", "Delete"),
        ],
    ),
    (
        "Editing",
        &[
            ("Undo", "<Control>z"),
            ("Redo", "<Control><Shift>z"),
            ("Rotate right", "<Control>r"),
            ("Rotate left", "<Control><Shift>r"),
            ("Crop", "<Control>k"),
            ("Resize", "<Control><Alt>r"),
            ("Adjust colour", "<Control><Alt>c"),
            ("Save", "<Control>s"),
            ("Save as", "<Control><Shift>s"),
        ],
    ),
    (
        "Markup",
        &[
            ("Delete the selected annotation", "Delete"),
            ("Move the selected annotation", "Left Right Up Down"),
            ("Move in bigger steps", "<Shift>Up <Shift>Down"),
            ("Finish editing text", "<Control>Return"),
            ("Apply the open tool", "Return"),
            ("Cancel the open tool", "Escape"),
        ],
    ),
];

pub fn show() {
    let dialog = adw::ShortcutsDialog::new();
    for (title, items) in SECTIONS {
        let section = adw::ShortcutsSection::new(Some(title));
        for (name, accel) in *items {
            section.add(adw::ShortcutsItem::new(name, accel));
        }
        dialog.add(section);
    }
    dialog.present(relm4::main_application().active_window().as_ref());
}
