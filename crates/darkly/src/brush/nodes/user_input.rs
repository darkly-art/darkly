//! User-exposed property node — a labeled scalar source that appears in the
//! brush properties panel.
//!
//! The system surfaces all `user_input` nodes as controls in the user-facing
//! properties panel, giving brush creators a way to expose named controls
//! without requiring end users to open the node graph.
//!
//! The node stores the value in the creator-specified range (min..max) and
//! outputs it normalized to 0–1: `(value - min) / (max - min)`.  This keeps
//! the graph operating in 0–1 space while letting the UI display real units.
//!
//! ## Parameters
//!
//! | Index | Type   | Name        | Description                                          |
//! |-------|--------|-------------|------------------------------------------------------|
//! | 0     | String | label       | Display name shown in the properties panel           |
//! | 1     | Float  | value       | Current value in min..max range                      |
//! | 2     | Float  | min         | Minimum value (default 0.0)                          |
//! | 3     | Float  | max         | Maximum value (default 1.0)                          |
//! | 4     | Int    | units       | Display unit (0=percent, 1=px, 2=degrees, 3=raw)     |
//! | 5     | String | icon        | Font Awesome class, e.g. `"fa-solid fa-circle"`      |
//! | 6     | String | description | Tooltip text shown on hover                          |

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::gpu::params::ParamDef;
use crate::nodegraph::{NodeRegistration, PortDef};

pub type BrushNodeRegistration = NodeRegistration<BrushWireType>;

/// Curated set of Font Awesome icons for brush property controls.
/// Each entry is (FA class, human-friendly label).
const ICON_OPTIONS: &[(&str, &str)] = &[
    ("", "None"),
    ("fa-solid fa-circle", "Circle"),
    ("fa-solid fa-droplet", "Droplet"),
    ("fa-solid fa-pen", "Pen"),
    ("fa-solid fa-paintbrush", "Paintbrush"),
    ("fa-solid fa-spray-can", "Spray Can"),
    ("fa-solid fa-fill-drip", "Fill"),
    ("fa-solid fa-eye-dropper", "Eyedropper"),
    ("fa-solid fa-arrows-left-right", "Width"),
    ("fa-solid fa-arrows-up-down", "Height"),
    ("fa-solid fa-up-right-and-down-left-from-center", "Size"),
    ("fa-solid fa-rotate", "Rotation"),
    ("fa-solid fa-sun", "Brightness"),
    ("fa-solid fa-moon", "Darkness"),
    ("fa-solid fa-star", "Star"),
    ("fa-solid fa-bolt", "Bolt"),
    ("fa-solid fa-feather", "Feather"),
    ("fa-solid fa-wand-magic-sparkles", "Magic"),
    ("fa-solid fa-sliders", "Sliders"),
    ("fa-solid fa-gauge", "Gauge"),
];

pub fn register() -> BrushNodeRegistration {
    NodeRegistration {
        type_id: "user_input",
        category: "input",
        display_name: "User Input",
        ports: vec![PortDef::output("value", BrushWireType::Scalar)
            .with_description("The user-controlled value, normalized to 0\u{2013}1")],
        params: &[
            ParamDef::String {
                name: "label",
                default: "",
            },
            ParamDef::Float {
                name: "value",
                min: 0.0,
                max: 1.0,
                default: 0.5,
            },
            ParamDef::FloatInput {
                name: "min",
                min: 0.0,
                max: 100000.0,
                default: 0.0,
            },
            ParamDef::FloatInput {
                name: "max",
                min: 0.0,
                max: 100000.0,
                default: 1.0,
            },
            ParamDef::Enum {
                name: "units",
                options: &["Percent", "Pixels", "Degrees", "Raw"],
                default: 0,
            },
            ParamDef::Icon {
                name: "icon",
                options: ICON_OPTIONS,
                default: "",
            },
            ParamDef::String {
                name: "description",
                default: "",
            },
        ],
        is_gpu: false,
    }
}

pub struct UserInputEvaluator;

impl BrushNodeEvaluator for UserInputEvaluator {
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let value = ctx.param_f32(1);
        let min = ctx.param_f32(2);
        let max = ctx.param_f32(3);
        let range = max - min;
        let normalized = if range.abs() > f32::EPSILON {
            ((value - min) / range).clamp(0.0, 1.0)
        } else {
            0.0
        };
        vec![("value".into(), ScalarValue::Scalar(normalized))]
    }
}
