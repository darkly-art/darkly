use crate::config::schema::{Pref, PrefKind, SchemaSection, WidgetHint};

const PREFS: &[Pref] = &[
    Pref {
        key: "canvas.width",
        display_name: "Default width",
        description: Some("Width in pixels for new documents."),
        kind: PrefKind::Int { min: 1, max: 16384 },
        widget: WidgetHint::NumberInput,
    },
    Pref {
        key: "canvas.height",
        display_name: "Default height",
        description: Some("Height in pixels for new documents."),
        kind: PrefKind::Int { min: 1, max: 16384 },
        widget: WidgetHint::NumberInput,
    },
    Pref {
        key: "canvas.backgroundColor",
        display_name: "Background color",
        description: Some("Fill color used for new documents."),
        kind: PrefKind::Str,
        widget: WidgetHint::Color,
    },
];

pub fn register() -> SchemaSection {
    SchemaSection {
        id: "canvas",
        display_name: "Canvas",
        description: Some("Default dimensions and background for new documents."),
        icon: Some("fa-solid fa-vector-square"),
        order: 10,
        prefs: PREFS,
    }
}
