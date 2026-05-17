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
        ],
        mouse_clicks: &[],
        settings: &[],
    }
}
