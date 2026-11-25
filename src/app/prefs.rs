//! The preferences dialog (☰ → Preferences…). Every change takes effect at once and is saved.

use iced::theme::Mode;
use iced::widget::{column, container, row, text};
use iced::{Alignment, Element, Length, Task};

use super::{App, Message, TOOLBAR_H, muted};
use crate::i18n::tr;
use crate::settings::{self, PdfFit, Settings, Theme};
use crate::style;
use crate::ui;

#[derive(Debug, Clone, Copy)]
pub enum PrefMsg {
    Theme(Theme),
    SlideshowSeconds(u32),
    PdfFit(PdfFit),
    SidebarOpen(bool),
    FloatingControls(bool),
    Portals(bool),
    Sandbox(bool),
    WrapAround(bool),
    WindowButtons(bool),
    SelectOnDrag(bool),
    Language(&'static str),
    ThumbnailCache(bool),
    NightMode(bool),
    CacheMb(u32),
    PreloadMegapixels(u32),
    Reset,
}

/// The theme to draw with: `LUMEN_THEME` first, then the preference, then the desktop.
pub fn effective_mode(env: Option<&str>, pref: Theme, system: Mode) -> Mode {
    match (env, pref) {
        (Some("light"), _) => Mode::Light,
        (Some("dark"), _) => Mode::Dark,
        (_, Theme::Light) => Mode::Light,
        (_, Theme::Dark) => Mode::Dark,
        _ => system,
    }
}

impl App {
    pub(super) fn apply_theme(&mut self) {
        let env = std::env::var("LUMEN_THEME").ok();
        self.mode = effective_mode(env.as_deref(), self.prefs.theme, self.system_mode);
        self.theme = style::theme(self.mode);
    }

    pub(super) fn prefs_update(&mut self, msg: PrefMsg) -> Task<Message> {
        let mut follow_up = Task::none();
        match msg {
            PrefMsg::Theme(t) => {
                self.prefs.theme = t;
                self.apply_theme();
            }
            PrefMsg::SlideshowSeconds(s) => self.prefs.slideshow_seconds = s,
            PrefMsg::PdfFit(f) => self.prefs.pdf_fit = f,
            PrefMsg::SidebarOpen(on) => self.prefs.sidebar_open = on,
            PrefMsg::FloatingControls(on) => self.prefs.floating_controls = on,
            PrefMsg::Portals(on) => {
                self.prefs.portals = on;
                settings::set_portals(on);
            }
            PrefMsg::Sandbox(on) => {
                self.prefs.sandbox = on;
                crate::sandbox::set_enabled(on);
            }
            PrefMsg::WrapAround(on) => self.prefs.wrap_around = on,
            PrefMsg::SelectOnDrag(on) => {
                self.prefs.select_on_drag = on;
                self.region = None;
            }
            PrefMsg::Language(code) => {
                crate::i18n::set_language(code);
                self.prefs.language = code.into();
                if crate::platform::caps().menu_bar && !cfg!(test) {
                    crate::platform::menu_bar::install(&self.menu_bar());
                }
            }
            PrefMsg::WindowButtons(on) => self.prefs.window_buttons = on,
            PrefMsg::ThumbnailCache(on) => self.prefs.thumbnail_cache = on,
            PrefMsg::NightMode(on) => {
                if on != self.night {
                    follow_up = self.update(Message::ToggleNight);
                }
                self.prefs.night_mode = on;
            }
            PrefMsg::CacheMb(mb) => self.prefs.cache_mb = mb,
            PrefMsg::PreloadMegapixels(mp) => self.prefs.preload_megapixels = mp,
            PrefMsg::Reset => {
                let night = self.prefs.night_mode;
                self.prefs = Settings { night_mode: night, ..Settings::default() };
                settings::set_portals(true);
                crate::i18n::set_language("");
                crate::sandbox::set_enabled(true);
                self.apply_theme();
            }
        }
        self.prefs.save();
        follow_up
    }

