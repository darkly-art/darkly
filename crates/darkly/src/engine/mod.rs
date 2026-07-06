mod bake_common;
mod brush_graph;
mod brush_library;
mod canvas_resize;
mod canvas_transform;
mod clipboard;
mod duplicate;
mod export;
mod filters;
mod flatten;
mod floating;
mod image_rescale;
mod layer_flip;
mod layers;
mod load;
mod merge;
mod painting;
pub mod protocol;
pub mod rendering;
pub mod save;
mod selection_support;
pub mod types;
mod undo_dispatch;
mod veils;
mod voids;

pub use export::ExportImageResult;
pub use load::LoadDocument;
pub use rendering::{PickSource, DEFAULT_THUMB_SIZE};
pub use save::{SaveError, SaveJob, SavePurpose, SaveReadbackKind};
pub use types::{
    BlendModeTypeInfo, ClipboardExport, EngineState, LayerInfo, LayerKindTypeInfo, ModifierInfo,
    ModifierTypeInfo, ParamInfo, StrokeOp, ToolTypeInfo, VeilInfo, VeilTypeInfo, VoidTypeInfo,
};

pub use perf::{BrushPerfDelta, FrameRenderPhases};

mod perf;
use crate::brush::gpu_context::BrushPerfCounters;

use crate::brush::checkpoint_ring::CheckpointRing;
use crate::brush::library::BrushLibrary;
use crate::brush::pipeline::BrushPipelines;
use crate::brush::preview_renderer::BrushStrokePreviewRenderer;
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
use crate::gpu::region_store::{EntryPixels, RegionScratch};
use crate::gpu::selection::SelectionPipelines;
use crate::gpu::transform::FloatingContent;
use crate::gpu::veil_preview::VeilPreviewRenderer;
use crate::gpu::view::{ViewParams, ViewTransform};
use crate::gpu::void_preview::VoidPreviewRenderer;
use crate::layer::LayerId;
use crate::undo::UndoStack;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Internal helper types
// ---------------------------------------------------------------------------

/// Deferred transform setup — waiting for async content bounds from the compositor.
/// `node_id` may refer to a raster layer or a mask filter; the format is
/// derived from the node's [`PixelBuffer`].
pub(crate) struct PendingTransform {
    pub node_id: LayerId,
}

/// Deferred layer/selection flip — waiting for the selection CPU cache (the
/// flip pivot is the selection bbox centre, read from that cache).
pub(crate) struct PendingFlip {
    pub node_id: LayerId,
    pub xform: crate::gpu::ortho_transform::OrthoXform,
}

/// Deferred destructive filter — waiting for the selection CPU cache (the
/// filter region is the selection bbox, read from that cache). Mirrors
/// [`PendingFlip`].
pub(crate) struct PendingFilter {
    pub node_id: LayerId,
    pub filter_type: String,
}

/// Deferred copy/cut — waiting for selection CPU cache to be populated.
pub(crate) struct PendingCopy {
    pub layer_id: LayerId,
    pub is_cut: bool,
}

/// Layer metadata snapshot captured at `copy_layer_rich` time. Combined with
/// the async pixel readback to produce a `LayerClipboard`. CPU-side fields
/// only — pixel data still flows through the existing readback pipeline.
pub(crate) struct RichCopyMetadata {
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub opacity: f32,
    pub blend_mode: String,
    /// Snapshot of any mask filter on the source. Pixel data is NOT
    /// captured in v1 — it requires a parallel readback that lands in v2.
    pub mask: Option<RichCopyMask>,
}

pub(crate) struct RichCopyMask {
    pub name: String,
    pub visible: bool,
    pub bounds: crate::coord::CanvasRect,
}

/// Deferred undo commit — waiting for async GPU diff rect result.
pub(crate) struct PendingUndoCommit {
    pub layer_id: LayerId,
    pub snapshot: crate::gpu::region_store::Snapshot,
}

