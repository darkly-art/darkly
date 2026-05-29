use crate::config::schema::{Pref, PrefKind, SchemaSection, WidgetHint};

const PIXEL_FILTER_OPTIONS: &[(&str, &str)] = &[
    ("auto", "Auto"),
    ("linear", "Always smooth"),
    ("nearest", "Always sharp"),
];

const PREFS: &[Pref] = &[Pref {
    key: "display.pixelFilter",
    display_name: "Zoom appearance",
    description: Some(
        "How the canvas looks when zoomed in or out. \
         Auto keeps edges smooth at normal zoom and shows crisp pixels when zoomed in past 100%.",
    ),
    kind: PrefKind::Enum {
        options: PIXEL_FILTER_OPTIONS,
    },
    widget: WidgetHint::Auto,
}];

pub fn register() -> SchemaSection {
    SchemaSection {
        id: "display",
        display_name: "Display",
        description: Some("Viewport display options."),
        icon: Some("fa-solid fa-eye"),
        order: 35,
        prefs: PREFS,
    }
}
