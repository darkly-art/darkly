use crate::config::schema::{Pref, PrefDefault, PrefKind, SchemaSection, WidgetHint};

const PREFS: &[Pref] = &[
    Pref {
        key: "colors.defaultForeground",
        display_name: "Default foreground",
        description: None,
        kind: PrefKind::Str,
        default: PrefDefault::Str("#000000"),
        widget: WidgetHint::Color,
        per_preset: &[],
    },
    Pref {
        key: "colors.defaultBackground",
        display_name: "Default background",
        description: None,
        kind: PrefKind::Str,
        default: PrefDefault::Str("#ffffff"),
        widget: WidgetHint::Color,
        per_preset: &[],
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
