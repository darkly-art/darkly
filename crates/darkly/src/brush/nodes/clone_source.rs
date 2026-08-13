//! Clone Source node — samples a frozen source snapshot at an offset,
//! turning the reused `paint` terminal into a clone-stamp brush.
//!
//! ## What it does
//!
//! Instead of a flat color (`paint_color`) or a bundle texture (`image`),
//! this node's `color` output is a per-fragment sample of a **frozen
//! source snapshot**. Wire `clone_source.color → stamp.color →
//! paint.rgba` and the terminal deposits copied pixels under the cursor,
//! inheriting shape, spacing, pressure-size, flow, opacity, erase,
//! selection, preview, and undo for free. No new terminal — see the plan
//! in `docs`/PR for why `paint` is the right base.
//!
//! ## Source binding
//!
//! Compilation calls [`CompileWgslCtx::request_source_texture`], which
//! sets [`crate::brush::wgsl::CompiledBrush::samples_source`] and reserves
//! the `@group(3)` source slot. `paint`'s `flush_dabs` binds the stroke's
//! source snapshot there: the pre-stroke snapshot of the painted layer
//! (same-layer clone), or a separate snapshot frozen at stroke start when
//! a source layer is pinned or `merged` is on
//! (`StrokeBuffer::save_source_snapshot`). The hover preview binds the registry
//! `_fallback` tile (there is no snapshot at hover — the cursor thumbnail
//! comes out neutral). The bind/sample plumbing is shared with `image`
//! via [`crate::brush::wgsl::sample_graph_texture`].
//!
//! Known limits: merged sampling clips to the canvas window (the
//! composite cache is exactly window-sized, while same-layer clone can
//! reach layer content beyond it), and a *group* pinned as source falls
//! back to the painted layer (groups have no node texture; their
//! composite cache is not snapshotted).
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
//! plane/canvas pixels. The snapshot is local to the *source's* frame —
//! its plane rect arrives as the per-node `source_offset` / `source_size`
//! uniforms, seeded each pen event from
//! [`CloneState`](crate::brush::eval::CloneState) (the frozen snapshot's
//! frame when one exists, else the paint target's current extent, so
//! same-layer clone keeps tracking mid-stroke layer growth). The sample
//! UV is `(src − source_offset) / source_size`. Out-of-source UVs read
//! transparent — see [`docs/coordinate-systems.md`].

use std::sync::Arc;

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wgsl::{
    sample_graph_texture, CompileWgslCtx, InputBinding, NodeWgsl, UniformField, WgslType,
};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::gpu::preview::{PreviewBackdrop, PreviewStaging};
use crate::nodegraph::{NodeRegistration, PortDef};

pub const TYPE_ID: &str = "clone_source";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration::compute(
        NodeRegistration {
            type_id: TYPE_ID,
            category: "texture",
            display_name: "Clone Source",
            description: "Samples pixels from a set source point onto the canvas under your cursor. Set the source with the clone set-source gesture, then paint. Feed into a Stamp Tip's color input.",
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
                // Layer (0) vs merged (1). Like `mode`, an exposed Bool
                // toggle read from the port default; the engine resolves
                // it at stroke start to pick the snapshot source.
                PortDef::input("merged", BrushWireType::Bool)
                    .with_range(0.0, 1.0, 0.0)
                    .with_step(1.0)
                    .with_label("Sample Merged")
                    .with_icon("fa6-solid:layer-group")
                    .exposed()
                    .with_description(
                        "Off: clone from the source layer. On: clone from the merged \
                         canvas (all layers composited).",
                    ),
                PortDef::output("color", BrushWireType::Vec4)
                    .with_description("RGBA sampled from the frozen source snapshot at the clone offset"),
            ],
            is_gpu: false,
            is_terminal: false,
            supports_erase: true,
            preview_staging: Some(PreviewStaging {
                icon: "fa6-solid:clone",
                backdrop: PreviewBackdrop::Stripes,
            }),
        },
        || Box::new(CloneSourceEvaluator),
    )
}

