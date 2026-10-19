//! A brush stroke's engine-side lifecycle.
//!
//! While the pen is down a stroke is a [`BrushStroke`]: the stroke engine
//! that turns pen events into dabs, the stroke buffer they render into, and
//! the [`StrokeConditions`] it started under. At pen-up it becomes the
//! [`LastStroke`], which keeps the raw events, the conditions and the buffer
//! until the next action settles it into an undo entry. Until then the
//! artist can edit it: in stroke edit, every change to the brush or the
//! foreground colour redraws it from its events with the brush as it then
//! is.
//!
//! The undo entry is committed from the stroke's own pre-stroke snapshot,
//! over the union of its dabs' write footprints: the rect every terminal
//! publishes for each dab it queues (`DabBatch::record_dab_footprint`), and
//! the same one the checkpoint ring restores by. Settling needs no readback,
//! and nothing that runs between pen-up and the settle can change what the
//! entry holds.

use darkly_macros::handlers;

use super::rendering::commit_undo_entry;
use super::DarklyEngine;
use crate::brush::eval::BrushGraphRunner;
use crate::brush::gpu_context::StrokeGpuSetup;
use crate::brush::paint_info::PaintInformation;
use crate::brush::pipeline::BrushPipelines;
use crate::brush::spacing::SpacingConfig;
use crate::brush::stroke_buffer::StrokeBuffer;
use crate::brush::stroke_engine::StrokeEngine;
use crate::coord::CanvasRect;
use crate::document::{canvas_per_reference_px, Document};
use crate::gpu::compositor::Compositor;
use crate::gpu::context::GpuContext;
use crate::gpu::paint_target::GpuPaintTarget;
use crate::layer::LayerId;
use crate::undo::GpuRegionAction;

/// What a stroke is painted under that comes from outside the brush graph,
/// frozen when the stroke starts so that a re-render of the same events
/// reproduces it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StrokeConditions {
    /// Foreground colour as picked, before the seed's colour jitter.
    pub color: [f32; 4],
    /// Seed for the brush's `random` nodes and for the colour jitter.
    pub seed: u32,
    /// Canvas pixels per CSS pixel: the stabilizer and the divergence
    /// tolerance measure on screen, at the zoom the stroke was painted at.
    pub canvas_per_css_px: f32,
    /// View rotation in radians: dab orientation is screen-relative.
    pub view_rotation: f32,
}

/// The stroke the pen is painting. The engine and its buffer live and die
/// together, so a stroke engine never exists without a scratch to render
/// to.
pub(crate) struct BrushStroke {
    pub engine: StrokeEngine,
    pub buffer: StrokeBuffer,
    pub conditions: StrokeConditions,
}

/// The most recent brush stroke, kept from pen-up until the next action
/// settles it.
pub(crate) struct LastStroke {
    layer_id: LayerId,
    /// The raw pen events, as the stroke engine recorded them.
    events: Vec<PaintInformation>,
    conditions: StrokeConditions,
    /// Scratch plus the pre-stroke snapshot: what a redraw composites onto
    /// and what the undo entry is committed from.
    buffer: StrokeBuffer,
    /// Union of the write footprints of the latest rendering's dabs, in
    /// canvas pixels: the undo entry's rect. `None` when no dab wrote.
    footprint: Option<CanvasRect>,
    /// Whether the artist is editing it, so brush and colour changes
    /// redraw it.
    editing: bool,
}

impl LastStroke {
    pub(crate) fn editing(&self) -> bool {
        self.editing
    }
}

/// The [`StrokeGpuSetup`] for painting `layer_id` with the session's
/// selection and `blend_mode`, under `view_rotation`. Takes the engine's
/// fields rather than the engine, so the setup borrows only those and the
/// caller can mutate the rest (the perf counters, the checkpoint ring)
/// between submissions. `None` when the node has no texture.
pub(crate) fn stroke_gpu_setup<'a>(
    gpu: &'a GpuContext,
    pipelines: &'a BrushPipelines,
    compositor: &'a Compositor,
    doc: &Document,
    blend_mode: u32,
    layer_id: LayerId,
    view_rotation: f32,
) -> Option<StrokeGpuSetup<'a>> {
    let paint_target =
        GpuPaintTarget::from_node(compositor.node_texture(layer_id)?, doc.canvas_rect());
    let selection_bind_group = doc
        .selection_active()
        .then(|| {
            compositor
                .selection_state()
                .map(|s| s.selection_bind_group())
        })
        .flatten()
        .unwrap_or(pipelines.default_selection_bind_group());
    Some(StrokeGpuSetup {
        device: &gpu.device,
        queue: &gpu.queue,
        pipelines,
        selection_bind_group,
        canvas_width: compositor.canvas_width(),
        canvas_height: compositor.canvas_height(),
        canvas_origin: [doc.canvas_origin.x, doc.canvas_origin.y],
        // Applies at commit (paint vs. erase). The per-dab pass never
        // branches on it: the scratch is a coverage accumulator under
        // whichever law the brush's `buildup` port selected.
        blend_mode,
        view_rotation,
        dpi_factor: canvas_per_reference_px(doc.dpi),
        paint_target,
    })
}

