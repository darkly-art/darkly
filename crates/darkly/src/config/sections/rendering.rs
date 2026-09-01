use crate::config::schema::{Pref, PrefKind, SchemaSection, WidgetHint};

// Two scales rather than one, because the two spaces are different things.
// The viewport is a viewing surface and can afford to trade resolution for
// speed; canvas space is document content, and a reduced-resolution round trip
// there would bake the loss into what the user exports.
const PREFS: &[Pref] = &[
    Pref {
        key: "rendering.screen_effect_scale",
        display_name: "Viewport effect scale",
        description: Some(
            "Fraction of native viewport resolution to render screen-space effects at. \
             1.0 = full res; lower values trade effect quality for speed.",
        ),
        kind: PrefKind::Float {
            min: 0.25,
            max: 1.0,
        },
        widget: WidgetHint::Auto,
    },
    Pref {
        key: "rendering.canvas_effect_scale",
        display_name: "Canvas effect scale",
        description: Some(
            "Fraction of canvas resolution to render effect layers at. Defaults to 1.0 — \
             this is document content, so the result is what gets exported.",
        ),
        kind: PrefKind::Float {
            min: 0.25,
            max: 1.0,
        },
        widget: WidgetHint::Auto,
    },
];

pub fn register() -> SchemaSection {
    SchemaSection {
        id: "rendering",
        display_name: "Rendering",
        description: Some("Viewport-level rendering knobs."),
        icon: Some("fa6-solid:gauge-high"),
        order: 70,
        prefs: PREFS,
    }
}
