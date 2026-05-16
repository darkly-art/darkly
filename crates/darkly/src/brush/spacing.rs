//! Distance-based dab spacing.
//!
//! Spacing is expressed as a proportion of the dab diameter — the default
//! ~10% matches Krita's `KisSpacingInformation::isotropicSpacing`.  At each
//! `move_to`, the stroke engine walks from the previous position to the
//! current position, placing dabs at spacing intervals along the path.

/// Hard architectural floor on dab spacing, in canvas pixels. Sub-pixel
/// stepping produces catastrophic dab counts for small brushes (one dab
/// per fractional pixel of stroke), so the engine refuses to advance by
/// less than this regardless of `SpacingConfig`.
pub const ABSOLUTE_MIN_SPACING_PX: f32 = 2.0;

/// How far apart dabs are placed along the stroke path.
#[derive(Clone, Copy, Debug)]
pub struct SpacingConfig {
    /// Spacing as a fraction of dab diameter (0.01–1.0).
    /// 0.1 = 10% of diameter (Krita default for most brushes).
    pub ratio: f32,
    /// Per-config minimum spacing in pixels — prevents microscopic gaps
    /// at small sizes. `distance()` additionally enforces an absolute
    /// floor of `ABSOLUTE_MIN_SPACING_PX` below this.
    pub min_px: f32,
}

impl Default for SpacingConfig {
    fn default() -> Self {
        Self {
            ratio: 0.10,
            min_px: ABSOLUTE_MIN_SPACING_PX,
        }
    }
}

impl SpacingConfig {
    /// Compute the actual spacing distance in pixels for a given dab diameter.
    /// Always returns at least `ABSOLUTE_MIN_SPACING_PX`, regardless of
    /// `ratio`, `min_px`, or `diameter_px` — including NaN inputs.
    pub fn distance(&self, diameter_px: f32) -> f32 {
        (diameter_px * self.ratio)
            .max(self.min_px)
            .max(ABSOLUTE_MIN_SPACING_PX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_spacing() {
        let s = SpacingConfig::default();
        // 100px diameter → 10px spacing
        assert!((s.distance(100.0) - 10.0).abs() < 1e-6);
    }

    #[test]
    fn min_spacing_clamp() {
        let s = SpacingConfig {
            ratio: 0.1,
            min_px: 2.0,
        };
        // 5px diameter → 0.5px spacing, clamped to 2.0
        assert!((s.distance(5.0) - 2.0).abs() < 1e-6);
    }

    /// 1px diameter at the default 10% ratio would yield 0.1px spacing
    /// without the absolute floor. The hard floor must clamp it to 1.0.
    #[test]
    fn absolute_floor_holds_for_tiny_diameter() {
        let s = SpacingConfig::default();
        assert!(s.distance(1.0) >= ABSOLUTE_MIN_SPACING_PX);
        assert!(s.distance(0.5) >= ABSOLUTE_MIN_SPACING_PX);
        assert!(s.distance(0.0) >= ABSOLUTE_MIN_SPACING_PX);
    }

    /// Even a `SpacingConfig` constructed with `min_px` below the absolute
    /// floor must not produce sub-pixel stepping. The floor lives in
    /// `distance()` itself, not just in `Default`.
    #[test]
    fn absolute_floor_overrides_low_min_px() {
        let s = SpacingConfig {
            ratio: 0.04,
            min_px: 0.0,
        };
        // 10px * 0.04 = 0.4px target; min_px = 0; absolute floor = 1.0.
        assert!(s.distance(10.0) >= ABSOLUTE_MIN_SPACING_PX);
    }

    /// NaN inputs must not produce sub-pixel (or NaN) stepping. Rust's
    /// `f32::max` returns the non-NaN argument when one side is NaN, so
    /// `NaN.max(1.0) == 1.0`. Pin this behaviour down so a future refactor
    /// to e.g. `min(NaN, 1.0)` doesn't silently regress.
    #[test]
    fn absolute_floor_holds_for_nan() {
        let s = SpacingConfig {
            ratio: f32::NAN,
            min_px: 1.0,
        };
        let d = s.distance(5.0);
        assert!(d >= ABSOLUTE_MIN_SPACING_PX, "got {d}");
        assert!(d.is_finite(), "got {d}");
    }
}
