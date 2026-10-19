//! The preferences dialog and the About dialog.

use crate::i18n::{n_, tr};
use std::{cell::RefCell, rc::Rc};

use relm4::{
    adw::{self, prelude::*},
    gtk,
};

use crate::{
    APP_ID, APP_NAME,
    settings::{Background, DefaultZoom, Settings},
};

const ZOOMS: [(&str, DefaultZoom); 2] = [
    (n_("Fit to window"), DefaultZoom::FitToWindow),
    (n_("Actual size"), DefaultZoom::ActualSize),
];

const BACKGROUNDS: [(&str, Background); 4] = [
    (n_("Follow the system"), Background::System),
    (n_("Dark"), Background::Dark),
    (n_("Light"), Background::Light),
    (n_("Checkerboard"), Background::Checkerboard),
];

fn combo<T: Copy + PartialEq + 'static>(
    title: &str,
    subtitle: &str,
    items: &'static [(&'static str, T)],
    current: T,
) -> adw::ComboRow {
    let names: Vec<String> = items.iter().map(|(name, _)| tr(name)).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let row = adw::ComboRow::builder()
        .title(title)
        .subtitle(subtitle)
        .model(&gtk::StringList::new(&names))
        .build();
    row.set_selected(
        items
            .iter()
            .position(|(_, value)| *value == current)
            .unwrap_or(0) as u32,
    );
    row
}

/// Show the preferences. Every change is applied at once through `on_change`.
pub fn show_preferences(current: &Settings, on_change: impl Fn(Settings) + 'static) {
    let settings = Rc::new(RefCell::new(current.clone()));
    let on_change = Rc::new(on_change);
    // Apply an edit to the shared settings and report the result.
    let update = {
        let settings = Rc::clone(&settings);
        move |edit: &dyn Fn(&mut Settings)| {
            let mut settings = settings.borrow_mut();
            edit(&mut settings);
            on_change(settings.clone());
        }
    };
    let update = Rc::new(update);

    let viewing = adw::PreferencesGroup::builder()
        .title(tr("Viewing"))
        .build();
    let zoom = combo(
        &tr("Default zoom"),
        &tr("How a new picture is first shown"),
        &ZOOMS,
        current.default_zoom,
    );
    zoom.connect_selected_notify({
        let update = Rc::clone(&update);
        move |row| {
            let value = ZOOMS[row.selected() as usize % ZOOMS.len()].1;
            update(&|s| s.default_zoom = value);
        }
    });
    viewing.add(&zoom);

    let background = combo(
        &tr("Background"),
        &tr("Behind pictures and pages"),
        &BACKGROUNDS,
        current.background,
    );
    background.connect_selected_notify({
        let update = Rc::clone(&update);
        move |row| {
            let value = BACKGROUNDS[row.selected() as usize % BACKGROUNDS.len()].1;
            update(&|s| s.background = value);
        }
    });
    viewing.add(&background);

    let smooth = adw::SwitchRow::builder()
        .title(tr("Smooth zoom"))
        .subtitle(tr("Animate zoom changes"))
        .active(current.smooth_zoom)
        .build();
    smooth.connect_active_notify({
        let update = Rc::clone(&update);
        move |row| {
            let value = row.is_active();
            update(&|s| s.smooth_zoom = value);
        }
    });
    viewing.add(&smooth);

    let slideshow = adw::PreferencesGroup::builder()
        .title(tr("Slideshow"))
        .build();
    let seconds = adw::SpinRow::builder()
        .title(tr("Time per picture"))
        .subtitle(tr("Seconds before moving on"))
        .adjustment(&gtk::Adjustment::new(
            current.slideshow_seconds as f64,
            1.0,
            120.0,
            1.0,
            5.0,
            0.0,
        ))
        .build();
    seconds.connect_value_notify({
        let update = Rc::clone(&update);
        move |row| {
            let value = row.value() as u32;
            update(&|s| s.slideshow_seconds = value);
        }
    });
    slideshow.add(&seconds);

    let general = adw::PreferencesGroup::builder()
        .title(tr("General"))
        .build();
    let remember = adw::SwitchRow::builder()
        .title(tr("Remember window and place"))
        .subtitle(tr(
            "Window size, the sidebar and the page a PDF was left on",
        ))
        .active(current.remember_state)
        .build();
    remember.connect_active_notify({
        let update = Rc::clone(&update);
        move |row| {
            let value = row.is_active();
            update(&|s| s.remember_state = value);
        }
    });
    general.add(&remember);

    let page = adw::PreferencesPage::new();
    page.add(&viewing);
    page.add(&slideshow);
    page.add(&general);
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title(&tr("Preferences"));
    dialog.add(&page);
    dialog.present(relm4::main_application().active_window().as_ref());
}

pub fn show_about() {
    let about = adw::AboutDialog::builder()
        .application_name(APP_NAME)
        .application_icon(APP_ID)
        .version(env!("CARGO_PKG_VERSION"))
        .developer_name("Imanol Gomez")
        .comments(tr(
            "A fast image and PDF viewer with light editing and markup.",
        ))
        .website("https://github.com/ImanolGo/lumen")
        .issue_url("https://github.com/ImanolGo/lumen/issues")
        .license_type(gtk::License::Gpl30)
        .copyright("© Imanol Gomez")
        .build();
    about.present(relm4::main_application().active_window().as_ref());
}
