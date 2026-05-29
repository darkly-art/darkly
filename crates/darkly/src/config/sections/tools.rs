use crate::config::schema::{Pref, PrefKind, SchemaSection, WidgetHint};

const SAMPLE_SOURCE_OPTIONS: &[(&str, &str)] = &[
    ("merged", "All layers merged"),
    ("currentLayer", "Current layer only"),
];

const PREFS: &[Pref] = &[Pref {
    key: "tools.colorPickerSampleSource",
    display_name: "Color picker sample source",
    description: Some(
        "What the color picker (and the modifier-held temporary pick) samples \
         from. \"All layers merged\" reads the final composite; \"Current \
         layer only\" reads the active raster layer in isolation, falling \
         back to the composite when the active node is a group or the \
         pointer is outside the layer's extent.",
    ),
    kind: PrefKind::Enum {
        options: SAMPLE_SOURCE_OPTIONS,
    },
    widget: WidgetHint::Auto,
}];

pub fn register() -> SchemaSection {
    SchemaSection {
        id: "tools",
        display_name: "Tools",
        description: Some("Tool-specific behavior."),
        icon: Some("fa-solid fa-screwdriver-wrench"),
        order: 35,
        prefs: PREFS,
    }
}
