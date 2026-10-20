//! Small free functions of the main window: dialogs, recent files, key handling, screenshots.

use super::*;

/// Ask for the password of an encrypted PDF; the answer comes back as a message.
pub(super) fn ask_password(name: &str, retry: bool, sender: ComponentSender<App>) {
    let body = if retry {
        tr_fmt(
            "The password for “{name}” was incorrect. Try again.",
            &[("name", name)],
        )
    } else {
        tr_fmt(
            "“{name}” is protected. Enter its password to open it.",
            &[("name", name)],
        )
    };
    let dialog = adw::AlertDialog::new(Some(&tr("Password Required")), Some(&body));
    crate::i18n::add_responses(
        &dialog,
        &[
            ("cancel", crate::i18n::n_("_Cancel")),
            ("open", crate::i18n::n_("_Open")),
        ],
    );
    dialog.set_response_appearance("open", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("open"));
    dialog.set_close_response("cancel");

    let entry = gtk::PasswordEntry::builder()
        .show_peek_icon(true)
        .activates_default(true)
        .build();
    dialog.set_extra_child(Some(&entry));

    let parent = relm4::main_application().active_window();
    entry.grab_focus();
    dialog.choose(parent.as_ref(), gio::Cancellable::NONE, move |response| {
        if response == "open" {
            sender.input(AppMsg::OpenWithPassword(entry.text().to_string()));
        } else {
            sender.input(AppMsg::PasswordCancelled);
        }
    });
}

pub(super) fn open_logged(
    path: &std::path::Path,
    password: Option<&str>,
) -> Result<Opened, OpenError> {
    document::open(path, password).inspect_err(|err| {
        // Needing a password is expected, not a failure worth logging.
        if let OpenError::Other(message) = err {
            tracing::warn!(path = %path.display(), "open failed: {message}");
        }
    })
}

/// The files the desktop lists as recent that still exist and can be opened, newest first.
pub(super) fn recent_files() -> Vec<gtk::RecentInfo> {
    let mut items: Vec<_> = gtk::RecentManager::default()
        .items()
        .into_iter()
        .filter(|item| {
            item.is_local() && item.exists() && DocumentKind::from_mime(&item.mime_type()).is_some()
        })
        .collect();
    items.sort_by_key(|item| std::cmp::Reverse(item.modified().to_unix()));
    items
}

/// Rebuild the "Recent Files" menu from the desktop-wide recent list.
pub(super) fn refresh_recent_menu(menu: &gio::Menu) {
    menu.remove_all();

    for info in recent_files().into_iter().take(RECENT_LIMIT) {
        let item = gio::MenuItem::new(Some(&info.display_name()), None);
        item.set_action_and_target_value(Some("win.open-recent"), Some(&info.uri().to_variant()));
        menu.append_item(&item);
    }

    if menu.n_items() == 0 {
        // No such action, so the entry shows up disabled.
        menu.append(Some(&tr("No Recent Files")), Some("win.none"));
    }
}

/// How many recent files the empty window shows.
pub(super) const RECENT_ON_START: usize = 6;

/// Fill the empty window's row of recent files, each with its thumbnail (when the desktop
/// has one) and its name. Clicking one opens it.
pub(super) fn refresh_recent_row(row: &gtk::Box, sender: &ComponentSender<App>) {
    while let Some(child) = row.first_child() {
        row.remove(&child);
    }
    for info in recent_files().into_iter().take(RECENT_ON_START) {
        let Some(path) = gio::File::for_uri(&info.uri()).path() else {
            continue;
        };
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .width_request(112)
            .height_request(84)
            .overflow(gtk::Overflow::Hidden)
            .css_classes(["card"])
            .build();
        match crate::thumbnails::cached(&path) {
            Some(texture) => picture.set_paintable(Some(&texture)),
            None => {
                let icon = if DocumentKind::from_mime(&info.mime_type()) == Some(DocumentKind::Pdf)
                {
                    "x-office-document-symbolic"
                } else {
                    "image-x-generic-symbolic"
                };
                if let Some(display) = gdk::Display::default() {
                    let paintable = gtk::IconTheme::for_display(&display).lookup_icon(
                        icon,
                        &[],
                        48,
                        picture.scale_factor(),
                        gtk::TextDirection::None,
                        gtk::IconLookupFlags::empty(),
                    );
                    picture.set_paintable(Some(&paintable));
                    picture.set_content_fit(gtk::ContentFit::ScaleDown);
                }
            }
        }
        let label = gtk::Label::builder()
            .label(info.display_name())
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .max_width_chars(14)
            .css_classes(["caption"])
            .build();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        content.append(&picture);
        content.append(&label);
        let button = gtk::Button::builder()
            .child(&content)
            .css_classes(["flat"])
            .tooltip_text(path.display().to_string())
            .build();
        let sender = sender.clone();
        button.connect_clicked(move |_| sender.input(AppMsg::Open(vec![path.clone()])));
        row.append(&button);
    }
}

