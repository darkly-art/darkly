use std::cell::RefCell;
use std::collections::HashMap;

/// A configuration value.
#[derive(Clone, Debug, PartialEq)]
pub enum ConfigValue {
    Float(f64),
    Int(i64),
    Bool(bool),
    Str(String),
}

/// Three-layer config store: user_overrides > preset > defaults.
struct Config {
    defaults: HashMap<String, ConfigValue>,
    preset: HashMap<String, ConfigValue>,
    user_overrides: HashMap<String, ConfigValue>,
}

thread_local! {
    static CONFIG: RefCell<Config> = RefCell::new(Config::new());
}

// ---------------------------------------------------------------------------
// Preset definitions
// ---------------------------------------------------------------------------

fn preset_krita() -> HashMap<String, ConfigValue> {
    [
        ("hotkeys.brushTool", "KeyB"),
        ("hotkeys.eraserTool", "KeyE"),
        ("hotkeys.fillTool", "KeyF"),
        ("hotkeys.gradientTool", "KeyG"),
        ("hotkeys.colorPickerTool", "KeyP"),
        ("hotkeys.rectSelectTool", "KeyR"),
        ("hotkeys.ellipseSelectTool", "Shift+KeyR"),
        ("hotkeys.lassoSelectTool", "KeyL"),
        ("hotkeys.magicWandTool", "KeyW"),
        ("hotkeys.clearSelection", "$mod+Shift+KeyA"),
        ("hotkeys.invertSelection", "$mod+Shift+KeyI"),
        ("hotkeys.clearSelectionContents", "Delete"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), ConfigValue::Str(v.to_string())))
    .collect()
}

fn preset_photoshop() -> HashMap<String, ConfigValue> {
    [
        ("hotkeys.brushTool", "KeyB"),
        ("hotkeys.eraserTool", "KeyE"),
        ("hotkeys.fillTool", "KeyG"),
        ("hotkeys.gradientTool", "Shift+KeyG"),
        ("hotkeys.colorPickerTool", "KeyI"),
        ("hotkeys.rectSelectTool", "KeyM"),
        ("hotkeys.ellipseSelectTool", "Shift+KeyM"),
        ("hotkeys.lassoSelectTool", "KeyL"),
        ("hotkeys.magicWandTool", "KeyW"),
        ("hotkeys.clearSelection", "$mod+KeyD"),
        ("hotkeys.invertSelection", "$mod+Shift+KeyI"),
        ("hotkeys.clearSelectionContents", "Delete"),
        ("hotkeys.isolateLayer", ""),
        ("bindings.layerEye.alt+click", "isolateLayer"),
        ("bindings.maskThumb.alt+click", "isolateMask"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), ConfigValue::Str(v.to_string())))
    .collect()
}

fn preset_gimp() -> HashMap<String, ConfigValue> {
    [
        ("hotkeys.brushTool", "KeyP"),
        ("hotkeys.eraserTool", "Shift+KeyE"),
        ("hotkeys.fillTool", "Shift+KeyB"),
        ("hotkeys.gradientTool", "KeyG"),
        ("hotkeys.colorPickerTool", "KeyO"),
        ("hotkeys.rectSelectTool", "KeyR"),
        ("hotkeys.ellipseSelectTool", "KeyE"),
        ("hotkeys.lassoSelectTool", "KeyF"),
        ("hotkeys.magicWandTool", "KeyU"),
        ("hotkeys.clearSelection", "$mod+Shift+KeyA"),
        ("hotkeys.invertSelection", "$mod+KeyI"),
        ("hotkeys.clearSelectionContents", "Delete"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), ConfigValue::Str(v.to_string())))
    .collect()
}

fn get_preset(name: &str) -> Option<HashMap<String, ConfigValue>> {
    match name {
        "Krita" => Some(preset_krita()),
        "Photoshop" => Some(preset_photoshop()),
        "GIMP" => Some(preset_gimp()),
        _ => None,
    }
}

/// Names of all available presets.
pub static PRESET_NAMES: &[&str] = &["Krita", "Photoshop", "GIMP"];

// ---------------------------------------------------------------------------
// Config implementation
// ---------------------------------------------------------------------------

