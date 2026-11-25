//! The size of a window that opens on a picture: what the picture needs, within reason.

use std::path::PathBuf;

use iced::Size;

/// Never smaller (where the screen allows), nor more of the screen.
const LEAST: Size = Size::new(720.0, 480.0);
const OF_SCREEN: f32 = 0.8;
/// No window is smaller than this.
pub const SMALLEST: Size = Size::new(520.0, 360.0);
/// Around the picture: the space beside it, and that with the header above it.
const AROUND: Size = Size::new(24.0, 24.0 + 48.0);

/// The size, in pixels, of the picture the window opens on, if it is one picture.
pub fn picture(paths: &[PathBuf]) -> Option<(u32, u32)> {
    // A script is given the size it wrote down.
    if crate::control::stdio() {
        return None;
    }
    match paths {
        [one] if one.is_file() => crate::raster::shown_size(one),
        _ => None,
    }
}

/// For a picture on a screen of that size (logical pixels) and scale.
pub fn fitting((w, h): (u32, u32), scale: f32, screen: Size) -> Size {
    let most =
        Size::new((screen.width * OF_SCREEN).max(SMALLEST.width), (screen.height * OF_SCREEN).max(SMALLEST.height));
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    let zoom = ((most.width - AROUND.width) / w).min((most.height - AROUND.height) / h).min(1.0 / scale.max(0.5));
    let fit = |picture: f32, around: f32, least: f32, most: f32| (picture * zoom + around).max(least).min(most).round();
    Size::new(fit(w, AROUND.width, LEAST.width, most.width), fit(h, AROUND.height, LEAST.height, most.height))
}

/// A first guess, before anything is known of the screen: no larger than a usual window.
pub fn guess(picture: (u32, u32)) -> Size {
    let usual = Size::new(crate::WINDOW.0 / OF_SCREEN, crate::WINDOW.1 / OF_SCREEN);
    fitting(picture, 1.0, usual)
}

/// Whether two sizes are the same but for rounding.
pub fn same(a: Size, b: Size) -> bool {
    (a.width - b.width).abs() <= 2.0 && (a.height - b.height).abs() <= 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Size = Size::new(1920.0, 1080.0);

    #[test]
    fn the_window_fits_the_picture_within_reason() {
        // At its own size, with the header.
        assert_eq!(fitting((1000, 600), 1.0, SCREEN), Size::new(1024.0, 672.0));
        // A small one does not make a tiny window.
        assert_eq!(fitting((64, 64), 1.0, SCREEN), LEAST);
        // A huge one keeps to 80 % of the screen, and its shape.
        let large = fitting((8000, 4000), 1.0, SCREEN);
        assert_eq!(large.width, 1536.0);
        assert!((large.height - (1512.0 / 2.0 + 72.0)).abs() < 1.0, "{large:?}");
        let tall = fitting((1000, 9000), 1.0, SCREEN);
        assert_eq!(tall, Size::new(720.0, 864.0));
        // On a screen of twice the density it takes half as much room.
        assert_eq!(fitting((2000, 1200), 2.0, SCREEN), Size::new(1024.0, 672.0));
        // A small screen comes before the least size.
        assert_eq!(fitting((64, 64), 1.0, Size::new(800.0, 500.0)), Size::new(640.0, 400.0));
    }

    #[test]
    fn only_one_picture_decides_the_size() {
        let fixture = |name: &str| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
        assert_eq!(picture(&[fixture("pictures/g.png")]), Some((400, 300)));
        assert_eq!(picture(&[fixture("pages.pdf")]), None);
        assert_eq!(picture(&[fixture("pictures/g.png"), fixture("pictures/g.jpg")]), None);
        assert_eq!(picture(&[fixture("pictures")]), None);
        assert_eq!(picture(&[]), None);
        assert!(guess((9000, 9000)).height <= crate::WINDOW.1);
    }
}
