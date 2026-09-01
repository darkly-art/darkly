//! Stroke-constant intrinsic uniforms.
//!
//! [`IntrinsicUniforms`] is a Rust `#[repr(C)]` mirror of the WGSL
//! `IntrinsicUniforms` defined in `_prelude.wgsl`. The duplication is
//! forced by the CPU↔WGSL boundary: bytemuck-packed bytes are written
//! from the Rust side and read from the WGSL side, so the two structs
//! **must** have byte-identical layouts. Treat this file and
//! `_prelude.wgsl` as one logical unit and edit both together; the
//! [`crate::brush::wgsl::CompiledBrush::uniform_size`] assertion in
//! the brush pipeline will catch drift, but only at runtime.

/// Stroke-constant intrinsic uniforms every compiled brush carries.
/// Mirrors the WGSL `IntrinsicUniforms` defined in `_prelude.wgsl`:
/// every terminal packs this struct at the front of the uniform buffer
/// (followed by node-contributed uniforms). Lives here (not on each
/// terminal) so a layout change in one place can't drift from the rest.
///
/// `cursor_preview_centre` / `cursor_preview_size` are written by the preview path
/// only; the stroke path writes zero and ignores them.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct IntrinsicUniforms {
    pub layer_offset: [i32; 2],
    pub layer_size: [u32; 2],
    pub canvas_size: [u32; 2],
    /// Plane-space offset of the canvas window (selection-mask anchor).
    pub canvas_origin: [i32; 2],
    pub cursor_preview_centre: [f32; 2],
    pub cursor_preview_size: [u32; 2],
    /// Active view rotation in radians (the `rotation` parameter passed to
    /// `ViewTransform::from_pan_zoom_rotate`). Subtracted from `theta` in the
    /// per-fragment skeleton so brush stamp orientation counteracts view
    /// rotation, so on-screen orientation stays put as the user rotates the
    /// view. See `_prelude.wgsl` and `wgsl/mod.rs::assemble_shader`.
    pub view_rotation: f32,
    /// How many dabs land on a given texel as the brush passes over it
    /// once: `diameter / spacing`. Stroke-constant, published by
    /// [`crate::brush::stroke_engine::StrokeEngine`], which owns the
    /// spacing that produced it.
    ///
    /// A terminal accumulating a per-dab quantity divides by this to
    /// express its rate per *pass* instead of per dab. Without it a
    /// "30% deposit" knob compounds once per dab (ten times over at the
    /// default 10% spacing), so it reads as 87%, and its meaning shifts
    /// whenever spacing or pressure changes the overlap count. 1.0 when
    /// unset, which makes the normalisation a no-op.
    pub dabs_per_pass: f32,
    /// Pads `IntrinsicUniforms` to 64 bytes (a multiple of 16) so the
    /// node-contributed uniforms packed after `intrinsic` keep 16-byte
    /// alignment. See the matching note in `_prelude.wgsl`.
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
