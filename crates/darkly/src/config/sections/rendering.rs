use crate::config::schema::{Pref, PrefKind, SchemaSection, WidgetHint};

// One scale for every effect, on either side of the screen-space divider. An
// effect layer is the same object wherever it sits, so the resolution it runs
// at is a global quality/speed preference rather than a property of the space.
const PREFS: &[Pref] = &[Pref {
    key: "rendering.effect_scale",
    display_name: "Effect scale",
    description: Some(
        "Fraction of native resolution to render effects at, both in the viewport and \
             on the canvas. 1.0 = full res; lower values trade quality for speed. Effect \
             layers are document content, so this is also the resolution they export at.",
    ),
    kind: PrefKind::Float {
        min: 0.25,
        max: 1.0,
    },
    widget: WidgetHint::Auto,
}];

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