/// Context for a pending async GPU readback — travels with the request and
/// is returned alongside the pixel data on completion.
///
/// All variants carry a `node_id` (where applicable) that may refer to either
/// a raster layer or a mask filter; the format is derived from the node's
/// [`PixelBuffer`] when the readback completes.
pub(crate) enum ReadbackContext {
    FloodFill {
        node_id: LayerId,
        seed_canvas: crate::coord::CanvasPoint,
        color: [u8; 4],
        tolerance: u8,
        /// Snapshot of the target's coordinate frame at request time. Carries
        /// the texture offset/size + canvas size + format the completion
        /// handler needs to translate `seed_canvas` from canvas coords to
        /// texture coords and project the resulting mask back into a
        /// canvas-aligned R8 buffer. See
        /// `crate::gpu::flood_fill::LayerFloodFillExtent`.
        extent: crate::gpu::flood_fill::LayerFloodFillExtent,
    },
    ColorPick,
    Copy {
        node_id: LayerId,
        region: [u32; 4],
        is_cut: bool,
    },
    MagicWand {
        was_active: bool,
        node_id: LayerId,
        seed_canvas: crate::coord::CanvasPoint,
        tolerance: u8,
        mode: crate::document::SelectionMode,
        /// See `FloodFill::extent` — same coordinate-frame snapshot.
        extent: crate::gpu::flood_fill::LayerFloodFillExtent,
    },
    /// Async readback of the selection GPU texture for CPU cache update.
    SelectionReadback,
    /// Async readback of the full composited canvas for image export
    /// (PNG/JPEG/WebP). Result lands on `pending_export_result` and is
    /// drained by `poll_export_result`.
    ExportImage {
        width: u32,
        height: u32,
    },
    /// Async readback for `.darkly` save flow. One readback per pixel-bearing
    /// entity (raster layer, mask, selection) plus one for the composite.
    /// The destination (blob path or composite slot) is encoded in `kind`;
    /// on completion, `complete_save_readback` routes the pixels accordingly.
    /// When every blob lands, `poll_save_result` hands back a `SaveBundle`.
    SaveDocument {
        kind: save::SaveReadbackKind,
        /// Source texture dimensions in pixels — readback rows come back
        /// `width × bpp` wide.
        width: u32,
        height: u32,
    },
    Thumbnail {
        node_id: LayerId,
        /// Dimensions of the readback buffer in pixels — the source layout
        /// the downscale samples from. This is the layer texture's own
        /// extent at readback time, not the canvas extent (layers may be
        /// smaller or larger than canvas; see `request_thumbnail_readback`).
        source_w: u32,
        source_h: u32,
        thumb_w: u32,
        thumb_h: u32,
    },
    /// Async readback of a freshly-rendered brush editor preview. Completion
    /// caches the bytes on the engine so the next `brush_stroke_preview()`
    /// call returns them synchronously.
    ///
    /// `width`/`height` are the source render dimensions (the layout of the
    /// readback bytes, always `BRUSH_STROKE_RENDER_SIZE`). The framer crops
    /// the painted region and resizes to the canonical `BRUSH_THUMBNAIL_SIZE`
    /// before PNG-encoding — same shape as `BrushThumbnailForSave`.
    BrushStrokePreview {
        width: u32,
        height: u32,
        /// Graph version at the time the render was issued — used to skip
        /// caching stale results if another render has superseded this one.
        graph_version: u64,
    },
    /// Async readback of the preview render used to bake a `.darkly-brush`
    /// archive's embedded `preview.png`. Completion PNG-encodes the pixels
    /// and installs the result on the library entry via
    /// `BrushLibrary::set_thumbnail`.
    BrushThumbnailForSave {
        name: String,
        width: u32,
        height: u32,
    },
    /// Async readback of a single-dab preview rendered from a library
    /// brush's graph. Completion PNG-encodes the pixels and installs the
    /// result in the library's dab thumbnail cache via
    /// `BrushLibrary::set_dab_thumbnail`. Used by the picker tiles to
    /// show a tip silhouette next to the stroke thumbnail.
    BrushDabThumbnail {
        name: String,
        width: u32,
        height: u32,
    },
    /// Async readback of a single-dab preview rendered from the active
    /// graph. Completion runs the pixels through the same
    /// `frame_dab_thumbnail` framer the baked dab thumbnails use, so the
    /// active preview is byte-for-byte identical to the picker tiles'
    /// thumbnail when the active brush matches a preset. The PNG bytes
    /// land in `active_dab_preview_cache`. The topology version (not
    /// graph version) travels with the request: scrub-only changes
    /// don't affect the rendered output thanks to
    /// [`crate::brush::reset_exposed_scrubs`], so they shouldn't
    /// discard in-flight readbacks either.
    ActiveBrushDab {
        topology_version: u64,
    },
    /// Async readback of the freshly-rendered `cursor_preview_mask` (the GPU
    /// texture sampled by the overlay's KIND_MASKED_STAMP) used to derive
    /// the cursor-preview coverage scale. Completion measures mean alpha
    /// and pushes the resulting normalize multiplier into the overlay
    /// uniform via `set_preview_coverage_scale`. Carries the topology
    /// version at request time so stale results from superseded
    /// compilations are dropped.
    ///
    /// `width` / `height` are the cursor_preview_mask dimensions at readback
    /// time — used to walk the row-padded buffer the GPU returned.
    BrushCursorPreviewScale {
        topology_version: u64,
        width: u32,
        height: u32,
    },
    /// Async readback of an undo-region staging buffer. On completion the
    /// handler flips the `cell` from `Pending` to `Ready`, dropping the
    /// staging buffer and moving the pixels onto the host heap.
    ///
    /// `cell` is a clone of the [`crate::gpu::region_store::UndoRegionEntry::pixels`]
    /// produced by `commit_region` / `restore_region`. The other clone lives
    /// on the entry — see [`crate::gpu::region_store::EntryPixels`].
    UndoRegionReady {
        cell: std::rc::Rc<std::cell::RefCell<EntryPixels>>,
    },
    /// Async readback of one picker preview frame, rendered offscreen
    /// (`gpu::veil_preview` for veils over the current canvas, `gpu::void_preview`
    /// for voids from scratch). Completion drops the raw RGBA bytes into
    /// `previews[(kind, type_id)].frames[frame_idx]`; the frontend drains all
    /// `total` frames once they land and plays them as a loop. Each frame is the
    /// job's aspect-fit `width × height` RGBA.
    PreviewFrame {
        kind: PreviewKind,
        type_id: &'static str,
        frame_idx: u32,
        total: u32,
    },
}

/// Which kind of effect a picker preview is generating. Keys the shared
/// `previews` map alongside the effect's `'static` type id, so a veil and a void
/// that happen to share a type-id string never collide.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum PreviewKind {
    Veil,
    Void,
}

/// One picker preview generation: the chosen preview dimensions plus the
/// per-frame RGBA slots, each filled when its async readback lands. `width` /
/// `height` are carried so `poll_preview` and the WASM bridge report the real
/// (aspect-fit) thumbnail size, which varies with the document's shape.
pub(crate) struct PreviewJob {
    pub width: u32,
    pub height: u32,
    pub frames: Vec<Option<Vec<u8>>>,
}

/// Cached thumbnail RGBA bytes per node id. Keyed uniformly across layers,
/// groups, and filters — the node id is sufficient to disambiguate, so the
/// previous separate `layer` and `mask` maps collapse into one.
pub(crate) struct ThumbnailCache {
    bytes: HashMap<LayerId, Vec<u8>>,
}

impl ThumbnailCache {
    fn new() -> Self {
        ThumbnailCache {
            bytes: HashMap::new(),
        }
    }

    pub(crate) fn get(&self, node_id: LayerId) -> Option<&Vec<u8>> {
        self.bytes.get(&node_id)
    }

    pub(crate) fn insert(&mut self, node_id: LayerId, bytes: Vec<u8>) {
        self.bytes.insert(node_id, bytes);
    }
}

// ---------------------------------------------------------------------------
// DarklyEngine — platform-agnostic editor core.
// ---------------------------------------------------------------------------

