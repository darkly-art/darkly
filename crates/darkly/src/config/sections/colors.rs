use crate::config::schema::{Pref, PrefKind, SchemaSection, WidgetHint};

const PREFS: &[Pref] = &[
    Pref {
        key: "colors.defaultForeground",
        display_name: "Default foreground",
        description: None,
        kind: PrefKind::Str,
        widget: WidgetHint::Color,
    },
    Pref {
        key: "colors.defaultBackground",
        display_name: "Default background",
        description: None,
        kind: PrefKind::Str,
        widget: WidgetHint::Color,
    },
    Pref {
        key: "colors.lockToBrush",
        display_name: "Lock colors to brush",
        description: Some(
            "Each brush keeps the foreground and background it was last used with; \
             switching brushes switches colors with them.",
        ),
        kind: PrefKind::Bool,
        widget: WidgetHint::Auto,
    },
    // The colour panel's Jitter tab. Amplitudes: each brush stroke draws one
    // offset per channel in `[-amp, amp]` at stroke start
    // (`brush::color_jitter`). Hidden from Settings: they are live tool state
    // with their own UI, like panel sizes.
    Pref {
        key: "colors.jitterHue",
        display_name: "Hue jitter",
        description: Some("Degrees either side of the foreground hue a stroke may land."),
        kind: PrefKind::Float {
            min: 0.0,
            max: 180.0,
        },
        widget: WidgetHint::Hidden,
    },
    Pref {
        key: "colors.jitterSaturation",
        display_name: "Saturation jitter",
        description: Some("How far a stroke's saturation may drift from the foreground's."),
        kind: PrefKind::Float { min: 0.0, max: 1.0 },
        widget: WidgetHint::Hidden,
    },
    Pref {
        key: "colors.jitterValue",
        display_name: "Value jitter",
        description: Some("How far a stroke's value may drift from the foreground's."),
        kind: PrefKind::Float { min: 0.0, max: 1.0 },
        widget: WidgetHint::Hidden,
    },
];

pub fn register() -> SchemaSection {
    SchemaSection {
        id: "colors",
        display_name: "Colors",
        description: Some(
            "Starting foreground and background swatches, and whether each brush remembers its own.",
        ),
        icon: Some("fa6-solid:palette"),
        order: 20,
        prefs: PREFS,
    }
}
