use crate::config::schema::Preset;

/// Krita is the implicit default — when no preset is loaded, what the user
/// sees IS Krita-style (because the action registry's `defaultHotkey` values
/// were chosen to match Krita's bindings).
///
/// We still register Krita as a named preset so users can pick it from the
/// preset menu and so "Apply Krita" semantically means "reset every action's
/// trigger to its default." When applied, this preset's empty facets cause
/// `apply_preset` to clear user_settings entirely.
pub fn register() -> Preset {
    Preset {
        name: "Krita",
        description: Some("Default Krita-style keybindings"),
        hotkeys: &[],
        mouse_clicks: &[],
        settings: &[],
    }
}