pub struct DarklyEngine {
    pub(crate) doc: Document,
    pub(crate) compositor: Compositor,
    pub(crate) gpu: GpuContext,
    /// Font collection + parley shaping state, used to realize vector (text)
    /// layers. Platform-agnostic: bundled fonts now; the platform layer feeds
    /// OS fonts later through [`crate::text::FontRegistry::register_font`].
    pub(crate) fonts: crate::text::FontRegistry,
    pub(crate) undo_stack: UndoStack,
    pub(crate) active_stroke_layer: Option<LayerId>,
    /// Session-level "isolate this node" flag. When set, the renderer shows
    /// only this node's contribution (e.g. an R8 mask is rendered grayscale,
    /// a layer is rendered without siblings/parents). Universal across node
    /// kinds — works for any future filter / filter filter too.
    pub(crate) isolated_node: Option<LayerId>,
    pub(crate) view_transform: ViewTransform,
    /// Decomposed view inputs from which `view_transform` is derived. The
    /// single source of truth for pan/zoom/rotation/screen — `set_view_transform`
    /// writes them, and `rebuild_view_transform` re-derives the matrix from
    /// them (plus the current `doc.width/height`) on any canvas-dimension
    /// change. `rotation` here is also what the brush stack threads into
    /// `IntrinsicUniforms.view_rotation` for stamp-counteracting-view-rotation.
    pub(crate) view_params: ViewParams,
    /// Persistent marching ants overlay (regenerated when selection changes).
    pub(crate) selection_overlay: Vec<OverlayPrimitive>,
    /// Transient tool overlay (set/cleared by the active tool).
    pub(crate) tool_overlay: Vec<OverlayPrimitive>,
    /// Internal clipboard — holds typed content for copy/paste within Darkly.
    pub(crate) clipboard: Option<Clipboard>,
    /// Active floating content (paste-in-place or interactive transform).
    pub(crate) floating: Option<FloatingContent>,

    // --- GPU Paint Infrastructure ---
    pub(crate) region_scratch: RegionScratch,
    pub(crate) paint_pipelines: PaintPipelines,
    /// Pre-stroke scratch snapshot for the current stroke. Lazily populated
    /// on the first stroke_to of a stroke; consumed at end_stroke (moved into
    /// `pending_undo_commit`) or by a sync commit path (flood fill, clear,
    /// fill_background — those take their own snapshots inline).
    pub(crate) scratch_snapshot: Option<crate::gpu::region_store::Snapshot>,
    /// Selection-texture snapshot held between `save_selection_for_undo` and
    /// the matching `commit_selection_undo`. Some selection ops (magic wand,
    /// mask-to-selection) save before an async readback and commit on
    /// completion — the snapshot lives across that boundary.
    pub(crate) pending_selection_snapshot: Option<crate::gpu::region_store::Snapshot>,

    // --- Brush Engine ---
    pub(crate) brush_pipelines: BrushPipelines,
    /// Active brush stroke engine (only during a BrushStroke operation).
    pub(crate) brush_stroke_engine: Option<StrokeEngine>,
    /// Shared tool session — a generic bag of per-tool state shared
    /// across every engine in a `DarklySession`. The brush module stores
    /// its [`crate::brush::state::BrushState`] entry here; other tools
    /// that grow cross-engine state in future register theirs the same
    /// way. Owned by `DarklySession` (WASM bridge) and cloned into every
    /// engine, so multi-tab editors see one source of truth for shared
    /// tool state with no sync step. The data lives behind an
    /// `Arc<RwLock<…>>`; engines take a read guard for stroke compile /
    /// preview, a write guard for JS-driven mutation methods.
    pub(crate) tool_session: crate::tool::SharedToolSession,

    /// Canvas-space positioning info for the brush preview overlay, cached
    /// after each `regenerate_brush_cursor_preview()` call. Consumed by the brush
    /// tool to size/rotate the hover overlay primitive. `None` when the
    /// graph has no `color_output.preview` wire.
    pub(crate) brush_cursor_preview_info: Option<crate::brush::eval::BrushCursorPreviewInfo>,

    /// Previous hover sample fed into `regenerate_brush_cursor_preview_with_pen`.
    /// Kept so segment-derived sensors (drawing_angle, motion, distance,
    /// speed) can be derived on the next hover using the same helper the
    /// stroke engine uses. Reset on pointer-leave / stroke-start via
    /// `clear_brush_cursor_preview_pose()` so a return-from-offscreen hover
    /// doesn't synthesize a spurious direction.
    pub(crate) last_cursor_preview_pose: Option<crate::brush::paint_info::PaintInformation>,

    // --- Full-stroke brush editor preview ---
    /// Renderer for the Krita-style S-curve preview shown in the brush
    /// editor widget. Reused across calls; holds its own scratch target.
    pub(crate) brush_stroke_preview_renderer: BrushStrokePreviewRenderer,
    /// Cached PNG bytes of the most recently-completed editor preview.
    /// `brush_stroke_preview()` returns this synchronously; it's refreshed
    /// asynchronously via `ReadbackContext::BrushStrokePreview`. The frontend
    /// uses the bytes directly as a `Blob` URL — same shape as
    /// `active_dab_preview_cache`. Always framed to `BRUSH_THUMBNAIL_SIZE`.
    pub(crate) brush_stroke_preview_cache: Option<Vec<u8>>,
    // `brush_graph_version` and `brush_topology_version` moved into the
    // shared `BrushState` (looked up via `tool_session`). The per-engine
    // `last_rendered_*` cursors below stay per-engine because they track
    // this engine's own render cache versus the shared monotonic counter.
    /// Graph version at the last time we issued a preview render. Compared
    /// against `BrushState::version` to skip redundant work.
    pub(crate) last_rendered_stroke_preview_version: u64,

