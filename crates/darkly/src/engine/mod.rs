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

pub use types::{
    ClipboardExport, LayerInfo, ParamInfo, StrokeOp, VeilInfo, VeilTypeInfo,
};

use crate::brush::dab_pool::DabTexturePool;
use crate::brush::pipelines::BrushPipelines;
use crate::brush::preset_library::PresetLibrary;
use crate::brush::stroke_engine::StrokeEngine;
use crate::brush::wire::BrushWireType;
use crate::clipboard::Clipboard;
use crate::document::Document;
use crate::gpu::compositor::Compositor;
use crate::gpu::context::GpuContext;
use crate::gpu::overlay::OverlayPrimitive;
use crate::gpu::paint_target::PaintPipelines;
use crate::gpu::readback::ReadbackScheduler;
use crate::gpu::diff_rect::DiffRectPass;
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

/// Tracks the bounding rect of a GPU stroke in progress.
pub(crate) struct GpuStrokeState {
    pub format: wgpu::TextureFormat,
    /// Tight bounding rect of all circles composited so far: [x, y, w, h].
    /// None until the first stroke_to call.
    pub stroke_rect: Option<[u32; 4]>,
}

impl GpuStrokeState {
    pub fn new(format: wgpu::TextureFormat) -> Self {
        GpuStrokeState { format, stroke_rect: None }
    }

    /// Expand the stroke rect to include a circle at (cx, cy) with the given radius.
    pub fn expand(&mut self, cx: f32, cy: f32, radius: f32, canvas_w: u32, canvas_h: u32) {
        let pad = 2.0; // softness + 1 pixel margin
        let x0 = (cx - radius - pad).max(0.0) as u32;
        let y0 = (cy - radius - pad).max(0.0) as u32;
        let x1 = ((cx + radius + pad).ceil() as u32).min(canvas_w);
        let y1 = ((cy + radius + pad).ceil() as u32).min(canvas_h);

        self.stroke_rect = Some(match self.stroke_rect {
            None => [x0, y0, x1 - x0, y1 - y0],
            Some([sx, sy, sw, sh]) => {
                let nx = sx.min(x0);
                let ny = sy.min(y0);
                let nx1 = (sx + sw).max(x1);
                let ny1 = (sy + sh).max(y1);
                [nx, ny, nx1 - nx, ny1 - ny]
            }
        });
    }
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
        ThumbnailCache { layer: HashMap::new(), mask: HashMap::new() }
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
    /// Active GPU stroke state (replaces CPU transaction for PaintCircle/EraseCircle).
    pub(crate) gpu_stroke: Option<GpuStrokeState>,

    // --- Brush Engine (Phase 4-5) ---
    pub(crate) dab_pool: DabTexturePool,
    pub(crate) brush_pipelines: BrushPipelines,
    /// Active brush stroke engine (only during a BrushStroke operation).
    pub(crate) brush_stroke_engine: Option<StrokeEngine>,
    /// The brush graph that will be compiled into a runner on each stroke start.
    /// Defaults to `brush::default_graph()`.  Updated via `set_brush_graph()`.
    pub(crate) active_brush_graph: crate::nodegraph::Graph<BrushWireType>,

    // --- Preset Library (Phase 7) ---
    pub(crate) preset_library: PresetLibrary,
    /// Resource name → TextureHandle for images uploaded by the current preset.
    /// Built by `upload_preset_resources()`, read by Image nodes via BrushGpuContext.
    pub(crate) resource_handles: std::collections::HashMap<String, crate::brush::wire::TextureHandle>,

    /// Global brush scale multiplier applied at composite time.
    /// Controls the canvas footprint of the brush independently from the
    /// node graph's internal rendering resolution.  Default 1.0.
    pub(crate) brush_global_scale: f32,

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
            &gpu.device, &gpu.queue, gpu.surface_format(),
            doc_width, doc_height, gpu.is_software,
        );
        let doc = Document::new(doc_width, doc_height);
        let undo_stack = UndoStack::new(50);
        let region_store = RegionStore::new(&gpu.device, doc_width, doc_height);
        let paint_pipelines = PaintPipelines::new(&gpu.device, &gpu.queue);
        let dab_pool = DabTexturePool::new(&gpu.device);
        let brush_pipelines = BrushPipelines::new(&gpu.device, &gpu.queue, dab_pool.bind_group_layout(), doc_width, doc_height);
        let selection_pipelines = SelectionPipelines::new(&gpu.device);
        let diff_rect = DiffRectPass::new(&gpu.device);
        let gpu_selection = GpuSelection::new(
            &gpu.device, doc_width, doc_height,
            brush_pipelines.selection_bind_group_layout(),
            &paint_pipelines.selection_bind_group_layout,
        );

        DarklyEngine {
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
            gpu_stroke: None,
            dab_pool,
            brush_pipelines,
            brush_stroke_engine: None,
            active_brush_graph: crate::brush::default_graph(),
            preset_library: {
                let mut lib = PresetLibrary::new();
                for bundle in crate::brush::builtin_presets::all() {
                    lib.insert(bundle);
                }
                lib
            },
            resource_handles: std::collections::HashMap::new(),
            brush_global_scale: 1.0,
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
        }
    }
}

// ---------------------------------------------------------------------------
// Test helpers (public so integration tests can use them)
// ---------------------------------------------------------------------------

impl DarklyEngine {
    /// Blocking readback of a layer's RGBA texture. For test assertions only.
    pub fn test_readback_layer(&self, layer_id: u64) -> Vec<u8> {
        let layer_tex = self.compositor.layer_texture(layer_id)
            .expect("layer texture not found");
        let w = self.doc.width;
        let h = self.doc.height;
        crate::gpu::test_utils::readback_texture(
            &self.gpu.device, &self.gpu.queue,
            &layer_tex.texture, wgpu::TextureFormat::Rgba8Unorm, w, h,
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
                ReadbackContext::Copy { is_mask, region, is_cut, layer_id } => {
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
        let def = ParamDef::Float { name: "speed", min: 0.0, max: 10.0, default: 1.0 };
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
        let def = ParamDef::Bool { name: "soft", default: true };
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
                    &ParamDef::Int { name: "scale", min: 1, max: 32, default: 2 },
                    Some(&ParamValue::Int(4)),
                ),
                ParamInfo::from_def(
                    &ParamDef::Bool { name: "soft", default: true },
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
