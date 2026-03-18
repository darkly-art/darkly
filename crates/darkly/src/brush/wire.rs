//! Brush-domain wire types and scalar values.
//!
//! Everything speaks 0-1.  Sensors output 0-1.  Curves map 0-1 → 0-1.
//! GPU stage inputs expect 0-1 and internally map to their actual range.

use serde::{Deserialize, Serialize};

use crate::nodegraph::WireKind;

// ── Wire types ──────────────────────────────────────────────────────

/// The set of data types that can flow along wires in a brush graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BrushWireType {
    /// Single f32 (0-1 normalized).
    Scalar,
    /// Integer value.
    Int,
    /// Boolean flag.
    Bool,
    /// Two-component vector (e.g. position, tilt).
    Vec2,
    /// Four-component vector.
    Vec4,
    /// RGBA color (linear, premultiplied alpha).
    Color,
    /// Handle to a GPU texture (dab, stamp, mask).
    Texture,
    /// Handle to a GPU mask texture.
    Mask,
}

impl WireKind for BrushWireType {
    fn compatible(from: Self, to: Self) -> bool {
        use BrushWireType::*;
        if from == to {
            return true;
        }
        // Implicit coercions — keep this small and obvious.
        matches!(
            (from, to),
            // Scalar widens to Int (truncates) or vice versa.
            (Scalar, Int) | (Int, Scalar) |
            // Scalar promotes to Color (grayscale).
            (Scalar, Color) |
            // Texture and Mask are interchangeable (same underlying format).
            (Texture, Mask) | (Mask, Texture)
        )
    }
}

// ── Scalar value ────────────────────────────────────────────────────

/// A GPU-texture handle stored in the slot table.
///
/// Index into the runner's texture slot array.  `u16` because we'll
/// never have more than a few thousand textures in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextureHandle(pub u16);

/// A value that fits in a slot in the evaluation table.
///
/// 16 bytes, `Copy`, no heap.  This is the universal currency of the
/// brush graph runtime — every wire carries one of these.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ScalarValue {
    Scalar(f32),
    Int(i32),
    Bool(bool),
    Vec2([f32; 2]),
    Vec4([f32; 4]),
    Color([f32; 4]),
    Texture(TextureHandle),
    Mask(TextureHandle),
}

impl ScalarValue {
    /// Extract as f32, coercing where sensible.
    pub fn as_f32(self) -> f32 {
        match self {
            Self::Scalar(v) => v,
            Self::Int(v) => v as f32,
            Self::Bool(v) => if v { 1.0 } else { 0.0 },
            _ => 0.0,
        }
    }

    /// Extract as [f32; 4] color, coercing scalar to grayscale.
    pub fn as_color(self) -> [f32; 4] {
        match self {
            Self::Color(c) => c,
            Self::Vec4(v) => v,
            Self::Scalar(v) => [v, v, v, 1.0],
            _ => [0.0, 0.0, 0.0, 1.0],
        }
    }

    /// Extract as [f32; 2].
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
            BrushWireType::Color => Self::Color(self.as_color()),
            BrushWireType::Texture => self, // can't coerce into a texture
            BrushWireType::Mask => self,
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
            BrushWireType::Color,
            BrushWireType::Texture,
        ] {
            assert!(BrushWireType::compatible(ty, ty));
        }
    }

    #[test]
    fn compatible_coercions() {
        assert!(BrushWireType::compatible(BrushWireType::Scalar, BrushWireType::Int));
        assert!(BrushWireType::compatible(BrushWireType::Scalar, BrushWireType::Color));
        assert!(BrushWireType::compatible(BrushWireType::Texture, BrushWireType::Mask));
    }

    #[test]
    fn incompatible_types() {
        assert!(!BrushWireType::compatible(BrushWireType::Color, BrushWireType::Scalar));
        assert!(!BrushWireType::compatible(BrushWireType::Vec2, BrushWireType::Color));
    }

    #[test]
    fn scalar_value_coerce() {
        let v = ScalarValue::Scalar(0.75);
        assert_eq!(v.coerce(BrushWireType::Color), ScalarValue::Color([0.75, 0.75, 0.75, 1.0]));
        assert_eq!(v.as_f32(), 0.75);
    }
}
