pub mod schema;
pub mod sections;

#[allow(dead_code)]
mod presets_gen {
    include!(concat!(env!("OUT_DIR"), "/presets_gen.rs"));
}

pub use presets_gen::{BASE_SETTINGS_OPTIONS, DEFAULTS_YAML, OVERLAYS};

use std::cell::RefCell;
use std::collections::HashMap;

/// On-disk schema version for `user_settings.json`. Bump whenever a change
/// to the schema or YAML layers cannot be auto-cleaned by
/// [`super::schema`]-driven validation — e.g. a pref key is renamed, a
/// pref's kind changes shape (str→int, scalar→list), or the file's
/// envelope itself changes. Pre-release we just discard mismatched files
/// (per CLAUDE.md "No Migrations"); post-release this is the discriminator
/// migrations key off.
///
/// Forward-compatible changes don't need a bump: new prefs get default
/// values, removed pref keys are dropped by `validateOverrides`, and
/// numeric range changes are clamped.
pub const CONFIG_VERSION: u32 = 1;

/// A configuration value.
#[derive(Clone, Debug, PartialEq)]
pub enum ConfigValue {
    Float(f64),
    Int(i64),
    Bool(bool),
    Str(String),
}

/// Three-layer config store:
///
/// ```text
/// user override → overlay[active editor] → defaults (defaults.yaml)
/// ```
///
/// All three layers are sourced from YAML at startup: `defaults.yaml` is the
/// editor-AGNOSTIC baseline (always applied), each `<editor>.yaml` is one
/// equal-status overlay, and the user layer collects personal overrides.
///
/// The active editor is whatever `app.baseSettings` resolves to in the user
/// layer. The startup PresetPicker writes it before any consumer reads a
/// resolved value, so `get_*` getters can panic on a missing setting just
/// as before.
struct Config {
    defaults: HashMap<String, ConfigValue>,
    overlays: HashMap<String, HashMap<String, ConfigValue>>,
    user: HashMap<String, ConfigValue>,
}

thread_local! {
    static CONFIG: RefCell<Config> = RefCell::new(Config::new());
}

impl Config {
    fn new() -> Self {
        let defaults = parse_yaml_preset(presets_gen::DEFAULTS_YAML)
            .unwrap_or_else(|e| panic!("failed to parse defaults.yaml: {e}"));
        let mut overlays = HashMap::new();
        for (name, yaml) in presets_gen::OVERLAYS {
            let map = parse_yaml_preset(yaml)
                .unwrap_or_else(|e| panic!("failed to parse overlay {name}: {e}"));
            overlays.insert((*name).to_string(), map);
        }
        Config {
            defaults,
            overlays,
            user: HashMap::new(),
        }
    }

    /// Resolve a key down the layer stack.
    fn get(&self, key: &str) -> Option<&ConfigValue> {
        if let Some(v) = self.user.get(key) {
            return Some(v);
        }
        if let Some(ConfigValue::Str(name)) = self.user.get("app.baseSettings") {
            if let Some(v) = self.overlays.get(name).and_then(|m| m.get(key)) {
                return Some(v);
            }
        }
        self.defaults.get(key)
    }

    /// What "Reset override on this key" would reveal — the layer below
    /// the user layer. Drives the Settings UI's "displayed default" and
    /// the Reset-affordance disabled state.
    fn base_value(&self, key: &str) -> Option<&ConfigValue> {
        if let Some(ConfigValue::Str(name)) = self.user.get("app.baseSettings") {
            if let Some(v) = self.overlays.get(name).and_then(|m| m.get(key)) {
                return Some(v);
            }
        }
        self.defaults.get(key)
    }
}

// ---------------------------------------------------------------------------
// YAML parsing — flattens the `{ hotkeys, mouse_clicks, settings }` shape
// into a dot-path key/value map, mirroring the legacy on-disk JSON model.
// ---------------------------------------------------------------------------

