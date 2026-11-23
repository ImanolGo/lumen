//! Symbolic toolbar icons (24×24, stroked), embedded in the binary and recoloured
//! to the theme's text colour at draw time.

use iced::widget::svg;
use std::sync::LazyLock;

macro_rules! icon {
    ($name:ident, $body:expr) => {
        pub fn $name() -> svg::Handle {
            static H: LazyLock<svg::Handle> = LazyLock::new(|| {
                svg::Handle::from_memory(
                    concat!(
                        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">"#,
                        $body,
                        "</svg>"
                    )
                    .as_bytes(),
                )
            });
            H.clone()
        }
    };
}

icon!(
    open,
    r#"<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/><path d="M12 17v-6"/><path d="m9.5 13.5 2.5-2.5 2.5 2.5"/>"#
);
icon!(folder, r#"<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>"#);
icon!(
    print,
    r#"<path d="M7 9V3h10v6"/><rect x="3" y="9" width="18" height="8" rx="2"/><path d="M7 14h10v7H7z"/><circle cx="17.5" cy="12" r=".6" fill="black"/>"#
);
icon!(zoom_in, r#"<circle cx="11" cy="11" r="7"/><path d="m20 20-4-4"/><path d="M11 8v6M8 11h6"/>"#);
icon!(zoom_out, r#"<circle cx="11" cy="11" r="7"/><path d="m20 20-4-4"/><path d="M8 11h6"/>"#);
icon!(
    fit,
    r#"<path d="M4 9V5a1 1 0 0 1 1-1h4M15 4h4a1 1 0 0 1 1 1v4M20 15v4a1 1 0 0 1-1 1h-4M9 20H5a1 1 0 0 1-1-1v-4"/>"#
);
icon!(
    fit_width,
    r#"<path d="M3 5v14M21 5v14"/><path d="M7 12h10"/><path d="m9.5 9.5-2.5 2.5 2.5 2.5M14.5 9.5l2.5 2.5-2.5 2.5"/>"#
);
icon!(prev, r#"<path d="m15 18-6-6 6-6"/>"#);
icon!(next, r#"<path d="m9 18 6-6-6-6"/>"#);
icon!(up, r#"<path d="m18 15-6-6-6 6"/>"#);
icon!(down, r#"<path d="m6 9 6 6 6-6"/>"#);
icon!(rotate_left, r#"<path d="M3 12a9 9 0 1 0 3-6.7L3 8"/><path d="M3 3v5h5"/>"#);
icon!(rotate_right, r#"<path d="M21 12a9 9 0 1 1-3-6.7L21 8"/><path d="M21 3v5h-5"/>"#);
icon!(
    fullscreen,
    r#"<path d="M8 3H5a2 2 0 0 0-2 2v3M21 8V5a2 2 0 0 0-2-2h-3M3 16v3a2 2 0 0 0 2 2h3M16 21h3a2 2 0 0 0 2-2v-3"/>"#
);
icon!(strip, r#"<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M9 4v16M5 8h2M5 11h2M5 14h2"/>"#);
icon!(
    broken,
    r#"<rect x="3" y="3" width="18" height="18" rx="2"/><path d="m3 15 5-5 4 4"/><path d="m14 12 7 7"/><path d="M16 8h.01"/>"#
);
icon!(drop_files, r#"<path d="M12 3v12"/><path d="m7 10 5 5 5-5"/><path d="M5 21h14"/>"#);

/// The full-colour application icon, shown on the empty screen.
pub fn app() -> svg::Handle {
    static H: LazyLock<svg::Handle> =
        LazyLock::new(|| svg::Handle::from_memory(include_bytes!("../assets/app-icon.svg").as_slice()));
    H.clone()
}
icon!(search, r#"<circle cx="10.5" cy="10.5" r="6"/><path d="m15 15 5 5"/>"#);
icon!(close, r#"<path d="M6 6l12 12M18 6 6 18"/>"#);
icon!(markup, r#"<path d="M4 20h4L19 9a2.8 2.8 0 0 0-4-4L4 16z"/><path d="m13.5 6.5 4 4"/>"#);
icon!(select, r#"<path d="M5 3l14 7-6 2-2 6z"/>"#);
icon!(
    highlighter,
    r#"<path d="m9 11-5 5v3h6l5-5"/><path d="m9 11 6-6 4 4-6 6z"/><path d="M14 21h7" stroke-width="3" opacity=".55"/>"#
);
icon!(pen, r#"<path d="M3 17c3-6 5-9 7-6s1 7 4 6 4-7 7-9"/>"#);
icon!(rectangle, r#"<rect x="4" y="6" width="16" height="12" rx="1"/>"#);
icon!(ellipse, r#"<ellipse cx="12" cy="12" rx="8.5" ry="6.5"/>"#);
icon!(line, r#"<path d="M5 19 19 5"/>"#);
icon!(arrow, r#"<path d="M5 19 19 5"/><path d="M10 5h9v9"/>"#);
icon!(text_tool, r#"<path d="M5 6V4h14v2"/><path d="M12 4v16"/><path d="M9 20h6"/>"#);
icon!(undo, r#"<path d="M9 14 4 9l5-5"/><path d="M4 9h11a5 5 0 0 1 0 10h-3"/>"#);
icon!(redo, r#"<path d="m15 14 5-5-5-5"/><path d="M20 9H9a5 5 0 0 0 0 10h3"/>"#);
icon!(delete, r#"<path d="M4 7h16"/><path d="M10 11v6M14 11v6"/><path d="M6 7l1 13h10l1-13"/><path d="M9 7V4h6v3"/>"#);
icon!(width_thin, r#"<path d="M5 12h14" stroke-width="1.2"/>"#);
icon!(width_medium, r#"<path d="M5 12h14" stroke-width="3"/>"#);
icon!(width_thick, r#"<path d="M5 12h14" stroke-width="5.5"/>"#);
icon!(save_copy, r#"<path d="M5 4h11l3 3v13H5z"/><path d="M8 4v5h7V4"/><path d="M8 20v-6h8v6"/>"#);
icon!(signature, r#"<path d="M3 17c2-1 3.5-5 5-5s0 5 2 5 3-4 4.5-4 1 3 2.5 3 2-1 3-1.5"/><path d="M3 21h18"/>"#);
icon!(
    lock,
    r#"<rect x="5" y="10" width="14" height="11" rx="2"/><path d="M8 10V7a4 4 0 0 1 8 0v3"/><path d="M12 14v3"/>"#
);
icon!(flip_h, r#"<path d="M12 3v18"/><path d="M8 7 3 12l5 5z"/><path d="m16 7 5 5-5 5z"/>"#);
icon!(flip_v, r#"<path d="M3 12h18"/><path d="M7 8l5-5 5 5z"/><path d="m7 16 5 5 5-5z"/>"#);
icon!(crop, r#"<path d="M6 2v14a2 2 0 0 0 2 2h14"/><path d="M18 22V8a2 2 0 0 0-2-2H2"/>"#);
icon!(
    adjust,
    r#"<path d="M4 6h10M18 6h2M4 12h4M12 12h8M4 18h12M20 18h0"/><circle cx="16" cy="6" r="2"/><circle cx="10" cy="12" r="2"/><circle cx="18" cy="18" r="2"/>"#
);
icon!(reset, r#"<path d="M3 12a9 9 0 1 0 3-6.7L3 8"/><path d="M3 3v5h5"/><path d="M12 8v4l3 2"/>"#);
icon!(
    pages,
    r#"<rect x="3" y="3" width="7" height="9" rx="1"/><rect x="14" y="3" width="7" height="9" rx="1"/><rect x="3" y="15" width="7" height="6" rx="1"/><rect x="14" y="15" width="7" height="6" rx="1"/>"#
);
icon!(move_back, r#"<path d="m15 18-6-6 6-6"/><path d="M5 5v14"/>"#);
icon!(move_forward, r#"<path d="m9 18 6-6-6-6"/><path d="M19 5v14"/>"#);
icon!(
    insert,
    r#"<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/><path d="M12 11v6M9 14h6"/>"#
);
