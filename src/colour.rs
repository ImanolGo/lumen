//! Colour profiles: a picture that says how its colours are meant (an ICC profile: Adobe RGB, Display
//! P3…) is turned into sRGB, which is what the screen and the rest of Lumen take pixels to be.

use moxcms::{ColorProfile, Layout, TransformOptions};

/// `rgba` (straight RGBA8) as sRGB, from the colours `icc` describes. `None` when the profile is not
/// for RGB pixels or cannot be read: the pixels are then shown as they are.
pub fn to_srgb(rgba: &[u8], icc: &[u8]) -> Option<Vec<u8>> {
    let from = ColorProfile::new_from_slice(icc).ok()?;
    let transform = from
        .create_transform_8bit(Layout::Rgba, &ColorProfile::new_srgb(), Layout::Rgba, TransformOptions::default())
        .ok()?;
    let mut out = vec![0; rgba.len()];
    transform.transform(rgba, &mut out).ok()?;
    Some(out)
}

/// Whether pixels with this profile are turned into sRGB when they are shown; then a copy of them
/// is sRGB, and must not carry the old profile.
pub fn is_converted(icc: &[u8]) -> bool {
    to_srgb(&[128, 128, 128, 255], icc).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Display P3 as moxcms writes it.
    fn p3() -> Vec<u8> {
        ColorProfile::new_display_p3().encode().unwrap()
    }

    #[test]
    fn a_wide_gamut_red_is_redder_than_srgb_can_show() {
        let red = [255, 0, 0, 255];
        let shown = to_srgb(&red, &p3()).unwrap();
        // P3's red lies outside sRGB: it is clipped at full red, with no green or blue in it.
        assert!(shown[0] == 255 && shown[1] < 10 && shown[2] < 10, "{shown:?}");
        // A P3 middle green comes out more saturated in sRGB terms than it went in.
        let green = to_srgb(&[100, 180, 100, 255], &p3()).unwrap();
        assert!(green[1] > 180 || green[0] < 100, "{green:?}");
        assert_eq!(green[3], 255, "alpha is left alone");
        // sRGB to sRGB changes nothing (but rounding).
        let same = to_srgb(&[100, 180, 100, 255], &ColorProfile::new_srgb().encode().unwrap()).unwrap();
        assert!(same.iter().zip([100, 180, 100, 255]).all(|(a, b)| a.abs_diff(b) <= 1), "{same:?}");
        assert!(is_converted(&p3()) && !is_converted(b"not a profile"));
    }
}
