//! The menus of the header bar and the main menu.

use super::*;

/// The menus the window shows.
pub(super) struct Menus {
    /// Picture edits.
    pub edit: gio::Menu,
    /// PDF page changes.
    pub pages: gio::Menu,
    pub main: gio::Menu,
}

pub(super) fn build() -> Menus {
    let edit_menu = gio::Menu::new();
    let history = gio::Menu::new();
    history.append(Some(&tr("Undo")), Some("win.undo"));
    history.append(Some(&tr("Redo")), Some("win.redo"));
    edit_menu.append_section(None, &history);
    let geometry = gio::Menu::new();
    geometry.append(Some(&tr("Rotate Right")), Some("win.rotate-right"));
    geometry.append(Some(&tr("Rotate Left")), Some("win.rotate-left"));
    geometry.append(Some(&tr("Flip Horizontally")), Some("win.flip-h"));
    geometry.append(Some(&tr("Flip Vertically")), Some("win.flip-v"));
    edit_menu.append_section(None, &geometry);
    let tools = gio::Menu::new();
    tools.append(Some(&tr("Crop…")), Some("win.crop"));
    tools.append(Some(&tr("Resize…")), Some("win.resize"));
    tools.append(Some(&tr("Adjust Colour…")), Some("win.adjust"));
    edit_menu.append_section(None, &tools);
    let export_section = gio::Menu::new();
    export_section.append(Some(&tr("Export as PDF…")), Some("win.export-image-pdf"));
    edit_menu.append_section(None, &export_section);
    let markup_section = gio::Menu::new();
    markup_section.append(Some(&tr("Markup")), Some("win.markup"));
    edit_menu.append_section(None, &markup_section);
    let saving = gio::Menu::new();
    saving.append(Some(&tr("Save")), Some("win.save"));
    saving.append(Some(&tr("Save As…")), Some("win.save-as"));
    edit_menu.append_section(None, &saving);

    let pages_menu = gio::Menu::new();
    pages_menu.append_section(None, &history);
    let page_turns = gio::Menu::new();
    page_turns.append(Some(&tr("Rotate Right")), Some("win.rotate-right"));
    page_turns.append(Some(&tr("Rotate Left")), Some("win.rotate-left"));
    pages_menu.append_section(None, &page_turns);
    let page_edits = gio::Menu::new();
    page_edits.append(Some(&tr("Delete Pages")), Some("win.delete-pages"));
    page_edits.append(
        Some(&tr("Insert Pages from File…")),
        Some("win.insert-pages"),
    );
    pages_menu.append_section(None, &page_edits);
    let page_exports = gio::Menu::new();
    page_exports.append(
        Some(&tr("Export Selected Pages…")),
        Some("win.export-pages"),
    );
    page_exports.append(Some(&tr("Export Page as PNG…")), Some("win.export-png"));
    pages_menu.append_section(None, &page_exports);
    pages_menu.append_section(None, &saving);

    let main_menu = gio::Menu::new();
    let print_section = gio::Menu::new();
    print_section.append(Some(&tr("Print…")), Some("win.print"));
    let paper_size = gio::Menu::new();
    paper_size.append(Some(&tr("Fit to Page")), Some("win.print-scaling::fit"));
    paper_size.append(Some(&tr("Actual Size")), Some("win.print-scaling::actual"));
    print_section.append_submenu(Some(&tr("Size on Paper")), &paper_size);
    main_menu.append_section(None, &print_section);
    let pdf_view = gio::Menu::new();
    pdf_view.append(Some(&tr("Fit Width")), Some("win.pdf-fit::width"));
    pdf_view.append(Some(&tr("Fit Page")), Some("win.pdf-fit::page"));
    pdf_view.append(Some(&tr("Night Mode")), Some("win.night-mode"));
    main_menu.append_section(Some(&tr("PDF View")), &pdf_view);
    let view_section = gio::Menu::new();
    view_section.append(Some(&tr("Fullscreen")), Some("win.fullscreen"));
    view_section.append(Some(&tr("Slideshow")), Some("win.slideshow"));
    main_menu.append_section(None, &view_section);
    let file_section = gio::Menu::new();
    file_section.append(Some(&tr("Properties")), Some("win.toggle-properties"));
    file_section.append(Some(&tr("Show in Folder")), Some("win.show-in-folder"));
    file_section.append(Some(&tr("Open With…")), Some("win.open-with"));
    file_section.append(Some(&tr("Move to Trash")), Some("win.trash"));
    main_menu.append_section(None, &file_section);
    let prefs_section = gio::Menu::new();
    prefs_section.append(Some(&tr("Preferences")), Some("win.preferences"));
    main_menu.append_section(None, &prefs_section);
    let about_section = gio::Menu::new();
    about_section.append(Some(&tr("Keyboard Shortcuts")), Some("win.shortcuts"));
    about_section.append(Some(&tr("About Lumen")), Some("win.about"));
    main_menu.append_section(None, &about_section);
    Menus {
        edit: edit_menu,
        pages: pages_menu,
        main: main_menu,
    }
}
