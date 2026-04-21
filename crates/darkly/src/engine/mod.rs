mod brush_graph;
mod brush_preset;
mod clipboard;
mod floating;
pub(crate) mod gpu_selection;
mod layers;
mod masks;
mod painting;
mod rendering;
mod selection;
pub mod types;
mod veils;

pub use types::{ClipboardExport, LayerInfo, ParamInfo, StrokeOp, VeilInfo, VeilTypeInfo};

use crate::brush::checkpoint_ring::CheckpointRing;
use crate::brush::dab_pool::DabTexturePool;
use crate::brush::pipelines::BrushPipelines;
use crate::brush::preset_library::PresetLibrary;
use crate::brush::stabilizer::StabilizerRegistry;
use crate::brush::stroke_buffer::StrokeBuffer;
use crate::brush::stroke_engine::StrokeEngine;
use crate::brush::wire::BrushWireType;
use crate::clipboard::Clipboard;
use crate::document::Document;
use crate::gpu::compositor::Compositor;
use crate::gpu::context::GpuContext;
use crate::gpu::diff_rect::DiffRectPass;
use crate::gpu::overlay::OverlayPrimitive;
use crate::gpu::paint_target::PaintPipelines;
use crate::gpu::readback::ReadbackScheduler;
use crate::gpu::region_store::RegionStore;
use crate::gpu::transform::FloatingContent;
use crate::gpu::view::ViewTransform;
use crate::undo::UndoStack;
use gpu_selection::{GpuSelection, SelectionPipelines};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Internal helper types
// ---------------------------------------------------------------------------

/// Deferred transform setup — waiting for async content bounds from the compositor.
pub(crate) struct PendingTransform {
    pub layer_id: u64,
    pub target_is_mask: bool,
}

/// Deferred copy/cut — waiting for selection CPU cache to be populated.
pub(crate) struct PendingCopy {
    pub layer_id: u64,
    pub is_cut: bool,
}

/// Deferred undo commit — waiting for async GPU diff rect result.
pub(crate) struct PendingUndoCommit {
    pub layer_id: u64,
    pub format: wgpu::TextureFormat,
}

/// Context for a pending async GPU readback — travels with the request and
/// is returned alongside the pixel data on completion.
pub(crate) enum ReadbackContext {
    FloodFill {
        layer_id: u64,
        mask_editing: bool,
        seed_x: i32,
        seed_y: i32,
        color: [u8; 4],
        tolerance: u8,
        canvas_w: u32,
        canvas_h: u32,
    },
    ColorPick,
    Copy {
        is_mask: bool,
        region: [u32; 4],
        is_cut: bool,
        layer_id: u64,
    },
    MagicWand {
        was_active: bool,
        seed_x: i32,
        seed_y: i32,
        tolerance: u8,
        mode: crate::document::SelectionMode,
    },
    MaskToSelection {
        was_active: bool,
    },
    /// Async readback of the selection GPU texture for CPU cache update.
    SelectionReadback,
    Thumbnail {
        layer_id: u64,
        is_mask: bool,
        thumb_w: u32,
        thumb_h: u32,
    },
}

/// Cached thumbnail data per layer.
pub(crate) struct ThumbnailCache {
    /// Cached RGBA thumbnail bytes per layer id (layer content).
    layer: HashMap<u64, Vec<u8>>,
    /// Cached RGBA thumbnail bytes per layer id (mask).
    mask: HashMap<u64, Vec<u8>>,
}

