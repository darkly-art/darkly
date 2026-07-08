//! Clone Source node — samples the frozen pre-stroke snapshot at an
//! offset, turning the reused `paint` terminal into a clone-stamp brush.
//!
//! ## What it does
//!
//! Instead of a flat color (`paint_color`) or a bundle texture (`image`),
//! this node's `color` output is a per-fragment sample of the **layer's
//! frozen pre-stroke snapshot** — the same layer-local texture `paint`'s
//! `commit` already produces (`StrokeResources::pre_stroke_texture`).
//! Wire `clone_source.color → stamp.color → paint.rgba` and the terminal
//! deposits copied pixels under the cursor, inheriting shape, spacing,
//! pressure-size, flow, opacity, erase, selection, preview, and undo for
//! free. No new terminal — see the plan in `docs`/PR for why `paint` is
//! the right base.
//!
//! ## Source binding
//!
//! Compilation calls [`CompileWgslCtx::request_source_texture`], which
//! sets [`crate::brush::wgsl::CompiledBrush::samples_source`] and reserves
//! the `@group(3)` source slot. `paint`'s `flush_dabs` binds the live
//! per-stroke snapshot there; the hover preview binds the registry
//! `_fallback` tile (there is no snapshot at hover — the cursor thumbnail
//! comes out neutral). The bind/sample plumbing is shared with `image`
//! via [`crate::brush::wgsl::sample_graph_texture`].
//!
//! ## Two modes
//!
//! Per fragment the node computes `src = target_pos + offset` and samples
//! the snapshot there. `offset` is the clone offset:
//!
//! - **Aligned** (`mode = 0`): `offset = source_anchor − dest_anchor` —
//!   constant for the whole stroke, so the source tracks the cursor.
//! - **Anchored** (`mode = 1`): `offset = source_anchor − center` — every
//!   dab samples the fixed `source_anchor` (the freeze-source toggle).
//!
//! `source_anchor` / `dest_anchor` are stroke-constant uniforms seeded by
//! the runner from the engine's [`CloneState`](crate::brush::eval::CloneState)
//! (set-source gesture + first-dab capture). `mode` is read from the
//! exposed port default and baked into the emitted WGSL.
//!
//! **Coordinate frame:** `target_pos`, `center`, and the anchors are all
//! plane/canvas pixels. The snapshot is layer-local and layer-sized, so
//! the sample UV is `(src − layer_offset) / layer_size` (from
//! `IntrinsicUniforms`), **not** `canvas_*`. Out-of-layer UVs read
//! transparent — see [`docs/coordinate-systems.md`].

use std::sync::Arc;

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wgsl::{
    sample_graph_texture, CompileWgslCtx, InputBinding, NodeWgsl, UniformField, WgslType,
};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{NodeRegistration, PortDef};

pub const TYPE_ID: &str = "clone_source";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration::compute(
        NodeRegistration {
            type_id: TYPE_ID,
            category: "texture",
            display_name: "Clone Source",
            description: "Samples pixels from a set source point onto the canvas under your cursor. Set the source with the clone set-source gesture, then paint. Feed into a Stamp Tip's colour input.",
            ports: vec![
                PortDef::input("center", BrushWireType::Vec2)
                    .with_description("Per-dab pen position in canvas pixels (wire Pen Input → Position)."),
                // Aligned (0) vs anchored (1). Exposed as a Bool toggle;
                // read from the port default and baked into the WGSL
                // offset expression at compile time.
                PortDef::input("mode", BrushWireType::Bool)
                    .with_range(0.0, 1.0, 0.0)
                    .with_step(1.0)
                    .with_label("Anchored")
                    .with_icon("fa6-solid:anchor")
                    .exposed()
                    .with_description(
                        "Off: the source tracks the cursor (aligned). On: every dab \
                         samples the fixed source point (anchored).",
                    ),
                PortDef::output("color", BrushWireType::Vec4)
                    .with_description("RGBA sampled from the frozen source snapshot at the clone offset"),
            ],
            params: &[],
            is_gpu: false,
            is_terminal: false,
            supports_erase: true,
        },
        || Box::new(CloneSourceEvaluator),
    )
}