impl DarklyEngine {
    /// Read the stabilize strength from the brush_settings node's "stabilize"
    /// port default in the active brush graph. Returns 0.0 if not found.
    fn active_stabilize_strength(&self) -> f32 {
        use crate::brush::state::BrushState;
        let tool = self.tool_session.read();
        let brush = tool
            .get::<BrushState>()
            .expect("BrushState registered at session init");
        crate::brush::nodes::brush_settings::read_scalar_input(&brush.graph, "stabilize")
            .unwrap_or(0.0)
    }

    /// Read the global prediction horizon (ms of look-ahead) from config.
    /// `0` = off. Prediction is a global editor preference, not per-brush:
    /// see `config/sections/input.rs`.
    fn prediction_horizon_ms(&self) -> f32 {
        crate::config::get_f64("input.predictionHorizon") as f32
    }

    /// Build the `SpacingConfig` for the active brush graph. Reads brush_settings
    /// port defaults (the same source the editor preview reads), so a real
    /// stroke and its preview stamp at the same intervals.
    fn active_spacing_config(&self) -> SpacingConfig {
        use crate::brush::state::BrushState;
        let tool = self.tool_session.read();
        let brush = tool
            .get::<BrushState>()
            .expect("BrushState registered at session init");
        crate::brush::nodes::brush_settings::spacing_config(
            &brush.graph,
            canvas_per_reference_px(self.doc.dpi),
        )
    }

    /// Read the active brush's base size from its `brush_settings.size` knob
    /// (the same out-of-band source the editor preview uses), so a real stroke and
    /// its preview render dabs at the same size.
    fn active_base_size(&self) -> f32 {
        use crate::brush::state::BrushState;
        let tool = self.tool_session.read();
        let brush = tool
            .get::<BrushState>()
            .expect("BrushState registered at session init");
        crate::brush::nodes::brush_settings::base_size(&brush.graph)
    }

    /// Read the active brush's stamp turn rate from its
    /// `brush_settings.stamp_angle_rate` knob: radians per brush diameter of
    /// travel, read out-of-band at stroke start like spacing and base size.
    fn active_stamp_angle_rate(&self) -> f32 {
        use crate::brush::state::BrushState;
        let tool = self.tool_session.read();
        let brush = tool
            .get::<BrushState>()
            .expect("BrushState registered at session init");
        crate::brush::nodes::brush_settings::stamp_angle_rate(&brush.graph)
    }

    /// The conditions a stroke starting now is painted under: the
    /// foreground colour, a seed (the pinned one, else fresh from the
    /// clock), and the view's scale and rotation.
    pub(crate) fn current_stroke_conditions(&self) -> StrokeConditions {
        let view = &self.view_params;
        StrokeConditions {
            color: self.foreground_color,
            seed: self.stroke_seed.unwrap_or_else(StrokeEngine::random_seed),
            // Canvas pixels per CSS pixel of pen travel at this view: the
            // stack measures resample spacing on screen, so smoothing feels
            // the same at every zoom and on every display.
            canvas_per_css_px: view.dpr / view.zoom.max(f32::MIN_POSITIVE),
            view_rotation: view.rotation,
        }
    }

    /// Compile the active brush graph, or `None` (logged) when it does not
    /// compile. The session read guard is held only for the compile and
    /// dropped before any GPU work, so other engines (multi-tab) can take
    /// the lock.
    pub(crate) fn compile_active_brush(&self) -> Option<BrushGraphRunner> {
        use crate::brush::state::BrushState;
        let tool = self.tool_session.read();
        let brush = tool
            .get::<BrushState>()
            .expect("BrushState registered at session init");
        crate::brush::compile_graph(&brush.graph)
            .map_err(|e| log::error!("brush graph compilation failed: {e:?}"))
            .ok()
    }