impl ThumbnailCache {
    fn new() -> Self {
        ThumbnailCache {
            layer: HashMap::new(),
            mask: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// DarklyEngine — platform-agnostic editor core.
// ---------------------------------------------------------------------------

pub struct DarklyEngine {
    pub(crate) doc: Document,
    pub(crate) compositor: Compositor,
    pub(crate) gpu: GpuContext,
    pub(crate) undo_stack: UndoStack,
    pub(crate) active_stroke_layer: Option<u64>,
    /// Which layer has mask editing active (GIMP's `edit_mask` flag).
    /// When set, strokes are routed to the mask instead of the layer.
    pub(crate) editing_mask_layer: Option<u64>,
    pub(crate) view_transform: ViewTransform,
    /// Persistent marching ants overlay (regenerated when selection changes).
    pub(crate) selection_overlay: Vec<OverlayPrimitive>,
    /// Transient tool overlay (set/cleared by the active tool).
    pub(crate) tool_overlay: Vec<OverlayPrimitive>,
    /// Internal clipboard — holds typed content for copy/paste within Darkly.
    pub(crate) clipboard: Option<Clipboard>,
    /// Active floating content (paste-in-place or interactive transform).
    pub(crate) floating: Option<FloatingContent>,

    // --- GPU Paint Infrastructure (Phase 2) ---
    pub(crate) region_store: RegionStore,
    pub(crate) paint_pipelines: PaintPipelines,
    /// True when the scratch texture has been saved for the current stroke.
    pub(crate) scratch_saved: bool,

    // --- Brush Engine (Phase 4-5) ---
    pub(crate) dab_pool: DabTexturePool,
    pub(crate) brush_pipelines: BrushPipelines,
    /// Active brush stroke engine (only during a BrushStroke operation).
    pub(crate) brush_stroke_engine: Option<StrokeEngine>,
    /// The brush graph that will be compiled into a runner on each stroke start.
    /// Defaults to `brush::default_graph()`.  Updated via `set_brush_graph()`.
    pub(crate) active_brush_graph: crate::nodegraph::Graph<BrushWireType>,

    /// Canvas-space positioning info for the brush preview overlay, cached
    /// after each `regenerate_brush_preview()` call. Consumed by the brush
    /// tool to size/rotate the hover overlay primitive. `None` when the
    /// graph has no `color_output.preview` wire.
    pub(crate) brush_preview_info: Option<crate::brush::eval::BrushPreviewInfo>,

    // --- Preset Library (Phase 7) ---
    pub(crate) preset_library: PresetLibrary,
    /// Resource name → TextureHandle for images uploaded by the current preset.
    /// Built by `upload_preset_resources()`, read by Image nodes via BrushGpuContext.
    pub(crate) resource_handles:
        std::collections::HashMap<String, crate::brush::wire::TextureHandle>,

    /// Stroke buffer for stabilizer-driven rewind + re-render.
    pub(crate) stroke_buffer: Option<StrokeBuffer>,

    /// Ring buffer of GPU texture checkpoints for partial re-render on divergence.
    pub(crate) checkpoint_ring: CheckpointRing,

    // --- Stabilizer ---
    pub(crate) stabilizer_registry: StabilizerRegistry,

    /// Composite blend mode for the current stroke: 0 = paint, 1 = erase.
    pub(crate) brush_blend_mode: u32,

    // --- Diff rect (undo region computation) ---
    pub(crate) diff_rect: DiffRectPass,
    pub(crate) pending_undo_commit: Option<PendingUndoCommit>,

    // --- GPU Selection (Phase 5) ---
    /// GPU-authoritative selection mask — owns the R8 texture and bind groups.
    /// Always allocated; `gpu_selection.active` tracks whether a selection exists.
    pub(crate) gpu_selection: GpuSelection,
    /// Reusable pipelines for selection boolean operations.
    pub(crate) selection_pipelines: SelectionPipelines,

    // --- Deferred operations ---
    /// Pending transform waiting for content bounds computation.
    pub(crate) pending_transform: Option<PendingTransform>,
    /// Pending copy/cut waiting for selection CPU cache.
    pub(crate) pending_copy: Option<PendingCopy>,

    // --- Async readback ---
    pub(crate) readbacks: ReadbackScheduler<ReadbackContext>,
    /// Completed copy result — picked up by the frontend on the next poll.
    pub(crate) pending_copy_result: Option<ClipboardExport>,
    /// Last picked color — returned immediately while async readback is in flight.
    pub(crate) last_picked_color: [u8; 4],
    pub(crate) thumbnail_cache: ThumbnailCache,
}

impl DarklyEngine {
    pub fn new(gpu: GpuContext, doc_width: u32, doc_height: u32) -> Self {
        let compositor = Compositor::new(
            &gpu.device,
            &gpu.queue,
            gpu.surface_format(),
            doc_width,
            doc_height,
            gpu.is_software,
        );
        let doc = Document::new(doc_width, doc_height);
        let undo_stack = UndoStack::new(50);
        let region_store = RegionStore::new(&gpu.device, doc_width, doc_height);
        let paint_pipelines = PaintPipelines::new(&gpu.device, &gpu.queue);
        let dab_pool = DabTexturePool::new(&gpu.device);
        let brush_pipelines = BrushPipelines::new(
            &gpu.device,
            &gpu.queue,
            dab_pool.bind_group_layout(),
            doc_width,
            doc_height,
        );
        let selection_pipelines = SelectionPipelines::new(&gpu.device);
        let diff_rect = DiffRectPass::new(&gpu.device);
        let gpu_selection = GpuSelection::new(
            &gpu.device,
            doc_width,
            doc_height,
            brush_pipelines.selection_bind_group_layout(),
            &paint_pipelines.selection_bind_group_layout,
        );

        let mut engine = DarklyEngine {
            doc,
            compositor,
            gpu,
            undo_stack,
            active_stroke_layer: None,
            editing_mask_layer: None,
            view_transform: ViewTransform::identity(),
            selection_overlay: Vec::new(),
            tool_overlay: Vec::new(),
            clipboard: None,
            floating: None,
            region_store,
            paint_pipelines,
            scratch_saved: false,
            dab_pool,
            brush_pipelines,
            brush_stroke_engine: None,
            active_brush_graph: crate::brush::default_graph(),
            brush_preview_info: None,
            preset_library: {
                let mut lib = PresetLibrary::new();
                for bundle in crate::brush::builtin_presets::all() {
                    lib.insert(bundle);
                }
                lib
            },
            resource_handles: std::collections::HashMap::new(),
            stroke_buffer: None,
            checkpoint_ring: CheckpointRing::new(),
            stabilizer_registry: StabilizerRegistry::new(),
            brush_blend_mode: 0,
            diff_rect,
            pending_undo_commit: None,
            gpu_selection,
            selection_pipelines,
            pending_transform: None,
            pending_copy: None,
            readbacks: ReadbackScheduler::new(),
            pending_copy_result: None,
            last_picked_color: [0, 0, 0, 0],
            thumbnail_cache: ThumbnailCache::new(),
        };

        // Populate the brush preview mask + cached info from the default
        // graph so the hover overlay is live immediately, without needing
        // the user to trigger a `compile_active` via a param change.
        engine.regenerate_brush_preview();
        engine
    }
}

// ---------------------------------------------------------------------------
// Test helpers (public so integration tests can use them)
// ---------------------------------------------------------------------------

impl DarklyEngine {
    /// Current overlay preview mask dimensions. Test-only accessor.
    pub fn compositor_preview_mask_size(&self) -> (u32, u32) {
        self.compositor.tool_overlay_ref().preview_mask_size()
    }

    /// Blocking readback of the overlay's preview mask texture. Test-only.
    pub fn test_readback_overlay_preview_mask(&self) -> Vec<u8> {
        let tex = self
            .compositor
            .overlay_preview_mask_texture()
            .expect("preview mask not allocated");
        let (w, h) = self.compositor_preview_mask_size();
        crate::gpu::test_utils::readback_texture(
            &self.gpu.device,
            &self.gpu.queue,
            tex,
            wgpu::TextureFormat::Rgba8Unorm,
            w,
            h,
        )
    }

    /// Test-only view of the selection mask's CPU cache. Returns `None`
    /// when no selection is active or when the cache hasn't been populated.
    pub fn test_selection_cpu_cache(&self) -> Option<&[u8]> {
        self.gpu_selection.cpu_cache.as_deref()
    }

    /// Blocking readback of a layer's RGBA texture. For test assertions only.
    pub fn test_readback_layer(&self, layer_id: u64) -> Vec<u8> {
        let layer_tex = self
            .compositor
            .layer_texture(layer_id)
            .expect("layer texture not found");
        let w = self.doc.width;
        let h = self.doc.height;
        crate::gpu::test_utils::readback_texture(
            &self.gpu.device,
            &self.gpu.queue,
            &layer_tex.texture,
            wgpu::TextureFormat::Rgba8Unorm,
            w,
            h,
        )
    }

    /// Block until all pending async readbacks complete. For tests only.
    /// Uses `device.poll(Wait)` to ensure mapping callbacks fire, then
    /// processes all completed readbacks.
    pub fn test_flush_readbacks(&mut self) {
        let _ = self.gpu.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        // Manually process completed readbacks (same as poll_pending's readback loop).
        let completed = self.readbacks.poll(&self.gpu.device);
        for (ctx, pixels) in completed {
            match ctx {
                ReadbackContext::Copy {
                    is_mask,
                    region,
                    is_cut,
                    layer_id,
                } => {
                    self.complete_copy(is_mask, region, is_cut, layer_id, pixels);
                }
                ReadbackContext::SelectionReadback => {
                    self.update_selection_overlay_from_readback(pixels);
                }
                _ => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::params::{ParamDef, ParamValue};

    #[test]
    fn param_info_serializes_flat() {
        let def = ParamDef::Float {
            name: "speed",
            min: 0.0,
            max: 10.0,
            default: 1.0,
        };
        let info = ParamInfo::from_def(&def, Some(&ParamValue::Float(2.5)));
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["kind"], "float");
        assert_eq!(json["name"], "speed");
        assert_eq!(json["min"], 0.0);
        assert_eq!(json["max"], 10.0);
        assert_eq!(json["default"], 1.0);
        assert_eq!(json["value"], 2.5);
    }

    #[test]
    fn param_info_bool_omits_min_max() {
        let def = ParamDef::Bool {
            name: "soft",
            default: true,
        };
        let info = ParamInfo::from_def(&def, None);
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["kind"], "bool");
        assert_eq!(json["name"], "soft");
        assert_eq!(json["default"], true);
        assert!(json.get("min").is_none());
        assert!(json.get("max").is_none());
        assert!(json.get("value").is_none());
    }

    #[test]
    fn veil_info_serializes_correctly() {
        let info = VeilInfo {
            type_id: "pixelate".into(),
            visible: true,
            index: 0,
            params: vec![
                ParamInfo::from_def(
                    &ParamDef::Int {
                        name: "scale",
                        min: 1,
                        max: 32,
                        default: 2,
                    },
                    Some(&ParamValue::Int(4)),
                ),
                ParamInfo::from_def(
                    &ParamDef::Bool {
                        name: "soft",
                        default: true,
                    },
                    Some(&ParamValue::Bool(false)),
                ),
            ],
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["type"], "pixelate");
        assert_eq!(json["visible"], true);
        assert_eq!(json["index"], 0);

        let params = json["params"].as_array().unwrap();
        assert_eq!(params.len(), 2);
        assert_eq!(params[0]["kind"], "int");
        assert_eq!(params[0]["name"], "scale");
        assert_eq!(params[0]["value"], 4);
        assert_eq!(params[1]["kind"], "bool");
        assert_eq!(params[1]["name"], "soft");
        assert_eq!(params[1]["value"], false);
    }
}
