use crate::config::schema::{Pref, PrefDefault, PrefKind, PresetValue, SchemaSection, WidgetHint};

// Conventions:
// - Defaults follow Krita.
// - Empty per_preset slice means every preset inherits the default.
// - "Krita" entries in per_preset are therefore usually redundant, but we
//   include them when they document a conscious divergence from another
//   preset so the row stays readable alongside GIMP / Photoshop overrides.

macro_rules! hk {
    // String with per-preset overrides.
    ($key:expr, $label:expr, $default:expr, widget = $widget:expr, presets = $presets:expr $(,)?) => {
        Pref {
            key: $key,
            display_name: $label,
            description: None,
            kind: PrefKind::Str,
            default: PrefDefault::Str($default),
            widget: $widget,
            per_preset: $presets,
        }
    };
    // Simple hotkey with no preset overrides (universal across presets).
    ($key:expr, $label:expr, $default:expr $(,)?) => {
        Pref {
            key: $key,
            display_name: $label,
            description: None,
            kind: PrefKind::Str,
            default: PrefDefault::Str($default),
            widget: WidgetHint::Hotkey,
            per_preset: &[],
        }
    };
}

const PREFS: &[Pref] = &[
    // Colors
    hk!("hotkeys.resetColors", "Reset colors", "KeyD"),
    hk!("hotkeys.swapColors", "Swap colors", "KeyX"),
    // Edit
    hk!("hotkeys.undo", "Undo", "$mod+KeyZ"),
    hk!("hotkeys.redo", "Redo", "$mod+Shift+KeyZ"),
    // Clipboard (universal)
    hk!("hotkeys.copy", "Copy", "$mod+KeyC"),
    hk!("hotkeys.cut", "Cut", "$mod+KeyX"),
    hk!("hotkeys.paste", "Paste", "$mod+KeyV"),
    hk!("hotkeys.pasteInPlace", "Paste in place", "$mod+Shift+KeyV"),
    // Tools
    hk!(
        "hotkeys.brushTool",
        "Brush",
        "KeyB",
        widget = WidgetHint::Hotkey,
        presets = &[("GIMP", PresetValue::Str("KeyP"))],
    ),
    hk!(
        "hotkeys.eraserTool",
        "Eraser",
        "KeyE",
        widget = WidgetHint::Hotkey,
        presets = &[("GIMP", PresetValue::Str("Shift+KeyE"))],
    ),
    hk!(
        "hotkeys.fillTool",
        "Fill bucket",
        "KeyF",
        widget = WidgetHint::Hotkey,
        presets = &[
            ("Photoshop", PresetValue::Str("KeyG")),
            ("GIMP", PresetValue::Str("Shift+KeyB")),
        ],
    ),
    hk!(
        "hotkeys.gradientTool",
        "Gradient",
        "KeyG",
        widget = WidgetHint::Hotkey,
        presets = &[("Photoshop", PresetValue::Str("Shift+KeyG"))],
    ),
    hk!(
        "hotkeys.colorPickerTool",
        "Color picker",
        "KeyP",
        widget = WidgetHint::Hotkey,
        presets = &[
            ("Photoshop", PresetValue::Str("KeyI")),
            ("GIMP", PresetValue::Str("KeyO")),
        ],
    ),
    hk!(
        "hotkeys.rectSelectTool",
        "Rectangular select",
        "KeyR",
        widget = WidgetHint::Hotkey,
        presets = &[("Photoshop", PresetValue::Str("KeyM"))],
    ),
    // Default matches Krita ("Shift+KeyR"). The baseline preset ("Krita")
    // clears the preset layer, so defaults must equal Krita's values.
    hk!(
        "hotkeys.ellipseSelectTool",
        "Elliptical select",
        "Shift+KeyR",
        widget = WidgetHint::Hotkey,
        presets = &[
            ("Photoshop", PresetValue::Str("Shift+KeyM")),
            ("GIMP", PresetValue::Str("KeyE")),
        ],
    ),
    hk!("hotkeys.lassoSelectTool", "Lasso select", "KeyL"),
    hk!(
        "hotkeys.magicWandTool",
        "Magic wand",
        "KeyW",
        widget = WidgetHint::Hotkey,
        presets = &[("GIMP", PresetValue::Str("KeyU"))],
    ),
    hk!("hotkeys.transformTool", "Transform", "KeyT"),
    // Selection
    hk!("hotkeys.selectAll", "Select all", "$mod+KeyA"),
    hk!(
        "hotkeys.clearSelection",
        "Clear selection",
        "$mod+Shift+KeyA",
        widget = WidgetHint::Hotkey,
        presets = &[("Photoshop", PresetValue::Str("$mod+KeyD"))],
    ),
    hk!(
        "hotkeys.invertSelection",
        "Invert selection",
        "$mod+Shift+KeyI",
        widget = WidgetHint::Hotkey,
        presets = &[("GIMP", PresetValue::Str("$mod+KeyI"))],
    ),
    hk!(
        "hotkeys.clearSelectionContents",
        "Delete selection contents",
        "Delete",
    ),
    // Floating content / transform
    hk!("hotkeys.commitFloating", "Commit transform", "Enter"),
    hk!("hotkeys.cancelFloating", "Cancel transform", "Escape"),
    // Brush controls
    hk!("hotkeys.brushSizeUp", "Increase brush size", "BracketRight"),
    hk!(
        "hotkeys.brushSizeDown",
        "Decrease brush size",
        "BracketLeft"
    ),
    // Layers
    hk!(
        "hotkeys.isolateLayer",
        "Isolate layer",
        "KeyI",
        widget = WidgetHint::Hotkey,
        presets = &[("Photoshop", PresetValue::Str(""))],
    ),
    // Preferences
    hk!("hotkeys.openSettings", "Open settings", "$mod+Comma"),
];

pub fn register() -> SchemaSection {
    SchemaSection {
        id: "hotkeys",
        display_name: "Keyboard shortcuts",
        description: Some("Key bindings for every action. Click a row to rebind."),
        icon: Some("fa-solid fa-keyboard"),
        order: 100,
        prefs: PREFS,
    }
}
