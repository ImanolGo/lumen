//! Linux: XDG folders, desktop portals, the freedesktop trash and thumbnail cache, inotify, Wayland's
//! clipboard, CUPS, Landlock and seccomp, D-Bus.

pub mod clipboard;
pub mod confine;
pub mod control;
pub mod dirs;
pub mod portal;
pub mod print;
pub mod startup;
pub mod thumbs;
pub mod trash;
pub mod watch;

pub mod dialogs {
    pub use super::portal::{choose, save_as};
}

pub mod shell {
    pub use super::portal::{open_uri, open_with, show_in_folder};
}

pub use super::unix_fs as fs;

pub const CAPS: super::Caps = super::Caps {
    sandbox: true,
    trash_undo: true,
    open_with: true,
    direct_print: true,
    shared_thumbnails: true,
    desktop_dialogs_optional: true,
    primary_selection: true,
    menu_bar: false,
    own_title_bar: true,
    frameless: true,
    title_in_header: true,
    title_centered: true,
    leading_inset: 0.0,
    confirm_first: false,
};

pub mod menu_bar {
    use crate::platform::MenuGroup;

    pub fn install(_groups: &[MenuGroup]) {}

    pub fn chosen() -> impl futures::Stream<Item = String> {
        futures::stream::pending()
    }
}