impl Config {
    fn new() -> Self {
        let mut defaults = HashMap::new();

        macro_rules! d {
            ($key:expr, float $v:expr) => {
                defaults.insert($key.into(), ConfigValue::Float($v));
            };
            ($key:expr, int $v:expr) => {
                defaults.insert($key.into(), ConfigValue::Int($v));
            };
            ($key:expr, bool $v:expr) => {
                defaults.insert($key.into(), ConfigValue::Bool($v));
            };
            ($key:expr, str $v:expr) => {
                defaults.insert($key.into(), ConfigValue::Str($v.into()));
            };
        }

        // Canvas (project-level)
        d!("canvas.width",           int 1920);
        d!("canvas.height",          int 1080);
        d!("canvas.backgroundColor", str "#1a1a1a");

        // Colors
        d!("colors.defaultForeground", str "#000000");
        d!("colors.defaultBackground", str "#ffffff");

        // UI
        d!("ui.leftSidebarWidth",  int 48);
        d!("ui.rightSidebarWidth", int 260);

        // Animation — frame scheduler divisors (fraction of master rAF rate).
        // Divisor 1 = every frame (100%), 2 = every other frame (50%), 4 = every 4th (25%).
        d!("animation.veil_divisor", int 2);
        d!("animation.overlay_divisor", int 4);

        // Hotkeys — navigation
        d!("hotkeys.nav.trigger", str "Space");
        d!("hotkeys.nav.rotate",  str "Shift");
        d!("hotkeys.nav.zoom",    str "Ctrl");

        // Hotkeys — colors
        d!("hotkeys.resetColors", str "KeyD");
        d!("hotkeys.swapColors",  str "KeyX");

        // Hotkeys — edit
        d!("hotkeys.undo", str "$mod+KeyZ");
        d!("hotkeys.redo", str "$mod+Shift+KeyZ");

        // Hotkeys — tools (Krita defaults)
        d!("hotkeys.brushTool",       str "KeyB");
        d!("hotkeys.eraserTool",      str "KeyE");
        d!("hotkeys.fillTool",        str "KeyF");
        d!("hotkeys.gradientTool",    str "KeyG");
        d!("hotkeys.colorPickerTool", str "KeyP");
        d!("hotkeys.rectSelectTool",    str "KeyR");
        d!("hotkeys.ellipseSelectTool", str "KeyJ");
        d!("hotkeys.lassoSelectTool",   str "KeyL");
        d!("hotkeys.magicWandTool",     str "KeyW");
        d!("hotkeys.transformTool",     str "KeyT");

        // Hotkeys — clipboard (universal — same across all presets)
        d!("hotkeys.copy",         str "$mod+KeyC");
        d!("hotkeys.cut",          str "$mod+KeyX");
        d!("hotkeys.paste",        str "$mod+KeyV");
        d!("hotkeys.pasteInPlace", str "$mod+Shift+KeyV");

        // Hotkeys — selection (default follows Krita)
        d!("hotkeys.selectAll",               str "$mod+KeyA");
        d!("hotkeys.clearSelection",          str "$mod+Shift+KeyA");
        d!("hotkeys.invertSelection",         str "$mod+Shift+KeyI");
        d!("hotkeys.clearSelectionContents",  str "Delete");

        // Hotkeys — floating content / transform
        d!("hotkeys.commitFloating", str "Enter");
        d!("hotkeys.cancelFloating", str "Escape");

        // Hotkeys — brush controls
        d!("hotkeys.brushSizeUp",   str "BracketRight");
        d!("hotkeys.brushSizeDown", str "BracketLeft");

        // Hotkeys — layers
        d!("hotkeys.isolateLayer",  str "KeyI");

        // UI bindings — modifier+click on UI elements → action name (empty = no action)
        d!("bindings.layerEye.alt+click",       str "");
        d!("bindings.layerEye.ctrl+click",      str "");
        d!("bindings.layerThumb.alt+click",     str "");
        d!("bindings.maskThumb.ctrl+click",     str "");

        Config {
            defaults,
            preset: HashMap::new(),
            user_overrides: HashMap::new(),
        }
    }

    fn get(&self, key: &str) -> Option<&ConfigValue> {
        self.user_overrides
            .get(key)
            .or_else(|| self.preset.get(key))
            .or_else(|| self.defaults.get(key))
    }

    fn default_for(&self, key: &str) -> Option<&ConfigValue> {
        self.defaults.get(key)
    }
}

// ---------------------------------------------------------------------------
// Public module-level API (delegates to thread-local)
// ---------------------------------------------------------------------------

/// Get a config value by dot-path key. Returns `None` only if the key
/// has no default, no preset override, and no user override.
pub fn get(key: &str) -> Option<ConfigValue> {
    CONFIG.with(|c| c.borrow().get(key).cloned())
}

