use crate::config::schema::{Pref, PrefDefault, PrefKind, PresetValue, SchemaSection, WidgetHint};

const PREFS: &[Pref] = &[
    Pref {
        key: "bindings.layerEye.alt+click",
        display_name: "Alt+click layer eye",
        description: Some("Action dispatched when Alt+clicking a layer's visibility eye."),
        kind: PrefKind::Str,
        default: PrefDefault::Str(""),
        widget: WidgetHint::MouseBinding,
        per_preset: &[("Photoshop", PresetValue::Str("isolateLayer"))],
    },
    Pref {
        key: "bindings.layerEye.ctrl+click",
        display_name: "Ctrl+click layer eye",
        description: None,
        kind: PrefKind::Str,
        default: PrefDefault::Str(""),
        widget: WidgetHint::MouseBinding,
        per_preset: &[],
    },
    Pref {
        key: "bindings.layerThumb.alt+click",
        display_name: "Alt+click layer thumbnail",
        description: None,
        kind: PrefKind::Str,
        default: PrefDefault::Str(""),
        widget: WidgetHint::MouseBinding,
        per_preset: &[],
    },
    Pref {
        key: "bindings.maskThumb.ctrl+click",
        display_name: "Ctrl+click mask thumbnail",
        description: None,
        kind: PrefKind::Str,
        default: PrefDefault::Str(""),
        widget: WidgetHint::MouseBinding,
        per_preset: &[("Photoshop", PresetValue::Str("isolateMask"))],
    },
];

pub fn register() -> SchemaSection {
    SchemaSection {
        id: "bindings",
        display_name: "Mouse bindings",
        description: Some("Modifier-clicks on UI elements that dispatch actions."),
        icon: Some("fa-solid fa-computer-mouse"),
        order: 90,
        prefs: PREFS,
    }
}
