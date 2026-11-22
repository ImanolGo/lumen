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
