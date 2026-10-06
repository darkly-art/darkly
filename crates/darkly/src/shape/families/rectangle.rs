//! Rectangle, optionally with circular corners.

use kurbo::{BezPath, RoundedRect, Shape};

use crate::gpu::params::{ParamDef, ParamSlots, ParamValue};
use crate::shape::{ShapeFamilyRegistration, TOLERANCE};
use crate::units::UnitType;

const PARAMS: &[ParamDef] = &[ParamDef::float("rounding", 0.0, 1.0, 0.0)
    .with_label("Rounding")
    .with_unit(UnitType::Percent)
    .with_description(
        "Corner radius as a share of the short side: 0% = square corners, 100% = fully round ends.",
    )];

pub fn register() -> ShapeFamilyRegistration {
    ShapeFamilyRegistration {
        type_id: "rectangle",
        display_name: "Rectangle",
        description: "A rectangle filling the dragged box, with optional rounded corners.",
        icon: "tabler:rectangle-filled",
        hotkey_action: "rectangleTool",
        params: PARAMS,
        outline,
    }
}

/// The radius is a fraction of the short side so it survives a gizmo scale
/// unchanged and matches the polygon family's rounding knob.
fn outline(params: &[ParamValue], w: f64, h: f64) -> BezPath {
    let rounding = (params.float_at(0, 0.0) as f64).clamp(0.0, 1.0);
    let radius = rounding * w.min(h) * 0.5;
    RoundedRect::new(0.0, 0.0, w, h, radius).to_path(TOLERANCE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn square_corners_cover_the_box() {
        let area = outline(&[ParamValue::Float(0.0)], 200.0, 100.0).area();
        assert!((area.abs() - 20_000.0).abs() < 1e-6);
    }

    #[test]
    fn full_rounding_on_a_square_is_a_disc() {
        let area = outline(&[ParamValue::Float(1.0)], 100.0, 100.0).area();
        assert!((area.abs() - PI * 50.0 * 50.0).abs() / (PI * 2500.0) < 0.01);
    }
}
