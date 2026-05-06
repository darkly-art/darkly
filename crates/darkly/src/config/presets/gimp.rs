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
        ],
        mouse_clicks: &[],
        settings: &[],
    }
}
