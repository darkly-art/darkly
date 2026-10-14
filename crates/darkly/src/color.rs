//! CPU colour math shared by the brush graph's `hsv` node and the global
//! per-stroke colour jitter: the twin of `shaders/lib/colorspace.wgsl`
//! (`rgb_to_hsv` / `hsv_to_rgb`) and `shaders/lib/hsv_adjust.wgsl`
//! (`hsv_adjust`). A brush graph is evaluated on either path, so the two
//! must stay in numerical parity: same branch structure, same operation
//! order, same epsilon.
//!
//! The HSV conversion follows Krita's `KoColorConversions::RGBToHSV` /
//! `HSVToRGB` (`libs/pigment/KoColorConversions.cpp`); the adjustment curves
//! are Krita's `HSVTransform`
//! (`plugins/color/colorspaceextensions/kis_hsv_adjustment.cpp`, Cyrille
//! Berger and Dmitry Kazakov).

/// Achromatic / wrap threshold, the WGSL lib's `EPS`.
pub const HSV_EPS: f32 = 1e-6;

/// RGB in `[0, 1]` to `(h, s, v)` with `h` in degrees `[0, 360)` (0 when
/// achromatic) and `s`, `v` in `[0, 1]`.
pub fn rgb_to_hsv(c: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = c;
    let mx = r.max(g.max(b));
    let mn = r.min(g.min(b));
    let v = mx;
    let s = if mx > HSV_EPS { (mx - mn) / mx } else { 0.0 };
    let mut h = 0.0;
    if s >= HSV_EPS {
        let delta = mx - mn;
        h = if r == mx {
            (g - b) / delta
        } else if g == mx {
            2.0 + (b - r) / delta
        } else {
            4.0 + (r - g) / delta
        };
        h *= 60.0;
        if h < 0.0 {
            h += 360.0;
        }
    }
    [h, s, v]
}

