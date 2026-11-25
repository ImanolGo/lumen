//! Look and feel: one set of design tokens for a calm, modern interface.
//!
//! The window is one surface: the header, the sidebar and the area behind pictures
//! and pages share a background, and what floats on it (menus, tool islands, the
//! zoom pill, dialogs) is a slightly raised panel with a hairline edge and a soft
//! shadow. Colour is neutral; Lumen's amber marks only what is active or chosen.
//! Light and dark follow the system.

use iced::font::Weight;
use iced::theme::{Mode, Palette};
use iced::widget::{button, container, svg};
use iced::{Background, Border, Color, Font, Shadow, Theme, Vector, color};

/// The interface typeface, bundled (see `assets/fonts`).
pub const FONT: Font = Font::with_name("Inter");

/// The interface font at `weight`.
pub fn font(weight: Weight) -> Font {
    Font { weight, ..FONT }
}

/// Type sizes: hints, secondary text, text, titles of panels and the window, of dialogs, and the
/// name on the empty window.
pub const CAPTION: f32 = 11.0;
pub const SMALL: f32 = 12.0;
pub const BODY: f32 = 13.0;
pub const TITLE: f32 = 15.0;
pub const HEADING: f32 = 18.0;
pub const DISPLAY: f32 = 24.0;

/// Corner radii: controls, panels.
pub const RADIUS: f32 = 8.0;
pub const PANEL_RADIUS: f32 = 12.0;

/// Lumen's amber, from its icon.
const AMBER_DARK: Color = color!(0xf5b041);
const AMBER_LIGHT: Color = color!(0xb45309);

pub fn theme(mode: Mode) -> Theme {
    match mode {
        Mode::Dark => Theme::custom(
            "Lumen Dark",
            Palette {
                background: color!(0x141416),
                text: color!(0xededf0),
                primary: AMBER_DARK,
                success: color!(0x5fd08a),
                warning: color!(0xf5b041),
                danger: color!(0xff6b61),
            },
        ),
        Mode::Light | Mode::None => Theme::custom(
            "Lumen Light",
            Palette {
                background: color!(0xf6f6f7),
                text: color!(0x18181b),
                primary: AMBER_LIGHT,
                success: color!(0x1f8a4c),
                warning: color!(0xb26a00),
                danger: color!(0xc9342a),
            },
        ),
    }
}

fn is_dark(theme: &Theme) -> bool {
    theme.extended_palette().is_dark
}

fn text_color(theme: &Theme) -> Color {
    theme.extended_palette().background.base.text
}

/// Something floating on the window: a little lighter in the dark, white in the light.
pub fn surface(theme: &Theme) -> Color {
    if is_dark(theme) { color!(0x1f1f23) } else { Color::WHITE }
}

/// A faint wash for hovered and selected things.
fn wash(theme: &Theme, amount: f32) -> Color {
    let base = if is_dark(theme) { Color::WHITE } else { Color::BLACK };
    Color { a: amount, ..base }
}

/// The edge of floating things.
fn hairline(theme: &Theme) -> Color {
    wash(theme, if is_dark(theme) { 0.09 } else { 0.08 })
}

/// The software renderer draws a shadow outside the part of the window it is drawing afresh, so
/// shadows pile up until what is under them is black; and they cost it most of its time.
fn shadows() -> bool {
    static SOFTWARE: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(|| std::env::var("ICED_BACKEND").is_ok_and(|v| v.contains("tiny-skia")));
    !*SOFTWARE
}

fn float_shadow(theme: &Theme) -> Shadow {
    if !shadows() {
        return Shadow::default();
    }
    Shadow {
        color: Color::from_rgba(0.0, 0.0, 0.0, if is_dark(theme) { 0.45 } else { 0.10 }),
        offset: Vector::new(0.0, 6.0),
        blur_radius: 24.0,
    }
}

/// The accent, for marking what is active.
pub fn accent(theme: &Theme) -> Color {
    theme.extended_palette().primary.base.color
}

/// The window's single surface, behind pictures and pages as well.
pub fn canvas(theme: &Theme) -> container::Style {
    container::Style { background: Some(theme.palette().background.into()), ..Default::default() }
}

/// The header and the sidebar: the same surface as the canvas, no edges.
pub fn bar(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(theme.palette().background.into()),
        text_color: Some(text_color(theme)),
        ..Default::default()
    }
}

