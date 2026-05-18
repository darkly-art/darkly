use crate::config::schema::Preset;

pub fn register() -> Preset {
    Preset {
        name: "GIMP",
        description: Some("GIMP-style keybindings"),
        hotkeys: &[
            // Tools
            ("brushTool", "KeyP"),
            ("toggleEraseMode", "Shift+KeyE"),
            ("fillTool", "Shift+KeyB"),
            ("colorPickerTool", "KeyO"),
            ("ellipseSelectTool", "KeyE"),
            ("magicWandTool", "KeyU"),
            // Selection
            ("invertSelection", "$mod+KeyI"),
            // GIMP's layers-delete action has no global accelerator (its
            // accel slot is { NULL } in layers-actions.c); `Delete` deletes
            // the active layer only when the Layers panel has focus, via
            // gimplayertreeview.c's `delete_action`. We model that with a
            // `layerPanel`-scoped binding.
            ("deleteLayer", "layerPanel:Delete"),
            // Layers: GIMP binds Layer > Duplicate Layer(s) to Ctrl+Shift+D
            // (image-actions.c → "layers-duplicate"). Note Ctrl+D in GIMP is
            // "Duplicate Image", a different operation that Darkly does not
            // expose.
            ("duplicateLayer", "$mod+Shift+KeyD"),
            // GIMP ships no default key for Merge Down ("layers-merge-down"
            // in layers-actions.c has a NULL accel slot) — Ctrl+M is bound
            // to "image-merge-visible" (merge visible into one), a related
            // but distinct op. Clear our Krita default rather than alias an
            // unrelated GIMP binding.
            ("mergeDown", ""),
            // GIMP's "image-flatten" likewise ships unbound. Same call.
            ("flattenImage", ""),
        ],
        mouse_clicks: &[],
        settings: &[],
    }
}
