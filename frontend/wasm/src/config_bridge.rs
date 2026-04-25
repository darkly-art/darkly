use darkly::config::{self, ConfigValue};
use wasm_bindgen::prelude::*;

/// Get a config value by dot-path key. Returns the JS equivalent
/// (number, string, boolean) or `undefined` if the key is unknown.
#[wasm_bindgen]
pub fn config_get(key: &str) -> JsValue {
    config_value_to_js(config::get(key).as_ref())
}

/// Set a setting for a config key. The value type is inferred from the JS
/// value: boolean → Bool, string → Str, number → Int or Float (Int if the
/// default for this key is Int and the number has no fractional part).
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

/// Remove a setting for a key, reverting it to its default.
#[wasm_bindgen]
pub fn config_reset(key: &str) {
    config::reset(key);
}

/// Clear all settings — every key falls back to its default.
#[wasm_bindgen]
pub fn config_reset_all() {
    config::reset_all();
}

/// List built-in template names.
#[wasm_bindgen]
pub fn config_preset_names() -> JsValue {
    let arr = js_sys::Array::new();
    for name in config::preset_names() {
        arr.push(&JsValue::from_str(&name));
    }
    arr.into()
}

/// Materialize a built-in template's full settings snapshot as a flat JS
/// object: `{ "key.path": value, ... }`. Returns `null` for unknown names.
#[wasm_bindgen]
pub fn config_preset_values(name: &str) -> JsValue {
    let Some(values) = config::preset_values(name) else {
        return JsValue::NULL;
    };
    let obj = js_sys::Object::new();
    for (k, v) in values {
        let js_v = config_value_to_js(Some(&v));
        js_sys::Reflect::set(&obj, &JsValue::from_str(&k), &js_v).ok();
    }
    obj.into()
}

/// Get the full preferences schema as JSON: an array of section objects,
/// each containing the section's metadata and the flat list of prefs with
/// their display label, kind, default, and widget hint. Sorted by section
/// order.
#[wasm_bindgen]
pub fn config_schema() -> String {
    serde_json::to_string(&config::schema_info()).unwrap_or_else(|_| "[]".into())
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