/// A page of a PDF: white paper, lifted a little.
pub fn page(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::WHITE.into()),
        border: Border { radius: 2.0.into(), ..Default::default() },
        shadow: if shadows() {
            Shadow {
                color: Color::from_rgba(0.0, 0.0, 0.0, if is_dark(theme) { 0.5 } else { 0.12 }),
                offset: Vector::new(0.0, 2.0),
                blur_radius: 12.0,
            }
        } else {
            Shadow::default()
        },
        ..Default::default()
    }
}

/// A tooltip.
pub fn pill(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(surface(theme).into()),
        text_color: Some(text_color(theme)),
        border: Border { radius: 6.0.into(), width: 1.0, color: hairline(theme) },
        shadow: float_shadow(theme),
        ..Default::default()
    }
}

/// A message about a problem: a floating panel with the danger colour as its accent.
pub fn toast_error(theme: &Theme) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(surface(theme).into()),
        text_color: Some(p.danger.base.color),
        border: Border { radius: PANEL_RADIUS.into(), width: 1.0, color: p.danger.base.color.scale_alpha(0.35) },
        shadow: float_shadow(theme),
        ..Default::default()
    }
}

/// A quiet icon button: nothing until hovered.
pub fn tool(theme: &Theme, status: button::Status) -> button::Style {
    let bg = match status {
        button::Status::Hovered => Some(wash(theme, 0.07)),
        button::Status::Pressed => Some(wash(theme, 0.12)),
        _ => None,
    };
    button::Style {
        background: bg.map(Background::Color),
        text_color: if status == button::Status::Disabled {
            text_color(theme).scale_alpha(DISABLED)
        } else {
            text_color(theme)
        },
        border: Border { radius: RADIUS.into(), ..Default::default() },
        ..Default::default()
    }
}

/// An icon button that is switched on: a soft accent wash.
pub fn tool_active(theme: &Theme, status: button::Status) -> button::Style {
    let a = accent(theme);
    button::Style {
        background: Some(a.scale_alpha(if status == button::Status::Hovered { 0.24 } else { 0.16 }).into()),
        text_color: text_color(theme),
        border: Border { radius: RADIUS.into(), ..Default::default() },
        ..Default::default()
    }
}

/// How strong text is when it is secondary, and when what it names cannot be done now.
const MUTED: f32 = 0.66;
const DISABLED: f32 = 0.5;

/// Secondary text.
pub fn muted(theme: &Theme) -> Color {
    text_color(theme).scale_alpha(MUTED)
}

/// What cannot be done now.
pub fn unavailable(theme: &Theme) -> Color {
    text_color(theme).scale_alpha(DISABLED)
}

/// A button that cannot be pressed now, whatever kind it is.
fn disabled(theme: &Theme) -> button::Style {
    button::Style {
        background: Some(wash(theme, 0.05).into()),
        text_color: text_color(theme).scale_alpha(DISABLED),
        border: Border { radius: RADIUS.into(), ..Default::default() },
        ..Default::default()
    }
}

fn filled(background: Color, text: Color) -> button::Style {
    button::Style {
        background: Some(background.into()),
        text_color: text,
        border: Border { radius: RADIUS.into(), ..Default::default() },
        ..Default::default()
    }
}

/// The one button that does the main thing.
pub fn primary(theme: &Theme, status: button::Status) -> button::Style {
    let p = theme.extended_palette();
    match status {
        button::Status::Disabled => disabled(theme),
        button::Status::Hovered | button::Status::Pressed => {
            let towards = if is_dark(theme) { Color::WHITE } else { Color::BLACK };
            filled(color_mix(p.primary.base.color, towards, 0.12), on_primary(theme))
        }
        _ => filled(p.primary.base.color, on_primary(theme)),
    }
}

/// Text on the accent colour.
fn on_primary(theme: &Theme) -> Color {
    if is_dark(theme) { color!(0x1a1206) } else { Color::WHITE }
}

/// The button that throws something away.
pub fn danger(theme: &Theme, status: button::Status) -> button::Style {
    let base = theme.palette().danger;
    let text = if is_dark(theme) { color!(0x2a0705) } else { Color::WHITE };
    match status {
        button::Status::Disabled => disabled(theme),
        button::Status::Hovered | button::Status::Pressed => {
            filled(color_mix(base, if is_dark(theme) { Color::WHITE } else { Color::BLACK }, 0.12), text)
        }
        _ => filled(base, text),
    }
}

pub fn secondary(theme: &Theme, status: button::Status) -> button::Style {
    let amount = match status {
        button::Status::Disabled => return disabled(theme),
        button::Status::Hovered => 0.10,
        button::Status::Pressed => 0.14,
        _ => 0.06,
    };
    filled(wash(theme, amount), text_color(theme))
}

