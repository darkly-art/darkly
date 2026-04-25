use crate::config::schema::{Pref, PrefDefault, PrefKind, SchemaSection, WidgetHint};

const PREFS: &[Pref] = &[Pref {
    key: "rendering.veil_scale",
    display_name: "Veil render scale",
    description: Some(
        "Fraction of native viewport resolution to render veils at. \
             1.0 = full res; lower values trade veil quality for speed.",
    ),
    kind: PrefKind::Float {
        min: 0.25,
        max: 1.0,
    },
    default: PrefDefault::Float(1.0),
    widget: WidgetHint::Auto,
}];

pub fn register() -> SchemaSection {
    SchemaSection {
        id: "rendering",
        display_name: "Rendering",
        description: Some("Viewport-level rendering knobs."),
        icon: Some("fa-solid fa-gauge-high"),
        order: 70,
        prefs: PREFS,
    }
}