/// The exposed `mode` toggle reads anchored at or above this threshold,
/// aligned below. The single source of truth for the 0/1 split — shared by
/// the compile-time bake ([`mode_is_anchored`]) and the engine's structural
/// [`crate::engine::DarklyEngine::clone_source_anchored`] query so the
/// frontend marker and the emitted WGSL can't disagree on the mode.
pub const MODE_ANCHORED_THRESHOLD: f32 = 0.5;

/// Whether a `mode` port default selects anchored (`true`) or aligned.
pub fn mode_default_is_anchored(mode_default: f32) -> bool {
    mode_default >= MODE_ANCHORED_THRESHOLD
}

/// Read the exposed `mode` port default and decide aligned vs anchored.
/// Baked at compile time (like `paint`'s flow); a wired `mode` (unusual)
/// falls back to aligned.
fn mode_is_anchored(cctx: &CompileWgslCtx) -> bool {
    match cctx.input("mode") {
        InputBinding::Default(v) => mode_default_is_anchored(v.as_f32()),
        InputBinding::Wired(_) => false,
    }
}

pub struct CloneSourceEvaluator;

impl BrushNodeEvaluator for CloneSourceEvaluator {
    /// CPU evaluation returns a neutral grey — `clone_source` is only
    /// meaningful per-fragment in the compiled shader, and the per-dab
    /// CPU dispatch path is dead for compiled-WGSL brushes. Mirrors
    /// `image`'s placeholder so mixed CPU/compiled graphs don't `NaN`
    /// through `color`.
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![("color".into(), ScalarValue::Vec4([0.5, 0.5, 0.5, 1.0]))]
    }

    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        if !cctx.consumed_outputs.contains("color") {
            // Nothing downstream samples the source — don't reserve the
            // binding or emit the sample.
            return Ok(wgsl);
        }

        let slot = cctx.request_source_texture();

        // Stroke-constant anchor uniforms, seeded per-stroke by the
        // runner from `CloneState` (keyed `n{id}_source_anchor` /
        // `n{id}_dest_anchor`). Default to `[0, 0]` when unseeded (the
        // hover preview, which has no live anchors).
        let sa_field = cctx.uniform_field_name("source_anchor");
        let da_field = cctx.uniform_field_name("dest_anchor");
        for field in [sa_field.clone(), da_field.clone()] {
            let key = field.clone();
            wgsl.uniform_fields.push(UniformField {
                name: field,
                ty: WgslType::Vec2,
                pack: Arc::new(move |outputs, bytes| {
                    let v = outputs.get(&key).map(|s| s.as_vec2()).unwrap_or([0.0, 0.0]);
                    bytes.extend_from_slice(bytemuck::bytes_of(&v));
                }),
            });
        }

        let center = cctx.input("center").as_vec2();
        let offset = if mode_is_anchored(cctx) {
            // Anchored: source stays pinned regardless of cursor travel.
            format!("(u.{sa_field} - ({center}))")
        } else {
            // Aligned: constant offset captured at stroke start.
            format!("(u.{sa_field} - u.{da_field})")
        };

        // Helper function so the per-fragment math is emitted once. It
        // references the module-scope `u` uniform and the `@group(3)`
        // source texture directly. `tp` is the fragment's plane-space
        // position in canvas pixels; `off` the clone offset.
        let fn_name = cctx.ident("clone_sample");
        let sample = sample_graph_texture(slot, "uv");
        wgsl.decls = format!(
            "fn {fn_name}(tp: vec2<f32>, off: vec2<f32>) -> vec4<f32> {{\n\
             \x20   let src = tp + off;\n\
             \x20   let lo = vec2<f32>(f32(u.intrinsic.layer_offset.x), f32(u.intrinsic.layer_offset.y));\n\
             \x20   let lsz = vec2<f32>(f32(u.intrinsic.layer_size.x), f32(u.intrinsic.layer_size.y));\n\
             \x20   let uv = (src - lo) / lsz;\n\
             \x20   if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {{\n\
             \x20       return vec4<f32>(0.0, 0.0, 0.0, 0.0);\n\
             \x20   }}\n\
             \x20   return {sample};\n\
             }}\n"
        );
        let out = cctx.ident("clone_c");
        wgsl.body = format!("    let {out} = {fn_name}(target_pos, {offset});\n");
        wgsl.outputs.insert("color".into(), out);
        Ok(wgsl)
    }

    /// Preview-mode body. At hover there is no frozen source snapshot to
    /// sample (the preview pipeline binds the registry `_fallback` tile to
    /// the declared source slot), so sampling it would stamp a meaningless
    /// flat tile. Instead emit an opaque neutral constant for the `color`
    /// output — the terminal deposits it through the brush tip, so the
    /// cursor preview shows the tip *shape* in neutral grey (matching
    /// Krita's `kis_duplicateop` and GIMP's source-tool outline). The
    /// output name matches `compile_wgsl`'s so the terminal's preview body,
    /// which resolves its `color` wire against the stroke pass's output
    /// expressions, still finds the variable.
    fn compile_cursor_preview_body(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        if !cctx.consumed_outputs.contains("color") {
            return Ok(wgsl);
        }
        let out = cctx.ident("clone_c");
        wgsl.body = format!("    let {out} = vec4<f32>(0.6, 0.6, 0.6, 1.0);\n");
        wgsl.outputs.insert("color".into(), out);
        Ok(wgsl)
    }
}

