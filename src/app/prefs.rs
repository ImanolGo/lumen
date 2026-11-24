//! The preferences dialog (☰ → Preferences…). Every change takes effect at once and is saved.

use iced::theme::Mode;
use iced::widget::{button, checkbox, column, container, row, scrollable, space, text};
use iced::{Alignment, Element, Font, Length, Task};

use super::{App, Message, muted};
use crate::i18n::tr;
use crate::settings::{self, PdfFit, Settings, Theme};
use crate::style;

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
                crate::sandbox::set_enabled(true);
                self.apply_theme();
            }
        }
        self.prefs.save();
        follow_up
    }

    pub(super) fn prefs_dialog<'a>(
        &'a self,
        title: Element<'a, Message>,
        close: Element<'a, Message>,
    ) -> Element<'a, Message> {
        let p = &self.prefs;
        let label = |t: String| text(t).size(13).width(170);
        let heading = |t: String| {
            text(t).size(12).style(muted).font(Font { weight: iced::font::Weight::Semibold, ..crate::style::FONT })
        };
        // A row of choices, the current one lit.
        let choices = |items: Vec<(String, bool, PrefMsg)>| {
            row(items.into_iter().map(|(name, on, msg)| {
                button(text(name).size(12).align_x(Alignment::Center))
                    .padding([4, 10])
                    .style(style::segment(on))
                    .on_press(Message::Pref(msg))
                    .into()
            }))
            .spacing(2)
        };
        let line = |name: String, control: Element<'a, Message>| {
            row![label(name), control].spacing(10).align_y(Alignment::Center)
        };
        let toggle = |name: String, on: bool, msg: fn(bool) -> PrefMsg| {
            checkbox(on).label(name).on_toggle(move |v| Message::Pref(msg(v))).text_size(13).size(16)
        };
        let theme = choices(vec![
            (tr("System"), p.theme == Theme::System, PrefMsg::Theme(Theme::System)),
            (tr("Light"), p.theme == Theme::Light, PrefMsg::Theme(Theme::Light)),
            (tr("Dark"), p.theme == Theme::Dark, PrefMsg::Theme(Theme::Dark)),
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
        let body = column![
            heading(tr("APPEARANCE")),
            line(tr("Theme"), theme.into()),
            toggle(tr("Show the sidebar when Lumen starts"), p.sidebar_open, PrefMsg::SidebarOpen),
            toggle(tr("Zoom control and arrows over the picture"), p.floating_controls, PrefMsg::FloatingControls),
            heading(tr("PDFS")),
            line(tr("When a PDF opens"), fit.into()),
            toggle(tr("Night mode"), self.night, PrefMsg::NightMode),
            heading(tr("SLIDESHOW")),
            line(tr("Time for each picture"), seconds.into()),
            heading(tr("SECURITY")),
            toggle(tr("Decode pictures in a sandbox"), p.sandbox, PrefMsg::Sandbox),
            toggle(tr("Wrap around at the first and last file"), p.wrap_around, PrefMsg::WrapAround),
            toggle(tr("Share thumbnails with the desktop's cache"), p.thumbnail_cache, PrefMsg::ThumbnailCache),
            text(tr("Each picture is opened by a separate process that can do nothing but decode it. Costs a few milliseconds a picture. LUMEN_NO_SANDBOX=1 turns it off."))
                .size(11)
                .style(muted),
            heading(tr("MEMORY AND SPEED")),
            line(tr("Pictures kept decoded"), cache.into()),
            line(tr("Decode neighbours up to"), preload.into()),
            heading(tr("DESKTOP")),
            toggle(tr("Use the desktop's file and print dialogs"), p.portals, PrefMsg::Portals),
            text(tr("Without them (or without a portal) Lumen shows its own. LUMEN_THEME and LUMEN_NO_PORTAL in the environment win over these."))
                .size(11)
                .style(muted),
        ]
        .spacing(8);
        column![
            title,
            scrollable(container(body).padding([0, 12])).height(Length::Shrink),
            row![
                button(text(tr("Reset to defaults")).size(13))
                    .padding([8, 14])
                    .style(style::secondary)
                    .on_press(Message::Pref(PrefMsg::Reset)),
                space().width(Length::Fill),
                close
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(14)
        .width(520)
        .into()
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
