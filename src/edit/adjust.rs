//! Exposure, contrast and saturation.

use image::DynamicImage;

/// Colour adjustments. Every field is 0 when it changes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Adjustments {
    /// Brightness in stops: -2 to 2.
    pub exposure: f32,
    /// -1 (flat grey) to 1 (very contrasty).
    pub contrast: f32,
    /// -1 (greyscale) to 1 (very saturated).
    pub saturation: f32,
}

impl Adjustments {
    pub fn is_neutral(&self) -> bool {
        *self == Self::default()
    }
}

/// Apply the adjustments in sRGB space, leaving alpha alone.
pub fn apply(image: DynamicImage, adjustments: Adjustments) -> DynamicImage {
    if adjustments.is_neutral() {
        return image;
    }
    let gain = 2f32.powf(adjustments.exposure);
    let contrast = 1.0 + adjustments.contrast;
    let saturation = 1.0 + adjustments.saturation;

    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();
    let mut raw = rgba.into_raw();
    let adjust = |pixels: &mut [u8]| {
        for pixel in pixels.as_chunks_mut::<4>().0 {
            let mut channels = [
                pixel[0] as f32 / 255.0,
                pixel[1] as f32 / 255.0,
                pixel[2] as f32 / 255.0,
            ];
            for c in &mut channels {
                *c *= gain;
                *c = (*c - 0.5) * contrast + 0.5;
            }
            // Saturation scales the distance from the pixel's own grey.
            let grey = 0.2126 * channels[0] + 0.7152 * channels[1] + 0.0722 * channels[2];
            for c in &mut channels {
                *c = grey + (*c - grey) * saturation;
            }
            let to_byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            pixel[..3].copy_from_slice(&channels.map(to_byte));
        }
    };

    // Big pictures are split into bands, one per core.
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    if raw.len() < 4_000_000 || threads == 1 {
        adjust(&mut raw);
    } else {
        let band = (raw.len() / 4).div_ceil(threads) * 4;
        std::thread::scope(|scope| {
            for part in raw.chunks_mut(band) {
                scope.spawn(|| adjust(part));
            }
        });
    }
    image::RgbaImage::from_raw(width, height, raw)
        .map(DynamicImage::ImageRgba8)
        .expect("the buffer keeps its size")
}

#[cfg(test)]
mod tests {
    use image::{Rgba, RgbaImage};

    use super::*;

    fn one_pixel(rgba: [u8; 4]) -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(1, 1, Rgba(rgba)))
    }

    fn result(adjustments: Adjustments, rgba: [u8; 4]) -> [u8; 4] {
        apply(one_pixel(rgba), adjustments)
            .to_rgba8()
            .get_pixel(0, 0)
            .0
    }

    #[test]
    fn neutral_changes_nothing() {
        assert_eq!(
            result(Adjustments::default(), [10, 200, 90, 255]),
            [10, 200, 90, 255]
        );
    }

    #[test]
    fn exposure_scales_in_stops() {
        let up = result(
            Adjustments {
                exposure: 1.0,
                ..Default::default()
            },
            [60, 60, 60, 255],
        );
        assert_eq!(up, [120, 120, 120, 255]);
        let down = result(
            Adjustments {
                exposure: -1.0,
                ..Default::default()
            },
            [60, 60, 60, 255],
        );
        assert_eq!(down, [30, 30, 30, 255]);
    }

    #[test]
    fn full_desaturation_gives_grey_and_keeps_alpha() {
        let [r, g, b, a] = result(
            Adjustments {
                saturation: -1.0,
                ..Default::default()
            },
            [200, 40, 40, 128],
        );
        assert!(r == g && g == b, "got {r} {g} {b}");
        assert_eq!(a, 128);
    }

    #[test]
    fn contrast_pushes_away_from_mid_grey() {
        let [r, ..] = result(
            Adjustments {
                contrast: 0.5,
                ..Default::default()
            },
            [200, 200, 200, 255],
        );
        assert!(r > 200);
        let [r, ..] = result(
            Adjustments {
                contrast: 0.5,
                ..Default::default()
            },
            [60, 60, 60, 255],
        );
        assert!(r < 60);
    }
}