fn parse_yaml_preset(yaml: &str) -> Result<HashMap<String, ConfigValue>, String> {
    let value: serde_yml::Value = serde_yml::from_str(yaml).map_err(|e| e.to_string())?;
    let map = match value {
        serde_yml::Value::Mapping(m) => m,
        // Empty doc: zero entries (legitimately allowed for overlays that
        // only define a `name:` field and nothing else).
        serde_yml::Value::Null => return Ok(HashMap::new()),
        other => return Err(format!("expected top-level mapping, got {other:?}")),
    };

    let mut out: HashMap<String, ConfigValue> = HashMap::new();

    for (k, v) in map {
        let Some(key) = k.as_str() else { continue };
        match key {
            "name" | "description" => {
                // Metadata: not a config key.
                continue;
            }
            "hotkeys" => collect_string_facet(&v, "hotkeys.", &mut out)?,
            "mouse_clicks" => collect_string_facet(&v, "mouseclicks.", &mut out)?,
            "settings" => collect_settings_facet(&v, &mut out)?,
            _ => {
                // Tolerate top-level scalar entries by treating the key as a
                // settings key (handy for future hand-written YAML).
                if let Some(cv) = yaml_to_config_value(&v) {
                    out.insert(key.to_string(), cv);
                }
            }
        }
    }

    Ok(out)
}

/// `hotkeys` / `mouse_clicks` facets: keys map to either a single string
/// (one binding) or a list of strings (multiple alternative bindings).
/// Multi-binding entries are joined with a `|` separator — consumers know
/// to split on it. (Legacy: the only known multi-binding action is
/// `isolateLayer` with `[layerThumb:alt+click, maskThumb:alt+click]`.)
fn collect_string_facet(
    v: &serde_yml::Value,
    prefix: &str,
    out: &mut HashMap<String, ConfigValue>,
) -> Result<(), String> {
    let m = match v {
        serde_yml::Value::Mapping(m) => m,
        serde_yml::Value::Null => return Ok(()),
        other => return Err(format!("{prefix} expected a mapping, got {other:?}")),
    };
    for (k, v) in m {
        let Some(key) = k.as_str() else { continue };
        let full_key = format!("{prefix}{key}");
        match v {
            serde_yml::Value::String(s) => {
                out.insert(full_key, ConfigValue::Str(s.clone()));
            }
            serde_yml::Value::Sequence(seq) => {
                let mut parts: Vec<String> = Vec::with_capacity(seq.len());
                for item in seq {
                    if let serde_yml::Value::String(s) = item {
                        parts.push(s.clone());
                    } else {
                        return Err(format!("{full_key}: list item is not a string"));
                    }
                }
                out.insert(full_key, ConfigValue::Str(parts.join("|")));
            }
            serde_yml::Value::Null => {
                // Tolerate `key: ` with no value as "empty string" — a key
                // explicitly unbinds the action.
                out.insert(full_key, ConfigValue::Str(String::new()));
            }
            other => return Err(format!("{full_key}: unexpected value {other:?}")),
        }
    }
    Ok(())
}

/// `settings` facet: keys are already fully-qualified dot-paths; values are
/// bool/int/float/string.
fn collect_settings_facet(
    v: &serde_yml::Value,
    out: &mut HashMap<String, ConfigValue>,
) -> Result<(), String> {
    let m = match v {
        serde_yml::Value::Mapping(m) => m,
        serde_yml::Value::Null => return Ok(()),
        other => return Err(format!("settings expected a mapping, got {other:?}")),
    };
    for (k, v) in m {
        let Some(key) = k.as_str() else { continue };
        if let Some(cv) = yaml_to_config_value(v) {
            out.insert(key.to_string(), cv);
        } else {
            return Err(format!("settings.{key}: unsupported value {v:?}"));
        }
    }
    Ok(())
}

fn yaml_to_config_value(v: &serde_yml::Value) -> Option<ConfigValue> {
    match v {
        serde_yml::Value::Bool(b) => Some(ConfigValue::Bool(*b)),
        serde_yml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(ConfigValue::Int(i))
            } else {
                n.as_f64().map(ConfigValue::Float)
            }
        }
        serde_yml::Value::String(s) => Some(ConfigValue::Str(s.clone())),
        serde_yml::Value::Null => None,
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Public module-level API (delegates to thread-local)
// ---------------------------------------------------------------------------

/// Get a config value by dot-path key. Returns `None` only if the key is
/// absent from every layer.
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

/// Layer-below-user value for a key (overlay → defaults). Drives "Reset"
/// affordances and the Settings UI's displayed-default text.
pub fn base_value(key: &str) -> Option<ConfigValue> {
    CONFIG.with(|c| c.borrow().base_value(key).cloned())
}

/// Set a value in the user layer.
pub fn set(key: &str, value: ConfigValue) {
    CONFIG.with(|c| {
        c.borrow_mut().user.insert(key.to_string(), value);
    });
}

