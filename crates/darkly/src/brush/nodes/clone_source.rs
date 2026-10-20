//! Canvas Sampler node (`clone_source`): samples the canvas at an offset
//! from each pixel, turning the reused `paint` terminal into a clone-stamp
//! brush (the Snapshot source) or a smudge (the Live source).
//!
//! ## Two sources
//!
//! The `source` port is a compile-time enum picking which canvas the node
//! reads and at which offset; the port surface, the off-frame rule
//! (transparent) and the neutral preview are shared.
//!
//! - **Snapshot** (`source = 0`): the canvas frozen at stroke start, at the
//!   clone offset. Everything below about anchors and modes is this arm.
//! - **Live** (`source = 1`): the stroke as it is being painted, one dab
//!   behind along the stroke: `target_pos - motion`. It requests
//!   [`LiveSource::StrokeAppearance`], which `paint` refreshes under each
//!   dab's read region before that dab's dispatch, and reports the read
//!   reach (`|motion|` plus the filter's texel) so the region covers the
//!   sample. A dab that has not moved samples nothing (Krita's first-run
//!   rule, `kis_colorsmudgeop.cpp:196-199`): there is no "one dab ago".
//!   Filtered by hand rather than by the sampler, so a dab reads the same
//!   bytes whatever the layer's frame was when it ran (see
//!   `compile_live`).
//!
//! ## What it does
//!
//! Instead of a flat color (`paint_color`) or a bundle texture (`image`),
//! this node's `color` output is a per-fragment sample of a **frozen
//! source snapshot**. Wire `clone_source.color → stamp.color →
//! paint.rgba` and the terminal deposits copied pixels under the cursor,
//! inheriting shape, spacing, pressure-size, flow, opacity, erase,
//! selection, preview, and undo for free. No new terminal; see the plan
//! in `docs`/PR for why `paint` is the right base.
//!
//! ## Source binding
//!
//! Compilation calls [`CompileWgslCtx::request_live_texture`], which
//! reserves
//! the `@group(3)` source slot. `paint`'s `flush_dabs` binds the stroke's
//! source snapshot there: the pre-stroke snapshot of the painted layer
//! (same-layer clone), or a separate snapshot frozen at stroke start when
//! a source layer is pinned or `merged` is on
//! (`StrokeBuffer::save_source_snapshot`). The hover preview binds the registry
//! `_fallback` tile (there is no snapshot at hover, so the cursor thumbnail
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
//! - **Aligned** (`mode = 0`): `offset = source_anchor − dest_anchor`:
//!   constant for the whole stroke, so the source tracks the cursor.
//! - **Anchored** (`mode = 1`): `offset = source_anchor − center`: every
//!   dab samples the fixed `source_anchor` (the freeze-source toggle).
//!
//! `source_anchor` / `dest_anchor` are stroke-constant uniforms seeded by
//! the runner from the engine's [`CloneState`](crate::brush::eval::CloneState)
//! (set-source gesture + first-dab capture). `mode` is read from the
//! exposed port default and baked into the emitted WGSL.
//!
//! **Coordinate frame:** `target_pos`, `center`, and the anchors are all
//! plane/canvas pixels. The snapshot is local to the *source's* frame:
//! its plane rect arrives as the per-node `source_offset` / `source_size`
//! uniforms, seeded each pen event from
//! [`CloneState`](crate::brush::eval::CloneState) (the frozen snapshot's
//! frame when one exists, else the paint target's current extent, so
//! same-layer clone keeps tracking mid-stroke layer growth). The sample
//! UV is `(src − source_offset) / source_size`. Out-of-source UVs read
//! transparent; see [`docs/coordinate-systems.md`].

