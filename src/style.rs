//! Look and feel: light and dark palettes that follow the system, and widget styles.

use iced::theme::{Mode, Palette};
use iced::widget::{button, container, svg};
use iced::{Background, Border, Color, Shadow, Theme, Vector, color};

pub fn theme(mode: Mode) -> Theme {
    match mode {
        Mode::Dark => Theme::custom(
            "Lumen Dark",
            Palette {
                background: color!(0x1c1b22),
                text: color!(0xecebf2),
                primary: color!(0xa99bff),
                success: color!(0x6fcf97),
                warning: color!(0xf2c94c),
                danger: color!(0xff7b72),
            },
        ),
        Mode::Light | Mode::None => Theme::custom(
            "Lumen Light",
            Palette {
                background: color!(0xfbfafd),
                text: color!(0x1f1d2b),
                primary: color!(0x5b4bc4),
                success: color!(0x1e8e3e),
                warning: color!(0xb26a00),
                danger: color!(0xc5221f),
            },
        ),
    }
}

fn is_dark(theme: &Theme) -> bool {
    theme.extended_palette().is_dark
}

/// The area behind pictures and pages: a touch darker than the chrome.
pub fn canvas(theme: &Theme) -> container::Style {
    let bg = if is_dark(theme) { color!(0x131218) } else { color!(0xecebf1) };
    container::Style { background: Some(bg.into()), ..Default::default() }
}

/// The toolbar and file strip.
pub fn bar(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(p.background.base.color.into()),
        text_color: Some(p.background.base.text),
        border: Border { width: 0.0, ..Default::default() },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, if p.is_dark { 0.5 } else { 0.08 }),
            offset: Vector::new(0.0, 1.0),
            blur_radius: 6.0,
        },
        ..Default::default()
    }
}

/// A page of a PDF: white paper with a soft shadow.
pub fn page(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::WHITE.into()),
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, if is_dark(theme) { 0.6 } else { 0.18 }),
            offset: Vector::new(0.0, 2.0),
            blur_radius: 10.0,
        },
        ..Default::default()
    }
}

/// Floating pill (zoom level, page counter, toasts).
pub fn pill(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(p.background.weak.color.scale_alpha(0.96).into()),
        text_color: Some(p.background.weak.text),
        border: Border { radius: 999.0.into(), width: 1.0, color: p.background.strong.color.scale_alpha(0.5) },
        shadow: Shadow { color: Color::from_rgba(0.0, 0.0, 0.0, 0.2), offset: Vector::new(0.0, 2.0), blur_radius: 8.0 },
        ..Default::default()
    }
}

pub fn toast_error(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(p.danger.weak.color.into()),
        text_color: Some(p.danger.weak.text),
        border: Border { radius: 10.0.into(), ..Default::default() },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.25),
            offset: Vector::new(0.0, 2.0),
            blur_radius: 10.0,
        },
        ..Default::default()
    }
}

/// Flat icon button that lights up on hover, like a GNOME/macOS toolbar.
pub fn tool(theme: &Theme, status: button::Status) -> button::Style {
    let p = theme.extended_palette();
    let bg = match status {
        button::Status::Hovered => Some(p.background.weak.color),
        button::Status::Pressed => Some(p.background.strong.color),
        _ => None,
    };
    button::Style {
        background: bg.map(Background::Color),
        text_color: if status == button::Status::Disabled {
            p.background.base.text.scale_alpha(0.35)
        } else {
            p.background.base.text
        },
        border: Border { radius: 8.0.into(), ..Default::default() },
        ..Default::default()
    }
}

/// Toggled-on variant of [`tool`].
pub fn tool_active(theme: &Theme, status: button::Status) -> button::Style {
    let p = theme.extended_palette();
    button::Style {
        background: Some(
            p.primary.weak.color.scale_alpha(if status == button::Status::Hovered { 0.45 } else { 0.3 }).into(),
        ),
        text_color: p.background.base.text,
        border: Border { radius: 8.0.into(), ..Default::default() },
        ..Default::default()
    }
}

/// Large, filled call-to-action button (empty state).
pub fn primary(theme: &Theme, status: button::Status) -> button::Style {
    let mut s = button::primary(theme, status);
    s.border.radius = 10.0.into();
    s
}

