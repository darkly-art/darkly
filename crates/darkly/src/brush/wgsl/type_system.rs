//! WGSL type system used by the brush compiler.
//!
//! [`WgslType`] is the restricted set of scalar / vector types a brush
//! node may declare for its per-dab fields and uniform fields. The size
//! / alignment values here are the std430 numbers — the compiler relies
//! on them to lay out the generated `DabRecord` and `Uniforms` structs
//! without runtime padding surprises.
//!
//! [`DabField`] / [`UniformField`] are the schema entries each node
//! contributes; the layout helpers ([`compute_struct_size`] etc.) walk a
//! field slice and produce the final byte size of the struct.

use std::collections::HashMap;
use std::sync::Arc;

use crate::brush::wire::ScalarValue;

/// WGSL scalar/vector types a node may declare for its dab fields and
/// uniform fields. Restricted to types that have natural std430 alignment
/// (no vec3 — its 16-byte alignment trips up adjacent f32 packing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WgslType {
    F32,
    U32,
    I32,
    Vec2,
    Vec4,
}

impl WgslType {
    /// Size in bytes (matches WGSL std430 size).
    pub fn size(self) -> usize {
        match self {
            Self::F32 | Self::U32 | Self::I32 => 4,
            Self::Vec2 => 8,
            Self::Vec4 => 16,
        }
    }

    /// std430 alignment in bytes.
    pub fn align(self) -> usize {
        match self {
            Self::F32 | Self::U32 | Self::I32 => 4,
            Self::Vec2 => 8,
            Self::Vec4 => 16,
        }
    }

    pub fn wgsl_name(self) -> &'static str {
        match self {
            Self::F32 => "f32",
            Self::U32 => "u32",
            Self::I32 => "i32",
            Self::Vec2 => "vec2<f32>",
            Self::Vec4 => "vec4<f32>",
        }
    }
}

/// Closure that serializes one value into a byte buffer. Used for
/// both per-dab record fields and stroke-constant uniform fields —
/// the input is a name→value map the terminal builds from the
/// runner's slot table (keyed by
/// [`crate::brush::wgsl::CompileWgslCtx::dab_field_name`] /
/// [`crate::brush::wgsl::CompileWgslCtx::uniform_field_name`]).
pub type ValuePacker = Arc<dyn Fn(&HashMap<String, ScalarValue>, &mut Vec<u8>) + Send + Sync>;

/// Alias for the dab-record packer (per-dab).
pub type DabPacker = ValuePacker;

/// Alias for the uniform-buffer packer (per-stroke).
pub type UniformPacker = ValuePacker;

/// One field a node contributes to the per-dab record.
#[derive(Clone)]
pub struct DabField {
    /// Field name inside the generated `DabRecord` struct. Must be
    /// unique across the graph — the compiler suffixes by node id when
    /// nodes use the helper
    /// [`crate::brush::wgsl::CompileWgslCtx::dab_field_name`].
    pub name: String,
    pub ty: WgslType,
    /// Writes this field's value into the dab record byte buffer.
    pub pack: DabPacker,
}

impl std::fmt::Debug for DabField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DabField")
            .field("name", &self.name)
            .field("ty", &self.ty)
            .finish_non_exhaustive()
    }
}

/// One field a node contributes to the stroke-constant uniform buffer.
#[derive(Clone)]
pub struct UniformField {
    pub name: String,
    pub ty: WgslType,
    pub pack: UniformPacker,
}

impl std::fmt::Debug for UniformField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UniformField")
            .field("name", &self.name)
            .field("ty", &self.ty)
            .finish_non_exhaustive()
    }
}

// ── Struct layout helpers ───────────────────────────────────────────────

pub(crate) fn align_to(value: usize, alignment: usize) -> usize {
    if alignment == 0 {
        return value;
    }
    (value + alignment - 1) & !(alignment - 1)
}

pub(crate) fn compute_struct_size(fields: &[DabField]) -> usize {
    let mut size = 0;
    let mut max_align = 4;
    for f in fields {
        size = align_to(size, f.ty.align());
        size += f.ty.size();
        max_align = max_align.max(f.ty.align());
    }
    align_to(size, max_align)
}

pub(crate) fn compute_struct_size_for_uniforms(fields: &[UniformField]) -> usize {
    let mut size = 0;
    let mut max_align = 4;
    for f in fields {
        size = align_to(size, f.ty.align());
        size += f.ty.size();
        max_align = max_align.max(f.ty.align());
    }
    align_to(size, max_align)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn align_to_basic() {
        assert_eq!(align_to(0, 4), 0);
        assert_eq!(align_to(1, 4), 4);
        assert_eq!(align_to(4, 4), 4);
        assert_eq!(align_to(5, 4), 8);
        assert_eq!(align_to(12, 16), 16);
        assert_eq!(align_to(16, 16), 16);
    }

    #[test]
    fn struct_size_simple() {
        let fields = vec![
            DabField {
                name: "pos".into(),
                ty: WgslType::Vec2,
                pack: Arc::new(|_, _| {}),
            },
            DabField {
                name: "radius".into(),
                ty: WgslType::F32,
                pack: Arc::new(|_, _| {}),
            },
            DabField {
                name: "pad".into(),
                ty: WgslType::F32,
                pack: Arc::new(|_, _| {}),
            },
        ];
        // vec2 (8) + f32 (4) + f32 (4) = 16, aligned to 8 = 16.
        assert_eq!(compute_struct_size(&fields), 16);
    }

    #[test]
    fn struct_size_with_vec4() {
        let fields = vec![
            DabField {
                name: "a".into(),
                ty: WgslType::F32,
                pack: Arc::new(|_, _| {}),
            },
            DabField {
                name: "color".into(),
                ty: WgslType::Vec4,
                pack: Arc::new(|_, _| {}),
            },
        ];
        // f32 (4) → align to 16 (pad 12) → vec4 (16) = 32.
        assert_eq!(compute_struct_size(&fields), 32);
    }
}
