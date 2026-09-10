use darkly::config::{self, ConfigValue};
use wasm_bindgen::prelude::*;

/// On-disk schema version for `user_settings.json`. The frontend stamps
/// every write with this and rejects loads whose version doesn't match.
#[wasm_bindgen]
pub fn config_version() -> u32 {
    config::CONFIG_VERSION
}

/// Get a config value by dot-path key. Returns the JS equivalent
/// (number, string, boolean) or `undefined` if the key is unknown.
#[wasm_bindgen]
pub fn config_get(key: &str) -> JsValue {
    config_value_to_js(config::get(key).as_ref())
}

/// Layer-below-user value for a key: `overlay[active][key] ?? defaults[key]`.
/// Drives the Settings UI's "displayed default" text and the disabled-state
/// of the Reset button.
#[wasm_bindgen]
pub fn config_base_value(key: &str) -> JsValue {
    config_value_to_js(config::base_value(key).as_ref())
}

/// Set a setting for a config key. The value type is inferred from the JS
/// value: boolean → Bool, string → Str, number → Int or Float (Int if the
/// schema kind for this key is Int and the number has no fractional part).
#[wasm_bindgen]
pub fn config_set(key: &str, value: JsValue) {
    let cv = if let Some(b) = value.as_bool() {
        ConfigValue::Bool(b)
    } else if let Some(s) = value.as_string() {
        ConfigValue::Str(s)
    } else if let Some(f) = value.as_f64() {
        if config::kind_is_int(key) && f.fract() == 0.0 {
            ConfigValue::Int(f as i64)
        } else {
            ConfigValue::Float(f)
        }
    } else {
        return;
    };
    config::set(key, cv);
}

/// Remove a setting for a key, revealing the overlay or default below.
#[wasm_bindgen]
pub fn config_reset(key: &str) {
    config::reset(key);
}

/// Clear every user override **except** `app.baseSettings`: the picker
/// choice survives a global reset.
#[wasm_bindgen]
pub fn config_reset_all() {
    config::reset_all();
}

/// List equal-status overlay names (e.g. `["GIMP", "Krita", "Photoshop"]`).
/// Order is alphabetical so no editor is privileged.
#[wasm_bindgen]
pub fn config_base_names() -> JsValue {
    let arr = js_sys::Array::new();
    for name in config::base_names() {
        arr.push(&JsValue::from_str(&name));
    }
    arr.into()
}

/// True if the schema's declared kind for `key` is `Int`. Used by the
/// frontend when round-tripping JS numbers through `config_set`.
#[wasm_bindgen]
pub fn config_kind_is_int(key: &str) -> bool {
    config::kind_is_int(key)
}

/// Get the full preferences schema as JSON: one `Catalog` per settings
/// section, each holding a single entry whose `params` are that section's
/// prefs as `ParamInfo`, the same shape the effect panels consume. Sorted by
/// section order.
///
/// Carries every declared pref, including those marked `Hidden`, which arrive
/// with `widget: "hidden"` for the UI to skip. This is the schema stored prefs
/// are validated against, so omitting one would erase it from disk on reload.
#[wasm_bindgen]
pub fn config_schema() -> String {
    serde_json::to_string(&darkly::catalog::settings_catalogs()).unwrap_or_else(|_| "[]".into())
}

/// Render a binding's chord for one platform's modifier vocabulary.
///
/// Takes the binding as written in the presets; the optional
/// `site@scope@brush:` prefix is stripped here, since only the chord is
/// artist-facing. `is_mac` selects the Apple glyphs (`⌘⇧⌥⌃`) over the
/// Windows/Linux names; only the browser knows which to use.
#[wasm_bindgen]
pub fn format_chord(binding: &str, is_mac: bool) -> String {
    use darkly::config::chord::{format_chord, parse_binding, Platform};
    let parsed = parse_binding(binding);
    let platform = if is_mac {
        Platform::Mac
    } else {
        Platform::Other
    };
    format_chord(&parsed.chord, platform)
}

fn config_value_to_js(value: Option<&ConfigValue>) -> JsValue {
    match value {
        Some(ConfigValue::Float(f)) => JsValue::from(*f),
        Some(ConfigValue::Int(i)) => JsValue::from(*i as f64),
        Some(ConfigValue::Str(s)) => JsValue::from_str(s),
        Some(ConfigValue::Bool(b)) => JsValue::from(*b),
        None => JsValue::UNDEFINED,
    }
}