pub fn secondary(theme: &Theme, status: button::Status) -> button::Style {
    let p = theme.extended_palette();
    let bg = match status {
        button::Status::Hovered => p.background.strong.color,
        button::Status::Pressed => p.background.stronger.color,
        _ => p.background.weak.color,
    };
    button::Style {
        background: Some(bg.into()),
        text_color: p.background.base.text,
        border: Border { radius: 10.0.into(), ..Default::default() },
        ..Default::default()
    }
}

/// A thumbnail in the file strip; the current one gets an accent ring.
pub fn thumb(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let p = theme.extended_palette();
        let ring = if selected {
            p.primary.base.color
        } else if status == button::Status::Hovered {
            p.background.strong.color
        } else {
            Color::TRANSPARENT
        };
        button::Style {
            background: Some(if selected { p.primary.weak.color.scale_alpha(0.25) } else { Color::TRANSPARENT }.into()),
            text_color: p.background.base.text,
            border: Border { radius: 8.0.into(), width: 2.0, color: ring },
            ..Default::default()
        }
    }
}

pub fn thumb_placeholder(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(p.background.weak.color.into()),
        border: Border { radius: 4.0.into(), ..Default::default() },
        ..Default::default()
    }
}

/// Symbolic icons take the text colour, like GTK's symbolic icons.
pub fn icon(theme: &Theme, status: svg::Status) -> svg::Style {
    let p = theme.extended_palette();
    let _ = status;
    svg::Style { color: Some(p.background.base.text) }
}

pub fn icon_muted(theme: &Theme, _status: svg::Status) -> svg::Style {
    let p = theme.extended_palette();
    svg::Style { color: Some(p.background.base.text.scale_alpha(0.45)) }
}

pub fn icon_on_primary(theme: &Theme, _status: svg::Status) -> svg::Style {
    svg::Style { color: Some(theme.extended_palette().primary.base.text) }
}

/// A search match on a page; the current one is stronger.
pub fn highlight(current: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| {
        let (fill, edge) = if current {
            (Color::from_rgba(1.0, 0.55, 0.0, 0.45), Color::from_rgb(0.95, 0.45, 0.0))
        } else {
            (Color::from_rgba(1.0, 0.85, 0.0, 0.38), Color::TRANSPARENT)
        };
        container::Style {
            background: Some(fill.into()),
            border: Border { radius: 2.0.into(), width: if current { 1.5 } else { 0.0 }, color: edge },
            ..Default::default()
        }
    }
}

/// Floating panel, such as the search bar.
pub fn panel(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(p.background.base.color.into()),
        text_color: Some(p.background.base.text),
        border: Border { radius: 12.0.into(), width: 1.0, color: p.background.strong.color.scale_alpha(0.6) },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, if p.is_dark { 0.5 } else { 0.15 }),
            offset: Vector::new(0.0, 3.0),
            blur_radius: 14.0,
        },
        ..Default::default()
    }
}

/// Segmented switch button (sidebar tabs).
pub fn segment(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| if selected { tool_active(theme, status) } else { tool(theme, status) }
}

/// Selected text on a PDF page.
pub fn selection(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(Color { a: 0.32, ..p.primary.base.color }.into()),
        border: Border { radius: 1.5.into(), ..Default::default() },
        ..Default::default()
    }
}

/// A colour choice in the markup bar: a filled dot, ringed when chosen.
pub fn swatch(fill: Color, selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let p = theme.extended_palette();
        let ring = if selected {
            p.primary.base.color
        } else if status == button::Status::Hovered {
            p.background.strong.color
        } else {
            p.background.strong.color.scale_alpha(0.5)
        };
        button::Style {
            background: Some(fill.into()),
            border: Border { radius: 11.0.into(), width: if selected { 2.5 } else { 1.0 }, color: ring },
            ..Default::default()
        }
    }
}

/// The text box being typed into on a page.
pub fn text_box(
    color: Color,
) -> impl Fn(&Theme, iced::widget::text_editor::Status) -> iced::widget::text_editor::Style {
    move |theme, _status| {
        let p = theme.extended_palette();
        iced::widget::text_editor::Style {
            background: Color::from_rgba(1.0, 1.0, 1.0, 0.75).into(),
            border: Border { radius: 3.0.into(), width: 1.5, color: p.primary.base.color },
            placeholder: Color { a: 0.45, ..color },
            value: color,
            selection: Color { a: 0.3, ..p.primary.base.color },
        }
    }
}

/// The dimmed backdrop and card of a confirmation dialog.
pub fn scrim(_theme: &Theme) -> container::Style {
    container::Style { background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.35).into()), ..Default::default() }
}