use std::sync::Arc;

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::input_value::InputValue;
use crate::brush::node::BrushNodeRegistration;
use crate::brush::paint_info::STATIONARY_MOTION_PX;
use crate::brush::texture_source::LiveSource;
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
            display_name: "Canvas Sampler",
            description: "Samples the canvas under your cursor at an offset. Snapshot: copies pixels from a set source point (clone; set the source with the clone set-source gesture, then paint). Live: pulls the stroke being painted along the pen's motion (smudge). Feed into a Stamp Tip's color input.",
            ports: vec![
                PortDef::input("source", BrushWireType::Enum)
                    .with_enum_options(["Snapshot", "Live"])
                    .with_value(InputValue::Int(SOURCE_SNAPSHOT))
                    .with_label("Source")
                    .with_description(
                        "Snapshot: the canvas frozen at stroke start (clone). \
                         Live: the stroke as it is being painted (smudge).",
                    ),
                PortDef::input("motion", BrushWireType::Vec2)
                    .with_visible_when("source", [SOURCE_LIVE])
                    .with_description(
                        "Per-dab motion in canvas pixels (wire Pen Input → Motion). \
                         Live samples one dab behind along this vector.",
                    ),
                PortDef::input("center", BrushWireType::Vec2)
                    .with_visible_when("source", [SOURCE_SNAPSHOT])
                    .with_description("Per-dab pen position in canvas pixels (wire Pen Input → Position)."),
                // Aligned (0) vs anchored (1). Exposed as a Bool toggle;
                // read from the port default and baked into the WGSL
                // offset expression at compile time.
                PortDef::input("mode", BrushWireType::Bool)
                    .with_range(0.0, 1.0, 0.0)
                    .with_step(1.0)
                    .with_label("Anchored")
                    .with_icon("fa6-solid:anchor")
                    .with_visible_when("source", [SOURCE_SNAPSHOT])
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
                    .with_visible_when("source", [SOURCE_SNAPSHOT])
                    .exposed()
                    .with_description(
                        "Off: clone from the source layer. On: clone from the merged \
                         canvas (all layers composited).",
                    ),
                PortDef::output("color", BrushWireType::Vec4)
                    .with_description("Straight RGBA sampled from the chosen source at the offset"),
                PortDef::output("coverage", BrushWireType::Scalar).with_description(
                    "1 where there is something under the finger to pick up, 0 where there \
                     is not: a touch that has not moved yet, or one reaching off the layer. \
                     Multiply into Paint's Coverage so a finger that picks up nothing \
                     changes nothing.",
                ),
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

/// `source` index of the Snapshot arm: the canvas frozen at stroke start.
pub const SOURCE_SNAPSHOT: i32 = 0;
/// `source` index of the Live arm: the stroke as it is being painted.
pub const SOURCE_LIVE: i32 = 1;

/// Whether a `source` index selects the Live arm. The one reading of the
/// enum, shared by the compile-time bake, the per-dab read reach and the
/// engine's structural query, so the three cannot disagree.
pub fn source_index_is_live(index: i32) -> bool {
    index == SOURCE_LIVE
}

/// The graph's first sampler on the Snapshot arm: the node a clone stroke
/// anchors and whose `mode` / `merged` toggles the engine reads. `None`
/// when the graph has no sampler or only Live ones, which need no source
/// point.
pub fn snapshot_sampler(
    graph: &crate::nodegraph::Graph<BrushWireType>,
) -> Option<&crate::nodegraph::NodeInstance<BrushWireType>> {
    graph
        .nodes()
        .values()
        .find(|n| n.type_id == TYPE_ID && !ports_select_live(&n.ports))
}

/// Whether a sampler instance's authored `source` selects the Live arm.
fn ports_select_live(ports: &[PortDef<BrushWireType>]) -> bool {
    ports
        .iter()
        .find(|p| p.name == "source")
        .is_some_and(|p| source_index_is_live(p.value.as_enum_index()))
}

/// Whether a graph needs a set-source anchor before it can paint: it has
/// a sampler on the Snapshot arm.
pub fn graph_needs_source(graph: &crate::nodegraph::Graph<BrushWireType>) -> bool {
    snapshot_sampler(graph).is_some()
}

/// A Bool toggle port reads "on" at or above this threshold, "off" below.
/// The single source of truth for the 0/1 split of this node's exposed
/// toggles, shared by the compile-time bake ([`mode_is_anchored`]) and the
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
    /// CPU evaluation returns a neutral grey: `clone_source` is only
    /// meaningful per-fragment in the compiled shader, and the per-dab
    /// CPU dispatch path is dead for compiled-WGSL brushes. Mirrors
    /// `image`'s placeholder so mixed CPU/compiled graphs don't `NaN`
    /// through `color`.
    fn evaluate_cpu(&self, _ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        vec![
            ("color".into(), ScalarValue::Vec4([0.5, 0.5, 0.5, 1.0])),
            ("coverage".into(), ScalarValue::Scalar(1.0)),
        ]
    }

    /// Both sources sample the canvas, so both stage their previews; the
    /// Live source smears rather than copies and has its own glyph.
    fn preview_staging(
        &self,
        ports: &[PortDef<BrushWireType>],
        declared: Option<PreviewStaging>,
    ) -> Option<PreviewStaging> {
        if ports_select_live(ports) {
            Some(PreviewStaging {
                icon: "mdi:fingerprint",
                backdrop: PreviewBackdrop::Stripes,
            })
        } else {
            declared
        }
    }

    /// The Live arm reads `|motion|` beyond the dab's footprint, plus one
    /// texel for the bilinear filter's reach past `target_pos - motion`;
    /// the Snapshot arm reads a frozen texture and needs no refresh.
    fn read_reach(&self, ctx: &EvalContext) -> [f32; 2] {
        if !source_index_is_live(ctx.input("source").as_f32() as i32) {
            return [0.0, 0.0];
        }
        let m = ctx.input("motion").as_vec2();
        [m[0].abs().ceil() + 1.0, m[1].abs().ceil() + 1.0]
    }

    fn compile_wgsl(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        if !cctx.consumed_outputs.contains("color") && !cctx.consumed_outputs.contains("coverage") {
            // Nothing downstream samples the source, so don't reserve the
            // binding or emit the sample.
            return Ok(wgsl);
        }
        let out = cctx.ident("clone_c");
        let coverage = cctx.ident("clone_cov");
        if source_index_is_live(cctx.input("source").enum_index()) {
            compile_live(cctx, &mut wgsl, &out, &coverage);
        } else {
            compile_snapshot(cctx, &mut wgsl, &out);
            // The frozen source always exists; off-frame is transparent, not
            // absent, so a clone through Move replaces with transparency.
            wgsl.body.push_str(&format!("    let {coverage} = 1.0;\n"));
        }
        wgsl.outputs.insert("color".into(), out);
        wgsl.outputs.insert("coverage".into(), coverage);
        Ok(wgsl)
    }

    /// Preview-mode body. At hover there is no frozen source snapshot to
    /// sample (the preview pipeline binds the registry `_fallback` tile to
    /// the declared source slot), so sampling it would stamp a meaningless
    /// flat tile. Instead emit an opaque neutral constant for the `color`
    /// output: the terminal deposits it through the brush tip, so the
    /// cursor preview shows the tip *shape* in neutral grey (matching
    /// Krita's `kis_duplicateop` and GIMP's source-tool outline). The
    /// output name matches `compile_wgsl`'s so the terminal's preview body,
    /// which resolves its `color` wire against the stroke pass's output
    /// expressions, still finds the variable.
    fn compile_cursor_preview_body(&self, cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        let mut wgsl = NodeWgsl::default();
        if !cctx.consumed_outputs.contains("color") && !cctx.consumed_outputs.contains("coverage") {
            return Ok(wgsl);
        }
        let out = cctx.ident("clone_c");
        wgsl.body = format!("    let {out} = vec4<f32>(0.6, 0.6, 0.6, 1.0);\n");
        wgsl.outputs.insert("color".into(), out);
        wgsl.outputs.insert("coverage".into(), "1.0".into());
        Ok(wgsl)
    }
}