/// The row of a list that the keyboard is on.
pub fn list_row(theme: &Theme, _status: button::Status) -> button::Style {
    filled(wash(theme, 0.09), text_color(theme))
}

/// A box to type into: a faint wash, and an accent ring while it has the focus.
pub fn input(theme: &Theme, status: iced::widget::text_input::Status) -> iced::widget::text_input::Style {
    use iced::widget::text_input::Status;
    let (fill, ring) = match status {
        Status::Focused { .. } => (0.05, 1.5),
        Status::Hovered => (0.08, 0.0),
        _ => (0.06, 0.0),
    };
    iced::widget::text_input::Style {
        background: wash(theme, fill).into(),
        border: Border { radius: RADIUS.into(), width: ring, color: accent(theme) },
        icon: muted(theme),
        placeholder: muted(theme),
        value: text_color(theme),
        selection: accent(theme).scale_alpha(0.3),
    }
}

/// One without a frame.
pub fn bare_input(theme: &Theme, status: iced::widget::text_input::Status) -> iced::widget::text_input::Style {
    iced::widget::text_input::Style {
        background: Color::TRANSPARENT.into(),
        border: Border::default(),
        ..input(theme, status)
    }
}

pub fn select(theme: &Theme, status: iced::widget::pick_list::Status) -> iced::widget::pick_list::Style {
    use iced::widget::pick_list::Status;
    let open = matches!(status, Status::Opened { .. });
    iced::widget::pick_list::Style {
        text_color: text_color(theme),
        placeholder_color: muted(theme),
        handle_color: text_color(theme),
        background: wash(theme, if matches!(status, Status::Active) { 0.06 } else { 0.09 }).into(),
        border: Border { radius: RADIUS.into(), width: if open { 1.5 } else { 0.0 }, color: accent(theme) },
    }
}

pub fn select_menu(theme: &Theme) -> iced::overlay::menu::Style {
    iced::overlay::menu::Style {
        background: surface(theme).into(),
        border: Border { radius: RADIUS.into(), width: 1.0, color: hairline(theme) },
        text_color: text_color(theme),
        selected_text_color: text_color(theme),
        selected_background: wash(theme, 0.09).into(),
        shadow: float_shadow(theme),
    }
}

pub fn check(theme: &Theme, status: iced::widget::checkbox::Status) -> iced::widget::checkbox::Style {
    use iced::widget::checkbox::Status;
    let (on, hovered, enabled) = match status {
        Status::Active { is_checked } => (is_checked, false, true),
        Status::Hovered { is_checked } => (is_checked, true, true),
        Status::Disabled { is_checked } => (is_checked, false, false),
    };
    let fill = match (on, hovered) {
        (true, _) => accent(theme),
        (false, true) => wash(theme, 0.10),
        (false, false) => wash(theme, 0.05),
    };
    let fade = if enabled { 1.0 } else { DISABLED };
    iced::widget::checkbox::Style {
        background: fill.scale_alpha(fade).into(),
        icon_color: on_primary(theme),
        border: Border {
            radius: 5.0.into(),
            width: if on { 0.0 } else { 1.0 },
            // The edge is all there is to see of an empty box.
            color: text_color(theme).scale_alpha(0.5 * fade),
        },
        text_color: Some(text_color(theme).scale_alpha(fade)),
    }
}

pub fn slider(theme: &Theme, status: iced::widget::slider::Status) -> iced::widget::slider::Style {
    use iced::widget::slider::{Handle, HandleShape, Rail, Status, Style};
    let active = !matches!(status, Status::Active);
    Style {
        rail: Rail {
            backgrounds: (accent(theme).into(), wash(theme, 0.16).into()),
            width: 3.0,
            border: Border { radius: 2.0.into(), ..Default::default() },
        },
        handle: Handle {
            shape: HandleShape::Circle { radius: if active { 8.0 } else { 7.0 } },
            background: Color::WHITE.into(),
            border_width: 1.0,
            border_color: if is_dark(theme) { color!(0x000000) } else { color!(0x8a8a90) },
        },
    }
}

/// A sunken area holding a list.
pub fn well(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(wash(theme, 0.03).into()),
        border: Border { radius: RADIUS.into(), width: 1.0, color: hairline(theme) },
        ..Default::default()
    }
}

/// The line between groups of tools.
pub fn separator(theme: &Theme) -> container::Style {
    container::Style { background: Some(wash(theme, 0.16).into()), ..Default::default() }
}