    // --- Active brush dab preview ---
    /// Cached PNG bytes of the most recently-completed active-dab
    /// preview, framed through the same `frame_dab_thumbnail` path used
    /// for baked thumbnails — so this is byte-identical to a
    /// `brush_dab_thumbnail(active_name)` call when the active brush
    /// matches a preset. `brush_active_dab_preview()` returns this
    /// synchronously; it's refreshed asynchronously via
    /// `ReadbackContext::ActiveBrushDab`.
    pub(crate) active_dab_preview_cache: Option<Vec<u8>>,
    /// Topology version at the last time we issued a dab render. Compared
    /// against `brush_topology_version` to skip redundant dab renders.
    pub(crate) last_rendered_dab_topology_version: u64,
    /// Topology version of the brush whose cursor-preview coverage scale
    /// we last requested a readback for. Compared against the current
    /// topology to skip re-issuing the readback when the brush hasn't
    /// changed shape — the scale is a property of the graph topology,
    /// not the cursor pose.
    pub(crate) last_requested_cursor_scale_topology_version: u64,
    /// Theme colors shared by the stroke preview, the dab preview, and
    /// the library thumbnail bake. (The cursor preview composes over the
    /// live canvas and ignores these.) The frontend sets these via
    /// `set_preview_theme()` when the UI theme toggles.
    pub(crate) preview_theme_fg: [f32; 4],
    pub(crate) preview_theme_bg: [f32; 4],

    // --- Picker previews (veil + void) ---
    /// Offscreen renderer for the veil picker's looping thumbnail previews.
    /// Reused across veils; holds its own preview-sized scratch textures and
    /// is fully independent of the live veil chain and document.
    pub(crate) veil_preview_renderer: VeilPreviewRenderer,
    /// Offscreen renderer for the void picker's looping thumbnail previews.
    /// Reused across voids; holds its own preview-sized output texture and is
    /// fully independent of the live layer stack and document.
    pub(crate) void_preview_renderer: VoidPreviewRenderer,
    /// In-flight + completed preview jobs, keyed by `(kind, &'static str type
    /// id)`. Frame slots fill in asynchronously as `ReadbackContext::PreviewFrame`
    /// readbacks land; `poll_preview` hands back the frames once every slot is
    /// `Some`. Overwritten on each `start_*_preview` so the picker always
    /// reflects the current document (no cross-open caching).
    pub(crate) previews: HashMap<(PreviewKind, &'static str), PreviewJob>,

    // --- Brush Library ---
    pub(crate) brush_library: BrushLibrary,

    /// Stroke buffer for stabilizer-driven rewind + re-render.
    pub(crate) stroke_buffer: Option<StrokeBuffer>,

    /// Ring buffer of GPU texture checkpoints for partial re-render on divergence.
    pub(crate) checkpoint_ring: CheckpointRing,

    // --- Stabilizer ---
    pub(crate) stabilizer_registry: StabilizerRegistry,

    /// Composite blend mode for the current stroke: 0 = paint, 1 = erase.
    pub(crate) brush_blend_mode: u32,

    /// Clone-brush set-source anchor in plane / canvas pixels, or `None`
    /// until the user sets it via the set-source gesture. Session state:
    /// persists across strokes and across brush / tool switches (so the
    /// source survives lifting the pen), but not across reload. A clone
    /// brush with no anchor set paints nothing (the no-op gate in
    /// `brush_stroke_to`).
    pub(crate) clone_source_anchor: Option<crate::coord::CanvasPoint>,

    // --- Diff rect (undo region computation) ---
    pub(crate) diff_rect: DiffRectPass,
    pub(crate) pending_undo_commit: Option<PendingUndoCommit>,

    // --- Selection ---
    /// Reusable GPU pipelines for selection boolean / invert operations.
    /// The selection's R8 textures + bind groups live in
    /// `compositor.selection_state`; the active toggle, tight bounds, and
    /// CPU readback cache live on `doc.selection.kind` (`SelectionFilter`).
    pub(crate) selection_pipelines: SelectionPipelines,

    // --- Deferred operations ---
    /// Pending transform waiting for content bounds computation.
    pub(crate) pending_transform: Option<PendingTransform>,
    /// Pending layer/selection flip waiting for the selection CPU cache.
    pub(crate) pending_flip: Option<PendingFlip>,
    /// Pending destructive filter waiting for the selection CPU cache.
    pub(crate) pending_filter: Option<PendingFilter>,
    /// Pending copy/cut waiting for selection CPU cache.
    pub(crate) pending_copy: Option<PendingCopy>,

    // --- Async readback ---
    pub(crate) readbacks: ReadbackScheduler<ReadbackContext>,
    /// Completed copy result — picked up by the frontend on the next poll.
    pub(crate) pending_copy_result: Option<ClipboardExport>,
    /// Metadata snapshot captured at `copy_layer_rich` time. When the async
    /// pixel readback completes, this snapshot is combined with the pixels
    /// to build a `LayerClipboard` and stash it in `pending_layer_clip`.
    pub(crate) pending_rich_metadata: Option<RichCopyMetadata>,
    /// Completed rich-copy result, ready for the frontend to drain. Holds
    /// the JSON-serialised `LayerClipboard` for transmission via the
    /// system clipboard's `web application/x-darkly-layer` custom MIME.
    pub(crate) pending_layer_clip: Option<String>,
    /// Last picked color — returned immediately while async readback is in flight.
    pub(crate) last_picked_color: [u8; 4],
    /// Completed image-export result — drained by `poll_export_result()`.
    pub(crate) pending_export_result: Option<ExportImageResult>,
    /// Active save job — populated by `start_save_document`, drained by
    /// `poll_save_result` once every pixel blob and the composite have
    /// landed. Only one save can be in flight per engine; a second
    /// `start_save_document` while this is `Some` errors with
    /// [`SaveError::InProgress`].
    pub(crate) active_save_job: Option<SaveJob>,
    pub(crate) thumbnail_cache: ThumbnailCache,
    /// Monotonic counter bumped each time a thumbnail readback lands in
    /// the cache. Mirrored to a Svelte-reactive epoch in the frontend so
    /// the layer panel's `$derived` can re-evaluate after async updates.
    /// `u32` because exact-`f64` representation is required for the wasm
    /// boundary; wraparound is irrelevant since the JS comparison is
    /// `!==`, not `>`.
    pub(crate) thumbnail_version: u32,

    /// Set once a layer-grow request has been refused for hitting
    /// `MAX_LAYER_DIM` — used to log the cap warning at most once per
    /// process lifetime.
    pub(crate) layer_growth_capped: bool,

    /// Per-stroke counter accumulator. Reset at `begin_stroke`; each
    /// per-event `BrushGpuContext` `+=`'s its drained counters into here.
    pub(crate) brush_perf: BrushPerfCounters,

    /// Mid-stroke full re-render fallbacks during this stroke (the
    /// per-engine counterpart to `brush_perf` — see [`BrushPerfCounters`]
    /// docs on why this isn't a field there). Bumped in `painting.rs`
    /// when the checkpoint ring's coverage invariant fails; surfaced
    /// via `test_stroke_full_rerender_events`.
    pub(crate) brush_full_rerender_events: u32,

    /// Snapshot of `brush_perf` taken on the last `drain_brush_perf_delta`
    /// call. Subtracted from the current accumulator on each drain to
    /// produce a per-interval delta. Reset to default at `begin_stroke`
    /// alongside `brush_perf`. Native bench harnesses only — production
    /// never reads this.
    pub(crate) last_brush_perf: BrushPerfCounters,

    /// Most recent `render()` sub-phase timings. Overwritten every frame;
    /// read by the WASM bridge when it logs a slow frame.
    pub(crate) last_frame_phases: FrameRenderPhases,
}

impl DarklyEngine {
    /// Convenience constructor for single-engine use (tests, headless,
    /// embedded host). Allocates a fresh `SharedToolSession` that's
    /// owned exclusively by this engine and seeds it with a default
    /// `BrushState`. Multi-tab hosts use `new_with_tool_session`
    /// instead, passing a `DarklySession`-owned handle so every engine
    /// reads the same tool state.
    pub fn new(gpu: GpuContext, doc_width: u32, doc_height: u32) -> Self {
        let session = crate::tool::SharedToolSession::new();
        session
            .write()
            .insert(crate::brush::state::BrushState::new());
        Self::new_with_tool_session(gpu, session, doc_width, doc_height)
    }

