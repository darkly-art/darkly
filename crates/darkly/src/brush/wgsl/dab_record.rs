//! Intrinsic per-dab record header — the fields every terminal packs
//! at the front of every dab record, before any node-contributed fields.
//!
//! The per-dab record overall is *dynamic* (its layout is the union of
//! every node's contributed fields, laid out in alignment-descending
//! order), so there is no single Rust `#[repr(C)]` struct mirroring it.
//! The intrinsic header (`pos`, `bbox_target_px`,
//! `inv_radius_target_px`) is the fixed prefix, packed by hand here so
//! all four terminals — paint, watercolor, smudge, liquify — share one
//! source of truth.

use std::sync::Arc;

use crate::brush::wgsl::type_system::{DabField, WgslType};

/// Numerical-stability floor for the target-px radius division. Not a
/// physical limit — the post-scale preview radius can legitimately drop
/// below 1 target px on extreme brush sizes (a sub-pixel-radius preview
/// is useless anyway). The clamp prevents 1/0 / huge inv values from
/// poisoning the fragment.
pub(crate) const EPS_RADIUS_TARGET_PX: f32 = 0.125;

/// Compile-time number of intrinsic fields the terminal packs itself
/// before per-node fields begin. Used by the terminal's packer to skip
/// over the header.
pub const INTRINSIC_DAB_HEADER_FIELDS: usize = 3;

/// Fields every per-dab record carries, regardless of upstream nodes:
/// dab centre (`pos`), bbox half-extent (`bbox_target_px`), and the
/// reciprocal of the dab's nominal radius (`inv_radius_target_px`).
///
/// **Invariant: the dab record describes the dab in the *target
/// texture's pixel space*.** Whichever texture the brush is rasterizing
/// into, `pos` is a coordinate in that texture's pixel grid,
/// `bbox_target_px` is a half-extent in those same pixels, and
/// `inv_radius_target_px` is `1.0 / radius_in_target_pixels`. Stroke
/// renders into the layer scratch where target px ≡ canvas px;
/// preview renders into the preview mask where target px ≢ canvas px.
/// Both paths pack a well-typed record for their target via
/// [`pack_intrinsic_dab_header`], and the WGSL is target-agnostic.
///
/// Why this matters: an earlier shape of this header carried `radius`
/// and `bbox_radius` in canvas pixels in both modes, which silently
/// broke the preview path's discard test when target ≠ canvas (the dab
/// filled the texture to a square edge on large brushes). Renaming
/// these fields to declare their frame makes the bug structurally
/// inexpressible.
///
/// `bbox_target_px` is the single source of truth for the dab's write
/// footprint — the vertex stage sizes the rasterized quad against it,
/// the fragment stage discards past it, and (in stroke mode) the CPU
/// layer-clip bbox is derived from the same value so save-points
/// cannot truncate the GPU writes.
pub fn intrinsic_dab_header() -> Vec<DabField> {
    // Order matters for std430 alignment: vec2 (8) → f32 (4) → f32 (4)
    // for total 16 bytes. The terminal packs all three via
    // `pack_intrinsic_dab_header`.
    vec![
        DabField {
            name: "pos".into(),
            ty: WgslType::Vec2,
            pack: Arc::new(|_outputs, _bytes| {
                // Terminal packs `pos` directly — placeholder packer
                // here is unused because the terminal owns this field.
                unreachable!("intrinsic pos packer should not be invoked");
            }),
        },
        DabField {
            name: "bbox_target_px".into(),
            ty: WgslType::F32,
            pack: Arc::new(|_outputs, _bytes| {
                unreachable!("intrinsic bbox_target_px packer should not be invoked");
            }),
        },
        DabField {
            name: "inv_radius_target_px".into(),
            ty: WgslType::F32,
            pack: Arc::new(|_outputs, _bytes| {
                unreachable!("intrinsic inv_radius_target_px packer should not be invoked");
            }),
        },
    ]
}

/// Pack the intrinsic dab header. Single source of truth — every
/// terminal's `evaluate_gpu` (stroke path) and
/// [`crate::brush::wgsl::render_compiled_preview`] (preview path) call
/// this. The fields are interpreted in the *target texture's pixel
/// space* (see the docblock on [`intrinsic_dab_header`]). Internally
/// inverts radius once so the fragment hot path is a multiply, not a
/// divide.
///
/// Stroke-only consumers — notably watercolor's pickup shader in
/// `watercolor.rs` — treat `1 / inv_radius_target_px` as
/// canvas-px radius. That's valid only because stroke's target ≡
/// canvas. Any new sampler that derives canvas-px sizes from the dab
/// record must restrict itself to stroke-mode dispatch.
pub fn pack_intrinsic_dab_header(
    bytes: &mut Vec<u8>,
    pos: [f32; 2],
    bbox_target_px: f32,
    radius_target_px: f32,
) {
    debug_assert!(radius_target_px > 0.0, "radius_target_px must be > 0");
    let inv_radius = 1.0 / radius_target_px.max(EPS_RADIUS_TARGET_PX);
    bytes.extend_from_slice(bytemuck::bytes_of(&pos));
    bytes.extend_from_slice(bytemuck::bytes_of(&bbox_target_px));
    bytes.extend_from_slice(bytemuck::bytes_of(&inv_radius));
}
