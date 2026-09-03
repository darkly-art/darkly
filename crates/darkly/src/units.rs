//! Display units for numeric values.
//!
//! Module-generic infrastructure: node-graph ports and effect parameters both
//! declare one, and both render through the same conversion + suffix table. It
//! lives at the crate root rather than under `nodegraph` because neither owns
//! it.

use serde::{Deserialize, Serialize};

/// Display unit for a numeric value.
///
/// Defines how a stored value is converted for display in the UI. The
/// conversion methods use `f32` math, so any numeric type (Scalar, Int) can
/// round-trip through them. Non-numeric types (Bool, Color) ignore this
/// field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub enum UnitType {
    /// Identity: display and internal are both raw values (shown as `0.50`).
    #[default]
    Normalized,
    /// Display as percentage: `display = value × 100`, suffix `%`.
    Percent,
    /// Wire unit is radians; display in degrees. `display = value × 180/π`, suffix `°`.
    Degrees,
    /// Identity with no suffix, useful for dimensionless multipliers.
    Raw,
    /// Identity with `px` suffix: value is in canvas pixels.
    Pixels,
}

impl UnitType {
    /// Convert from port-space to display-space.
    pub fn to_display(self, value: f32) -> f32 {
        match self {
            Self::Normalized | Self::Raw | Self::Pixels => value,
            Self::Percent => value * 100.0,
            Self::Degrees => value * (180.0 / std::f32::consts::PI),
        }
    }

    /// Convert from display-space back to port-space.
    pub fn from_display(self, display: f32) -> f32 {
        match self {
            Self::Normalized | Self::Raw | Self::Pixels => display,
            Self::Percent => display / 100.0,
            Self::Degrees => display * (std::f32::consts::PI / 180.0),
        }
    }

    /// Suffix string for display formatting.
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Normalized => "",
            Self::Percent => "%",
            Self::Degrees => "°",
            Self::Raw => "",
            Self::Pixels => "px",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_type_conversion_round_trip() {
        for unit in [
            UnitType::Normalized,
            UnitType::Percent,
            UnitType::Degrees,
            UnitType::Raw,
        ] {
            for &val in &[0.0, 0.25, 0.5, 0.75, 1.0] {
                let display = unit.to_display(val);
                let back = unit.from_display(display);
                assert!(
                    (back - val).abs() < 1e-6,
                    "{:?}: to_display({}) = {}, from_display({}) = {} (expected {})",
                    unit,
                    val,
                    display,
                    display,
                    back,
                    val,
                );
            }
        }
    }

    #[test]
    fn unit_type_display_values() {
        use std::f32::consts::PI;
        assert!((UnitType::Percent.to_display(0.5) - 50.0).abs() < 1e-6);
        // Degrees: wire unit is radians, display is degrees.
        assert!((UnitType::Degrees.to_display(PI) - 180.0).abs() < 1e-4);
        assert!((UnitType::Degrees.to_display(PI / 2.0) - 90.0).abs() < 1e-4);
        assert!((UnitType::Degrees.to_display(0.0) - 0.0).abs() < 1e-6);
        assert!((UnitType::Degrees.from_display(90.0) - PI / 2.0).abs() < 1e-4);
        assert!((UnitType::Normalized.to_display(0.5) - 0.5).abs() < 1e-6);
        assert!((UnitType::Raw.to_display(0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn unit_type_suffix() {
        assert_eq!(UnitType::Percent.suffix(), "%");
        assert_eq!(UnitType::Degrees.suffix(), "°");
        assert_eq!(UnitType::Normalized.suffix(), "");
        assert_eq!(UnitType::Raw.suffix(), "");
    }
}