    pub fn new_with_tool_session(
        gpu: GpuContext,
        tool_session: crate::tool::SharedToolSession,
        doc_width: u32,
        doc_height: u32,
    ) -> Self {
        // Allocate the document first so the compositor can read its root id
        // (which replaces the legacy `ROOT_ID = 0` constant).
        let doc = Document::new(doc_width, doc_height);
        let compositor = Compositor::new(
            &gpu.device,
            &gpu.queue,
            gpu.surface_format(),
            doc_width,
            doc_height,
            doc.root_id(),
        );
        let undo_stack = UndoStack::new(50);
        let region_scratch = RegionScratch::new(&gpu.device, doc_width, doc_height);
        // One selection-mask layout shared by both pipelines (and the cached
        // selection bind group), so a single bind group serves every consumer.
        let selection_mask_bgl = crate::gpu::selection::selection_mask_bgl(&gpu.device);
        let paint_pipelines = PaintPipelines::new(&gpu.device, &gpu.queue, &selection_mask_bgl);
        let brush_pipelines = BrushPipelines::new(&gpu.device, &gpu.queue, &selection_mask_bgl);
        let selection_pipelines = SelectionPipelines::new(&gpu.device);
        let diff_rect = DiffRectPass::new(&gpu.device);

        let mut engine = DarklyEngine {
            doc,
            compositor,
            gpu,
            fonts: crate::text::FontRegistry::new(),
            undo_stack,
            active_stroke_layer: None,
            isolated_node: None,
            view_transform: ViewTransform::identity(),
            view_params: ViewParams::default(),
            selection_overlay: Vec::new(),
            tool_overlay: Vec::new(),
            clipboard: None,
            floating: None,
            region_scratch,
            paint_pipelines,
            scratch_snapshot: None,
            pending_selection_snapshot: None,
            brush_pipelines,
            brush_stroke_engine: None,
            tool_session,
            brush_cursor_preview_info: None,
            last_cursor_preview_pose: None,
            brush_stroke_preview_renderer: BrushStrokePreviewRenderer::new(),
            brush_stroke_preview_cache: None,
            last_rendered_stroke_preview_version: 0,
            active_dab_preview_cache: None,
            last_rendered_dab_topology_version: 0,
            last_requested_cursor_scale_topology_version: 0,
            // Default theme: dark (white on dark). Frontend overrides via
            // `set_preview_theme()` as soon as the UI loads.
            preview_theme_fg: [1.0, 1.0, 1.0, 1.0],
            preview_theme_bg: [0.0, 0.0, 0.0, 1.0],
            veil_preview_renderer: VeilPreviewRenderer::new(),
            void_preview_renderer: VoidPreviewRenderer::new(),
            previews: HashMap::new(),
            brush_library: {
                let mut lib = BrushLibrary::new();
                for brush in crate::brush::builtin_brushes::all() {
                    lib.insert(brush);
                }
                lib
            },
            stroke_buffer: None,
            checkpoint_ring: CheckpointRing::new(),
            stabilizer_registry: StabilizerRegistry::new(),
            brush_blend_mode: 0,
            clone_source_anchor: None,
            diff_rect,
            pending_undo_commit: None,
            selection_pipelines,
            pending_transform: None,
            pending_flip: None,
            pending_filter: None,
            pending_copy: None,
            readbacks: ReadbackScheduler::new(),
            pending_copy_result: None,
            pending_rich_metadata: None,
            pending_layer_clip: None,
            last_picked_color: [0, 0, 0, 0],
            pending_export_result: None,
            active_save_job: None,
            thumbnail_cache: ThumbnailCache::new(),
            thumbnail_version: 0,
            layer_growth_capped: false,
            brush_perf: BrushPerfCounters::default(),
            brush_full_rerender_events: 0,
            last_brush_perf: BrushPerfCounters::default(),
            last_frame_phases: FrameRenderPhases::default(),
        };

        // Snapshot the default graph's port defaults so reset-to-default
        // works even before the user loads a brush.
        engine.snapshot_brush_defaults();

        // Populate the brush preview mask + cached info from the default
        // graph so the hover overlay is live immediately, without needing
        // the user to trigger a `compile_active` via a param change.
        engine.regenerate_brush_cursor_preview();

        // Eagerly allocate the document selection filter + its GPU state.
        // The selection is a typed Filter on `doc.selection`; the R8 GPU
        // textures + bind groups live in `compositor.selection_state`. Both
        // are zero-cost when no selection is active (visible=false), so
        // allocating up-front keeps the consumer code branch-free.
        let selection_mod_id = engine.doc.ensure_selection_filter();
        engine.compositor.ensure_selection_state(
            &engine.gpu.device,
            selection_mod_id,
            engine.brush_pipelines.selection_bind_group_layout(),
        );

        engine
    }
}

// ---------------------------------------------------------------------------
// Shared picker-preview plumbing
// ---------------------------------------------------------------------------

impl DarklyEngine {
    /// Encode one preview frame and submit its async readback. The effect-
    /// specific render — a veil's ping-pong pass or a void's generate pass —
    /// lives in the `encode` closure; everything else (the command encode, the
    /// readback request keyed by `(kind, type_id, frame_idx, total)`, and the
    /// scheduler submission) is shared between the veil and void preview paths.
    ///
    /// An associated function rather than a `&mut self` method so the caller can
    /// hand it disjoint borrows: `gpu` / `readbacks` here, while `encode` and
    /// `output` borrow the per-effect preview renderer.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn encode_preview_frame(
        gpu: &GpuContext,
        readbacks: &mut ReadbackScheduler<ReadbackContext>,
        kind: PreviewKind,
        type_id: &'static str,
        frame_idx: u32,
        total: u32,
        output: &wgpu::Texture,
        format: wgpu::TextureFormat,
        rect: crate::coord::LayerRect,
        encode: impl FnOnce(&mut wgpu::CommandEncoder),
    ) {
        gpu.encode("preview-frame", |encoder| {
            encode(encoder);
            let request =
                crate::gpu::readback::request_readback(&gpu.device, encoder, output, format, rect);
            readbacks.submit(
                request,
                ReadbackContext::PreviewFrame {
                    kind,
                    type_id,
                    frame_idx,
                    total,
                },
            );
        });
    }