/// The Snapshot arm's sample helper: `src` (plane pixels) into the frame
/// `lo + [0, lsz)`, transparent outside it. `graph_smp` repeats, so within
/// half a texel of the frame's border the filter would wrap to the
/// opposite edge; clamping to texel centres keeps it inside the texture.
fn frame_sample_decl(fn_name: &str, slot: u32) -> String {
    let sample = sample_graph_texture(slot, "clamp(uv, half, vec2<f32>(1.0) - half)");
    format!(
        "fn {fn_name}(src: vec2<f32>, lo: vec2<f32>, lsz: vec2<f32>) -> vec4<f32> {{\n\
         \x20   let uv = (src - lo) / lsz;\n\
         \x20   if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {{\n\
         \x20       return vec4<f32>(0.0, 0.0, 0.0, 0.0);\n\
         \x20   }}\n\
         \x20   let half = vec2<f32>(0.5) / lsz;\n\
         \x20   return {sample};\n\
         }}\n"
    )
}

/// The Snapshot arm: the frozen source snapshot at the clone offset.
fn compile_snapshot(cctx: &CompileWgslCtx, wgsl: &mut NodeWgsl, out: &str) {
    let slot = cctx.request_live_texture(LiveSource::StrokeSnapshot);

    // Stroke-constant uniforms, seeded per pen event by the runner
    // from `CloneState` (keyed `n{id}_source_anchor` etc.): the two
    // clone anchors plus the source snapshot's plane frame. Each
    // field carries its own unseeded default (the hover preview and
    // non-clone paths have no live `CloneState`); `source_size`
    // MUST default to `[1, 1]`: a zero size NaNs the UV, and NaN
    // passes the `uv < 0 || uv > 1` bounds check.
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

    // The source frame comes from the per-node uniforms above, not
    // `u.intrinsic.layer_*`, which is the *painted* layer's frame and
    // diverges from the source under cross-layer / merged clone.
    let fn_name = cctx.ident("clone_sample");
    wgsl.decls = frame_sample_decl(&fn_name, slot);
    wgsl.body =
        format!("    let {out} = {fn_name}(target_pos + {offset}, u.{so_field}, u.{ssz_field});\n");
}