/// Remove a user override for a key. Reveals the overlay/default below.
pub fn reset(key: &str) {
    CONFIG.with(|c| {
        c.borrow_mut().user.remove(key);
    });
}

/// Clear every user override **except** `app.baseSettings` — the picker
/// choice survives a global reset so the user isn't bumped back to the
/// first-run picker by clicking "Reset everything".
pub fn reset_all() {
    CONFIG.with(|c| {
        let mut cfg = c.borrow_mut();
        let base = cfg.user.remove("app.baseSettings");
        cfg.user.clear();
        if let Some(v) = base {
            cfg.user.insert("app.baseSettings".to_string(), v);
        }
    });
}

/// Equal-status overlay display names (alphabetical order).
pub fn base_names() -> Vec<String> {
    presets_gen::OVERLAYS
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect()
}

/// True if the declared `PrefKind` for `key` is `Int` (used by the WASM
/// bridge to disambiguate JS numbers when serializing back to Rust).
pub fn kind_is_int(key: &str) -> bool {
    for section in sections::registrations() {
        for pref in section.prefs {
            if pref.key == key {
                return matches!(pref.kind, schema::PrefKind::Int { .. });
            }
        }
    }
    false
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
        CONFIG.with(|c| {
            c.borrow_mut().user.clear();
        });
    }

    fn pick(name: &str) {
        set("app.baseSettings", ConfigValue::Str(name.to_string()));
    }

    #[test]
    fn defaults_from_yaml() {
        reset_state();
        // Agnostic defaults present without picking an editor.
        assert_eq!(get_i64("animation.veil_divisor"), 2);
        assert_eq!(get_i64("canvas.width"), 1920);
        assert_eq!(get_str("hotkeys.nav.trigger"), "Space");
        assert!(!get_bool("input.fingerPainting"));
        // Universal hotkey defined in defaults.yaml.
        assert_eq!(get_str("hotkeys.undo"), "$mod+KeyZ");
    }

    #[test]
    fn overlay_resolves_above_defaults() {
        reset_state();
        pick("Krita");
        // Krita-specific override.
        assert_eq!(get_str("hotkeys.brushTool"), "KeyB");
        // Defaults still show through where the overlay is silent.
        assert_eq!(get_str("hotkeys.undo"), "$mod+KeyZ");

        // Switching to Photoshop swaps the overlay live.
        pick("Photoshop");
        assert_eq!(get_str("hotkeys.rectSelectTool"), "KeyM");
        assert_eq!(get_str("hotkeys.undo"), "$mod+KeyZ");
    }

    #[test]
    fn user_wins_over_overlay_and_defaults() {
        reset_state();
        pick("Krita");
        set("hotkeys.brushTool", ConfigValue::Str("KeyZ".into()));
        assert_eq!(get_str("hotkeys.brushTool"), "KeyZ");
        reset("hotkeys.brushTool");
        // Falls back to overlay value, not defaults.
        assert_eq!(get_str("hotkeys.brushTool"), "KeyB");
    }

    #[test]
    fn reset_all_preserves_base_choice() {
        reset_state();
        pick("Photoshop");
        set("hotkeys.brushTool", ConfigValue::Str("KeyZ".into()));
        reset_all();
        // Override is gone…
        assert_eq!(get_str("hotkeys.brushTool"), "KeyB");
        // …but the base choice survives.
        assert_eq!(get_str("app.baseSettings"), "Photoshop");
    }

    #[test]
    fn base_value_skips_user_layer() {
        reset_state();
        pick("Krita");
        set("hotkeys.brushTool", ConfigValue::Str("KeyZ".into()));
        // `base_value` is what a Reset would reveal — the overlay value.
        match base_value("hotkeys.brushTool") {
            Some(ConfigValue::Str(s)) => assert_eq!(s, "KeyB"),
            other => panic!("expected overlay value, got {other:?}"),
        }
    }

    #[test]
    fn base_names_lists_overlays_alphabetically() {
        let names = base_names();
        assert!(!names.is_empty(), "expected at least one overlay");
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    #[test]
    fn kind_is_int_uses_schema() {
        // `canvas.width` is an int pref → true.
        assert!(kind_is_int("canvas.width"));
        // `nav.panSensitivity` is a float pref → false.
        assert!(!kind_is_int("nav.panSensitivity"));
        // Unknown key → false (defensive).
        assert!(!kind_is_int("bogus.key"));
    }
}