/// A circle of `color` (or of the shared current colour, for `None`), `size` pixels wide.
/// A fixed colour gets a ring around it while it is the current one.
pub(super) fn color_circle(
    color: Option<annotate::Color>,
    current: &Rc<Cell<annotate::Color>>,
    size: i32,
) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .content_width(size)
        .content_height(size)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    let current = Rc::clone(current);
    area.set_draw_func(move |area, cr, w, h| {
        let shown = color.unwrap_or_else(|| current.get());
        let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
        let ring = color.is_some_and(|c| c == current.get());
        let radius = cx.min(cy) - if color.is_some() { 4.0 } else { 1.0 };
        cr.arc(cx, cy, radius, 0.0, std::f64::consts::TAU);
        cr.set_source_rgba(
            shown.r as f64,
            shown.g as f64,
            shown.b as f64,
            shown.a as f64,
        );
        let _ = cr.fill_preserve();
        // A faint edge, so white and black still read on any background.
        let fg = area.color();
        cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.2);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
        if ring {
            cr.arc(cx, cy, radius + 2.5, 0.0, std::f64::consts::TAU);
            cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.9);
            cr.set_line_width(2.0);
            let _ = cr.stroke();
        }
    });
    area
}

/// Move the newest trashed file that came from `path` back there.
pub(super) fn restore_from_trash(path: &std::path::Path) -> Result<(), String> {
    let trash = gio::File::for_uri("trash:///");
    let items = trash
        .enumerate_children(
            "standard::name,trash::orig-path,trash::deletion-date",
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )
        .map_err(|e| e.to_string())?;
    let wanted = path.to_string_lossy();
    let newest = items
        .flatten()
        .filter(|info| {
            info.attribute_as_string("trash::orig-path")
                .is_some_and(|p| p.as_str() == wanted)
        })
        .max_by_key(|info| {
            info.attribute_string("trash::deletion-date")
                .map(|d| d.to_string())
        })
        .ok_or_else(|| tr("The file is no longer in the trash."))?;
    trash
        .child(newest.name())
        .move_(
            &gio::File::for_path(path),
            gio::FileCopyFlags::NONE,
            gio::Cancellable::NONE,
            None,
        )
        .map_err(|e| e.to_string())
}

/// Whether the keyboard focus is in `entry` (a `GtkEntry` holds its text in a child).
pub(super) fn entry_has_focus(entry: &gtk::Entry) -> bool {
    entry
        .root()
        .and_then(|root| root.focus())
        .is_some_and(|focus| {
            focus.is_ancestor(entry) || focus == *entry.upcast_ref::<gtk::Widget>()
        })
}

/// Render the window, as it is on screen, into a PNG file.
pub(super) fn screenshot(window: &adw::ApplicationWindow, path: &str) -> Result<(), String> {
    let (width, height) = (window.width(), window.height());
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, width as f64, height as f64);
    let node = snapshot.to_node().ok_or("the window has nothing to draw")?;
    let renderer = window.renderer().ok_or("the window is not realized")?;
    let bounds = gtk::graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
    renderer
        .render_texture(node, Some(&bounds))
        .save_to_png(path)
        .map_err(|err| err.to_string())
}

/// Whether `accel` is a key without modifiers that text entries also use (not a function key).
pub(super) fn is_plain_key(accel: &str) -> bool {
    // Shift alone still types characters.
    let key = accel.strip_prefix("<Shift>").unwrap_or(accel);
    !key.starts_with('<') && !(key.starts_with('F') && key[1..].parse::<u8>().is_ok())
}

/// Whether keys should go to something other than the view: a text field, a dialog
/// shown over the window, or an open menu.
pub(super) fn typing(window: &gtk::ApplicationWindow) -> bool {
    let Some(focus) = gtk::prelude::RootExt::focus(window) else {
        return false;
    };
    let in_window = focus.native().is_some_and(|native| {
        native.upcast_ref::<gtk::Widget>() == window.upcast_ref::<gtk::Widget>()
    });
    focus.is::<gtk::Text>()
        || focus.is::<gtk::TextView>()
        || focus.ancestor(adw::Dialog::static_type()).is_some()
        || !in_window
}

/// Give buttons that show only an icon an accessible name, taken from their tooltip, so
/// screen readers announce "Print" instead of nothing. Called once on the finished window.
pub(super) fn label_icon_buttons(widget: &gtk::Widget) {
    let icon_only = widget
        .downcast_ref::<gtk::Button>()
        .is_some_and(|b| b.label().is_none())
        || widget.is::<gtk::MenuButton>()
        || widget.is::<gtk::ColorDialogButton>();
    if let (true, Some(tooltip)) = (icon_only, widget.tooltip_text()) {
        widget.update_property(&[gtk::accessible::Property::Label(&tooltip)]);
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        label_icon_buttons(&current);
        child = current.next_sibling();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "needs a trash on the same file system: run with LUMEN_TRASH_DIR=~/some/dir"]
    fn a_trashed_file_comes_back() {
        use relm4::gtk::prelude::FileExt;
        let dir = std::path::PathBuf::from(std::env::var("LUMEN_TRASH_DIR").unwrap());
        let file = dir.join(format!("restore-{}.txt", std::process::id()));
        std::fs::write(&file, "x").unwrap();
        relm4::gtk::gio::File::for_path(&file)
            .trash(relm4::gtk::gio::Cancellable::NONE)
            .unwrap();
        assert!(!file.exists());
        super::restore_from_trash(&file).unwrap();
        assert!(file.exists());
        std::fs::remove_file(&file).unwrap();
    }

    use super::is_plain_key;

    #[test]
    fn plain_keys_are_those_text_entries_use() {
        for key in [
            "Right",
            "space",
            "<Shift>space",
            "BackSpace",
            "Page_Down",
            "Home",
            "Escape",
        ] {
            assert!(is_plain_key(key), "{key}");
        }
        for key in [
            "<Control>p",
            "<Shift>F3",
            "F3",
            "F11",
            "<Alt>Return",
            "<Control><Shift>z",
        ] {
            assert!(!is_plain_key(key), "{key}");
        }
    }
}
