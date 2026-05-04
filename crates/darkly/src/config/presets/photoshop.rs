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
        ],
        // No preset-specific mouse overrides for isolation: the action's
        // defaults (`layerThumb:alt+click` and `maskThumb:alt+click`)
        // match the "alt+click on a preview" rule we want everywhere.
        mouse_clicks: &[],
        settings: &[],
    }
}
