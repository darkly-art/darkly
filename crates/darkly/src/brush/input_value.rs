//! Authored value carried by a disconnected brush-graph input.
//!
//! Every brush-node input is a [`crate::nodegraph::PortDef`]. When the input
//! is unwired, the value the artist authored lives here: a scalar slider
//! value, an enum-dropdown index, a texture name, curve control points, a
//! color. Wired inputs ignore this and take the upstream expression instead
//! (see [`crate::brush::wgsl::InputBinding`]).
//!
//! `InputValue` is deliberately **separate** from
//! [`crate::gpu::params::ParamValue`]: that type is the filter/veil effect
//! system's value vocabulary and is untouched by the brush graph. On disk an
//! `InputValue` takes the shape of [`PortableValue`], the brush YAML's
//! untyped DTO, which [`InputValue::from_portable`] coerces back into the
//! brush-node shape using the port's wire type.
//!
//! Distinct from [`crate::brush::wire::ScalarValue`], which is the `Copy`,
//! 16-byte *runtime* value that flows on a wire during per-dab evaluation.
//! `InputValue` owns `String`/`Vec` and only appears at authoring / compile
//! time; [`InputValue::as_scalar_value`] bridges the two for the scalar cases.

use serde::{Deserialize, Serialize};

use crate::brush::wire::ScalarValue;

/// The authored value on a disconnected brush-graph input port.
///
/// `#[serde(untagged)]` with the same top-down, more-specific-first ordering
/// discipline as [`crate::gpu::params::ParamValue`]: a JSON `true`/`false`
/// only matches `Bool`; a whole JSON number matches `Int` before `Scalar`; a
/// fractional number falls through to `Scalar`. The array shapes are disjoint
/// by nesting/length: `Curve` is an array of `[x, y]` *pairs*, `Vec2` is 2
/// flat numbers, `Vec4` is 4.
///
/// There is no separate `Enum` variant: an enum-dropdown index is an `Int`,
/// and the port's [`crate::brush::wire::BrushWireType::Enum`] wire type is
/// what marks it as a dropdown. The value only carries data; the wire type
/// carries the interpretation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub enum InputValue {
    Bool(bool),
    Int(i32),
    Scalar(f32),
    String(String),
    Curve(Vec<[f32; 2]>),
    Vec2([f32; 2]),
    Vec4([f32; 4]),
}

impl Default for InputValue {
    fn default() -> Self {
        Self::Scalar(0.0)
    }
}

impl InputValue {
    /// Bridge to the runtime [`ScalarValue`] for the scalar-family cases.
    /// Non-wirable shapes (`String`, `Curve`) can never reach the runtime
    /// slot table (the connect guard rejects wiring them), so they collapse
    /// to the neutral scalar default.
    pub fn as_scalar_value(&self) -> ScalarValue {
        match self {
            Self::Scalar(v) => ScalarValue::Scalar(*v),
            Self::Int(v) => ScalarValue::Int(*v),
            Self::Bool(v) => ScalarValue::Bool(*v),
            Self::Vec2(v) => ScalarValue::Vec2(*v),
            Self::Vec4(v) => ScalarValue::Vec4(*v),
            Self::String(_) | Self::Curve(_) => ScalarValue::default(),
        }
    }

    /// Read as `f32`, coercing the scalar-family shapes. Data shapes
    /// (`String`, `Curve`, vectors) return `0.0`.
    pub fn as_f32(&self) -> f32 {
        match self {
            Self::Scalar(v) => *v,
            Self::Int(v) => *v as f32,
            Self::Bool(v) => *v as u8 as f32,
            _ => 0.0,
        }
    }

    /// Read as a compile-time enum / branch-selector index. Enum values are
    /// stored as `Int`; `Scalar`/`Bool` coerce for tolerance.
    pub fn as_enum_index(&self) -> i32 {
        match self {
            Self::Int(v) => *v,
            Self::Bool(v) => *v as i32,
            Self::Scalar(v) => *v as i32,
            _ => 0,
        }
    }

    /// Read as a boolean flag. `Bool` directly; numeric shapes threshold at
    /// `0.5` to match the historical `default >= 0.5` reads.
    pub fn as_bool(&self) -> bool {
        match self {
            Self::Bool(v) => *v,
            _ => self.as_f32() >= 0.5,
        }
    }

    /// Read as a string (texture name, icon class). Non-string shapes return
    /// the empty string.
    pub fn as_str(&self) -> &str {
        match self {
            Self::String(s) => s.as_str(),
            _ => "",
        }
    }

    /// Read as curve control points. Non-curve shapes return the identity
    /// two-point ramp so a mis-typed input still produces a valid LUT.
    pub fn as_curve(&self) -> &[[f32; 2]] {
        match self {
            Self::Curve(pts) => pts.as_slice(),
            _ => &[[0.0, 0.0], [1.0, 1.0]],
        }
    }

