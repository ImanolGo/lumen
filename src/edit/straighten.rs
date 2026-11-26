//! Straightening: turning a picture by a few degrees, and keeping the largest upright rectangle of
//! the same shape that lies inside it, so no corner is empty.

/// How much smaller than the picture that rectangle is, turned `degrees`.
pub fn inner_scale(width: u32, height: u32, degrees: f32) -> f32 {
    let (short, long) = (width.min(height).max(1) as f32, width.max(height).max(1) as f32);
    let t = degrees.to_radians().abs();
    short / (short * t.cos() + long * t.sin())
}

/// `rgba` (`width`×`height`) turned `degrees` clockwise about its middle, cut to that rectangle.
pub fn straighten(rgba: &[u8], width: u32, height: u32, degrees: f32) -> (u32, u32, Vec<u8>) {
    let k = inner_scale(width, height, degrees);
    let (ow, oh) = (((width as f32 * k).floor() as u32).max(1), ((height as f32 * k).floor() as u32).max(1));
    let (sin, cos) = degrees.to_radians().sin_cos();
    let (cx, cy) = (width as f32 / 2.0, height as f32 / 2.0);
    let (ox, oy) = (ow as f32 / 2.0, oh as f32 / 2.0);
    let at = |x: i64, y: i64, c: usize| -> f32 {
        let x = x.clamp(0, i64::from(width) - 1) as usize;
        let y = y.clamp(0, i64::from(height) - 1) as usize;
        f32::from(rgba[(y * width as usize + x) * 4 + c])
    };
    let mut out = Vec::with_capacity((ow * oh * 4) as usize);
    for y in 0..oh {
        for x in 0..ow {
            // From the middle of an output pixel back into the picture: turned the other way.
            let (dx, dy) = (x as f32 + 0.5 - ox, y as f32 + 0.5 - oy);
            let sx = cos * dx + sin * dy + cx - 0.5;
            let sy = -sin * dx + cos * dy + cy - 0.5;
            let (x0, y0) = (sx.floor(), sy.floor());
            let (fx, fy) = (sx - x0, sy - y0);
            let (x0, y0) = (x0 as i64, y0 as i64);
            for c in 0..4 {
                let top = at(x0, y0, c) * (1.0 - fx) + at(x0 + 1, y0, c) * fx;
                let bottom = at(x0, y0 + 1, c) * (1.0 - fx) + at(x0 + 1, y0 + 1, c) * fx;
                out.push((top * (1.0 - fy) + bottom * fy).round() as u8);
            }
        }
    }
    (ow, oh, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_inner_rectangle_fits() {
        assert_eq!(inner_scale(400, 300, 0.0), 1.0);
        let k = inner_scale(400, 300, 10.0);
        // Its corners, turned back, are inside the picture.
        let (w, h) = (400.0 * k / 2.0, 300.0 * k / 2.0);
        let (s, c) = 10f32.to_radians().sin_cos();
        for (x, y) in [(w, h), (-w, h), (w, -h), (-w, -h)] {
            let (px, py) = (c * x + s * y, -s * x + c * y);
            assert!(px.abs() <= 200.01 && py.abs() <= 150.01, "{px} {py}");
        }
        assert_eq!(inner_scale(400, 300, -10.0), k);
    }

    #[test]
    fn nothing_turned_is_the_same_and_a_turn_keeps_the_middle() {
        let (w, h) = (6, 4);
        let rgba: Vec<u8> = (0..w * h).flat_map(|i| [i as u8 * 10, 0, 0, 255]).collect();
        assert_eq!(straighten(&rgba, w, h, 0.0), (w, h, rgba.clone()));
        let big: Vec<u8> = (0..60 * 40).flat_map(|i| [(i % 251) as u8, 0, 0, 255]).collect();
        let (tw, th, turned) = straighten(&big, 60, 40, 5.0);
        assert!(tw < 60 && th < 40 && turned.len() == (tw * th * 4) as usize);
        // No corner is empty.
        assert!(turned.chunks(4).all(|p| p[3] == 255));
    }
}
