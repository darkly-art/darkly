use crate::units::UnitType;
use std::collections::BTreeMap;

/// A `const`-constructible parameter value, used for schema-level defaults:
/// today, a [`ParamKind::List`]'s per-entry overrides. [`ParamValue`] owns
/// `String`s and `Vec`s that can't be built in a `const`, so the schema carries
/// this `'static`-friendly mirror and lifts it through
/// [`to_value`](ConstParamValue::to_value).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConstParamValue {
    Bool(bool),
    Int(i32),
    Float(f32),
    Str(&'static str),
    Color([f32; 3]),
    Vec2([f32; 2]),
    /// Curve control points, mirroring [`ParamKind::Curve`]'s default.
    Curve(&'static [[f32; 2]]),
    /// A levels transfer, mirroring [`ParamKind::Levels`]'s default.
    Levels([f32; 5]),
    /// Per-entry named overlays over a list item's own defaults, mirroring
    /// [`ParamKind::List`]'s default.
    List(&'static [&'static [(&'static str, ConstParamValue)]]),
}

impl ConstParamValue {
    /// Lift to a [`ParamValue`]. Every scalar shape mirrors directly; a nested
    /// [`List`](ConstParamValue::List) lifts to an empty list, because
    /// expanding its entries would need the `item` defs it overlays and a
    /// schema declares no list of lists.
    fn to_value(self) -> ParamValue {
        match self {
            ConstParamValue::Bool(b) => ParamValue::Bool(b),
            ConstParamValue::Int(i) => ParamValue::Int(i),
            ConstParamValue::Float(f) => ParamValue::Float(f),
            ConstParamValue::Str(s) => ParamValue::String(s.to_string()),
            ConstParamValue::Color(c) => ParamValue::Color(c),
            ConstParamValue::Vec2(v) => ParamValue::Vec2(v),
            ConstParamValue::Curve(pts) => ParamValue::Curve(pts.to_vec()),
            ConstParamValue::Levels(a) => ParamValue::Levels(a),
            ConstParamValue::List(_) => ParamValue::List(Vec::new()),
        }
    }
}

/// Schema definition for a single effect parameter (filter or veil).
/// Each module defines a `const` array of these describing its parameters.
///
/// Authored through the `const fn` constructors and `with_*` builders, matching
/// how [`PortDef`](crate::nodegraph::PortDef) is written:
///
/// ```ignore
/// ParamDef::float("hue", -180.0, 180.0, 0.0)
///     .with_label("Hue")
///     .with_description("Rotation applied to every pixel's hue.")
///     .with_unit(UnitType::Degrees)
/// ```
///
/// The metadata every parameter shares lives on the struct; only the
/// value-shaped part varies, in [`ParamKind`]. Adding the next shared field is
/// one struct field plus one builder rather than an edit at every declaration.
#[derive(Clone, Debug)]
pub struct ParamDef {
    pub name: &'static str,
    /// Display label. `None` → the UI title-cases `name`.
    pub label: Option<&'static str>,
    /// One-sentence summary of what this parameter does, in the same painter
    /// vocabulary as a registration's `description`.
    pub description: Option<&'static str>,
    /// Display unit. Renders a suffix and, for [`UnitType::Percent`] /
    /// [`UnitType::Degrees`], converts on the way to the UI, so a parameter
    /// already stored in display space declares [`UnitType::Raw`].
    pub unit: UnitType,
    pub kind: ParamKind,
}

/// The value-shaped half of a [`ParamDef`]: what this parameter stores, its
/// range, and its default.
#[derive(Clone, Debug)]
pub enum ParamKind {
    Float {
        min: f32,
        max: f32,
        default: f32,
    },
    Int {
        min: i32,
        max: i32,
        default: i32,
    },
    Bool {
        default: bool,
    },
    String {
        default: &'static str,
    },
    Curve {
        default: &'static [[f32; 2]],
    },
    /// Levels adjustment, a black/gamma/white/output transfer, stored as
    /// `[inBlack, inWhite, gamma, outBlack, outWhite]` (all normalized `[0,1]`
    /// except `gamma`, the raw `0.1-10` exponent). Baked into the same LUT as a
    /// [`Curve`](ParamKind::Curve) by the shared LUT-filter scaffold.
    Levels {
        default: [f32; 5],
    },
    /// Enum displayed as a dropdown.  Stored as Int (index into `options`).
    Enum {
        options: &'static [&'static str],
        default: i32,
    },
    /// Float displayed as a plain text input instead of a scrub bar.
    /// Use for values where dragging is impractical (large ranges, precise entry).
    FloatInput {
        min: f32,
        max: f32,
        default: f32,
    },
    /// Icon picker displayed as a dropdown with FA icon previews.
    /// Stored as String (FA class name).  `options` lists the available icons.
    Icon {
        options: &'static [(&'static str, &'static str)],
        default: &'static str,
    },
    /// RGB color picked as normalized sRGB `[0,1]`, stored *as picked*, with
    /// **no `srgbToLinear` conversion**. That conversion is for paint colors
    /// composited in linear space; filter/veil params operate on already-stored
    /// texel values (like the Curves LUT), so they carry the sRGB triple raw.
    /// See `frontend/src/lib/color.ts`'s `hexToRgb01`/`rgb01ToHex`.
    Color {
        default: [f32; 3],
    },
    /// A 2D vector (direction + magnitude), edited via the draggable offset pad.
    /// `max` is the magnitude clamp (the pad's edge radius); values are stored
    /// with magnitude ≤ `max`.
    Vec2 {
        max: f32,
        default: [f32; 2],
    },
    /// A dynamic list of homogeneous entries: each entry is a named group of
    /// values matching `item`. `max_len` caps the entry count (surfaced in the
    /// schema so the list editor never hardcodes an effect-specific limit).
    /// `default` supplies per-entry named overrides layered on top of `item`'s
    /// own defaults; entries not overridden fall back to the item schema.
    List {
        item: &'static [ParamDef],
        max_len: usize,
        default: &'static [&'static [(&'static str, ConstParamValue)]],
    },
}

impl ParamDef {
    const fn of(name: &'static str, kind: ParamKind) -> Self {
        ParamDef {
            name,
            label: None,
            description: None,
            unit: UnitType::Raw,
            kind,
        }
    }

    pub const fn float(name: &'static str, min: f32, max: f32, default: f32) -> Self {
        Self::of(name, ParamKind::Float { min, max, default })
    }

    pub const fn int(name: &'static str, min: i32, max: i32, default: i32) -> Self {
        Self::of(name, ParamKind::Int { min, max, default })
    }

    pub const fn boolean(name: &'static str, default: bool) -> Self {
        Self::of(name, ParamKind::Bool { default })
    }

    pub const fn string(name: &'static str, default: &'static str) -> Self {
        Self::of(name, ParamKind::String { default })
    }

    pub const fn curve(name: &'static str, default: &'static [[f32; 2]]) -> Self {
        Self::of(name, ParamKind::Curve { default })
    }

    pub const fn levels(name: &'static str, default: [f32; 5]) -> Self {
        Self::of(name, ParamKind::Levels { default })
    }

    pub const fn enumeration(
        name: &'static str,
        options: &'static [&'static str],
        default: i32,
    ) -> Self {
        Self::of(name, ParamKind::Enum { options, default })
    }

    pub const fn float_input(name: &'static str, min: f32, max: f32, default: f32) -> Self {
        Self::of(name, ParamKind::FloatInput { min, max, default })
    }

    pub const fn icon(
        name: &'static str,
        options: &'static [(&'static str, &'static str)],
        default: &'static str,
    ) -> Self {
        Self::of(name, ParamKind::Icon { options, default })
    }

    pub const fn color(name: &'static str, default: [f32; 3]) -> Self {
        Self::of(name, ParamKind::Color { default })
    }

    pub const fn vec2(name: &'static str, max: f32, default: [f32; 2]) -> Self {
        Self::of(name, ParamKind::Vec2 { max, default })
    }

    pub const fn list(
        name: &'static str,
        item: &'static [ParamDef],
        max_len: usize,
        default: &'static [&'static [(&'static str, ConstParamValue)]],
    ) -> Self {
        Self::of(
            name,
            ParamKind::List {
                item,
                max_len,
                default,
            },
        )
    }

    pub const fn with_label(mut self, label: &'static str) -> Self {
        self.label = Some(label);
        self
    }

    pub const fn with_description(mut self, description: &'static str) -> Self {
        self.description = Some(description);
        self
    }

    pub const fn with_unit(mut self, unit: UnitType) -> Self {
        self.unit = unit;
        self
    }
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
///
/// The array-shaped variants are disjoint by *length* under untagged serde,
/// which enforces exact-length fixed arrays: `Curve` is an array of `[x, y]`
/// *pairs*, `Levels` is 5 flat numbers, `Color` is 3, `Vec2` is 2. `List` is
/// an array of *objects*, matching nothing else. Order follows length-specificity
/// after `Curve`: `Levels`, `Color`, `Vec2`, then `List` last.
///
/// **Known-benign collision:** `List(vec![])` serializes as `[]`, which
/// deserializes back as `Curve(vec![])` (an empty `Vec<[f32;2]>` also matches
/// `[]`, and `Curve` is tried first; reordering only moves the ambiguity).
/// The def-less document path (`layer_kinds/filter.rs`) hits this. It is
/// behaviorally invisible because every consumer of a `List` param treats any
/// non-`List` variant as the empty list (→ passthrough); pinned by
/// `paramvalue_round_trips_preserve_variant`.
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
    /// RGB color, normalized sRGB `[0,1]` (see [`ParamDef::Color`]).
    Color([f32; 3]),
    /// A 2D vector, 2 flat numbers (see [`ParamDef::Vec2`]).
    Vec2([f32; 2]),
    /// A dynamic list of named-value entries. `BTreeMap` keeps serialization
    /// deterministic and `PartialEq` stable, which the compositor's
    /// `filter_caches` change detection relies on.
    List(Vec<BTreeMap<String, ParamValue>>),
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
        .map(|def| def.value_from_json(map.get(def.name)))
        .collect()
}

/// Clamp a 2D vector's magnitude to `max` (the [`ParamDef::Vec2`] radius),
/// preserving direction. Zero and sub-`max` vectors pass through unchanged.
fn clamp_magnitude(v: [f32; 2], max: f32) -> [f32; 2] {
    let mag = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if mag > max && mag > 0.0 {
        let s = max / mag;
        [v[0] * s, v[1] * s]
    } else {
        v
    }
}

/// Expand a [`ParamKind::List`]'s schema defaults into concrete entries: each
/// default entry starts from the `item` schema's own per-field defaults, then
/// applies that entry's named overrides on top.
fn list_default(
    item: &[ParamDef],
    entries: &[&[(&'static str, ConstParamValue)]],
) -> Vec<BTreeMap<String, ParamValue>> {
    entries
        .iter()
        .map(|overrides| {
            item.iter()
                .map(|d| {
                    let key = d.name;
                    let val = overrides
                        .iter()
                        .find(|(k, _)| *k == key)
                        .map(|(_, v)| v.to_value())
                        .unwrap_or_else(|| d.default_value());
                    (key.to_string(), val)
                })
                .collect()
        })
        .collect()
}

impl ParamDef {
    pub fn default_value(&self) -> ParamValue {
        match &self.kind {
            ParamKind::Float { default, .. } => ParamValue::Float(*default),
            ParamKind::Int { default, .. } => ParamValue::Int(*default),
            ParamKind::Bool { default, .. } => ParamValue::Bool(*default),
            ParamKind::String { default, .. } => ParamValue::String(default.to_string()),
            ParamKind::Curve { default, .. } => ParamValue::Curve(default.to_vec()),
            ParamKind::Levels { default, .. } => ParamValue::Levels(*default),
            ParamKind::Enum { default, .. } => ParamValue::Int(*default),
            ParamKind::FloatInput { default, .. } => ParamValue::Float(*default),
            ParamKind::Icon { default, .. } => ParamValue::String(default.to_string()),
            ParamKind::Color { default, .. } => ParamValue::Color(*default),
            ParamKind::Vec2 { default, .. } => ParamValue::Vec2(*default),
            ParamKind::List { item, default, .. } => ParamValue::List(list_default(item, default)),
        }
    }

    /// Read one param from an optional JSON value (missing → schema default),
    /// coercing to this def's concrete [`ParamValue`] variant. The `List` arm
    /// recurses over its `item` defs per entry, so no arm is duplicated.
    pub fn value_from_json(&self, raw: Option<&serde_json::Value>) -> ParamValue {
        match &self.kind {
            ParamKind::Float { default, .. } => {
                ParamValue::Float(raw.and_then(|v| v.as_f64()).unwrap_or(*default as f64) as f32)
            }
            ParamKind::Int { default, .. } => {
                ParamValue::Int(raw.and_then(|v| v.as_f64()).unwrap_or(*default as f64) as i32)
            }
            ParamKind::Bool { default, .. } => {
                ParamValue::Bool(raw.and_then(|v| v.as_bool()).unwrap_or(*default))
            }
            ParamKind::String { default, .. } => {
                ParamValue::String(raw.and_then(|v| v.as_str()).unwrap_or(default).to_string())
            }
            ParamKind::Curve { default, .. } => {
                let points = raw
                    .and_then(|v| serde_json::from_value::<Vec<[f32; 2]>>(v.clone()).ok())
                    .unwrap_or_else(|| default.to_vec());
                ParamValue::Curve(points)
            }
            ParamKind::Levels { default, .. } => {
                let arr = raw
                    .and_then(|v| serde_json::from_value::<[f32; 5]>(v.clone()).ok())
                    .unwrap_or(*default);
                ParamValue::Levels(arr)
            }
            ParamKind::Enum { default, .. } => {
                ParamValue::Int(raw.and_then(|v| v.as_f64()).unwrap_or(*default as f64) as i32)
            }
            ParamKind::FloatInput { default, .. } => {
                ParamValue::Float(raw.and_then(|v| v.as_f64()).unwrap_or(*default as f64) as f32)
            }
            ParamKind::Icon { default, .. } => {
                ParamValue::String(raw.and_then(|v| v.as_str()).unwrap_or(default).to_string())
            }
            ParamKind::Color { default, .. } => {
                let c = raw
                    .and_then(|v| serde_json::from_value::<[f32; 3]>(v.clone()).ok())
                    .unwrap_or(*default);
                ParamValue::Color(c)
            }
            ParamKind::Vec2 { default, max, .. } => {
                let v = raw
                    .and_then(|v| serde_json::from_value::<[f32; 2]>(v.clone()).ok())
                    .unwrap_or(*default);
                ParamValue::Vec2(clamp_magnitude(v, *max))
            }
            ParamKind::List {
                item,
                max_len,
                default,
                ..
            } => {
                let entries = match raw.and_then(|v| v.as_array()) {
                    Some(arr) => arr
                        .iter()
                        .take(*max_len)
                        .map(|entry| {
                            let obj = entry.as_object();
                            item.iter()
                                .map(|d| {
                                    let key = d.name;
                                    let child = obj.and_then(|o| o.get(key));
                                    (key.to_string(), d.value_from_json(child))
                                })
                                .collect::<BTreeMap<String, ParamValue>>()
                        })
                        .collect(),
                    None => list_default(item, default),
                };
                ParamValue::List(entries)
            }
        }
    }

    /// Coerce an externally-typed scalar (e.g. a value parsed from YAML)
    /// into the concrete `ParamValue` variant this def expects. Floats
    /// also accept bare integers, since YAML's `1` and `1.0` are
    /// distinct but the param's natural type is the same.
    pub fn coerce_portable(&self, v: PortableValue) -> Result<ParamValue, ParamTypeMismatch> {
        let actual = v.kind_label();
        let mismatch = |expected: &'static str| Err(ParamTypeMismatch { expected, actual });
        match &self.kind {
            ParamKind::Bool { .. } => match v {
                PortableValue::Bool(b) => Ok(ParamValue::Bool(b)),
                _ => mismatch("bool"),
            },
            ParamKind::Int { .. } | ParamKind::Enum { .. } => match v {
                PortableValue::Int(i) => Ok(ParamValue::Int(i as i32)),
                _ => mismatch("integer"),
            },
            ParamKind::Float { .. } | ParamKind::FloatInput { .. } => match v {
                PortableValue::Float(f) => Ok(ParamValue::Float(f as f32)),
                PortableValue::Int(i) => Ok(ParamValue::Float(i as f32)),
                _ => mismatch("number"),
            },
            ParamKind::String { .. } | ParamKind::Icon { .. } => match v {
                PortableValue::String(s) => Ok(ParamValue::String(s)),
                _ => mismatch("string"),
            },
            ParamKind::Curve { .. } => match v {
                PortableValue::Curve(c) => Ok(ParamValue::Curve(c)),
                _ => mismatch("curve (list of [x, y] pairs)"),
            },
            ParamKind::Levels { .. } => match v {
                PortableValue::Levels(a) => Ok(ParamValue::Levels(a)),
                _ => mismatch("levels (5 numbers)"),
            },
            ParamKind::Color { .. } => match v {
                PortableValue::Color(c) => Ok(ParamValue::Color(c)),
                _ => mismatch("color (3 numbers)"),
            },
            ParamKind::Vec2 { max, .. } => match v {
                PortableValue::Vec2(a) => Ok(ParamValue::Vec2(clamp_magnitude(a, *max))),
                _ => mismatch("vec2 (2 numbers)"),
            },
            ParamKind::List { item, .. } => match v {
                PortableValue::List(entries) => {
                    let out = entries
                        .into_iter()
                        .map(|entry| {
                            item.iter()
                                .map(|d| {
                                    let key = d.name;
                                    let val = match entry.get(key) {
                                        Some(pv) => d.coerce_portable(pv.clone())?,
                                        None => d.default_value(),
                                    };
                                    Ok((key.to_string(), val))
                                })
                                .collect::<Result<BTreeMap<_, _>, ParamTypeMismatch>>()
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(ParamValue::List(out))
                }
                _ => mismatch("list (array of entries)"),
            },
        }
    }
}

/// Returned by [`ParamDef::coerce_portable`] when the externally-typed
/// scalar does not match the def's expected variant. Carries just the
/// type labels; the caller adds parameter-name / node-type context.
#[derive(Debug)]
pub struct ParamTypeMismatch {
    pub expected: &'static str,
    pub actual: &'static str,
}

/// Untyped scalar/composite value parsed from an external format (YAML,
/// JSON). Coerced into the concrete [`ParamValue`] via
/// [`ParamDef::coerce_portable`] using the registration metadata.
///
/// `#[serde(untagged)]` tries variants top-down, more-specific first,
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
    Color([f32; 3]),
    Vec2([f32; 2]),
    List(Vec<BTreeMap<String, PortableValue>>),
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
            ParamValue::Color(c) => Self::Color(*c),
            ParamValue::Vec2(v) => Self::Vec2(*v),
            ParamValue::List(entries) => Self::List(
                entries
                    .iter()
                    .map(|e| {
                        e.iter()
                            .map(|(k, v)| (k.clone(), Self::from_param(v)))
                            .collect()
                    })
                    .collect(),
            ),
        }
    }

    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::Bool(_) => "bool",
            Self::Int(_) => "integer",
            Self::Float(_) => "float",
            Self::String(_) => "string",
            Self::Curve(_) => "curve",
            Self::Levels(_) => "levels",
            Self::Color(_) => "color",
            Self::Vec2(_) => "vec2",
            Self::List(_) => "list",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Every parameter a user can see carries a label and a description, so no
    /// documentation table ships with a blank cell and no properties panel
    /// falls back to a raw snake_case field name. Walks the catalogs rather
    /// than the registries directly, which is exactly the set that reaches
    /// both the UI and the export, including the item schema inside a `List`,
    /// where a blank cell is easiest to miss.
    #[test]
    fn every_param_has_a_label_and_description() {
        fn check(owner: &str, defs: &[ParamDef]) {
            for d in defs {
                assert!(
                    d.label.is_some_and(|l| !l.is_empty()),
                    "`{owner}.{}` has no label",
                    d.name
                );
                assert!(
                    d.description.is_some_and(|s| !s.is_empty()),
                    "`{owner}.{}` has no description",
                    d.name
                );
                if let ParamKind::List { item, .. } = &d.kind {
                    check(&format!("{owner}.{}", d.name), item);
                }
            }
        }

        let mut checked = 0usize;
        for reg in crate::gpu::filter::FilterPipelineRegistry::new().types() {
            check(reg.type_id, reg.params);
            checked += reg.params.len();
        }
        for reg in crate::gpu::veil::VeilRegistry::new().types() {
            check(reg.type_id, reg.params);
            checked += reg.params.len();
        }
        for reg in crate::gpu::void::VoidRegistry::new().types() {
            check(reg.type_id, reg.params);
            checked += reg.params.len();
        }
        assert!(checked > 0, "no parameters found: the scan found nothing");
    }

    // A small list schema used across the List/Vec2/Color tests: one entry is a
    // named group of `{ offset: Vec2, scale: Float, color: Color }`, with two
    // default entries exercising per-entry overrides on top of item defaults.
    const ITEM: &[ParamDef] = &[
        ParamDef::vec2("offset", 64.0, [0.0, 0.0]),
        ParamDef::float("scale", 0.9, 1.1, 1.0),
        ParamDef::color("color", [1.0, 1.0, 1.0]),
    ];
    const LIST: ParamDef = ParamDef::list(
        "aberrations",
        ITEM,
        4,
        &[
            &[("scale", ConstParamValue::Float(1.004))],
            &[
                ("offset", ConstParamValue::Vec2([2.0, 0.0])),
                ("color", ConstParamValue::Color([0.0, 1.0, 0.0])),
            ],
        ],
    );

    /// Regression: `ParamValue::Int(n)` must round-trip through JSON without
    /// degrading to `ParamValue::Float`. The bug: Rough Watercolor's shape
    /// node was configured with `algorithm = Int(1)` (Perlin), but after
    /// `brush_load` (graph → JSON → graph) the variant became `Float(1.0)`,
    /// and `shape.rs`'s `match Some(ParamValue::Int(v))` silently fell
    /// through to the default `0` (Sine). Port defaults (which are floats
    /// natively) round-tripped fine, so the UI showed correct numbers
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
            // 3-point curve stays a Curve (not mistaken for Levels' 5 numbers).
            ParamValue::Curve(vec![[0.0, 0.0], [0.5, 0.5], [1.0, 1.0]]),
            // Single-point curve `[[a, b]]` (array of ONE pair) must stay Curve
            // and never be read as a 2-number `Vec2`.
            ParamValue::Curve(vec![[3.0, 5.0]]),
            ParamValue::Levels([0.0, 1.0, 1.0, 0.0, 1.0]),
            ParamValue::Levels([0.1, 0.9, 2.2, 0.05, 0.95]),
            // Color/Vec2 with whole-number components (adversarial: whole floats
            // serialize as `1.0`, not `1`, so they don't degrade to Int).
            ParamValue::Color([1.0, 0.0, 0.0]),
            ParamValue::Color([0.25, 0.5, 0.75]),
            ParamValue::Vec2([1.0, 2.0]),
            ParamValue::Vec2([-4.0, 0.0]),
            // A list entry mixing every value kind, including a Curve nested
            // inside an entry (array-of-objects matches nothing but List).
            ParamValue::List(vec![BTreeMap::from([
                ("offset".to_string(), ParamValue::Vec2([4.0, 0.0])),
                ("scale".to_string(), ParamValue::Float(1.004)),
                ("color".to_string(), ParamValue::Color([1.0, 0.0, 0.0])),
                (
                    "curve".to_string(),
                    ParamValue::Curve(vec![[0.0, 0.0], [1.0, 1.0]]),
                ),
                ("count".to_string(), ParamValue::Int(3)),
                ("on".to_string(), ParamValue::Bool(true)),
            ])]),
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
                (ParamValue::Color(a), ParamValue::Color(b)) => a == b,
                (ParamValue::Vec2(a), ParamValue::Vec2(b)) => a == b,
                (ParamValue::List(a), ParamValue::List(b)) => a == b,
                _ => false,
            };
            assert!(ok, "round-trip changed variant: {v:?} → {json} → {back:?}");
        }
    }

    /// Pinned benign collision: `List(vec![])` serializes as `[]`, which
    /// deserializes back as `Curve(vec![])` (empty vec matches Curve, tried
    /// first). Behaviorally invisible: every List consumer treats a non-List
    /// as the empty list. This test documents the degradation so a future
    /// reorder that changes it is a deliberate, reviewed choice.
    #[test]
    fn empty_list_degrades_to_empty_curve() {
        let v = ParamValue::List(vec![]);
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(json, "[]");
        let back: ParamValue = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ParamValue::Curve(vec![]));
    }

    /// A `List` def's schema defaults expand into concrete entries: each entry
    /// starts from the item schema's own defaults, with the per-entry overrides
    /// layered on top.
    #[test]
    fn list_default_expands_entries_with_overrides() {
        let ParamValue::List(entries) = LIST.default_value() else {
            panic!("List default must be a List value");
        };
        assert_eq!(entries.len(), 2);
        // Entry 0: `scale` overridden, offset/color from item defaults.
        assert_eq!(entries[0]["scale"], ParamValue::Float(1.004));
        assert_eq!(entries[0]["offset"], ParamValue::Vec2([0.0, 0.0]));
        assert_eq!(entries[0]["color"], ParamValue::Color([1.0, 1.0, 1.0]));
        // Entry 1: offset + color overridden, scale falls back to item default.
        assert_eq!(entries[1]["offset"], ParamValue::Vec2([2.0, 0.0]));
        assert_eq!(entries[1]["color"], ParamValue::Color([0.0, 1.0, 0.0]));
        assert_eq!(entries[1]["scale"], ParamValue::Float(1.0));
    }

    /// `param_values_from_json` fills missing entry fields from item defaults and
    /// clamps an over-range Vec2 to the def's `max` magnitude.
    #[test]
    fn list_from_json_fills_missing_fields_and_clamps_vec2() {
        let obj = serde_json::json!({
            "aberrations": [
                { "offset": [100.0, 0.0] }, // magnitude 100 > max 64 → clamped
                { "scale": 0.95 },
            ]
        });
        let vals = param_values_from_json(&obj, std::slice::from_ref(&LIST));
        let ParamValue::List(entries) = &vals[0] else {
            panic!("expected a List value");
        };
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["offset"], ParamValue::Vec2([64.0, 0.0]));
        assert_eq!(entries[0]["scale"], ParamValue::Float(1.0));
        assert_eq!(entries[0]["color"], ParamValue::Color([1.0, 1.0, 1.0]));
        assert_eq!(entries[1]["scale"], ParamValue::Float(0.95));
        assert_eq!(entries[1]["offset"], ParamValue::Vec2([0.0, 0.0]));
    }

    /// Portable coercion (the YAML/def-driven path) round-trips the new kinds,
    /// including a nested List.
    #[test]
    fn portable_coercion_round_trips_new_kinds() {
        let color_def = ParamDef::color("c", [0.0; 3]);
        let color = ParamValue::Color([0.5, 0.25, 0.75]);
        let back = color_def
            .coerce_portable(PortableValue::from_param(&color))
            .unwrap();
        assert_eq!(back, color);

        let list = LIST.default_value();
        let back = LIST
            .coerce_portable(PortableValue::from_param(&list))
            .unwrap();
        assert_eq!(back, list);
    }
}
