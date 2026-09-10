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
