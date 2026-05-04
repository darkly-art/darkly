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
            // action is reachable only via alt+click on the layer eye.
            ("isolateLayer", ""),
        ],
        mouse_clicks: &[
            // Override the default `layerItem:alt+click` (Krita) with the
            // Photoshop site convention — alt+click on the eye icon.
            ("isolateLayer", "layerEye:alt+click"),
        ],
        settings: &[],
    }
}