    /// Whether `runner` has anything to paint: a brush that samples a source
    /// needs a set-source anchor, and with none set it has nothing to copy.
    /// The frontend arms the set-source gesture and shows a hint in that
    /// state.
    pub(crate) fn brush_can_paint(&self, runner: &BrushGraphRunner) -> bool {
        !runner.samples_source() || self.clone_source_anchor.is_some()
    }

    /// A stroke engine for `runner` under `conditions`, with the active
    /// brush's out-of-band settings (spacing, base size, stabilizer strength,
    /// stamp turn rate) and the session's clone anchor.
    pub(crate) fn build_stroke_engine(
        &self,
        runner: BrushGraphRunner,
        conditions: &StrokeConditions,
    ) -> StrokeEngine {
        // Derive stabilizer from the pen_input node's "stabilize" port.
        let strength = self.active_stabilize_strength();
        let stabilizer_config = if strength > 0.0 {
            crate::brush::stabilizer::StabilizerConfig {
                algorithm: "laplacian".into(),
                params: vec![crate::gpu::params::ParamValue::Float(strength)],
            }
        } else {
            crate::brush::stabilizer::StabilizerConfig::default()
        };
        let stabilizer = crate::brush::stabilizer::stroke_stabilizer_stack(
            &self.stabilizer_registry,
            &stabilizer_config,
            self.prediction_horizon_ms(),
            conditions.canvas_per_css_px,
        );
        // What counts as a move is measured on screen too.
        let divergence_epsilon =
            crate::brush::stroke_engine::DIVERGENCE_EPSILON * conditions.canvas_per_css_px;
        #[cfg(any(test, feature = "testing"))]
        let divergence_epsilon = self.test_divergence_epsilon.unwrap_or(divergence_epsilon);

        // The colour selector's jitter is a pure function of the stroke
        // seed, so a pinned seed replays the same colour.
        let color = crate::brush::color_jitter::ColorJitter::from_config()
            .apply(conditions.color, conditions.seed);
        StrokeEngine::new(
            runner,
            color,
            self.active_spacing_config(),
            self.active_base_size(),
            stabilizer,
            divergence_epsilon,
            self.clone_source_anchor.map(|p| [p.x as f32, p.y as f32]),
            conditions.seed,
            self.active_stamp_angle_rate(),
            self.doc.dpi,
        )
    }

    /// A stroke buffer for `engine` over `layer_id`'s paint target: sized to
    /// the target, its pre-stroke snapshot holding the target's pixels now,
    /// and, for a brush that samples a source, that source frozen. `None`
    /// when the target has no texture.
    pub(crate) fn open_stroke_buffer(
        &mut self,
        layer_id: LayerId,
        engine: &StrokeEngine,
    ) -> Option<StrokeBuffer> {
        // Sized to the layer's bounds: for paste-extent layers larger than
        // the canvas, dabs landing on off-canvas pixels are saved and
        // restored correctly on undo.
        let layer_tex = self.compositor.node_texture(layer_id)?;
        let layer_extent = layer_tex.layer_extent();
        // The terminal decides what its scratch holds: colour for most
        // brushes, a displacement field for liquify.
        let mut buffer = StrokeBuffer::new(
            &self.gpu.device,
            layer_extent.width,
            layer_extent.height,
            &self.brush_pipelines,
            engine.scratch_format(),
            engine.dab_pass(),
        );
        let paint_target = GpuPaintTarget::from_node(layer_tex, self.doc.canvas_rect());
        self.gpu.encode("stroke-buffer-init", |encoder| {
            buffer.save_pre_stroke(
                &self.gpu.device,
                encoder,
                &self.brush_pipelines,
                &paint_target,
            );
        });
        self.capture_source_snapshot(layer_id, &mut buffer, engine.samples_source());
        Some(buffer)
    }

