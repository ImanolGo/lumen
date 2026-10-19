//! Root Relm4 component: the main window.

use std::{cell::RefCell, path::PathBuf};

use relm4::{
    ComponentParts, ComponentSender, SimpleComponent,
    adw::{self, prelude::*},
    gtk::{self, gdk, gio, glib},
};

use crate::{
    APP_NAME,
    document::{self, DocumentInfo, DocumentKind},
    image::ImageDocument,
    widgets::LumenCanvas,
};

/// Action name, canvas handler and keyboard accelerators.
type ViewAction<'a> = (&'a str, fn(&LumenCanvas), &'a [&'a str]);

const RECENT_LIMIT: usize = 10;

thread_local! {
    /// Lets `open_files` (called from the GApplication `open` signal) reach the component.
    static INPUT: RefCell<Option<relm4::Sender<AppMsg>>> = const { RefCell::new(None) };
}

/// Open the first local file of `files` in the main window.
pub fn open_files(files: &[gio::File]) {
    let Some(path) = files.iter().find_map(gio::File::path) else {
        return;
    };
    INPUT.with(|input| {
        if let Some(sender) = input.borrow().as_ref() {
            sender.emit(AppMsg::Open(path));
        }
    });
}

pub struct App {
    recent_menu: gio::Menu,
    document: Option<DocumentInfo>,
    image: Option<ImageDocument>,
    error: Option<String>,
}

#[derive(Debug)]
pub enum AppMsg {
    /// Show the file chooser.
    OpenDialog,
    /// Open a specific file (from the dialog, CLI, and later drag-and-drop).
    Open(PathBuf),
}

impl App {
    fn title(&self) -> String {
        self.document
            .as_ref()
            .map(DocumentInfo::file_name)
            .unwrap_or_else(|| APP_NAME.to_owned())
    }

    fn subtitle(&self) -> String {
        self.document
            .as_ref()
            .map(|d| d.summary.clone())
            .unwrap_or_default()
    }

    fn page(&self) -> &'static str {
        if self.image.is_some() { "image" } else { "empty" }
    }

    fn status_icon(&self) -> &'static str {
        match (&self.error, &self.document) {
            (Some(_), _) => "dialog-error-symbolic",
            (None, Some(d)) if d.kind == DocumentKind::Pdf => "x-office-document-symbolic",
            (None, Some(_)) => "image-x-generic-symbolic",
            (None, None) => "document-open-symbolic",
        }
    }

    fn status_title(&self) -> String {
        match (&self.error, &self.document) {
            (Some(_), _) => "Could Not Open File".to_owned(),
            (None, Some(d)) => d.file_name(),
            (None, None) => "Open a Picture or PDF".to_owned(),
        }
    }

    fn status_description(&self) -> String {
        match (&self.error, &self.document) {
            (Some(err), _) => err.clone(),
            // Placeholder until the real viewers land in Phases 1 and 2.
            (None, Some(d)) => format!("{} — viewer coming in the next step", d.summary),
            (None, None) => "Press Ctrl+O or pass a file on the command line".to_owned(),
        }
    }

    fn show_open_dialog(sender: ComponentSender<Self>) {
        let images = gtk::FileFilter::new();
        images.set_name(Some("Images"));
        images.add_pixbuf_formats();

        let pdfs = gtk::FileFilter::new();
        pdfs.set_name(Some("PDF Documents"));
        pdfs.add_mime_type("application/pdf");

        let all = gtk::FileFilter::new();
        all.set_name(Some("Images and PDFs"));
        all.add_pixbuf_formats();
        all.add_mime_type("application/pdf");

        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&all);
        filters.append(&images);
        filters.append(&pdfs);

        let dialog = gtk::FileDialog::builder()
            .title("Open")
            .modal(true)
            .filters(&filters)
            .default_filter(&all)
            .build();

        let parent = relm4::main_application().active_window();
        dialog.open(parent.as_ref(), gio::Cancellable::NONE, move |result| {
            // Cancelling the dialog is reported as an error; nothing to do then.
            if let Some(path) = result.ok().and_then(|f| f.path()) {
                sender.input(AppMsg::Open(path));
            }
        });
    }
}

#[relm4::component(pub)]
impl SimpleComponent for App {
    type Init = ();
    type Input = AppMsg;
    type Output = ();