/// A Bool toggle port reads "on" at or above this threshold, "off" below.
/// The single source of truth for the 0/1 split of this node's exposed
/// toggles — shared by the compile-time bake ([`mode_is_anchored`]) and the
/// engine's structural queries (`clone_source_anchored`,
/// `clone_sample_merged`) so the frontend, the stroke-start resolution, and
/// the emitted WGSL can't disagree.
pub const MODE_ANCHORED_THRESHOLD: f32 = 0.5;

/// Whether a Bool toggle port default reads as "on".
fn toggle_default_is_on(default: f32) -> bool {
    default >= MODE_ANCHORED_THRESHOLD
}

/// Whether a `mode` port default selects anchored (`true`) or aligned.
pub fn mode_default_is_anchored(mode_default: f32) -> bool {
    toggle_default_is_on(mode_default)
}

/// Whether a `merged` port default selects sample-merged (`true`) or
/// clone-from-source-layer.
pub fn merged_default_is_on(merged_default: f32) -> bool {
    toggle_default_is_on(merged_default)
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

        // Stroke-constant uniforms, seeded per pen event by the runner
        // from `CloneState` (keyed `n{id}_source_anchor` etc.): the two
        // clone anchors plus the source snapshot's plane frame. Each
        // field carries its own unseeded default (the hover preview and
        // non-clone paths have no live `CloneState`) — `source_size`
        // MUST default to `[1, 1]`: a zero size NaNs the UV, and NaN
        // passes the `uv < 0 || uv > 1` bounds check below.
        let sa_field = cctx.uniform_field_name("source_anchor");
        let da_field = cctx.uniform_field_name("dest_anchor");
        let so_field = cctx.uniform_field_name("source_offset");
        let ssz_field = cctx.uniform_field_name("source_size");
        for (field, default) in [
            (sa_field.clone(), [0.0f32, 0.0]),
            (da_field.clone(), [0.0, 0.0]),
            (so_field.clone(), [0.0, 0.0]),
            (ssz_field.clone(), [1.0, 1.0]),
        ] {
            let key = field.clone();
            wgsl.uniform_fields.push(UniformField {
                name: field,
                ty: WgslType::Vec2,
                pack: Arc::new(move |outputs, bytes| {
                    let v = outputs.get(&key).map(|s| s.as_vec2()).unwrap_or(default);
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
        // position in canvas pixels; `off` the clone offset. The source
        // frame comes from the per-node uniforms above — not
        // `u.intrinsic.layer_*`, which is the *painted* layer's frame and
        // diverges from the source under cross-layer / merged clone.
        let fn_name = cctx.ident("clone_sample");
        let sample = sample_graph_texture(slot, "uv");
        wgsl.decls = format!(
            "fn {fn_name}(tp: vec2<f32>, off: vec2<f32>) -> vec4<f32> {{\n\
             \x20   let src = tp + off;\n\
             \x20   let lo = u.{so_field};\n\
             \x20   let lsz = u.{ssz_field};\n\
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
        // Three inputs (center, mode, merged) + one output (color).
        assert_eq!(reg.node.ports.len(), 4);
        assert!(reg.node.ports.iter().any(|p| p.name == "color"));
        assert!(reg.node.ports.iter().any(|p| p.name == "center"));
        assert!(!reg.node.ports.iter().any(|p| p.name == "angle"));

        // Anchored `mode` is a Bool toggle defaulting to aligned (0.0).
        let mode = reg
            .node
            .ports
            .iter()
            .find(|p| p.name == "mode")
            .expect("mode port");
        assert_eq!(mode.wire_type, BrushWireType::Bool);
        assert_eq!(mode.value.as_f32(), 0.0);
        assert!(!mode_default_is_anchored(mode.value.as_f32()));

        // Sample-merged `merged` is a Bool toggle defaulting to off
        // (clone from the source layer).
        let merged = reg
            .node
            .ports
            .iter()
            .find(|p| p.name == "merged")
            .expect("merged port");
        assert_eq!(merged.wire_type, BrushWireType::Bool);
        assert_eq!(merged.value.as_f32(), 0.0);
        assert!(!merged_default_is_on(merged.value.as_f32()));
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