    pub(super) fn prefs_dialog(&self) -> Element<'_, Message> {
        let p = &self.prefs;
        // The name gives way rather than the choices.
        let label = |t: String| text(t).size(style::BODY).width(Length::FillPortion(2));
        let heading = |t: String| text(t).size(style::BODY).font(style::font(iced::font::Weight::Semibold));
        let choices = |items: Vec<(String, bool, PrefMsg)>| {
            ui::choices(items.into_iter().map(|(name, on, msg)| (name, on, Message::Pref(msg))).collect())
        };
        let line = |name: String, control: Element<'static, Message>| {
            row![label(name), container(control).width(Length::FillPortion(3))].spacing(10).align_y(Alignment::Center)
        };
        let toggle = |name: String, on: bool, msg: fn(bool) -> PrefMsg| {
            ui::check(on).label(name).on_toggle(move |v| Message::Pref(msg(v)))
        };
        let theme = choices(vec![
            (tr("System"), p.theme == Theme::System, PrefMsg::Theme(Theme::System)),
            (tr("Light"), p.theme == Theme::Light, PrefMsg::Theme(Theme::Light)),
            (tr("Dark"), p.theme == Theme::Dark, PrefMsg::Theme(Theme::Dark)),
        ]);
        // Languages are called what they call themselves.
        let in_use = p.language.as_str();
        let language = choices(
            std::iter::once((tr("System"), in_use.is_empty(), PrefMsg::Language("")))
                .chain(
                    crate::i18n::LANGUAGES
                        .iter()
                        .map(|&(code, name)| (name.to_string(), in_use == code, PrefMsg::Language(code))),
                )
                .collect(),
        );
        let drag = choices(vec![
            (tr("Selects a part"), p.select_on_drag, PrefMsg::SelectOnDrag(true)),
            (tr("Moves it"), !p.select_on_drag, PrefMsg::SelectOnDrag(false)),
        ]);
        let fit = choices(vec![
            (tr("Fit width"), p.pdf_fit == PdfFit::Width, PrefMsg::PdfFit(PdfFit::Width)),
            (tr("Whole page"), p.pdf_fit == PdfFit::Page, PrefMsg::PdfFit(PdfFit::Page)),
        ]);
        let seconds = choices(
            settings::SLIDESHOW_SECONDS
                .iter()
                .map(|&s| (format!("{s} s"), p.slideshow_seconds == s, PrefMsg::SlideshowSeconds(s)))
                .collect(),
        );
        let cache = choices(
            settings::CACHE_MB.iter().map(|&m| (format!("{m} MB"), p.cache_mb == m, PrefMsg::CacheMb(m))).collect(),
        );
        let preload = choices(
            settings::PRELOAD_MEGAPIXELS
                .iter()
                .map(|&m| {
                    let name = if m == 0 { tr("Off") } else { format!("{m} MP") };
                    (name, p.preload_megapixels == m, PrefMsg::PreloadMegapixels(m))
                })
                .collect(),
        );
        let caps = crate::platform::caps();
        // What is said is what is true on this system.
        let (sandbox, sandbox_note) = if caps.sandbox {
            (
                tr("Decode pictures in a sandbox"),
                tr(
                    "Each picture is opened by a separate process that can do nothing but decode it. Costs a few milliseconds a picture.",
                ),
            )
        } else {
            (
                tr("Decode pictures in a separate process"),
                tr(
                    "A picture that crashes the decoder costs only that picture. On this system the process is not shut in: it could still reach your files.",
                ),
            )
        };
        let desktop = caps.desktop_dialogs_optional.then(|| {
            column![
                heading(tr("System")),
                toggle(tr("Use the desktop's file and print dialogs"), p.portals, PrefMsg::Portals),
                text(tr("Without them Lumen shows its own.")).size(style::SMALL).style(muted)
            ]
            .spacing(8)
        });
        let thumbnails = if caps.shared_thumbnails {
            tr("Share thumbnails with the desktop's cache")
        } else {
            tr("Keep thumbnails for the next time")
        };
        let section = |part: iced::widget::Column<'static, Message>| part.spacing(8);
        let body = column![
            section(column![
                heading(tr("General")),
                line(tr("Theme"), theme),
                line(tr("Language"), language),
                line(tr("Dragging over a picture"), drag),
                toggle(tr("Show the sidebar when Lumen starts"), p.sidebar_open, PrefMsg::SidebarOpen),
                toggle(tr("Zoom control and arrows over the picture"), p.floating_controls, PrefMsg::FloatingControls),
                toggle(tr("Wrap around at the first and last file"), p.wrap_around, PrefMsg::WrapAround),
                caps.frameless.then(|| toggle(
                    tr("Minimise and maximise buttons"),
                    p.window_buttons,
                    PrefMsg::WindowButtons
                )),
            ]),
            section(column![
                heading(tr("PDF")),
                line(tr("When a PDF opens"), fit),
                toggle(tr("Night mode"), self.night, PrefMsg::NightMode),
            ]),
            section(column![heading(tr("Slideshow")), line(tr("Time for each picture"), seconds)]),
            section(column![
                heading(tr("Performance")),
                line(tr("Pictures kept decoded"), cache),
                line(tr("Decode neighbours up to"), preload),
            ]),
            section(column![
                heading(tr("Privacy and safety")),
                toggle(sandbox, p.sandbox, PrefMsg::Sandbox),
                text(sandbox_note).size(style::SMALL).style(muted),
                toggle(thumbnails, p.thumbnail_cache, PrefMsg::ThumbnailCache),
            ]),
            desktop,
        ]
        .spacing(18);
        ui::dialog(tr(super::commands::preferences_title()), ui::FORM, body)
            .aside(ui::action(ui::Kind::Other, tr("Reset to defaults"), Some(Message::Pref(PrefMsg::Reset))))
            .cancel(tr("Close"), Message::Dialog(None))
            .view(TOOLBAR_H)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_beats_the_preference_which_beats_the_desktop() {
        assert_eq!(effective_mode(None, Theme::System, Mode::Dark), Mode::Dark);
        assert_eq!(effective_mode(None, Theme::System, Mode::Light), Mode::Light);
        assert_eq!(effective_mode(None, Theme::Light, Mode::Dark), Mode::Light);
        assert_eq!(effective_mode(None, Theme::Dark, Mode::Light), Mode::Dark);
        assert_eq!(effective_mode(Some("light"), Theme::Dark, Mode::Dark), Mode::Light);
        assert_eq!(effective_mode(Some("dark"), Theme::Light, Mode::Light), Mode::Dark);
        assert_eq!(effective_mode(Some("nonsense"), Theme::System, Mode::Dark), Mode::Dark);
    }
}