/// A key on the empty window and in the list of shortcuts.
pub fn key_cap(theme: &Theme) -> container::Style {
    container::Style {
        border: Border { radius: 4.0.into(), width: 1.0, color: wash(theme, 0.18) },
        ..Default::default()
    }
}

/// The window while files are held over it.
pub fn drop_area(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(theme.palette().background.scale_alpha(0.85).into()),
        border: Border { radius: 16.0.into(), width: 2.0, color: accent(theme) },
        ..Default::default()
    }
}

/// The icon of a floating control, as faded as it is.
pub fn icon_faded(alpha: f32) -> impl Fn(&Theme, svg::Status) -> svg::Style {
    move |theme, _| svg::Style { color: Some(text_color(theme).scale_alpha(0.85 * alpha)) }
}

/// A row of the file list or the contents: a faint wash when hovered or chosen.
pub fn thumb(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let bg = match (selected, status) {
            (true, _) => Some(wash(theme, 0.06)),
            (false, button::Status::Hovered) => Some(wash(theme, 0.04)),
            _ => None,
        };
        button::Style {
            background: bg.map(Background::Color),
            text_color: text_color(theme),
            border: Border { radius: 10.0.into(), ..Default::default() },
            ..Default::default()
        }
    }
}

/// The frame of a thumbnail: an accent ring around the file on screen.
pub fn thumb_frame(selected: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| container::Style {
        border: Border {
            radius: 7.0.into(),
            width: if selected { 2.0 } else { 0.0 },
            color: if selected { accent(theme) } else { Color::TRANSPARENT },
        },
        ..Default::default()
    }
}

pub fn thumb_placeholder(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(wash(theme, 0.05).into()),
        border: Border { radius: 5.0.into(), ..Default::default() },
        ..Default::default()
    }
}

/// Symbolic icons take the text colour.
pub fn icon(theme: &Theme, _status: svg::Status) -> svg::Style {
    svg::Style { color: Some(text_color(theme).scale_alpha(0.85)) }
}

pub fn icon_muted(theme: &Theme, _status: svg::Status) -> svg::Style {
    svg::Style { color: Some(text_color(theme).scale_alpha(DISABLED)) }
}

pub fn icon_accent(theme: &Theme, _status: svg::Status) -> svg::Style {
    svg::Style { color: Some(accent(theme)) }
}

pub fn icon_on_primary(theme: &Theme, _status: svg::Status) -> svg::Style {
    svg::Style { color: Some(on_primary(theme)) }
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

/// Something floating: menus, dialogs, the search bar, tool islands.
pub fn panel(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(surface(theme).into()),
        text_color: Some(text_color(theme)),
        border: Border { radius: PANEL_RADIUS.into(), width: 1.0, color: hairline(theme) },
        shadow: float_shadow(theme),
        ..Default::default()
    }
}

/// A small tab or chip: the chosen one has a faint wash, the others are quiet.
pub fn segment(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let bg = match (selected, status) {
            (true, _) => Some(wash(theme, 0.09)),
            (false, button::Status::Hovered) => Some(wash(theme, 0.05)),
            _ => None,
        };
        button::Style {
            background: bg.map(Background::Color),
            text_color: if selected { text_color(theme) } else { muted(theme) },
            border: Border { radius: RADIUS.into(), ..Default::default() },
            ..Default::default()
        }
    }
}

/// Selected text on a PDF page.
pub fn selection(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgba(0.26, 0.52, 0.96, if is_dark(theme) { 0.38 } else { 0.30 }).into()),
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
pub fn scrim(theme: &Theme) -> container::Style {
    let a = if is_dark(theme) { 0.5 } else { 0.25 };
    container::Style { background: Some(Color::from_rgba(0.0, 0.0, 0.0, a).into()), ..Default::default() }
}

/// The tint of a form field waiting to be filled in, as most PDF viewers show it.
fn field_fill(focused: bool) -> Color {
    Color::from_rgba(0.25, 0.45, 1.0, if focused { 0.05 } else { 0.12 })
}

fn field_border(theme: &Theme, focused: bool) -> Border {
    let accent = theme.extended_palette().primary.base.color;
    Border { radius: 2.0.into(), width: 1.0, color: Color { a: if focused { 0.9 } else { 0.35 }, ..accent } }
}