/// CPU-side spec of the clone offset formula, mirrored by the WGSL
/// emitted in [`CloneSourceEvaluator::compile_wgsl`]. This is the one
/// unavoidable CPU↔WGSL duplication (the same kind `IntrinsicUniforms`
/// carries) — kept tiny and covered by a unit test so the two-mode
/// semantics can't silently drift.
///
/// `center` is the per-dab pen position; the returned offset is added to
/// `target_pos` before sampling.
#[cfg(test)]
pub(crate) fn clone_offset(
    anchored: bool,
    source_anchor: [f32; 2],
    dest_anchor: [f32; 2],
    center: [f32; 2],
) -> [f32; 2] {
    if anchored {
        [source_anchor[0] - center[0], source_anchor[1] - center[1]]
    } else {
        [
            source_anchor[0] - dest_anchor[0],
            source_anchor[1] - dest_anchor[1],
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_shape() {
        let reg = register();
        assert_eq!(reg.node.type_id, "clone_source");
        assert_eq!(reg.node.category, "texture");
        // Two inputs (center, mode) + one output (color).
        assert_eq!(reg.node.ports.len(), 3);
        assert!(reg.node.ports.iter().any(|p| p.name == "color"));
        assert!(reg.node.ports.iter().any(|p| p.name == "center"));
        assert!(!reg.node.ports.iter().any(|p| p.name == "angle"));
        assert!(reg.node.params.is_empty());

        // Anchored `mode` is a Bool toggle defaulting to aligned (0.0).
        let mode = reg
            .node
            .ports
            .iter()
            .find(|p| p.name == "mode")
            .expect("mode port");
        assert_eq!(mode.wire_type, BrushWireType::Bool);
        assert_eq!(mode.default, 0.0);
        assert!(!mode_default_is_anchored(mode.default));
    }

    /// The two-mode offset formula: aligned is a stroke-constant shift
    /// (source tracks the cursor); anchored is per-dab (source pinned).
    #[test]
    fn offset_formula_aligned_vs_anchored() {
        let source = [100.0, 40.0];
        let dest = [10.0, 10.0];

        // Aligned: offset is source − dest, independent of the current dab.
        let a0 = clone_offset(false, source, dest, [10.0, 10.0]);
        let a1 = clone_offset(false, source, dest, [200.0, 90.0]);
        assert_eq!(a0, [90.0, 30.0]);
        assert_eq!(
            a1,
            [90.0, 30.0],
            "aligned offset must not depend on dab centre"
        );

        // Anchored: offset is source − center, so it changes per dab, and
        // `center + offset = source_anchor` for every dab (source pinned).
        let center = [200.0, 90.0];
        let off = clone_offset(true, source, dest, center);
        assert_eq!(off, [-100.0, -50.0]);
        assert_eq!([center[0] + off[0], center[1] + off[1]], source);
    }
}
