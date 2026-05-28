use crate::config::schema::{Pref, PrefDefault, PrefKind, SchemaSection, WidgetHint};

const PIXEL_FILTER_OPTIONS: &[(&str, &str)] = &[
    ("auto", "Auto (sharp when zoomed in)"),
    ("linear", "Linear (smooth)"),
    ("nearest", "Nearest (hard pixels)"),
];

const PREFS: &[Pref] = &[Pref {
    key: "display.pixelFilter",
    display_name: "Pixel filter",
    description: Some(
        "How canvas pixels are sampled when drawn to screen. \
         Auto switches to nearest-neighbor (hard pixel edges) when zoomed past 100%, \
         and linear (smooth) otherwise.",
    ),
    kind: PrefKind::Enum {
        options: PIXEL_FILTER_OPTIONS,
    },
    default: PrefDefault::Str("auto"),
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