/// The Live arm: the stroke's appearance one dab behind, at
/// `target_pos - motion`, bilinear in straight alpha. The appearance
/// mirror is layer-sized in the paint target's frame, the frame the
/// intrinsic header carries.
///
/// Filtered by hand from four texel loads rather than through
/// `graph_smp`: the sample point is split so its integer texel comes from
/// the pixel's layer-local texel and its fractional weight from `motion`
/// alone. The result then does not depend on the layer's frame, which the
/// hardware filter's fixed-point weights do (a dab re-rendered after the
/// layer grew would read a different LSB), and the taps clamp to the
/// layer's edge, so the filter never reaches the opposite edge, whose
/// mirror texels this dab never refreshed. The taps are premultiplied
/// before they are mixed and the result is returned straight: the mirror
/// is straight alpha, and mixing straight texels across a transparent
/// edge darkens the colour instead of thinning the coverage.
///
/// Every symbol the helper names (`u`, `graph_tex_N`) is declared in both
/// shader variants, so the shared decls compile into the preview module,
/// whose body never calls it.
fn compile_live(cctx: &CompileWgslCtx, wgsl: &mut NodeWgsl, out: &str, coverage: &str) {
    let slot = cctx.request_live_texture(LiveSource::StrokeAppearance);
    let live_fn = cctx.ident("clone_live");
    let sample_ty = cctx.ident("CloneLiveSample");
    let motion = cctx.input("motion").as_vec2();
    let tex = format!("graph_tex_{slot}");
    let pre_fn = cctx.ident("clone_premul");
    // `coverage` is 0 where there is no sample (a dab that has not moved,
    // a sample point off the layer) and 1 where there is one, transparent
    // or not: the difference between a finger that brings nothing and one
    // that brings bare paper.
    wgsl.decls = format!(
        "struct {sample_ty} {{ color: vec4<f32>, coverage: f32 }}\n\
         fn {pre_fn}(c: vec4<f32>) -> vec4<f32> {{\n\
         \x20   return vec4<f32>(c.rgb * c.a, c.a);\n\
         }}\n\
         fn {live_fn}(tp: vec2<f32>, motion: vec2<f32>) -> {sample_ty} {{\n\
         \x20   let none = {sample_ty}(vec4<f32>(0.0, 0.0, 0.0, 0.0), 0.0);\n\
         \x20   if (abs(motion.x) < {STATIONARY_MOTION_PX:.6} && abs(motion.y) < {STATIONARY_MOTION_PX:.6}) {{\n\
         \x20       return none;\n\
         \x20   }}\n\
         \x20   // `tp` is a pixel centre, so in texel space (centres at\n\
         \x20   // integers) the sample point is the pixel's texel minus motion.\n\
         \x20   let back = floor(-motion);\n\
         \x20   let w = -motion - back;\n\
         \x20   let base = vec2<i32>(floor(tp)) - u.intrinsic.layer_offset + vec2<i32>(back);\n\
         \x20   let size = vec2<i32>(u.intrinsic.layer_size);\n\
         \x20   let at = vec2<f32>(base) + w;\n\
         \x20   if (any(at < vec2<f32>(-0.5)) || any(at > vec2<f32>(size) - vec2<f32>(0.5))) {{\n\
         \x20       return none;\n\
         \x20   }}\n\
         \x20   let hi = size - vec2<i32>(1);\n\
         \x20   let p0 = clamp(base, vec2<i32>(0), hi);\n\
         \x20   let p1 = clamp(base + vec2<i32>(1), vec2<i32>(0), hi);\n\
         \x20   // The mirror is straight alpha; interpolate premultiplied so a\n\
         \x20   // transparent neighbour dilutes coverage, not colour, then\n\
         \x20   // return straight for the stamp.\n\
         \x20   let top = mix({pre_fn}(textureLoad({tex}, p0, 0)), {pre_fn}(textureLoad({tex}, vec2<i32>(p1.x, p0.y), 0)), w.x);\n\
         \x20   let bottom = mix({pre_fn}(textureLoad({tex}, vec2<i32>(p0.x, p1.y), 0)), {pre_fn}(textureLoad({tex}, p1, 0)), w.x);\n\
         \x20   let c = mix(top, bottom, w.y);\n\
         \x20   if (c.a <= 0.0) {{\n\
         \x20       return {sample_ty}(vec4<f32>(0.0, 0.0, 0.0, 0.0), 1.0);\n\
         \x20   }}\n\
         \x20   return {sample_ty}(vec4<f32>(c.rgb / c.a, c.a), 1.0);\n\
         }}\n"
    );
    let sample = cctx.ident("clone_s");
    wgsl.body = format!(
        "    let {sample} = {live_fn}(target_pos, {motion});\n\
         \x20   let {out} = {sample}.color;\n\
         \x20   let {coverage} = {sample}.coverage;\n"
    );
}