/// A text field of a PDF form. The page is paper-white in either theme, so the text is the field's own colour.
pub fn form_input(ink: Color) -> impl Fn(&Theme, iced::widget::text_input::Status) -> iced::widget::text_input::Style {
    move |theme, status| {
        let focused = matches!(status, iced::widget::text_input::Status::Focused { .. });
        iced::widget::text_input::Style {
            background: field_fill(focused).into(),
            border: field_border(theme, focused),
            icon: ink,
            placeholder: Color { a: 0.4, ..ink },
            value: ink,
            selection: Color { a: 0.3, ..theme.extended_palette().primary.base.color },
        }
    }
}

pub fn form_text_area(
    ink: Color,
) -> impl Fn(&Theme, iced::widget::text_editor::Status) -> iced::widget::text_editor::Style {
    move |theme, status| {
        let focused = matches!(status, iced::widget::text_editor::Status::Focused { .. });
        iced::widget::text_editor::Style {
            background: field_fill(focused).into(),
            border: field_border(theme, focused),
            placeholder: Color { a: 0.4, ..ink },
            value: ink,
            selection: Color { a: 0.3, ..theme.extended_palette().primary.base.color },
        }
    }
}

/// A check box, radio button or signature place of a PDF form.
pub fn form_box(theme: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: Some(field_fill(hovered).into()),
        text_color: Color::from_rgb(0.05, 0.05, 0.1),
        border: field_border(theme, hovered),
        ..Default::default()
    }
}

pub fn form_choice(ink: Color) -> impl Fn(&Theme, iced::widget::pick_list::Status) -> iced::widget::pick_list::Style {
    move |theme, status| {
        let open = !matches!(status, iced::widget::pick_list::Status::Active);
        iced::widget::pick_list::Style {
            text_color: ink,
            placeholder_color: Color { a: 0.4, ..ink },
            handle_color: Color { a: 0.6, ..ink },
            background: field_fill(open).into(),
            border: field_border(theme, open),
        }
    }
}

/// A page in the page organiser: ringed when selected, dashed when pages would be dropped in front of it.
pub fn page_cell(theme: &Theme, selected: bool, target: bool) -> container::Style {
    let p = theme.extended_palette();
    let (fill, ring, width) = if target {
        (p.primary.weak.color.scale_alpha(0.15), p.primary.base.color, 3.0)
    } else if selected {
        (p.primary.weak.color.scale_alpha(0.25), p.primary.base.color, 2.0)
    } else {
        (Color::TRANSPARENT, Color::TRANSPARENT, 2.0)
    };
    container::Style {
        background: Some(fill.into()),
        text_color: Some(p.background.base.text),
        border: Border { radius: 10.0.into(), width, color: ring },
        ..Default::default()
    }
}

/// Where dragged pages can be dropped at the end of the document.
pub fn drop_slot(theme: &Theme, target: bool) -> container::Style {
    let p = theme.extended_palette();
    container::Style {
        background: Some(if target { p.primary.weak.color.scale_alpha(0.2) } else { Color::TRANSPARENT }.into()),
        border: Border { radius: 10.0.into(), width: 2.0, color: p.background.strong.color },
        ..Default::default()
    }
}

/// A floating control over the picture, faded by `alpha` while it appears and disappears.
pub fn floating(theme: &Theme, status: button::Status, alpha: f32, radius: f32) -> button::Style {
    let bg = match status {
        button::Status::Hovered => wash(theme, 0.08),
        button::Status::Pressed => wash(theme, 0.13),
        _ => Color::TRANSPARENT,
    };
    button::Style {
        background: Some(Color { a: bg.a * alpha, ..bg }.into()),
        text_color: text_color(theme).scale_alpha(alpha),
        border: Border { radius: radius.into(), ..Default::default() },
        ..Default::default()
    }
}

/// The rounded panel behind floating controls.
pub fn floating_panel(theme: &Theme, alpha: f32) -> container::Style {
    let shadow = float_shadow(theme);
    container::Style {
        background: Some(surface(theme).scale_alpha(0.96 * alpha).into()),
        border: Border { radius: PANEL_RADIUS.into(), width: 1.0, color: hairline(theme).scale_alpha(alpha) },
        shadow: Shadow { color: shadow.color.scale_alpha(alpha), ..shadow },
        ..Default::default()
    }
}

