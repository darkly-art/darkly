//! Distance-based dab spacing.
//!
//! Spacing is expressed as a proportion of the dab diameter — the default
//! ~10% matches Krita's `KisSpacingInformation::isotropicSpacing`.  At each
//! `move_to`, the stroke engine walks from the previous position to the
//! current position, placing dabs at spacing intervals along the path.

/// How far apart dabs are placed along the stroke path.
#[derive(Clone, Copy, Debug)]
pub struct SpacingConfig {
    /// Spacing as a fraction of dab diameter (0.01–1.0).
    /// 0.1 = 10% of diameter (Krita default for most brushes).
    pub ratio: f32,
    /// Minimum spacing in pixels — prevents microscopic gaps at small sizes.
    pub min_px: f32,
}

impl Default for SpacingConfig {
    fn default() -> Self {
        Self {
            ratio: 0.10,
            min_px: 1.0,
        }
    }
}

impl SpacingConfig {
    /// Compute the actual spacing distance in pixels for a given dab diameter.
    pub fn distance(&self, diameter_px: f32) -> f32 {
        (diameter_px * self.ratio).max(self.min_px)
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
}
