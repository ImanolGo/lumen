//! Everything that talks to the operating system, behind the same functions on every system: the
//! rest of the code never names one.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as system;

#[cfg(not(target_os = "linux"))]
mod desktop;
#[cfg(not(target_os = "linux"))]
use desktop as system;

pub mod keys;
#[cfg(unix)]
pub mod unix_fs;
#[cfg(unix)]
pub mod unix_print;

pub use system::{
    clipboard, confine, control, dialogs, dirs, fs, menu_bar, print, shell, startup, thumbs, trash, watch,
};

/// What this system can do, for the interface to show only that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code, reason = "some are for the interface packages of docs/improvement-plan.md")]
pub struct Caps {
    /// Pictures are decoded in a process that is shut in.
    pub sandbox: bool,
    /// What went to the trash can be brought back from here.
    pub trash_undo: bool,
    /// The system can offer a choice of programs to open a file with.
    pub open_with: bool,
    /// Lumen's own print dialog, which sends to the printer itself.
    pub direct_print: bool,
    /// A thumbnail cache shared with other programs.
    pub shared_thumbnails: bool,
    /// Whether to use the desktop's file and print dialogs is the person's choice.
    pub desktop_dialogs_optional: bool,
    /// Selecting text copies it for a middle click.
    pub primary_selection: bool,
    /// The system's menu bar has the commands.
    pub menu_bar: bool,
    /// Lumen's header is the window's title bar.
    pub own_title_bar: bool,
}

pub fn caps() -> Caps {
    system::CAPS
}

/// There is no dialog of the desktop's: Lumen shows its own instead.
#[derive(Debug, Clone, Copy)]
pub struct NoDialog;

impl std::fmt::Display for NoDialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("no file chooser portal is available")
    }
}

impl std::error::Error for NoDialog {}

/// One menu of the system's menu bar. The first is the program's own.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct MenuGroup {
    pub title: String,
    /// `None` is a line between items.
    pub items: Vec<Option<MenuEntry>>,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct MenuEntry {
    /// What [`menu_bar::chosen`] says when it is picked.
    pub id: &'static str,
    pub label: String,
    /// As "Ctrl+Shift+S"; empty for none.
    pub keys: &'static str,
}