    /// Freeze the source a brush that samples one reads into `buffer`, or
    /// drop a frozen one when the brush reads the painted layer or nothing.
    ///
    /// Resolution is merged, then pinned layer, then same-layer. The first
    /// two freeze a separate snapshot (required for merged, where the
    /// destination would feed back mid-stroke; safe against per-frame
    /// procedural sources and mid-stroke disposal; deterministic under
    /// stabilizer divergence rewind). Same-layer reads the pre-stroke
    /// snapshot, which is the self-clone freeze.
    pub(crate) fn capture_source_snapshot(
        &mut self,
        layer_id: LayerId,
        buffer: &mut StrokeBuffer,
        samples_source: bool,
    ) {
        buffer.clear_source_snapshot();
        if !samples_source {
            return;
        }
        // "Sample Merged": a structural port-default read, not a compile.
        let sample_merged = self.clone_sample_merged();
        if sample_merged {
            // Merged freezes the root composite, so make sure it is fresh
            // (a no-op when clean).
            self.compositor.render_offscreen(
                &self.gpu.device,
                &self.gpu.queue,
                &mut self.doc,
                self.isolated_node,
            );
        }
        let canvas_rect = self.doc.canvas_rect();
        let source = if sample_merged {
            // The composite cache is straight-alpha Rgba8Unorm with content
            // at the top-left, exactly canvas-window-sized.
            Some(GpuPaintTarget::from_canvas_texture(
                self.compositor.composited_texture(),
                self.compositor.composited_view(),
                wgpu::TextureFormat::Rgba8Unorm,
                canvas_rect,
            ))
        } else {
            // Pinned layer, validated document-side first: the document is
            // authoritative, and layer removal is orphan-keep (the entity
            // parks in the slotmap for undo, and the compositor keeps its
            // texture until the next sync), so only the parent link says
            // whether the pin is still in the tree. A detached pin or a group
            // (which has no `node_textures` entry) falls back to the painted
            // layer, matching Krita's saved-node fallback. Cloning *from* a
            // group's composite is a known gap: it would need a group-cache
            // snapshot like merged's.
            self.clone_source_layer
                .filter(|&sid| sid != layer_id && self.doc.parent_of(sid).is_some())
                .and_then(|sid| self.compositor.node_texture(sid))
                .map(|src_tex| GpuPaintTarget::from_node(src_tex, canvas_rect))
        };
        if let Some(source) = source {
            self.gpu.encode("clone-source-snapshot", |encoder| {
                buffer.save_source_snapshot(
                    &self.gpu.device,
                    encoder,
                    &self.brush_pipelines,
                    &source,
                );
            });
        }
    }

    /// Keep the stroke that just ended on `layer_id` as the last stroke. The
    /// document is marked changed now, at pen-up, because the stroke's undo
    /// entry is pushed only when it settles.
    pub(crate) fn keep_last_stroke(&mut self, layer_id: LayerId, stroke: BrushStroke) {
        self.settle_last_stroke();
        let BrushStroke {
            engine,
            buffer,
            conditions,
        } = stroke;
        let footprint = engine.save_points.full_bbox();
        self.last_stroke = Some(LastStroke {
            layer_id,
            events: engine.end().events,
            conditions,
            buffer,
            footprint,
            editing: false,
        });
        self.doc.mark_changed();
    }

    /// Settle the last stroke: commit its undo entry, from its pre-stroke
    /// snapshot over its footprint, and drop it. Runs before anything that
    /// pushes another undo entry or edits the document, so entries keep the
    /// order their edits happened in.
    pub(crate) fn settle_last_stroke(&mut self) {
        let Some(stroke) = self.last_stroke.take() else {
            return;
        };
        let Some(footprint) = stroke.footprint else {
            return;
        };
        let Some(layer_tex) = self.compositor.node_texture(stroke.layer_id) else {
            return;
        };
        let target = GpuPaintTarget::from_node(layer_tex, self.doc.canvas_rect());
        // Each dab's footprint is clamped to the target as it stood then,
        // and a target only grows while a stroke is kept, so this changes
        // nothing in practice; it keeps the copy inside the texture
        // regardless.
        let Some(rect) = footprint.intersect(target.canvas_extent()) else {
            return;
        };
        let entry = commit_undo_entry(
            &self.gpu,
            &mut self.readbacks,
            "brush-stroke-undo",
            |encoder| {
                stroke.buffer.commit_pre_stroke_undo(
                    &self.gpu.device,
                    encoder,
                    &self.brush_pipelines,
                    stroke.layer_id,
                    &target,
                    rect,
                )
            },
        );
        // The change was recorded at pen-up and at every redraw, so the
        // push is bookkeeping: a save in between keeps the document clean.
        self.push_recorded_undo(Box::new(GpuRegionAction::new(entry)));
    }