    view! {
        main_window = adw::ApplicationWindow {
            set_default_size: (1000, 720),
            #[watch]
            set_title: Some(&model.title()),

            #[wrap(Some)]
            set_content = &adw::ToolbarView {
                add_top_bar = &adw::HeaderBar {
                    pack_start = &gtk::Button {
                        set_icon_name: "document-open-symbolic",
                        set_tooltip_text: Some("Open… (Ctrl+O)"),
                        set_action_name: Some("win.open"),
                    },

                    pack_start = &gtk::MenuButton {
                        set_icon_name: "document-open-recent-symbolic",
                        set_tooltip_text: Some("Recent Files"),
                        set_menu_model: Some(&recent_menu),
                    },

                    #[wrap(Some)]
                    set_title_widget = &adw::WindowTitle {
                        #[watch]
                        set_title: &model.title(),
                        #[watch]
                        set_subtitle: &model.subtitle(),
                    },
                },

                #[wrap(Some)]
                set_content = &gtk::Stack {
                    add_named[Some("empty")] = &adw::StatusPage {
                        #[watch]
                        set_icon_name: Some(model.status_icon()),
                        #[watch]
                        set_title: &model.status_title(),
                        #[watch]
                        set_description: Some(&model.status_description()),
                    },

                    add_named[Some("image")] = &gtk::ScrolledWindow {
                        #[name = "canvas"]
                        #[wrap(Some)]
                        set_child = &LumenCanvas::default() {
                            #[watch]
                            set_texture: model.image.as_ref().map(|i| &i.texture),
                        },
                    },

                    #[watch]
                    set_visible_child_name: model.page(),
                },
            },
        }
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        INPUT.with(|input| *input.borrow_mut() = Some(sender.input_sender().clone()));

        let recent_menu = gio::Menu::new();
        refresh_recent_menu(&recent_menu);
        let model = App {
            recent_menu: recent_menu.clone(),
            document: None,
            image: None,
            error: None,
        };
        let widgets = view_output!();

        // Window actions + keyboard shortcuts.
        let open_action = gio::SimpleAction::new("open", None);
        let open_sender = sender.clone();
        open_action.connect_activate(move |_, _| open_sender.input(AppMsg::OpenDialog));
        widgets.main_window.add_action(&open_action);

        let canvas = &widgets.canvas;
        let view_actions: [ViewAction; 4] = [
            ("zoom-in", LumenCanvas::zoom_in, &["<Control>plus", "<Control>equal"]),
            ("zoom-out", LumenCanvas::zoom_out, &["<Control>minus"]),
            ("best-fit", LumenCanvas::best_fit, &["<Control>0"]),
            ("actual-size", LumenCanvas::actual_size, &["<Control>1"]),
        ];
        let app = relm4::main_application();
        for (name, handler, accels) in view_actions {
            let action = gio::SimpleAction::new(name, None);
            let canvas = canvas.clone();
            action.connect_activate(move |_, _| handler(&canvas));
            widgets.main_window.add_action(&action);
            app.set_accels_for_action(&format!("win.{name}"), accels);
        }
        app.set_accels_for_action("win.open", &["<Control>o"]);
        app.set_accels_for_action("window.close", &["<Control>w"]);

        let open_recent = gio::SimpleAction::new("open-recent", Some(glib::VariantTy::STRING));
        let recent_sender = sender.clone();
        open_recent.connect_activate(move |_, uri| {
            let path = uri
                .and_then(|u| u.str())
                .and_then(|u| gio::File::for_uri(u).path());
            if let Some(path) = path {
                recent_sender.input(AppMsg::Open(path));
            }
        });
        widgets.main_window.add_action(&open_recent);

        // Drop files onto the window to open them.
        let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        drop.connect_drop(move |_, value, _, _| {
            let Ok(list) = value.get::<gdk::FileList>() else {
                return false;
            };
            match list.files().iter().find_map(gio::File::path) {
                Some(path) => {
                    sender.input(AppMsg::Open(path));
                    true
                }
                None => false,
            }
        });
        widgets.main_window.add_controller(drop);

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>) {
        match msg {
            AppMsg::OpenDialog => Self::show_open_dialog(sender),
            AppMsg::Open(path) => match document::probe(&path) {
                Ok(info) => {
                    tracing::info!(path = %info.path.display(), kind = ?info.kind, "opened");
                    self.error = None;
                    self.image = None;
                    if info.kind == DocumentKind::Image {
                        match ImageDocument::load(&info.path) {
                            Ok(image) => self.image = Some(image),
                            Err(err) => self.error = Some(format!("{err:#}")),
                        }
                    }
                    if self.error.is_none() {
                        gtk::RecentManager::default().add_item(&gio::File::for_path(&info.path).uri());
                        refresh_recent_menu(&self.recent_menu);
                    }
                    self.document = Some(info);
                }
                Err(err) => {
                    tracing::warn!(path = %path.display(), "open failed: {err:#}");
                    self.document = None;
                    self.image = None;
                    self.error = Some(format!("{err:#}"));
                }
            },
        }
    }
}

/// Rebuild the "Recent Files" menu from the desktop-wide recent list.
fn refresh_recent_menu(menu: &gio::Menu) {
    menu.remove_all();

    let mut items: Vec<_> = gtk::RecentManager::default()
        .items()
        .into_iter()
        .filter(|item| {
            item.is_local()
                && item.exists()
                && DocumentKind::from_mime(&item.mime_type()).is_some()
        })
        .collect();
    items.sort_by_key(|item| std::cmp::Reverse(item.modified().to_unix()));

    for info in items.into_iter().take(RECENT_LIMIT) {
        let item = gio::MenuItem::new(Some(&info.display_name()), None);
        item.set_action_and_target_value(Some("win.open-recent"), Some(&info.uri().to_variant()));
        menu.append_item(&item);
    }

    if menu.n_items() == 0 {
        // No such action, so the entry shows up disabled.
        menu.append(Some("No Recent Files"), Some("win.none"));
    }
}
