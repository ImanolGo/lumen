//! Everything that talks to the operating system, behind the same functions on every system: the
//! rest of the code never names one.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as system;

#[cfg(not(target_os = "linux"))]
mod other;
#[cfg(not(target_os = "linux"))]
use other as system;

pub use system::{clipboard, confine, control, dialogs, dirs, fs, print, shell, startup, thumbs, trash, watch};

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
    /// Selecting text copies it for a middle click.
    pub primary_selection: bool,
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