    /// Return the completed preview for `(kind, type_id)` as
    /// `(width, height, frames)` once every frame has landed, else `None`. Each
    /// frame is `width × height` tightly-packed RGBA8. Shared by both pickers.
    pub fn poll_preview(
        &self,
        kind: PreviewKind,
        type_id: &str,
    ) -> Option<(u32, u32, Vec<Vec<u8>>)> {
        let (_, job) = self
            .previews
            .iter()
            .find(|((k, t), _)| *k == kind && *t == type_id)?;
        if job.frames.is_empty() || job.frames.iter().any(Option::is_none) {
            return None;
        }
        let frames = job.frames.iter().map(|f| f.clone().unwrap()).collect();
        Some((job.width, job.height, frames))
    }
}

// ---------------------------------------------------------------------------
// Test helpers (public so integration tests can use them)
// ---------------------------------------------------------------------------

impl DarklyEngine {
    /// Master rAF tick counter. Advances exactly once per `render` call,
    /// starting at 0. All divisor-throttled subsystems inside the compositor
    /// (veils, overlay, voids) gate against this single counter; surfacing
    /// it lets the frontend's camera-void upload throttle stay in lockstep
    /// with them rather than running its own drifting counter.
    pub fn frame_count(&self) -> u64 {
        self.compositor.frame_count()
    }

    /// Per-event drain of the brush perf accumulator. Returns the delta
    /// against the previous drain (per-flush vectors taken via `mem::take`,
    /// scalars `saturating_sub`'d) and resnapshots `last_brush_perf` so the
    /// next call subtracts against this point.
    ///
    /// **Native bench harnesses only.** Mutates `brush_perf`'s per-flush
    /// vectors via `std::mem::take`. WASM frontends do not call this.
    pub fn drain_brush_perf_delta(&mut self) -> BrushPerfDelta {
        let delta = BrushPerfDelta::between(&mut self.brush_perf, &self.last_brush_perf);
        self.last_brush_perf = self.brush_perf.clone();
        delta
    }

    /// Current overlay preview mask dimensions. Test-only accessor.
    #[cfg(any(test, feature = "testing"))]
    pub fn compositor_cursor_preview_mask_size(&self) -> (u32, u32) {
        self.compositor.tool_overlay().cursor_preview_mask_size()
    }

    /// Current cursor-preview coverage scale uniform — the multiplier the
    /// overlay shader applies to KIND_MASKED_STAMP sampled coverage.
    /// Test-only.
    #[cfg(any(test, feature = "testing"))]
    pub fn cursor_preview_coverage_scale(&self) -> f32 {
        self.compositor.tool_overlay().preview_coverage_scale()
    }

    /// Test-only view of the selection mask's CPU cache. Returns `None`
    /// when no selection is active or when the cache hasn't been populated.
    pub fn test_selection_cpu_cache(&self) -> Option<&[u8]> {
        self.selection_cpu_cache()
    }

