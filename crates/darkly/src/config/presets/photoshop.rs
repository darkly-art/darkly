use crate::config::schema::Preset;

pub fn register() -> Preset {
    Preset {
        name: "Photoshop",
        description: Some("Adobe Photoshop-style keybindings"),
        hotkeys: &[
            // Tools
            ("fillTool", "KeyG"),
            ("gradientTool", "Shift+KeyG"),
            ("colorPickerTool", "KeyI"),
            ("rectSelectTool", "KeyM"),
            ("ellipseSelectTool", "Shift+KeyM"),
            // Selection
            ("clearSelection", "$mod+KeyD"),
            // Photoshop has no keyboard shortcut for isolate-layer; the
            // action is reachable only via alt+click on a thumbnail.
            ("isolateLayer", ""),
            // Photoshop: Delete deletes the active layer when the Layers
            // panel has focus. Scoped to `layerPanel` so it coexists with
            // the global `Delete` → clearSelectionContents binding.
            ("deleteLayer", "layerPanel:Delete"),
        ],
        mouse_clicks: &[
            // Color picker modifier: Photoshop uses Alt+drag (not Ctrl+drag,
            // which is the Krita default we inherit elsewhere).
            ("sampleColor", "canvas@paint:alt+drag"),
        ],
        settings: &[],
    }
}
