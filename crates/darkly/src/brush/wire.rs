//! Brush-domain wire types and scalar values.
//!
//! Everything speaks 0-1.  Sensors output 0-1.  Curves map 0-1 → 0-1.
//! GPU stage inputs expect 0-1 and internally map to their actual range.
//!
//! Wire types are strictly the WGSL data shape — `Scalar` / `Int` /
//! `Bool` / `Vec2` / `Vec4`. There is no separate `Color`, `Texture`,
//! or `Mask` "semantic" type: an RGBA color is just a `Vec4`, a
//! coverage value is just a `Scalar`. Two wires connect iff their
//! underlying shapes match; if a node needs to go from RGBA to a
//! scalar channel, that's an explicit `split_color` node.
//!
//! Pre-WGSL there was a `Texture` wire that carried a runtime
//! `TextureHandle` — that whole infrastructure was deleted in
//! commit `ff5a2eb`. The vestigial wire-type labels (`Texture`,
//! `Mask`, `Color`) stuck around as labels even though nothing
//! texture-shaped flows on the wires anymore. They've now been
//! dropped so the wire vocabulary matches what's actually on the wire.

use serde::{Deserialize, Serialize};

use crate::nodegraph::WireKind;

// ── Wire types ──────────────────────────────────────────────────────

/// The set of data types that can flow along wires in a brush graph.
/// Strictly mirrors WGSL shapes — no semantic skins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub enum BrushWireType {
    /// Single `f32`. Used for sensor outputs, coverage values, mask
    /// luminance — anything one-channel.
    Scalar,
    /// `i32` value (enum index, count).
    Int,
    /// `bool` flag.
    Bool,
    /// `vec2<f32>` (position, tilt, motion).
    Vec2,
    /// `vec4<f32>` (RGBA color, premultiplied; arbitrary 4-vector).
    Vec4,
}

impl WireKind for BrushWireType {
    fn compatible(from: Self, to: Self) -> bool {
        use BrushWireType::*;
        if from == to {
            return true;
        }
        // Keep coercions small and obvious.
        matches!(
            (from, to),
            // Scalar widens to Int (truncates) or vice versa.
            (Scalar, Int) | (Int, Scalar)
        )
    }
}

// ── Scalar value ────────────────────────────────────────────────────

/// A value that fits in a slot in the evaluation table.
///
/// Every wire in the brush graph carries one of these.  A single enum
/// rather than generic typed slots because the slot table is a flat
/// `Vec<Option<ScalarValue>>` — uniform size means no boxing, no trait
/// objects, and direct array indexing.  16 bytes, `Copy`, no heap.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ScalarValue {
    Scalar(f32),
    Int(i32),
    Bool(bool),
    Vec2([f32; 2]),
    Vec4([f32; 4]),
}

impl ScalarValue {
    /// Extract as f32, coercing where sensible.
    pub fn as_f32(self) -> f32 {
        match self {
            Self::Scalar(v) => v,
            Self::Int(v) => v as f32,
            Self::Bool(v) => v as u8 as f32,
            _ => 0.0,
        }
    }

    /// Extract as `[f32; 4]`, broadcasting scalar to grayscale (RGB)
    /// with full alpha.
    pub fn as_color(self) -> [f32; 4] {
        match self {
            Self::Vec4(v) => v,
            Self::Scalar(v) => [v, v, v, 1.0],
            _ => [0.0, 0.0, 0.0, 1.0],
        }
    }

    /// Extract as `[f32; 2]`, broadcasting scalar to both components.
    pub fn as_vec2(self) -> [f32; 2] {
        match self {
            Self::Vec2(v) => v,
            Self::Scalar(v) => [v, v],
            _ => [0.0, 0.0],
        }
    }

    /// Coerce this value to match a target wire type.
    pub fn coerce(self, target: BrushWireType) -> Self {
        match target {
            BrushWireType::Scalar => Self::Scalar(self.as_f32()),
            BrushWireType::Int => Self::Int(self.as_f32() as i32),
            BrushWireType::Bool => Self::Bool(self.as_f32() > 0.5),
            BrushWireType::Vec2 => Self::Vec2(self.as_vec2()),
            BrushWireType::Vec4 => Self::Vec4(self.as_color()),
        }
    }
}

impl Default for ScalarValue {
    fn default() -> Self {
        Self::Scalar(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatible_same_type() {
        for ty in [
            BrushWireType::Scalar,
            BrushWireType::Int,
            BrushWireType::Vec2,
            BrushWireType::Vec4,
        ] {
            assert!(BrushWireType::compatible(ty, ty));
        }
    }

    #[test]
    fn compatible_scalar_int_coercions() {
        assert!(BrushWireType::compatible(
            BrushWireType::Scalar,
            BrushWireType::Int
        ));
        assert!(BrushWireType::compatible(
            BrushWireType::Int,
            BrushWireType::Scalar
        ));
    }

    #[test]
    fn incompatible_unrelated_types() {
        assert!(!BrushWireType::compatible(
            BrushWireType::Vec4,
            BrushWireType::Scalar
        ));
        assert!(!BrushWireType::compatible(
            BrushWireType::Vec2,
            BrushWireType::Vec4
        ));
    }

    #[test]
    fn scalar_value_coerce_to_vec4_broadcasts_grayscale() {
        let v = ScalarValue::Scalar(0.75);
        assert_eq!(
            v.coerce(BrushWireType::Vec4),
            ScalarValue::Vec4([0.75, 0.75, 0.75, 1.0])
        );
        assert_eq!(v.as_f32(), 0.75);
    }
}
