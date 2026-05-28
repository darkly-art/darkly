use crate::config::schema::{Pref, PrefDefault, PrefKind, SchemaSection, WidgetHint};

const SAMPLE_SOURCE_OPTIONS: &[(&str, &str)] = &[
    ("merged", "All layers merged"),
    ("currentLayer", "Current layer only"),
];

const PREFS: &[Pref] = &[Pref {
    key: "tools.colorPickerSampleSource",
    display_name: "Eyedropper sample source",
    description: Some(
        "What the eyedropper (and the modifier-held temporary pick) samples \
         from. \"All layers merged\" reads the final composite; \"Current \
         layer only\" reads the active raster layer in isolation, falling \
         back to the composite when the active node is a group or the \
         pointer is outside the layer's extent.",
    ),
    kind: PrefKind::Enum {
        options: SAMPLE_SOURCE_OPTIONS,
    },
    default: PrefDefault::Str("merged"),
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
