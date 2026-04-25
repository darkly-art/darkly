use crate::config::schema::{Pref, PrefDefault, PrefKind, SchemaSection, WidgetHint};

const PREFS: &[Pref] = &[
    Pref {
        key: "nav.panSensitivity",
        display_name: "Pan sensitivity",
        description: Some("Scales how far the canvas moves per pixel of drag."),
        kind: PrefKind::Float {
            min: 0.05,
            max: 4.0,
        },
        default: PrefDefault::Float(0.5),
        widget: WidgetHint::Auto,
        per_preset: &[],
    },
    Pref {
        key: "hotkeys.nav.trigger",
        display_name: "Navigation modifier",
        description: Some("Held key that engages canvas pan / zoom / rotate."),
        kind: PrefKind::Str,
        default: PrefDefault::Str("Space"),
        widget: WidgetHint::Hotkey,
        per_preset: &[],
    },
    Pref {
        key: "hotkeys.nav.rotate",
        display_name: "Rotate modifier",
        description: None,
        kind: PrefKind::Str,
        default: PrefDefault::Str("Shift"),
        widget: WidgetHint::Hotkey,
        per_preset: &[],
    },
    Pref {
        key: "hotkeys.nav.zoom",
        display_name: "Zoom modifier",
        description: None,
        kind: PrefKind::Str,
        default: PrefDefault::Str("Ctrl"),
        widget: WidgetHint::Hotkey,
        per_preset: &[],
    },
];

pub fn register() -> SchemaSection {
    SchemaSection {
        id: "nav",
        display_name: "Navigation",
        description: Some("Canvas pan, zoom, and rotate controls."),
        icon: Some("fa-solid fa-arrows-up-down-left-right"),
        order: 50,
        prefs: PREFS,
    }
}
