/// Schema definition for a single effect parameter (filter or veil).
/// Each module defines a `const` array of these describing its parameters.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ParamDef {
    Float {
        name: &'static str,
        min: f32,
        max: f32,
        default: f32,
    },
    Int {
        name: &'static str,
        min: i32,
        max: i32,
        default: i32,
    },
    Bool {
        name: &'static str,
        default: bool,
    },
    String {
        name: &'static str,
        default: &'static str,
    },
    Curve {
        name: &'static str,
        default: &'static [[f32; 2]],
    },
    /// Enum displayed as a dropdown.  Stored as Int (index into `options`).
    Enum {
        name: &'static str,
        options: &'static [&'static str],
        default: i32,
    },
    /// Float displayed as a plain text input instead of a scrub bar.
    /// Use for values where dragging is impractical (large ranges, precise entry).
    FloatInput {
        name: &'static str,
        min: f32,
        max: f32,
        default: f32,
    },
    /// Icon picker displayed as a dropdown with FA icon previews.
    /// Stored as String (FA class name).  `options` lists the available icons.
    Icon {
        name: &'static str,
        options: &'static [(&'static str, &'static str)],
        default: &'static str,
    },
}

/// A concrete runtime parameter value, read from an effect instance.
///
/// Variants are ordered for `#[serde(untagged)]` deserialization: serde
/// tries them top-down, so the more-specific shapes (`Bool`, `Int`) must
/// precede `Float`. JSON `true`/`false` only deserializes as `Bool`; whole
/// JSON numbers (`1`, `2`) match `i32`; only fractional numbers (`1.5`)
/// fall through to `Float`. Putting `Float` first would silently coerce
/// every `Int(n)` into `Float(n as f32)` on round-trip and break enum
/// param matching (`match Some(ParamValue::Int(v))` would fall through).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    Bool(bool),
    Int(i32),
    Float(f32),
    String(String),
    Curve(Vec<[f32; 2]>),
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
    defs.iter()
        .map(|def| match def {
            ParamDef::Float { name, default, .. } => {
                let v = map
                    .get(*name)
                    .and_then(|v| v.as_f64())
                    .unwrap_or(*default as f64) as f32;
                ParamValue::Float(v)
            }
            ParamDef::Int { name, default, .. } => {
                let v = map
                    .get(*name)
                    .and_then(|v| v.as_f64())
                    .unwrap_or(*default as f64) as i32;
                ParamValue::Int(v)
            }
            ParamDef::Bool { name, default } => {
                let v = map.get(*name).and_then(|v| v.as_bool()).unwrap_or(*default);
                ParamValue::Bool(v)
            }
            ParamDef::String { name, default } => {
                let v = map
                    .get(*name)
                    .and_then(|v| v.as_str())
                    .unwrap_or(default)
                    .to_string();
                ParamValue::String(v)
            }
            ParamDef::Curve { name, default } => {
                let points = map
                    .get(*name)
                    .and_then(|v| serde_json::from_value::<Vec<[f32; 2]>>(v.clone()).ok())
                    .unwrap_or_else(|| default.to_vec());
                ParamValue::Curve(points)
            }
            ParamDef::Enum { name, default, .. } => {
                let v = map
                    .get(*name)
                    .and_then(|v| v.as_f64())
                    .unwrap_or(*default as f64) as i32;
                ParamValue::Int(v)
            }
            ParamDef::FloatInput { name, default, .. } => {
                let v = map
                    .get(*name)
                    .and_then(|v| v.as_f64())
                    .unwrap_or(*default as f64) as f32;
                ParamValue::Float(v)
            }
            ParamDef::Icon { name, default, .. } => {
                let v = map
                    .get(*name)
                    .and_then(|v| v.as_str())
                    .unwrap_or(default)
                    .to_string();
                ParamValue::String(v)
            }
        })
        .collect()
}

impl ParamDef {
    pub fn default_value(&self) -> ParamValue {
        match self {
            ParamDef::Float { default, .. } => ParamValue::Float(*default),
            ParamDef::Int { default, .. } => ParamValue::Int(*default),
            ParamDef::Bool { default, .. } => ParamValue::Bool(*default),
            ParamDef::String { default, .. } => ParamValue::String(default.to_string()),
            ParamDef::Curve { default, .. } => ParamValue::Curve(default.to_vec()),
            ParamDef::Enum { default, .. } => ParamValue::Int(*default),
            ParamDef::FloatInput { default, .. } => ParamValue::Float(*default),
            ParamDef::Icon { default, .. } => ParamValue::String(default.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: `ParamValue::Int(n)` must round-trip through JSON without
    /// degrading to `ParamValue::Float`. The bug: Rough Watercolor's circle
    /// node was configured with `algorithm = Int(1)` (Perlin), but after
    /// `brush_load` (graph → JSON → graph) the variant became `Float(1.0)`,
    /// and `circle.rs`'s `match Some(ParamValue::Int(v))` silently fell
    /// through to the default `0` (Sine). Port defaults — which are floats
    /// natively — round-tripped fine, so the UI showed correct numbers
    /// while the GPU rendered the wrong shape. Fix was to reorder the
    /// `#[serde(untagged)]` variants so the more-specific `Bool` and `Int`
    /// are attempted before `Float`.
    #[test]
    fn paramvalue_round_trips_preserve_variant() {
        for v in [
            ParamValue::Bool(true),
            ParamValue::Bool(false),
            ParamValue::Int(0),
            ParamValue::Int(1),
            ParamValue::Int(-3),
            ParamValue::Float(0.0),
            ParamValue::Float(1.0),
            ParamValue::Float(1.5),
            ParamValue::Float(-2.25),
            ParamValue::String("hello".into()),
            ParamValue::Curve(vec![[0.0, 0.0], [1.0, 1.0]]),
        ] {
            let json = serde_json::to_string(&v).unwrap();
            let back: ParamValue = serde_json::from_str(&json).unwrap();
            let ok = match (&v, &back) {
                (ParamValue::Bool(a), ParamValue::Bool(b)) => a == b,
                (ParamValue::Int(a), ParamValue::Int(b)) => a == b,
                (ParamValue::Float(a), ParamValue::Float(b)) => a == b,
                (ParamValue::String(a), ParamValue::String(b)) => a == b,
                (ParamValue::Curve(a), ParamValue::Curve(b)) => a == b,
                _ => false,
            };
            assert!(ok, "round-trip changed variant: {v:?} → {json} → {back:?}");
        }
    }
}
