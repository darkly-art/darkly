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
];

pub fn register() -> SchemaSection {
    SchemaSection {
        id: "colors",
        display_name: "Colors",
        description: Some("Starting foreground and background swatches."),
        icon: Some("fa-solid fa-palette"),
        order: 20,
        prefs: PREFS,
    }
}