/// CPU-side spec of the clone offset formula, mirrored by the WGSL
/// emitted in [`CloneSourceEvaluator::compile_wgsl`]. This is the one
/// unavoidable CPU↔WGSL duplication (the same kind `IntrinsicUniforms`
/// carries), kept tiny and covered by a unit test so the two-mode
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
        // Five inputs (source, motion, center, mode, merged) + two outputs
        // (color, coverage).
        assert_eq!(reg.node.ports.len(), 7);
        let port = |name: &str| {
            reg.node
                .ports
                .iter()
                .find(|p| p.name == name)
                .unwrap_or_else(|| panic!("{name} port"))
        };
        assert_eq!(port("source").wire_type, BrushWireType::Enum);
        assert_eq!(port("source").value.as_enum_index(), SOURCE_SNAPSHOT);
        assert_eq!(port("motion").wire_type, BrushWireType::Vec2);
        let visible = |name: &str| port(name).visible_when.clone();
        assert_eq!(
            visible("motion"),
            Some(("source".to_string(), vec![SOURCE_LIVE]))
        );
        for name in ["center", "mode", "merged"] {
            assert_eq!(
                visible(name),
                Some(("source".to_string(), vec![SOURCE_SNAPSHOT])),
                "{name} belongs to the Snapshot arm"
            );
        }
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

    /// The Live arm reaches `ceil(|m|) + 1` past the footprint per axis,
    /// the filter's texel included; the Snapshot arm reaches nothing.
    #[test]
    fn read_reach_covers_the_motion_and_the_filter_on_the_live_arm_only() {
        let reach = |source: i32, motion: [f32; 2]| {
            let ports: Vec<_> = register()
                .node
                .ports
                .into_iter()
                .map(|p| match p.name.as_str() {
                    "source" => p.with_value(InputValue::Int(source)),
                    "motion" => p.with_value(InputValue::Vec2(motion)),
                    _ => p,
                })
                .collect();
            let node_id = crate::nodegraph::NodeId("sampler".into());
            CloneSourceEvaluator.read_reach(&EvalContext {
                input_slots: &[],
                input_values: &[],
                port_defs: &ports,
                lut: None,
                stroke_seed: 0,
                dab_index: 0,
                base_size: 1.0,
                dabs_per_pass: 1.0,
                dpi: crate::document::REFERENCE_DPI,
                node_id: &node_id,
            })
        };
        assert_eq!(reach(SOURCE_LIVE, [2.5, -3.0]), [4.0, 4.0]);
        assert_eq!(reach(SOURCE_LIVE, [0.0, 0.0]), [1.0, 1.0]);
        assert_eq!(reach(SOURCE_SNAPSHOT, [2.5, -3.0]), [0.0, 0.0]);
    }

    #[test]
    fn source_index_reads_live_only_at_one() {
        assert!(!source_index_is_live(SOURCE_SNAPSHOT));
        assert!(source_index_is_live(SOURCE_LIVE));
        assert!(!source_index_is_live(2));
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
