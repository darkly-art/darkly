pub mod presets;
pub mod schema;
pub mod sections;

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

/// Two-layer config store: `settings` (the user's data) on top of `defaults`
/// (immutable, baked into the binary). There is no "preset" layer at runtime
/// — built-in templates are queried via [`preset_values`] and applied by
/// callers as bulk writes into `settings`.
struct Config {
    defaults: HashMap<String, ConfigValue>,
    settings: HashMap<String, ConfigValue>,
}

thread_local! {
    static CONFIG: RefCell<Config> = RefCell::new(Config::new());
}

// ---------------------------------------------------------------------------
// Schema-driven defaults
// ---------------------------------------------------------------------------

impl Config {
    fn new() -> Self {
        let mut defaults = HashMap::new();
        for section in sections::registrations() {
            for pref in section.prefs {
                defaults.insert(pref.key.to_string(), pref.default.to_config_value());
            }
        }
        Config {
            defaults,
            settings: HashMap::new(),
        }
    }

    fn get(&self, key: &str) -> Option<&ConfigValue> {
        self.settings.get(key).or_else(|| self.defaults.get(key))
    }

    fn default_for(&self, key: &str) -> Option<&ConfigValue> {
        self.defaults.get(key)
    }
}

/// Flatten a built-in preset's structured facets into a key/value map ready
/// to bulk-write into user_settings. Returns `None` if the name isn't a
/// registered built-in preset.
///
/// Schema-defined defaults are NOT included here — applying a preset writes
/// only the keys the preset *changes*, leaving the underlying defaults to
/// show through for everything the preset is silent about.
pub fn preset_values(name: &str) -> Option<HashMap<String, ConfigValue>> {
    let preset = presets::registrations()
        .into_iter()
        .find(|p| p.name == name)?;
    let mut out = HashMap::new();
    for (action_id, key) in preset.hotkeys {
        out.insert(
            format!("hotkeys.{action_id}"),
            ConfigValue::Str((*key).to_string()),
        );
    }
    for (action_id, chord) in preset.mouse_clicks {
        out.insert(
            format!("mouseclicks.{action_id}"),
            ConfigValue::Str((*chord).to_string()),
        );
    }
    for (key, value) in preset.settings {
        out.insert((*key).to_string(), value.to_config_value());
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Public module-level API (delegates to thread-local)
// ---------------------------------------------------------------------------

/// Get a config value by dot-path key. Returns `None` only if the key has
/// no default and no setting.
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

/// Set a setting value.
pub fn set(key: &str, value: ConfigValue) {
    CONFIG.with(|c| {
        c.borrow_mut().settings.insert(key.to_string(), value);
    });
}

/// Remove a setting, reverting the key to its default.
pub fn reset(key: &str) {
    CONFIG.with(|c| {
        c.borrow_mut().settings.remove(key);
    });
}

/// Clear all settings — every key falls back to its default.
pub fn reset_all() {
    CONFIG.with(|c| {
        c.borrow_mut().settings.clear();
    });
}

/// List all built-in preset names.
pub fn preset_names() -> Vec<String> {
    presets::registrations()
        .into_iter()
        .map(|p| p.name.to_string())
        .collect()
}

/// Check whether the default type for a key is `Int` (used by the WASM
/// bridge to disambiguate JS numbers).
pub fn default_is_int(key: &str) -> bool {
    CONFIG.with(|c| matches!(c.borrow().default_for(key), Some(ConfigValue::Int(_))))
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

/// Return the full schema as a serializable view. Used by the WASM bridge to
/// feed the Settings UI.
pub fn schema_info() -> Vec<schema::SectionInfo> {
    let mut out: Vec<_> = sections::registrations()
        .iter()
        .map(schema::SectionInfo::from_section)
        .collect();
    out.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.id.cmp(b.id)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset_state() {
        // Each test starts fresh: clear any settings left by a previous one.
        reset_all();
    }

    #[test]
    fn defaults_are_set() {
        reset_state();
        assert_eq!(get_i64("animation.veil_divisor"), 2);
        assert_eq!(get_i64("animation.overlay_divisor"), 4);
        assert_eq!(get_str("hotkeys.nav.trigger"), "Space");
        assert_eq!(get_i64("canvas.width"), 1920);
        assert!(!get_bool("input.fingerPainting"));
    }

    #[test]
    fn setting_wins_over_default() {
        reset_state();
        set("animation.veil_divisor", ConfigValue::Int(1));
        assert_eq!(get_i64("animation.veil_divisor"), 1);
        reset("animation.veil_divisor");
        assert_eq!(get_i64("animation.veil_divisor"), 2);
    }

    #[test]
    fn preset_values_flatten_facets() {
        reset_state();

        // Krita is the implicit-default preset: no overrides, empty map.
        let krita = preset_values("Krita").expect("Krita preset");
        assert!(krita.is_empty(), "Krita preset should declare no overrides");

        // Photoshop overrides colorPickerTool's hotkey and isolateLayer's
        // mouse click. The flattened map uses the conventional namespaces.
        let photoshop = preset_values("Photoshop").expect("Photoshop preset");
        assert_eq!(
            photoshop.get("hotkeys.colorPickerTool"),
            Some(&ConfigValue::Str("KeyI".into()))
        );
        assert_eq!(
            photoshop.get("mouseclicks.isolateLayer"),
            Some(&ConfigValue::Str("layerEye:alt+click".into()))
        );

        // Photoshop is silent about canvas.width — it's not in the flattened
        // map (resolution falls back to the schema default).
        assert!(!photoshop.contains_key("canvas.width"));

        // Unknown preset name → None.
        assert!(preset_values("Bogus").is_none());
    }

    #[test]
    fn reset_all_clears_settings() {
        reset_state();
        set("animation.veil_divisor", ConfigValue::Int(1));
        set("canvas.width", ConfigValue::Int(3840));
        reset_all();
        assert_eq!(get_i64("animation.veil_divisor"), 2);
        assert_eq!(get_i64("canvas.width"), 1920);
    }

    #[test]
    fn get_f64_coerces_int() {
        reset_state();
        assert_eq!(get_f64("animation.veil_divisor"), 2.0);
    }
}