    /// Redraw the last stroke from its raw events with the brush, the
    /// foreground colour and the clone source as they are now, keeping the
    /// rest of its conditions. A no-op unless the artist is editing it.
    ///
    /// The new rendering lands the way a stroke's own flush lands: the
    /// terminal's whole-scratch prologue, every dab, then the commit, which
    /// composites the scratch onto the pre-stroke snapshot over the whole
    /// target, so the previous rendering needs no separate erase.
    pub(crate) fn redraw_last_stroke(&mut self) {
        if !self.last_stroke.as_ref().is_some_and(LastStroke::editing) {
            return;
        }
        let Some(runner) = self.compile_active_brush() else {
            return;
        };
        if !self.brush_can_paint(&runner) {
            return;
        }
        let Some(mut stroke) = self.last_stroke.take() else {
            return;
        };
        stroke.conditions.color = self.foreground_color;
        let mut engine = self.build_stroke_engine(runner, &stroke.conditions);
        for event in &stroke.events {
            engine.stabilize(*event);
        }
        stroke.buffer.ensure_scratch(
            &self.gpu.device,
            &self.brush_pipelines,
            engine.scratch_format(),
            engine.dab_pass(),
        );

        let mut perf = crate::brush::gpu_context::BrushPerfCounters::default();
        if engine.samples_source() {
            // A merged source reads the composite, which must not hold the
            // rendering being replaced: return the target to its pre-stroke
            // pixels (the prologue with no dabs, then the commit) before the
            // source is frozen, as it was when the stroke began.
            if let Some(setup) = self.redraw_gpu_setup(&stroke) {
                let mut ctx = setup.ctx(&mut stroke.buffer, "stroke-edit-restore");
                engine.begin_stroke(&mut ctx, None);
                engine.commit(&mut ctx);
                perf += ctx.submit_final();
            }
            self.compositor.mark_node_pixels_dirty(stroke.layer_id);
        }
        self.capture_source_snapshot(stroke.layer_id, &mut stroke.buffer, engine.samples_source());

        if let Some(setup) = self.redraw_gpu_setup(&stroke) {
            // The frozen source's frame when one exists, else the target's.
            engine.set_clone_source_frame(
                stroke
                    .buffer
                    .source_snapshot_frame()
                    .unwrap_or_else(|| setup.paint_target.canvas_extent()),
            );
            self.brush_pipelines.reset_uniform_rings();
            let mut ctx = setup.ctx(&mut stroke.buffer, "stroke-edit-redraw");
            engine.render_whole(&mut ctx);
            perf += ctx.submit_final();
            stroke.footprint = engine.save_points.full_bbox();
        }
        self.brush_perf += perf;
        self.compositor.mark_node_pixels_dirty(stroke.layer_id);
        self.doc.mark_changed();
        self.last_stroke = Some(stroke);
    }

    /// The GPU setup a redraw of `stroke` renders with: the stroke's own
    /// view rotation, the session's selection and blend mode.
    fn redraw_gpu_setup(&self, stroke: &LastStroke) -> Option<StrokeGpuSetup<'_>> {
        stroke_gpu_setup(
            &self.gpu,
            &self.brush_pipelines,
            &self.compositor,
            &self.doc,
            self.brush_blend_mode,
            stroke.layer_id,
            stroke.conditions.view_rotation,
        )
    }
}

#[handlers]
impl DarklyEngine {
    /// Enter stroke edit on the last brush stroke: until it settles, every
    /// change to the brush or the foreground colour redraws it from its raw
    /// pen events with the brush as it then is. Redraws once on entry, so a
    /// change made since pen-up applies too. `false` when there is no stroke
    /// to edit.
    #[handler]
    pub fn stroke_edit_begin(&mut self) -> bool {
        let Some(stroke) = self.last_stroke.as_mut() else {
            return false;
        };
        stroke.editing = true;
        self.redraw_last_stroke();
        true
    }

    /// Leave stroke edit, committing the last stroke as it now stands as one
    /// undo entry. Also what the frontend sends when the artist leaves the
    /// brush tool or the layer, so it settles a stroke that was never
    /// edited too; a no-op when there is none.
    #[handler]
    pub fn stroke_edit_end(&mut self) {
        self.settle_last_stroke();
    }

    /// Set the foreground colour every following brush stroke paints with:
    /// raw sRGB RGBA floats in `0..=1`, as picked (the compositor is
    /// display-referred, so no gamma conversion is applied). Redraws the
    /// last stroke when the artist is editing it.
    #[handler]
    pub fn set_foreground_color(&mut self, rgba: [f32; 4]) {
        if self.foreground_color == rgba {
            return;
        }
        self.foreground_color = rgba;
        self.redraw_last_stroke();
    }
}
