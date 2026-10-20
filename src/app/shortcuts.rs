//! The keyboard shortcuts: the one table of accelerators the window registers, and the
//! shortcuts dialog built from it, so the two cannot drift apart.

use relm4::adw::{self, prelude::*};

use crate::i18n::{n_, tr};

/// Every window action's accelerators. `app.rs` registers them from here, and the dialog
/// below looks them up by action name.
pub const ACCELS: &[(&str, &[&str])] = &[
    ("page-next", &["Page_Down"]),
    ("page-prev", &["Page_Up"]),
    ("page-first", &["Home"]),
    ("page-last", &["End"]),
    ("tool-select", &["v"]),
    ("tool-rectangle", &["r"]),
    ("tool-ellipse", &["o"]),
    ("tool-arrow", &["a"]),
    ("tool-line", &["l"]),
    ("tool-pen", &["p"]),
    ("tool-text", &["t"]),
    ("tool-highlight", &["h"]),
    ("trash", &["<Control>Delete"]),
    ("show-in-folder", &[]),
    ("open-with", &[]),
    ("jump-back", &["<Alt>Left"]),
    ("jump-forward", &["<Alt>Right"]),
    ("screen-down", &["space"]),
    ("screen-up", &["<Shift>space"]),
    ("next", &["Right"]),
    ("prev", &["Left", "BackSpace"]),
    ("toggle-sidebar", &["F9"]),
    ("toggle-properties", &["<Alt>Return"]),
    ("find-next", &["F3"]),
    ("find-prev", &["<Shift>F3"]),
    ("find-close", &[]),
    ("copy", &[]),
    ("print", &["<Control>p"]),
    ("undo", &["<Control>z"]),
    ("redo", &["<Control><Shift>z", "<Control>y"]),
    ("rotate-right", &["<Control>r"]),
    ("rotate-left", &["<Control><Shift>r"]),
    ("flip-h", &[]),
    ("flip-v", &[]),
    ("crop", &["<Control>k"]),
    ("resize", &["<Control><Alt>r"]),
    ("adjust", &["<Control><Alt>c"]),
    ("save", &["<Control>s"]),
    ("save-as", &["<Control><Shift>s"]),
    ("apply-tool", &[]),
    ("markup", &["<Control><Shift>a"]),
    ("preferences", &["<Control>comma"]),
    ("fullscreen", &["F11"]),
    ("slideshow", &["F5"]),
    ("leave-presentation", &[]),
    ("about", &[]),
    ("shortcuts", &["<Control>question", "<Control>slash"]),
    ("delete-pages", &[]),
    ("insert-pages", &[]),
    ("export-pages", &[]),
    ("export-png", &[]),
    ("export-image-pdf", &[]),
    ("zoom-in", &["<Control>plus", "<Control>equal"]),
    ("zoom-out", &["<Control>minus"]),
    ("best-fit", &["<Control>0"]),
    ("actual-size", &["<Control>1"]),
    ("night-mode", &["<Control><Shift>n"]),
    ("find", &["<Control>f"]),
    ("goto-page", &["<Control>g"]),
    ("open", &["<Control>o"]),
];

/// The accelerators of window action `name`.
pub fn accels(name: &str) -> &'static [&'static str] {
    ACCELS
        .iter()
        .find(|(action, _)| *action == name)
        .map_or(&[], |(_, accels)| accels)
}