/// Get a float value. Coerces Int → f64. Panics if the key is missing.
pub fn get_f64(key: &str) -> f64 {
    match get(key) {
        Some(ConfigValue::Float(f)) => f,
        Some(ConfigValue::Int(i)) => i as f64,
        other => panic!("config key {key:?}: expected numeric, got {other:?}"),
    }
}

/// Get an integer value. Panics if the key is missing or wrong type.
pub fn get_i64(key: &str) -> i64 {
    match get(key) {
        Some(ConfigValue::Int(i)) => i,
        other => panic!("config key {key:?}: expected int, got {other:?}"),
    }
}

/// Get a string value. Panics if the key is missing or wrong type.
pub fn get_str(key: &str) -> String {
    match get(key) {
        Some(ConfigValue::Str(s)) => s,
        other => panic!("config key {key:?}: expected string, got {other:?}"),
    }
}

/// Get a boolean value. Panics if the key is missing or wrong type.
pub fn get_bool(key: &str) -> bool {
    match get(key) {
        Some(ConfigValue::Bool(b)) => b,
        other => panic!("config key {key:?}: expected bool, got {other:?}"),
    }
}

/// Set a user override for a config key.
pub fn set(key: &str, value: ConfigValue) {
    CONFIG.with(|c| {
        c.borrow_mut()
            .user_overrides
            .insert(key.to_string(), value);
    });
}

/// Remove a user override, reverting the key to preset or default value.
pub fn reset(key: &str) {
    CONFIG.with(|c| {
        c.borrow_mut().user_overrides.remove(key);
    });
}

/// Clear all user overrides.
pub fn reset_all() {
    CONFIG.with(|c| {
        c.borrow_mut().user_overrides.clear();
    });
}

/// Apply a named preset. Replaces the preset layer entirely.
/// Returns `false` if the preset name is unknown.
pub fn apply_preset(name: &str) -> bool {
    match get_preset(name) {
        Some(overrides) => {
            CONFIG.with(|c| {
                c.borrow_mut().preset = overrides;
            });
            true
        }
        None => false,
    }
}

/// List all available preset names.
pub fn preset_names() -> &'static [&'static str] {
    PRESET_NAMES
}

/// Check whether the default type for a key is `Int` (used by the WASM bridge
/// to disambiguate JS numbers).
pub fn default_is_int(key: &str) -> bool {
    CONFIG.with(|c| {
        matches!(c.borrow().default_for(key), Some(ConfigValue::Int(_)))
    })
}

/// Iterate over all default key/value pairs.
pub fn defaults() -> Vec<(String, ConfigValue)> {
    CONFIG.with(|c| {
        c.borrow()
            .defaults
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_set() {
        assert_eq!(get_i64("animation.veil_divisor"), 2);
        assert_eq!(get_i64("animation.overlay_divisor"), 4);
        assert_eq!(get_str("hotkeys.nav.trigger"), "Space");
        assert_eq!(get_i64("canvas.width"), 1920);
    }

    #[test]
    fn user_override_wins() {
        set("animation.veil_divisor", ConfigValue::Int(1));
        assert_eq!(get_i64("animation.veil_divisor"), 1);
        reset("animation.veil_divisor");
        assert_eq!(get_i64("animation.veil_divisor"), 2);
    }

    #[test]
    fn preset_layer() {
        assert_eq!(get_str("hotkeys.colorPickerTool"), "KeyP");
        apply_preset("Photoshop");
        assert_eq!(get_str("hotkeys.colorPickerTool"), "KeyI");

        // User override wins over preset
        set("hotkeys.colorPickerTool", ConfigValue::Str("KeyZ".into()));
        assert_eq!(get_str("hotkeys.colorPickerTool"), "KeyZ");

        // Reset user override, preset still active
        reset("hotkeys.colorPickerTool");
        assert_eq!(get_str("hotkeys.colorPickerTool"), "KeyI");

        // Reset preset
        apply_preset("Krita");
        assert_eq!(get_str("hotkeys.colorPickerTool"), "KeyP");
    }

    #[test]
    fn reset_all_clears_overrides() {
        set("animation.veil_divisor", ConfigValue::Int(1));
        set("canvas.width", ConfigValue::Int(3840));
        reset_all();
        assert_eq!(get_i64("animation.veil_divisor"), 2);
        assert_eq!(get_i64("canvas.width"), 1920);
    }

    #[test]
    fn get_f64_coerces_int() {
        assert_eq!(get_f64("animation.veil_divisor"), 2.0);
    }
}