/// Inverse of [`rgb_to_hsv`]: `h` in degrees, `s` and `v` in `[0, 1]`.
pub fn hsv_to_rgb(hsv: [f32; 3]) -> [f32; 3] {
    let [mut h, s, v] = hsv;
    if s < HSV_EPS {
        return [v, v, v];
    }
    if h > 360.0 - HSV_EPS {
        h -= 360.0;
    }
    h /= 60.0;
    // `floor` then truncate, as the WGSL does (`i32(floor(h))`): a plain
    // `as i32` would round toward zero and disagree for negative hues.
    let i = h.floor() as i32;
    let f = h - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// Wrap a hue in degrees into `[0, 360)`.
pub fn wrap360(h: f32) -> f32 {
    h - (h / 360.0).floor() * 360.0
}

/// Saturation scale, Krita `HSVTransform`: a nonlinear boost for `ds > 0`
/// (`ds = 0.5` doubles, `ds = 1` quadruples, capped at 1), a plain fade for
/// `ds <= 0` (`ds = -1` is fully grey).
pub fn apply_sat(s: f32, ds: f32) -> f32 {
    if ds > 0.0 {
        (s * (1.0 + ds + 2.0 * ds * ds)).min(1.0)
    } else {
        s * (ds + 1.0)
    }
}

/// Value scale, Krita `HSVTransform`: `dv > 0` lerps toward 1, `dv < 0` fades
/// toward 0, `dv = 0` is the identity.
pub fn apply_val(v: f32, dv: f32) -> f32 {
    if dv > 0.0 {
        v + dv * (1.0 - v)
    } else {
        v * (dv + 1.0)
    }
}

/// Rotate hue by `dh` degrees and scale saturation and value by `ds` / `dv`
/// in `[-1, 1]`. All-zero deltas return the input bit-exactly: no round trip
/// through HSV, so an identity adjustment never drifts.
pub fn hsv_adjust(rgb: [f32; 3], dh: f32, ds: f32, dv: f32) -> [f32; 3] {
    if dh == 0.0 && ds == 0.0 && dv == 0.0 {
        return rgb;
    }
    let [h, s, v] = rgb_to_hsv(rgb);
    hsv_to_rgb([wrap360(h + dh), apply_sat(s, ds), apply_val(v, dv)])
}

/// [`hsv_adjust`] on the colour channels of a straight (non-premultiplied)
/// RGBA value; alpha passes through untouched.
pub fn hsv_adjust_rgba(rgba: [f32; 4], dh: f32, ds: f32, dv: f32) -> [f32; 4] {
    let [r, g, b] = hsv_adjust([rgba[0], rgba[1], rgba[2]], dh, ds, dv);
    [r, g, b, rgba[3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    #[test]
    fn primaries_and_grey_to_hsv() {
        assert_eq!(rgb_to_hsv([1.0, 0.0, 0.0]), [0.0, 1.0, 1.0]);
        assert_eq!(rgb_to_hsv([0.0, 1.0, 0.0]), [120.0, 1.0, 1.0]);
        assert_eq!(rgb_to_hsv([0.0, 0.0, 1.0]), [240.0, 1.0, 1.0]);
        let [h, s, v] = rgb_to_hsv([0.5, 0.5, 0.5]);
        assert_eq!((h, s), (0.0, 0.0), "grey has no hue and no saturation");
        assert_eq!(v, 0.5);
        assert_eq!(rgb_to_hsv([0.0, 0.0, 0.0]), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn hsv_round_trips_the_rgb_cube() {
        for r in 0..=4 {
            for g in 0..=4 {
                for b in 0..=4 {
                    let rgb = [r as f32 / 4.0, g as f32 / 4.0, b as f32 / 4.0];
                    let back = hsv_to_rgb(rgb_to_hsv(rgb));
                    assert!(close(rgb, back), "{rgb:?} -> {back:?}");
                }
            }
        }
    }

    #[test]
    fn zero_deltas_are_bit_identical() {
        for rgb in [
            [0.1, 0.7, 0.3],
            [1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0],
            [0.33, 0.0, 0.99],
        ] {
            let out = hsv_adjust(rgb, 0.0, 0.0, 0.0);
            assert_eq!(out.map(f32::to_bits), rgb.map(f32::to_bits));
        }
        let rgba = [0.2, 0.4, 0.6, 0.5];
        assert_eq!(
            hsv_adjust_rgba(rgba, 0.0, 0.0, 0.0).map(f32::to_bits),
            rgba.map(f32::to_bits)
        );
    }

    #[test]
    fn hue_rotation_walks_the_primaries() {
        assert!(close(
            hsv_adjust([1.0, 0.0, 0.0], 120.0, 0.0, 0.0),
            [0.0, 1.0, 0.0]
        ));
        assert!(close(
            hsv_adjust([1.0, 0.0, 0.0], -120.0, 0.0, 0.0),
            [0.0, 0.0, 1.0]
        ));
        // A full turn (and a negative full turn) lands back on the input.
        assert!(close(
            hsv_adjust([0.2, 0.5, 0.9], 360.0, 0.0, 0.0),
            [0.2, 0.5, 0.9]
        ));
        assert!(close(
            hsv_adjust([0.2, 0.5, 0.9], -720.0, 0.0, 0.0),
            [0.2, 0.5, 0.9]
        ));
    }

    #[test]
    fn saturation_and_value_curves_match_krita() {
        assert!(
            (apply_sat(0.25, 0.5) - 0.5).abs() < 1e-6,
            "ds = 0.5 doubles"
        );
        assert_eq!(apply_sat(0.5, 1.0), 1.0, "the boost caps at 1");
        assert_eq!(apply_sat(0.5, -1.0), 0.0, "ds = -1 is fully grey");
        assert!(
            (apply_val(0.5, 0.5) - 0.75).abs() < 1e-6,
            "dv > 0 lerps toward 1"
        );
        assert_eq!(apply_val(0.5, -1.0), 0.0, "dv = -1 is black");
        assert_eq!(apply_val(0.5, 0.0), 0.5);
    }

    #[test]
    fn alpha_passes_through() {
        let out = hsv_adjust_rgba([1.0, 0.0, 0.0, 0.25], 90.0, -0.5, 0.5);
        assert_eq!(out[3], 0.25);
    }

    #[test]
    fn wrap360_covers_both_directions() {
        assert_eq!(wrap360(0.0), 0.0);
        assert_eq!(wrap360(370.0), 10.0);
        assert_eq!(wrap360(-10.0), 350.0);
        assert_eq!(wrap360(720.0), 0.0);
    }
}
