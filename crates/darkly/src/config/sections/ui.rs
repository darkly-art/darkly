use crate::config::schema::{Pref, PrefDefault, PrefKind, SchemaSection, WidgetHint};

const THEME_OPTIONS: &[(&str, &str)] = &[
    ("dark", "Dark"),
    ("light", "Light"),
    ("system", "Follow system"),
];

const PREFS: &[Pref] = &[
    Pref {
        key: "ui.theme",
        display_name: "Theme",
        description: Some("Dark, light, or follow the operating system."),
        kind: PrefKind::Enum {
            options: THEME_OPTIONS,
        },
        default: PrefDefault::Str("dark"),
        widget: WidgetHint::Auto,
        per_preset: &[],
    },
    // Brush builder pane state — persisted via the unified backend so it
    // survives reloads, but hidden from the Settings UI: it's UI state,
    // not a configurable preference.
    Pref {
        key: "ui.brushBuilder.previewVisible",
        display_name: "Brush preview pane visible",
        description: None,
        kind: PrefKind::Bool,
        default: PrefDefault::Bool(true),
        widget: WidgetHint::Hidden,
        per_preset: &[],
    },
    Pref {
        key: "ui.brushBuilder.previewWidth",
        display_name: "Brush preview width",
        description: None,
        kind: PrefKind::Int { min: 160, max: 800 },
        default: PrefDefault::Int(320),
        widget: WidgetHint::Hidden,
        per_preset: &[],
    },
    Pref {
        key: "ui.brushBuilder.previewHeight",
        display_name: "Brush preview height",
        description: None,
        kind: PrefKind::Int { min: 60, max: 400 },
        default: PrefDefault::Int(120),
        widget: WidgetHint::Hidden,
        per_preset: &[],
    },
];

pub fn register() -> SchemaSection {
    SchemaSection {
        id: "ui",
        display_name: "Interface",
        description: None,
        icon: Some("fa-solid fa-display"),
        order: 30,
        prefs: PREFS,
    }
}