    /// Blocking readback of the document selection's R8 mask texture (window-
    /// sized, one byte per pixel — 255 selected, 0 unselected). Test-only;
    /// lets selection-modify tests inspect mask values directly rather than
    /// inferring them through paint. Returns `None` before the selection state
    /// is allocated.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_readback_selection(&self) -> Option<Vec<u8>> {
        let state = self.compositor.selection_state()?;
        Some(crate::gpu::test_utils::readback_texture(
            &self.gpu.device,
            &self.gpu.queue,
            state.texture(),
            wgpu::TextureFormat::R8Unorm,
            state.width,
            state.height,
        ))
    }

    /// Test-only public accessor for the selection filter's id.
    pub fn selection_filter_id_test(&self) -> Option<LayerId> {
        self.doc.selection_id()
    }

    /// Test-only pointer to the document. Used by the load-refusal
    /// tests to assert that a failed `open_document` does NOT swap the
    /// engine's doc out from under the caller — the original allocation
    /// must still be the live one. Equality on a raw `*const Document`
    /// is enough to spot the move (since `mem::replace` would replace
    /// the slotmap and its heap allocations).
    pub fn document_ptr_for_test(&self) -> *const crate::document::Document {
        &self.doc
    }

    /// Test-only assertion that the document's selection slot holds a Filter
    /// whose kind is `Selection`. Returns `None` if the slot is empty.
    pub fn test_selection_filter_kind_is_selection(&self) -> Option<bool> {
        let id = self.doc.selection?;
        self.doc.find_filter(id).map(|m| m.as_selection().is_some())
    }

    /// Test-only access to the selection's `PixelBuffer.bounds`.
    pub fn test_selection_pixel_buffer_bounds(&self) -> Option<crate::coord::CanvasRect> {
        let id = self.doc.selection?;
        self.doc
            .find_filter(id)
            .and_then(|m| m.pixels())
            .map(|p| p.bounds)
    }

    /// Test-only: whether the undo / redo stacks have anything to apply.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_can_undo(&self) -> bool {
        self.undo_stack.can_undo()
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn test_can_redo(&self) -> bool {
        self.undo_stack.can_redo()
    }

    /// Test-only: a pixel-bearing node's document-side `PixelBuffer.bounds`
    /// (raster layer or mask filter). Used to assert extents scale on rescale.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_node_pixel_bounds(&self, id: LayerId) -> Option<crate::coord::CanvasRect> {
        self.doc.node_pixel_bounds(id)
    }

    /// Test-only: the mask filter id attached to `host_id`, if any (the first
    /// non-selection pixel-bearing filter on the host).
    #[cfg(any(test, feature = "testing"))]
    pub fn test_mask_id(&self, host_id: LayerId) -> Option<LayerId> {
        let node = self.doc.find_node(host_id)?;
        node.filters().iter().copied().find(|&mid| {
            self.doc
                .find_filter(mid)
                .map(|m| m.as_selection().is_none() && m.pixels().is_some())
                .unwrap_or(false)
        })
    }

    /// Number of GPU textures the compositor currently holds across the
    /// unified node-texture pool (raster layers and pixel-bearing filters
    /// like masks). Test-only metric for leak-cycle regression tests (P3).
    pub fn test_node_texture_count(&self) -> usize {
        self.compositor.test_node_texture_count()
    }

    /// Force-drain both undo stacks and run `on_evict` on every entry,
    /// releasing any tombstoned GPU textures the actions own. Test-only
    /// hook for leak-cycle assertions that need to observe the post-
    /// eviction texture count without rebuilding the engine.
    pub fn test_drain_undo_for_teardown(&mut self) {
        self.drain_undo_for_teardown();
    }

    /// Blocking readback of a node's GPU texture (raster layer or mask
    /// filter). For test assertions only. Format and extent come from the
    /// texture's own metadata — callers don't need to know whether the id
    /// refers to a layer or a filter.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_readback_layer(&self, node_id: LayerId) -> Vec<u8> {
        let tex = self
            .compositor
            .node_texture(node_id)
            .expect("node texture not found");
        let ext = tex.layer_extent();
        crate::gpu::test_utils::readback_texture(
            &self.gpu.device,
            &self.gpu.queue,
            tex.texture(),
            tex.format(),
            ext.width,
            ext.height,
        )
    }

    /// Plant a persistent void frame (camera void's last webcam frame) into
    /// the void's aux texture — the same entry point the load path uses via
    /// `restore_void_pixels`. For test assertions only: on native there is no
    /// webcam, so this is the only way to give a camera void a frame to read
    /// back.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_plant_void_frame(
        &mut self,
        layer_id: LayerId,
        width: u32,
        height: u32,
        bytes: &[u8],
    ) {
        self.compositor.restore_void_pixels(
            &self.gpu.device,
            &self.gpu.queue,
            layer_id,
            width,
            height,
            bytes,
        );
    }

    /// Blocking readback of a void layer's persistent frame through the exact
    /// `pixel_data_for` path the save flow uses (`queue_pixel_readback`). For
    /// test assertions only. Returns `None` when the void declares no
    /// persistent frame. This is the readback the camera void's aux texture
    /// must support — it requires `COPY_SRC` on that texture.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_readback_void_frame(&self, layer_id: LayerId) -> Option<Vec<u8>> {
        let data = self.compositor.pixel_data_for(layer_id)?;
        Some(crate::gpu::test_utils::readback_texture(
            &self.gpu.device,
            &self.gpu.queue,
            data.texture,
            data.format,
            data.width,
            data.height,
        ))
    }

    /// Blocking readback of the root composited canvas. For test assertions
    /// only. Returns canvas-sized RGBA8 pixels (padding excluded). Forces an
    /// offscreen composite first because headless `render()` skips the
    /// compositor (no surface to present to).
    #[cfg(any(test, feature = "testing"))]
    pub fn test_readback_canvas(&mut self) -> Vec<u8> {
        self.compositor
            .render_offscreen(&self.gpu.device, &self.gpu.queue, &mut self.doc);
        let texture = self.compositor.composited_texture();
        let w = self.compositor.canvas_width();
        let h = self.compositor.canvas_height();
        crate::gpu::test_utils::readback_texture(
            &self.gpu.device,
            &self.gpu.queue,
            texture,
            wgpu::TextureFormat::Rgba8Unorm,
            w,
            h,
        )
    }

    /// Blocking readback of the present pass output (composite cache run
    /// through the present shader into a canvas-sized RGBA8 target). For test
    /// assertions about the present stage itself — premultiplied-alpha
    /// handling, the transparency checker, OOB workspace background, etc. —
    /// which `test_readback_canvas` cannot cover because it reads the
    /// pre-present composite cache.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_readback_present(&mut self) -> Vec<u8> {
        self.compositor
            .test_present_to_canvas(&self.gpu.device, &self.gpu.queue, &mut self.doc)
    }

    /// Blocking readback of the present pass through the **production** view
    /// transform into a `viewport_w × viewport_h` target — what the surface
    /// would actually show. Unlike [`Self::test_readback_present`] (which forces
    /// an identity 1:1 transform) this exercises the real screen↔canvas mapping,
    /// so it can observe resize-induced squash/offset bugs that the
    /// identity/composite-cache readbacks are blind to.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_readback_viewport(&mut self, viewport_w: u32, viewport_h: u32) -> Vec<u8> {
        self.compositor.test_present_to_viewport(
            &self.gpu.device,
            &self.gpu.queue,
            &mut self.doc,
            viewport_w,
            viewport_h,
        )
    }

    /// Test-only animation tick. Headless `render()` returns early without
    /// driving `update_animations`, so tests that need to advance the
    /// veil / overlay / void master clock call this directly. Pass a
    /// monotonically increasing `wall_time` in seconds.
    pub fn test_tick_animations(&mut self, wall_time: f32) {
        self.compositor
            .update_animations(&self.gpu.queue, wall_time, &self.doc);
    }

    /// Blocking readback of a mask filter's R8 texture. For test assertions
    /// only. Resolves the mask filter on the host and reads its texture
    /// from the unified node-texture pool. Returns one byte per pixel.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_readback_mask(&self, host_id: LayerId) -> Vec<u8> {
        let mask_id = self
            .doc
            .mask_filter_id(host_id)
            .expect("host has no mask filter");
        let tex = self
            .compositor
            .node_texture(mask_id)
            .expect("mask texture not found");
        let ext = tex.layer_extent();
        crate::gpu::test_utils::readback_texture(
            &self.gpu.device,
            &self.gpu.queue,
            tex.texture(),
            tex.format(),
            ext.width,
            ext.height,
        )
    }

    /// Test-only: the current selection marching-ants overlay primitives.
    /// These are `FLAG_CANVAS_SPACE` (plane-space) dashed lines, so their
    /// `p0`/`p1` are plane coordinates — used to assert ant placement keeps
    /// tracking the selection's plane bounds across a crop.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_selection_overlay(&self) -> Vec<crate::gpu::overlay::OverlayPrimitive> {
        self.selection_overlay.clone()
    }

    /// Peek at the cached thumbnail bytes for any node id without queuing a
    /// fresh readback. Test-only — production callers go through
    /// [`node_thumbnail`] which intentionally also queues. The regression
    /// tests in `thumbnail_reactivity.rs` need a non-side-effecting peek so
    /// they can prove the auto-queue path populated the cache.
    pub fn test_thumbnail_cache_peek(&self, node_id: LayerId) -> Option<Vec<u8>> {
        self.thumbnail_cache.get(node_id).cloned()
    }

    /// Count of mid-stroke full-re-render fallbacks observed during the
    /// most recent stroke. Used by integration tests to assert that the
    /// checkpoint ring's coverage invariant kept fallback at zero across
    /// a stroke.
    pub fn test_stroke_full_rerender_events(&self) -> u32 {
        self.brush_full_rerender_events
    }

    /// Total dabs placed during the most recent stroke. `brush_perf` is
    /// reset at `begin_stroke`, so call this between `end_stroke` and the
    /// next `begin_stroke` to read the just-finished stroke's count.
    pub fn test_stroke_total_dabs(&self) -> u64 {
        self.brush_perf.dabs_placed as u64
    }

    // -----------------------------------------------------------------
    // BrushState accessors — keep call sites compact.
    // -----------------------------------------------------------------
    //
    // The brush's shared state lives in `tool_session` (generic) under
    // the type-keyed slot for `BrushState`. These helpers hide the
    // double indirection (lock guard → typed lookup) for the cases that
    // only need to read or bump a scalar.

    /// A clone of the active brush graph. Tests and the few external
    /// inspectors use this; per-frame paths inside the engine take a
    /// `tool_session.read()` guard directly to skip the clone.
    pub fn active_brush_graph(&self) -> crate::nodegraph::Graph<BrushWireType> {
        self.tool_session
            .read()
            .get::<crate::brush::state::BrushState>()
            .expect("BrushState registered at session init")
            .graph
            .clone()
    }

    /// Version counter snapshot. Bumped on every brush-graph mutation —
    /// drives editor-preview cache invalidation.
    pub fn brush_graph_version(&self) -> u64 {
        self.tool_session
            .read()
            .get::<crate::brush::state::BrushState>()
            .expect("BrushState registered at session init")
            .version
    }

    /// Topology version counter snapshot. Bumped only on changes that
    /// affect the brush's *identity* (graph topology, params, unwired
    /// non-exposed defaults). Drives dab-thumbnail cache invalidation.
    pub fn brush_topology_version(&self) -> u64 {
        self.tool_session
            .read()
            .get::<crate::brush::state::BrushState>()
            .expect("BrushState registered at session init")
            .topology_version
    }

    /// Bump the brush-graph version counter (e.g. after a scrub).
    pub(crate) fn bump_brush_graph_version(&self) {
        let mut tool = self.tool_session.write();
        let brush = tool
            .get_mut::<crate::brush::state::BrushState>()
            .expect("BrushState registered at session init");
        brush.version = brush.version.wrapping_add(1);
    }

    /// Bump both version counters (e.g. after a topology change).
    pub(crate) fn bump_brush_topology_version(&self) {
        let mut tool = self.tool_session.write();
        let brush = tool
            .get_mut::<crate::brush::state::BrushState>()
            .expect("BrushState registered at session init");
        brush.version = brush.version.wrapping_add(1);
        brush.topology_version = brush.topology_version.wrapping_add(1);
    }

    /// Block until all pending async readbacks complete. For tests only.
    /// Uses `device.poll(Wait)` to ensure mapping callbacks fire, then
    /// dispatches every completed readback through the shared handler —
    /// same semantics as a real frame's `poll_pending`.
    pub fn test_flush_readbacks(&mut self) {
        let _ = self.gpu.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        let completed = self.readbacks.poll(&self.gpu.device);
        for (ctx, pixels) in completed {
            self.handle_completed_readback(ctx, pixels);
        }
    }

    /// Block on the GPU device only — fire map callbacks — WITHOUT
    /// polling or dispatching the readback scheduler. On native this
    /// stands in for the browser event loop that resolves buffer
    /// mappings on web. For tests that must verify another code path
    /// (e.g. `poll_save_result`) does the scheduler drain itself.
    #[cfg(test)]
    pub fn test_wait_gpu(&mut self) {
        let _ = self.gpu.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
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
        // Per the no-duplicate-display-name rule, only the type_id ships
        // with the instance — display name is resolved by the UI via the
        // veil_types() registry table.
        assert!(json.get("displayName").is_none());
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