/// What a row of the dialog shows: the keys of a window action, or keys that are handled
/// elsewhere (a text field, the canvas, a list) and so have no action.
#[derive(Clone, Copy)]
enum Keys {
    Action(&'static str),
    Literal(&'static str),
}
use Keys::{Action, Literal};

/// The accelerator text of one row: all the keys of an action, space separated.
fn keys_text(keys: Keys) -> String {
    match keys {
        Action(name) => accels(name).join(" "),
        Literal(text) => text.to_owned(),
    }
}

/// Sections of (title, keys) rows, in the order they are shown.
const SECTIONS: &[(&str, &[(&str, Keys)])] = &[
    (
        n_("General"),
        &[
            (n_("Open"), Action("open")),
            (n_("Print"), Action("print")),
            (n_("Toggle markup toolbar"), Action("markup")),
            (n_("Preferences"), Action("preferences")),
            (n_("Keyboard shortcuts"), Action("shortcuts")),
            (n_("Fullscreen"), Action("fullscreen")),
            (n_("Slideshow"), Action("slideshow")),
            (n_("Leave fullscreen or the slideshow"), Literal("Escape")),
            (n_("Toggle sidebar"), Action("toggle-sidebar")),
            (n_("Properties"), Action("toggle-properties")),
            (n_("Move the file to the trash"), Action("trash")),
            (n_("Close window"), Literal("<Control>w")),
        ],
    ),
    (
        n_("Pictures and zoom"),
        &[
            (n_("Next file"), Action("next")),
            (n_("First file"), Literal("Home")),
            (n_("Last file"), Literal("End")),
            (n_("Previous file"), Action("prev")),
            (n_("Zoom in"), Action("zoom-in")),
            (n_("Zoom out"), Action("zoom-out")),
            (n_("Fit to window"), Action("best-fit")),
            (n_("Actual size"), Action("actual-size")),
            (n_("Copy the picture"), Literal("<Control>c")),
        ],
    ),
    (
        n_("PDF documents"),
        &[
            (n_("Scroll down a screen"), Action("screen-down")),
            (n_("Scroll up a screen"), Action("screen-up")),
            (n_("Next page"), Action("page-next")),
            (n_("Previous page"), Action("page-prev")),
            (n_("First page"), Action("page-first")),
            (n_("Last page"), Action("page-last")),
            (n_("Go to page"), Action("goto-page")),
            (n_("Back after following a link"), Action("jump-back")),
            (n_("Forward again"), Action("jump-forward")),
            (n_("Night mode"), Action("night-mode")),
            (n_("Find"), Action("find")),
            (n_("Next match"), Action("find-next")),
            (n_("Previous match"), Action("find-prev")),
            (n_("Copy selected text"), Literal("<Control>c")),
            (
                n_("Delete selected pages (in the sidebar)"),
                Literal("Delete"),
            ),
        ],
    ),
    (
        n_("Editing"),
        &[
            (n_("Undo"), Action("undo")),
            (n_("Redo"), Action("redo")),
            (n_("Rotate right"), Action("rotate-right")),
            (n_("Rotate left"), Action("rotate-left")),
            (n_("Crop"), Action("crop")),
            (n_("Resize"), Action("resize")),
            (n_("Adjust colour"), Action("adjust")),
            (n_("Save"), Action("save")),
            (n_("Save as"), Action("save-as")),
        ],
    ),
    (
        n_("Markup"),
        &[
            (n_("Delete the selected annotation"), Literal("Delete")),
            (
                n_("Move the selected annotation"),
                Literal("Left Right Up Down"),
            ),
            (
                n_("Move in bigger steps"),
                Literal("<Shift>Left <Shift>Right <Shift>Up <Shift>Down"),
            ),
            (
                n_("Choose a tool: select, rectangle, ellipse, arrow, line, pen, text, highlight"),
                Literal("v r o a l p t h"),
            ),
            (n_("Finish editing text"), Literal("Return")),
            (n_("New line in text"), Literal("<Shift>Return")),
            (n_("Apply the open tool"), Literal("Return")),
            (n_("Cancel the open tool"), Literal("Escape")),
        ],
    ),
];

pub fn show() {
    let dialog = adw::ShortcutsDialog::new();
    for (title, items) in SECTIONS {
        let section = adw::ShortcutsSection::new(Some(&tr(title)));
        for (name, keys) in *items {
            section.add(adw::ShortcutsItem::new(&tr(name), &keys_text(*keys)));
        }
        dialog.add(section);
    }
    dialog.present(relm4::main_application().active_window().as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every action that has a shortcut is in the dialog, so a new one cannot be forgotten.
    #[test]
    fn every_shortcut_is_listed_in_the_dialog() {
        let listed: Vec<&str> = SECTIONS
            .iter()
            .flat_map(|(_, rows)| rows.iter())
            .filter_map(|(_, keys)| match keys {
                Action(name) => Some(*name),
                Literal(_) => None,
            })
            .collect();
        // Tool letters, Home/End (shown as plain keys) and the like are listed as text.
        let described_as_text = [
            "page-first",
            "page-last",
            "close",
            "tool-select",
            "tool-rectangle",
            "tool-ellipse",
            "tool-arrow",
            "tool-line",
            "tool-pen",
            "tool-text",
            "tool-highlight",
        ];
        for (action, accels) in ACCELS {
            if !accels.is_empty() && !described_as_text.contains(action) {
                assert!(
                    listed.contains(action),
                    "{action} is missing from the dialog"
                );
            }
        }
        for name in &listed {
            assert!(
                !accels(name).is_empty(),
                "the dialog lists {name}, which has no shortcut"
            );
        }
    }

    #[test]
    fn no_two_actions_share_an_accelerator() {
        let mut seen = std::collections::HashMap::new();
        for (action, accels) in ACCELS {
            for accel in *accels {
                if let Some(other) = seen.insert(*accel, *action) {
                    // Home and End mean the first/last page in a PDF and file in a picture:
                    // one action each, so these never collide.
                    panic!("{accel} is used by both {other} and {action}");
                }
            }
        }
    }
}
