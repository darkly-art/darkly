/// Schema definition for a single effect parameter (filter or veil).
/// Each module defines a `const` array of these describing its parameters.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
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
    /// Levels adjustment — a black/gamma/white/output transfer, stored as
    /// `[inBlack, inWhite, gamma, outBlack, outWhite]` (all normalized `[0,1]`
    /// except `gamma`, the raw `0.1–10` exponent). Baked into the same LUT as a
    /// [`Curve`](ParamDef::Curve) by the shared LUT-filter scaffold.
    Levels {
        name: &'static str,
        default: [f32; 5],
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
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub enum ParamValue {
    Bool(bool),
    Int(i32),
    Float(f32),
    String(String),
    Curve(Vec<[f32; 2]>),
    /// Levels transfer `[inBlack, inWhite, gamma, outBlack, outWhite]`.
    /// A flat 5-number array; disjoint from `Curve` (an array of `[x, y]`
    /// pairs) under `#[serde(untagged)]`, so it follows `Curve` here.
    Levels([f32; 5]),
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
            ParamDef::Levels { name, default } => {
                let arr = map
                    .get(*name)
                    .and_then(|v| serde_json::from_value::<[f32; 5]>(v.clone()).ok())
                    .unwrap_or(*default);
                ParamValue::Levels(arr)
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
            ParamDef::Levels { default, .. } => ParamValue::Levels(*default),
            ParamDef::Enum { default, .. } => ParamValue::Int(*default),
            ParamDef::FloatInput { default, .. } => ParamValue::Float(*default),
            ParamDef::Icon { default, .. } => ParamValue::String(default.to_string()),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            ParamDef::Float { name, .. }
            | ParamDef::FloatInput { name, .. }
            | ParamDef::Int { name, .. }
            | ParamDef::Bool { name, .. }
            | ParamDef::String { name, .. }
            | ParamDef::Curve { name, .. }
            | ParamDef::Levels { name, .. }
            | ParamDef::Enum { name, .. }
            | ParamDef::Icon { name, .. } => name,
        }
    }

    /// Coerce an externally-typed scalar (e.g. a value parsed from YAML)
    /// into the concrete `ParamValue` variant this def expects. Floats
    /// also accept bare integers, since YAML's `1` and `1.0` are
    /// distinct but the param's natural type is the same.
    pub fn coerce_portable(&self, v: PortableValue) -> Result<ParamValue, ParamTypeMismatch> {
        let actual = v.kind_label();
        let mismatch = |expected: &'static str| Err(ParamTypeMismatch { expected, actual });
        match self {
            ParamDef::Bool { .. } => match v {
                PortableValue::Bool(b) => Ok(ParamValue::Bool(b)),
                _ => mismatch("bool"),
            },
            ParamDef::Int { .. } | ParamDef::Enum { .. } => match v {
                PortableValue::Int(i) => Ok(ParamValue::Int(i as i32)),
                _ => mismatch("integer"),
            },
            ParamDef::Float { .. } | ParamDef::FloatInput { .. } => match v {
                PortableValue::Float(f) => Ok(ParamValue::Float(f as f32)),
                PortableValue::Int(i) => Ok(ParamValue::Float(i as f32)),
                _ => mismatch("number"),
            },
            ParamDef::String { .. } | ParamDef::Icon { .. } => match v {
                PortableValue::String(s) => Ok(ParamValue::String(s)),
                _ => mismatch("string"),
            },
            ParamDef::Curve { .. } => match v {
                PortableValue::Curve(c) => Ok(ParamValue::Curve(c)),
                _ => mismatch("curve (list of [x, y] pairs)"),
            },
            ParamDef::Levels { .. } => match v {
                PortableValue::Levels(a) => Ok(ParamValue::Levels(a)),
                _ => mismatch("levels (5 numbers)"),
            },
        }
    }
}

/// Returned by [`ParamDef::coerce_portable`] when the externally-typed
/// scalar does not match the def's expected variant. Carries just the
/// type labels — the caller adds parameter-name / node-type context.
#[derive(Debug)]
pub struct ParamTypeMismatch {
    pub expected: &'static str,
    pub actual: &'static str,
}

/// Untyped scalar/composite value parsed from an external format (YAML,
/// JSON). Coerced into the concrete [`ParamValue`] via
/// [`ParamDef::coerce_portable`] using the registration metadata.
///
/// `#[serde(untagged)]` tries variants top-down — more-specific first
/// so a YAML `true` doesn't slip into `Int(1)`, `42` lands in `Int`
/// before `Float`, and the variant carrying its source typing through
/// the round trip means `algorithm: 0` reads back as `0` (not `0.0`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum PortableValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Curve(Vec<[f32; 2]>),
    Levels([f32; 5]),
}

impl PortableValue {
    pub fn from_param(p: &ParamValue) -> Self {
        match p {
            ParamValue::Bool(b) => Self::Bool(*b),
            ParamValue::Int(i) => Self::Int(*i as i64),
            ParamValue::Float(f) => Self::Float(*f as f64),
            ParamValue::String(s) => Self::String(s.clone()),
            ParamValue::Curve(c) => Self::Curve(c.clone()),
            ParamValue::Levels(a) => Self::Levels(*a),
        }
    }

    fn kind_label(&self) -> &'static str {
        match self {
            Self::Bool(_) => "bool",
            Self::Int(_) => "integer",
            Self::Float(_) => "float",
            Self::String(_) => "string",
            Self::Curve(_) => "curve",
            Self::Levels(_) => "levels",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: `ParamValue::Int(n)` must round-trip through JSON without
    /// degrading to `ParamValue::Float`. The bug: Rough Watercolor's shape
    /// node was configured with `algorithm = Int(1)` (Perlin), but after
    /// `brush_load` (graph → JSON → graph) the variant became `Float(1.0)`,
    /// and `shape.rs`'s `match Some(ParamValue::Int(v))` silently fell
    /// through to the default `0` (Sine). Port defaults — which are floats
    /// natively — round-tripped fine, so the UI showed correct numbers
    /// while the GPU rendered the wrong silhouette. Fix was to reorder the
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
            ParamValue::Levels([0.0, 1.0, 1.0, 0.0, 1.0]),
            ParamValue::Levels([0.1, 0.9, 2.2, 0.05, 0.95]),
        ] {
            let json = serde_json::to_string(&v).unwrap();
            let back: ParamValue = serde_json::from_str(&json).unwrap();
            let ok = match (&v, &back) {
                (ParamValue::Bool(a), ParamValue::Bool(b)) => a == b,
                (ParamValue::Int(a), ParamValue::Int(b)) => a == b,
                (ParamValue::Float(a), ParamValue::Float(b)) => a == b,
                (ParamValue::String(a), ParamValue::String(b)) => a == b,
                (ParamValue::Curve(a), ParamValue::Curve(b)) => a == b,
                (ParamValue::Levels(a), ParamValue::Levels(b)) => a == b,
                _ => false,
            };
            assert!(ok, "round-trip changed variant: {v:?} → {json} → {back:?}");
        }
    }
}
