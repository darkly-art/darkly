/// Schema definition for a single effect parameter (filter or veil).
/// Each module defines a `const` array of these describing its parameters.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ParamDef {
    Float { name: &'static str, min: f32, max: f32, default: f32 },
    Int   { name: &'static str, min: i32, max: i32, default: i32 },
    Bool  { name: &'static str, default: bool },
    String { name: &'static str, default: &'static str },
}

/// A concrete runtime parameter value, read from an effect instance.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    Float(f32),
    Int(i32),
    Bool(bool),
    String(String),
}

/// Convert a JSON object of `{ "name": value, ... }` into `Vec<ParamValue>`
/// using `ParamDef` metadata.
///
/// This is the platform-agnostic version of parameter conversion. Any
/// non-WASM bridge (Tauri IPC, CEF IPC, napi-rs, tests) can use this
/// directly instead of reimplementing the same logic with its own types.
pub fn param_values_from_json(obj: &serde_json::Value, defs: &[ParamDef]) -> Vec<ParamValue> {
    let map = match obj.as_object() {
        Some(m) => m,
        None => return defs.iter().map(|d| d.default_value()).collect(),
    };
    defs.iter().map(|def| match def {
        ParamDef::Float { name, default, .. } => {
            let v = map.get(*name)
                .and_then(|v| v.as_f64())
                .unwrap_or(*default as f64) as f32;
            ParamValue::Float(v)
        }
        ParamDef::Int { name, default, .. } => {
            let v = map.get(*name)
                .and_then(|v| v.as_f64())
                .unwrap_or(*default as f64) as i32;
            ParamValue::Int(v)
        }
        ParamDef::Bool { name, default } => {
            let v = map.get(*name)
                .and_then(|v| v.as_bool())
                .unwrap_or(*default);
            ParamValue::Bool(v)
        }
        ParamDef::String { name, default } => {
            let v = map.get(*name)
                .and_then(|v| v.as_str())
                .unwrap_or(default)
                .to_string();
            ParamValue::String(v)
        }
    }).collect()
}

impl ParamDef {
    pub fn default_value(&self) -> ParamValue {
        match self {
            ParamDef::Float { default, .. } => ParamValue::Float(*default),
            ParamDef::Int { default, .. } => ParamValue::Int(*default),
            ParamDef::Bool { default, .. } => ParamValue::Bool(*default),
            ParamDef::String { default, .. } => ParamValue::String(default.to_string()),
        }
    }
}
