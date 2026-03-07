use darkly::config::{self, ConfigValue};
use wasm_bindgen::prelude::*;

/// Get a config value by dot-path key. Returns the JS equivalent
/// (number, string, boolean) or `undefined` if the key is unknown.
#[wasm_bindgen]
pub fn config_get(key: &str) -> JsValue {
    match config::get(key) {
        Some(ConfigValue::Float(f)) => JsValue::from(f),
        Some(ConfigValue::Int(i)) => JsValue::from(i as f64),
        Some(ConfigValue::Str(s)) => JsValue::from_str(&s),
        Some(ConfigValue::Bool(b)) => JsValue::from(b),
        None => JsValue::UNDEFINED,
    }
}

/// Set a user override for a config key. The value type is inferred
/// from the JS value: boolean → Bool, string → Str, number → Int or Float
/// (Int if the default for this key is Int and the number has no fractional part).
#[wasm_bindgen]
pub fn config_set(key: &str, value: JsValue) {
    let cv = if let Some(b) = value.as_bool() {
        ConfigValue::Bool(b)
    } else if let Some(s) = value.as_string() {
        ConfigValue::Str(s)
    } else if let Some(f) = value.as_f64() {
        if config::default_is_int(key) && f.fract() == 0.0 {
            ConfigValue::Int(f as i64)
        } else {
            ConfigValue::Float(f)
        }
    } else {
        return;
    };
    config::set(key, cv);
}

/// Remove a user override for a key, reverting it to the preset or default value.
#[wasm_bindgen]
pub fn config_reset(key: &str) {
    config::reset(key);
}

/// Clear all user overrides.
#[wasm_bindgen]
pub fn config_reset_all() {
    config::reset_all();
}

/// Apply a named preset (e.g., "Krita", "Photoshop", "GIMP").
/// Returns false if the preset name is unknown.
#[wasm_bindgen]
pub fn config_apply_preset(name: &str) -> bool {
    config::apply_preset(name)
}

/// Get a JS array of available preset names.
#[wasm_bindgen]
pub fn config_preset_names() -> JsValue {
    let arr = js_sys::Array::new();
    for name in config::preset_names() {
        arr.push(&JsValue::from_str(name));
    }
    arr.into()
}

/// Get all default values as a flat JS object: `{ "key.path": value, ... }`.
#[wasm_bindgen]
pub fn config_defaults() -> JsValue {
    let obj = js_sys::Object::new();
    for (key, value) in config::defaults() {
        let js_val = match value {
            ConfigValue::Float(f) => JsValue::from(f),
            ConfigValue::Int(i) => JsValue::from(i as f64),
            ConfigValue::Str(s) => JsValue::from_str(&s),
            ConfigValue::Bool(b) => JsValue::from(b),
        };
        js_sys::Reflect::set(&obj, &JsValue::from_str(&key), &js_val).ok();
    }
    obj.into()
}
