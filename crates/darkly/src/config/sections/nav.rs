use crate::config::schema::{Pref, PrefKind, SchemaSection, WidgetHint};

const PREFS: &[Pref] = &[
    Pref {
        key: "nav.panSensitivity",
        display_name: "Pan sensitivity",
        description: Some("Scales how far the canvas moves per pixel of drag."),
        kind: PrefKind::Float {
            min: 0.05,
            max: 4.0,
        },
        widget: WidgetHint::Auto,
    },
    Pref {
        key: "hotkeys.nav.trigger",
        display_name: "Navigation modifier",
        description: Some("Held key that engages canvas pan / zoom / rotate."),
        kind: PrefKind::Str,
        widget: WidgetHint::Hotkey,
    },
    Pref {
        key: "hotkeys.nav.rotate",
        display_name: "Rotate modifier",
        description: None,
        kind: PrefKind::Str,
        widget: WidgetHint::Hotkey,
    },
    Pref {
        key: "hotkeys.nav.zoom",
        display_name: "Zoom modifier",
        description: None,
        kind: PrefKind::Str,
        widget: WidgetHint::Hotkey,
    },
    Pref {
        key: "hotkeys.nav.rotateSnap",
        display_name: "Rotate snap modifier",
        description: Some("Held while rotating to hard-snap the canvas to 15° increments."),
        kind: PrefKind::Str,
        widget: WidgetHint::Hotkey,
    },
    Pref {
        key: "nav.rotateDetent",
        display_name: "Snap rotation to 45° marks",
        description: Some("Free canvas rotation sticks to the nearest 45° mark when close."),
        kind: PrefKind::Bool,
        widget: WidgetHint::Auto,
    },
];

pub fn register() -> SchemaSection {
    SchemaSection {
        id: "nav",
        display_name: "Navigation",
        description: Some("Canvas pan, zoom, and rotate controls."),
        icon: Some("fa6-solid:arrows-up-down-left-right"),
        order: 50,
        prefs: PREFS,
    }
}
