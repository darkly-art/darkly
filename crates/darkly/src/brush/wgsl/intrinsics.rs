//! Stroke-constant intrinsic uniforms.
//!
//! [`IntrinsicUniforms`] is a Rust `#[repr(C)]` mirror of the WGSL
//! `IntrinsicUniforms` defined in `_prelude.wgsl`. The duplication is
//! forced by the CPU↔WGSL boundary — bytemuck-packed bytes are written
//! from the Rust side and read from the WGSL side, so the two structs
//! **must** have byte-identical layouts. Treat this file and
//! `_prelude.wgsl` as one logical unit and edit both together; the
//! [`crate::brush::wgsl::CompiledBrush::uniform_size`] assertion in
//! the brush pipeline will catch drift, but only at runtime.

/// Stroke-constant intrinsic uniforms every compiled brush carries.
/// Mirrors the WGSL `IntrinsicUniforms` defined in `_prelude.wgsl` —
/// every terminal packs this struct at the front of the uniform buffer
/// (followed by node-contributed uniforms). Lives here (not on each
/// terminal) so a layout change in one place can't drift from the rest.
///
/// `preview_centre` / `preview_size` are written by the preview path
/// only; the stroke path writes zero and ignores them.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct IntrinsicUniforms {
    pub layer_offset: [i32; 2],
    pub layer_size: [u32; 2],
    pub canvas_size: [u32; 2],
    pub preview_centre: [f32; 2],
    pub preview_size: [u32; 2],
    pub _pad: [u32; 2],
}

/// Size in bytes of the WGSL/Rust `IntrinsicUniforms` struct. Read by
/// the terminal-side flush path when sizing its uniform ring.
pub const INTRINSIC_UNIFORMS_SIZE: usize = std::mem::size_of::<IntrinsicUniforms>();

/// Pack the intrinsic uniforms (layer offset/size, canvas size, preview
/// centre, preview size) at the front of the uniform buffer. Followed
/// by node-contributed uniforms via
/// [`crate::brush::wgsl::pack_uniforms`]. Single source of truth;
/// collapsed from four duplicated terminal-impl methods.
pub fn pack_intrinsic_uniforms(bytes: &mut Vec<u8>, intrinsic: IntrinsicUniforms) {
    bytes.extend_from_slice(bytemuck::bytes_of(&intrinsic));
}