    /// Coerce an externally-typed portable value (parsed from YAML/JSON) into
    /// the concrete `InputValue` the port expects, given the port's wire type.
    /// A mismatch is a hard error, so a curve in an enum slot fails loudly
    /// rather than silently defaulting.
    pub fn from_portable(
        wire_type: crate::brush::wire::BrushWireType,
        v: PortableValue,
    ) -> Result<Self, ParamTypeMismatch> {
        use crate::brush::wire::BrushWireType as W;
        let actual = v.kind_label();
        let mismatch = |expected: &'static str| Err(ParamTypeMismatch { expected, actual });
        match wire_type {
            W::Bool => match v {
                PortableValue::Bool(b) => Ok(Self::Bool(b)),
                _ => mismatch("bool"),
            },
            W::Int | W::Enum => match v {
                PortableValue::Int(i) => Ok(Self::Int(i)),
                _ => mismatch("integer"),
            },
            W::Scalar => match v {
                PortableValue::Float(f) => Ok(Self::Scalar(f)),
                PortableValue::Int(i) => Ok(Self::Scalar(i as f32)),
                _ => mismatch("number"),
            },
            W::String => match v {
                PortableValue::String(s) => Ok(Self::String(s)),
                _ => mismatch("string"),
            },
            W::Curve => match v {
                PortableValue::Curve(c) => Ok(Self::Curve(c)),
                _ => mismatch("curve (list of [x, y] pairs)"),
            },
            W::Vec2 => match v {
                PortableValue::Vec2(a) => Ok(Self::Vec2(a)),
                _ => mismatch("vec2 (2 numbers)"),
            },
            W::Vec4 => match v {
                PortableValue::Color(c) => Ok(Self::Vec4([c[0], c[1], c[2], 1.0])),
                _ => mismatch("color / vec4"),
            },
        }
    }

    /// Convert into the [`PortableValue`] DTO for on-disk YAML. The inverse of
    /// [`Self::from_portable`].
    pub fn to_portable(&self) -> PortableValue {
        match self {
            Self::Bool(b) => PortableValue::Bool(*b),
            Self::Int(i) => PortableValue::Int(*i),
            Self::Scalar(f) => PortableValue::Float(*f),
            Self::String(s) => PortableValue::String(s.clone()),
            Self::Curve(c) => PortableValue::Curve(c.clone()),
            Self::Vec2(v) => PortableValue::Vec2(*v),
            Self::Vec4(v) => PortableValue::Color([v[0], v[1], v[2]]),
        }
    }
}

/// Untyped value parsed from brush YAML (or JSON), the on-disk shape of an
/// [`InputValue`]. Coerced into the concrete `InputValue` by
/// [`InputValue::from_portable`] using the port's wire type.
///
/// `#[serde(untagged)]` tries variants top-down, more-specific first, so a
/// YAML `true` doesn't slip into `Int(1)`, `42` lands in `Int` before
/// `Float`, and the variant carrying its source typing through the round trip
/// means `algorithm: 0` reads back as `0` (not `0.0`).
///
/// Each numeric variant is exactly as wide as the `InputValue` it carries. A
/// wider type would serialize an `f32`'s rounding error at `f64` precision,
/// writing an authored `0.12` as `0.11999999731779099`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PortableValue {
    Bool(bool),
    Int(i32),
    Float(f32),
    String(String),
    Curve(Vec<[f32; 2]>),
    Color([f32; 3]),
    Vec2([f32; 2]),
}

impl PortableValue {
    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::Bool(_) => "bool",
            Self::Int(_) => "integer",
            Self::Float(_) => "float",
            Self::String(_) => "string",
            Self::Curve(_) => "curve",
            Self::Color(_) => "color",
            Self::Vec2(_) => "vec2",
        }
    }
}

/// Returned by [`InputValue::from_portable`] when the externally-typed value
/// does not match the port's wire type. Carries just the type labels; the
/// caller adds input-name / node-type context.
#[derive(Debug)]
pub struct ParamTypeMismatch {
    pub expected: &'static str,
    pub actual: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::wire::BrushWireType;

    #[test]
    fn round_trips_preserve_variant() {
        for v in [
            InputValue::Bool(true),
            InputValue::Bool(false),
            InputValue::Int(0),
            InputValue::Int(1),
            InputValue::Int(-3),
            InputValue::Scalar(0.0),
            InputValue::Scalar(1.0),
            InputValue::Scalar(1.5),
            InputValue::String("paper".into()),
            InputValue::Curve(vec![[0.0, 0.0], [1.0, 1.0]]),
            InputValue::Curve(vec![[3.0, 5.0]]),
            InputValue::Vec2([1.0, 2.0]),
            InputValue::Vec4([1.0, 0.0, 0.0, 1.0]),
        ] {
            let json = serde_json::to_string(&v).unwrap();
            let back: InputValue = serde_json::from_str(&json).unwrap();
            assert_eq!(v, back, "round-trip changed variant via {json}");
        }
    }

    #[test]
    fn from_portable_rejects_type_mismatch() {
        // A curve into an enum slot must fail loudly.
        let err = InputValue::from_portable(
            BrushWireType::Enum,
            PortableValue::Curve(vec![[0.0, 0.0], [1.0, 1.0]]),
        );
        assert!(err.is_err());
        // A bare int coerces into a scalar slot (YAML `1` vs `1.0`).
        assert_eq!(
            InputValue::from_portable(BrushWireType::Scalar, PortableValue::Int(6)).unwrap(),
            InputValue::Scalar(6.0)
        );
    }

    #[test]
    fn portable_round_trip() {
        for v in [
            InputValue::Bool(true),
            InputValue::Int(2),
            InputValue::Scalar(0.37),
            InputValue::String("tex".into()),
            InputValue::Curve(vec![[0.0, 0.0], [0.5, 0.2], [1.0, 1.0]]),
        ] {
            let wire = match &v {
                InputValue::Bool(_) => BrushWireType::Bool,
                InputValue::Int(_) => BrushWireType::Enum,
                InputValue::Scalar(_) => BrushWireType::Scalar,
                InputValue::String(_) => BrushWireType::String,
                InputValue::Curve(_) => BrushWireType::Curve,
                _ => BrushWireType::Scalar,
            };
            let back = InputValue::from_portable(wire, v.to_portable()).unwrap();
            assert_eq!(v, back);
        }
    }
}
