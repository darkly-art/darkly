/// Schema definition for a single effect parameter (filter or veil).
/// Each module defines a `const` array of these describing its parameters.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ParamDef {
    Float { name: &'static str, min: f32, max: f32, default: f32 },
    Int   { name: &'static str, min: i32, max: i32, default: i32 },
    Bool  { name: &'static str, default: bool },
}

/// A concrete runtime parameter value, read from an effect instance.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(untagged)]
pub enum ParamValue {
    Float(f32),
    Int(i32),
    Bool(bool),
}