/// A round floating button with its own backdrop (the previous/next arrows).
pub fn floating_disc(theme: &Theme, status: button::Status, alpha: f32) -> button::Style {
    let shadow = float_shadow(theme);
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let bg = if hovered { color_mix(surface(theme), text_color(theme), 0.08) } else { surface(theme) };
    button::Style {
        background: Some(bg.scale_alpha(0.96 * alpha).into()),
        text_color: text_color(theme).scale_alpha(alpha),
        border: Border { radius: 22.0.into(), width: 1.0, color: hairline(theme).scale_alpha(alpha) },
        shadow: Shadow { color: shadow.color.scale_alpha(alpha), ..shadow },
        ..Default::default()
    }
}

fn color_mix(a: Color, b: Color, t: f32) -> Color {
    Color::from_rgba(a.r + (b.r - a.r) * t, a.g + (b.g - a.g) * t, a.b + (b.b - a.b) * t, a.a)
}

/// A line of a drop-down menu.
pub fn menu_item(theme: &Theme, status: button::Status) -> button::Style {
    button::Style {
        background: matches!(status, button::Status::Hovered | button::Status::Pressed)
            .then(|| wash(theme, 0.07).into()),
        text_color: text_color(theme),
        border: Border { radius: 6.0.into(), ..Default::default() },
        ..Default::default()
    }
}

/// The thumb of the main view's scrollbars.
pub fn scroll_thumb(theme: &Theme, active: bool) -> Color {
    wash(theme, if active { 0.35 } else { 0.18 })
}

/// A thin, quiet scrollbar that shows itself more when used.
pub fn scrollbar(theme: &Theme, status: iced::widget::scrollable::Status) -> iced::widget::scrollable::Style {
    use iced::widget::scrollable::{AutoScroll, Rail, Scroller, Status, Style};
    let active = !matches!(status, Status::Active { .. });
    let rail = Rail {
        background: None,
        border: Border::default(),
        scroller: Scroller {
            background: wash(theme, if active { 0.35 } else { 0.18 }).into(),
            border: Border { radius: 4.0.into(), ..Default::default() },
        },
    };
    Style {
        container: container::Style::default(),
        vertical_rail: rail,
        horizontal_rail: rail,
        gap: None,
        auto_scroll: AutoScroll {
            background: surface(theme).into(),
            border: Border { radius: 999.0.into(), width: 1.0, color: hairline(theme) },
            shadow: float_shadow(theme),
            icon: text_color(theme),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `c` over an opaque `under`.
    fn over(c: Color, under: Color) -> Color {
        color_mix(under, Color { a: 1.0, ..c }, c.a)
    }

    fn luminance(c: Color) -> f32 {
        let l = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        0.2126 * l(c.r) + 0.7152 * l(c.g) + 0.0722 * l(c.b)
    }

    fn contrast(a: Color, b: Color) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn everything_can_be_read_in_both_themes() {
        let mut bad = Vec::new();
        for mode in [Mode::Light, Mode::Dark] {
            let t = theme(mode);
            let canvas = t.palette().background;
            let panel = surface(&t);
            let fill = |s: button::Style| match s.background {
                Some(Background::Color(c)) => c,
                _ => Color::TRANSPARENT,
            };
            let mut check = |what: &str, fg: Color, bg: Color, least: f32| {
                let c = contrast(over(fg, bg), bg);
                if c < least {
                    bad.push(format!("{mode:?} {what}: {c:.2} < {least}"));
                }
            };
            for (place, bg) in [("window", canvas), ("panel", panel)] {
                check(&format!("text on {place}"), text_color(&t), bg, 4.5);
                check(&format!("muted text on {place}"), muted(&t), bg, 4.5);
                check(&format!("danger on {place}"), t.palette().danger, bg, 4.5);
                check(&format!("icon on {place}"), icon(&t, svg::Status::Idle).color.unwrap(), bg, 3.0);
                check(&format!("muted icon on {place}"), icon_muted(&t, svg::Status::Idle).color.unwrap(), bg, 3.0);
                check(&format!("active icon on {place}"), accent(&t), bg, 3.0);
                let off = disabled(&t);
                check(&format!("disabled on {place}"), off.text_color, over(fill(off), bg), 3.0);
                check(&format!("empty check box on {place}"), text_color(&t).scale_alpha(0.5), bg, 3.0);
                check(&format!("focus ring on {place}"), accent(&t), bg, 3.0);
                let plain = secondary(&t, button::Status::Active);
                check(&format!("button on {place}"), plain.text_color, over(fill(plain), bg), 4.5);
                for status in [button::Status::Active, button::Status::Hovered] {
                    for (name, s) in [("primary", primary(&t, status)), ("danger", danger(&t, status))] {
                        check(&format!("{name} {status:?}"), s.text_color, fill(s), 4.5);
                    }
                }
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }
}
