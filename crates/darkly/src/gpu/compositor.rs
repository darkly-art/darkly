use crate::coord::CanvasRect;
use crate::document::Document;
use crate::gpu::atlas::LayerTexture;
use crate::gpu::blend::BlendPipelines;
use crate::gpu::compose_walk::{blend_bind_group, ProjectionState};
use crate::gpu::content_bounds::ContentBoundsPass;
use crate::gpu::effect_layers::EffectInstance;
use crate::gpu::histogram::HistogramPass;
use crate::gpu::overlay::ToolOverlay;
use crate::gpu::revisions::{Revisions, Tick};
use crate::gpu::screen_run::ScreenRun;
use crate::gpu::vector_content::VectorSubsystem;
use crate::gpu::view::{ViewTransform, DEFAULT_WORKSPACE_BG};
use crate::gpu::void::VoidRegistry;
use crate::gpu::void_content::LayerContent;
use crate::gpu::{blit_region, create_texture_with_view, create_uniform_buffer};
use crate::layer::{Layer, LayerId, RasterLayer, VectorLayer, VoidLayer};
use std::collections::HashMap;

/// Convert a `display.pixelFilter` string value to the float code stamped
/// into `ViewTransform.flags[0]`. Unknown values fall back to auto.
fn pixel_filter_from_str(mode: &str) -> f32 {
    match mode {
        "linear" => 0.0,
        "nearest" => 1.0,
        _ => 2.0,
    }
}

/// Maximum allowed layer texture dimension in either axis. Strokes that
/// would push the layer past this are clipped to current bounds.
pub const MAX_LAYER_DIM: u32 = 16384;

/// Layer-growth quantum. Bounds are rounded outward to multiples of this so
/// repeated cross-stroke growth amortizes — a typical stroke triggers 0–3
/// reallocations regardless of dab count.
pub const LAYER_GROWTH_CHUNK: u32 = 256;

/// Scale a node's canvas extent about `origin` by `(sx, sy)` — the per-node
/// extent math shared by image rescale's GPU pass and the engine's validation
/// (so both predict the same new size). Width/height clamp to a 1px minimum.
pub(crate) fn scaled_extent_about(
    e: crate::coord::CanvasRect,
    origin: crate::coord::CanvasPoint,
    sx: f32,
    sy: f32,
) -> crate::coord::CanvasRect {
    let nx0 = origin.x + ((e.origin.x - origin.x) as f32 * sx).round() as i32;
    let ny0 = origin.y + ((e.origin.y - origin.y) as f32 * sy).round() as i32;
    let nw = ((e.width as f32 * sx).round() as i32).max(1) as u32;
    let nh = ((e.height as f32 * sy).round() as i32).max(1) as u32;
    crate::coord::CanvasRect::from_xywh(nx0, ny0, nw, nh)
}

/// Map a node's canvas extent `e` through an orthogonal transform applied to
/// the `frame` rect (the canvas window for canvas ops). Pure integer pixel
/// algebra — the exact counterpart of [`scaled_extent_about`], shared by the
/// ortho GPU pass and the engine's document-side bookkeeping so both agree on
/// where every node lands. Rotations swap the frame's width/height and recentre
/// it (GIMP's `offset = (old_dim − new_dim)/2`); flips leave the frame put.
pub(crate) fn ortho_extent_about(
    e: crate::coord::CanvasRect,
    frame: crate::coord::CanvasRect,
    xform: crate::gpu::ortho_transform::OrthoXform,
) -> crate::coord::CanvasRect {
    let i0 = e.origin.x - frame.origin.x;
    let j0 = e.origin.y - frame.origin.y;
    let (ni0, nj0, nw, nh) = xform.map_local(i0, j0, e.width, e.height, frame.width, frame.height);
    let (ox, oy) = if xform.swaps_dims() {
        (
            frame.origin.x + (frame.width as i32 - frame.height as i32) / 2,
            frame.origin.y + (frame.height as i32 - frame.width as i32) / 2,
        )
    } else {
        (frame.origin.x, frame.origin.y)
    };
    crate::coord::CanvasRect::from_xywh(ox + ni0, oy + nj0, nw, nh)
}

/// Transient `w`×`h` texture used as the source/destination of an in-place
/// region mirror ([`Compositor::flip_node_region`]). Needs copy + sampling +
/// render-target usage; dropped when the encoder's work completes.
fn create_ortho_scratch(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> (wgpu::Texture, wgpu::TextureView) {
    create_texture_with_view(
        device,
        width,
        height,
        format,
        "ortho-scratch",
        wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
    )
}

/// One node's entry in the per-node texture pool: its GPU texture plus, for
/// R8 mask nodes, the "use my texture as a mask" bind group derived 1:1 from
/// it. Bundling the bind group with the texture it samples makes "evict the
/// texture but not its bind group" unrepresentable — one `remove`, one swap,
/// one slot.
pub(super) struct NodeSlot {
    pub(super) texture: LayerTexture,
    /// Cached mask bind group over `texture`'s view. `Some` only for mask
    /// (R8) nodes; consumed by the blend pipeline at composite time.
    /// Visibility gating happens in the render loop (which falls back to
    /// `default_mask_bind_group` for hidden masks).
    pub(super) mask_bg: Option<wgpu::BindGroup>,
}

/// Copy a node's `region` into a scratch texture, run a caller-supplied pass
/// into a second scratch, and copy the result back in place — the shared
/// copy-out → pass → copy-back plumbing behind both the layer/selection flip
/// ([`Compositor::flip_node_region`]) and destructive filters
/// ([`Compositor::filter_node_region`]). `run_pass` is handed `(device, queue,
/// encoder, src_view, mask_view, out_view, w, h, format)` and writes the
/// transformed region into `out_view`; `mask_view` is forwarded untouched so a
/// pass can gate on a selection shape. Returns `true` when the region was
/// non-empty and the pass ran (the caller marks the node dirty), `false` when
/// the node is missing or the clipped region is empty.
///
/// Takes `&node_textures` (not `&mut self`) so a caller can borrow it alongside
/// a disjoint `&self.<pass>` field captured by `run_pass` — an `&mut self`
/// method couldn't express that split (cf. `commit_undo_region`).
#[allow(clippy::too_many_arguments)]
fn run_filter_region<F>(
    node_textures: &HashMap<LayerId, NodeSlot>,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    encoder: &mut wgpu::CommandEncoder,
    node_id: LayerId,
    region: CanvasRect,
    mask_view: Option<&wgpu::TextureView>,
    run_pass: F,
) -> bool
where
    F: FnOnce(
        &wgpu::Device,
        &wgpu::Queue,
        &mut wgpu::CommandEncoder,
        &wgpu::TextureView,
        Option<&wgpu::TextureView>,
        &wgpu::TextureView,
        u32,
        u32,
        wgpu::TextureFormat,
    ),
{
    let (extent, format) = match node_textures.get(&node_id) {
        Some(s) => (s.texture.canvas_extent(), s.texture.format()),
        None => return false,
    };
    let region = match extent.intersect(region) {
        Some(r) if r.width > 0 && r.height > 0 => r,
        _ => return false,
    };
    let (w, h) = (region.width, region.height);
    let lx = (region.origin.x - extent.origin.x) as u32;
    let ly = (region.origin.y - extent.origin.y) as u32;

    let (src_scratch, src_view) = create_ortho_scratch(device, w, h, format);
    let (out_scratch, out_view) = create_ortho_scratch(device, w, h, format);
    let node_tex = node_textures[&node_id].texture.texture();

    blit_region(encoder, node_tex, (lx, ly), &src_scratch, (0, 0), w, h);
    run_pass(
        device, queue, encoder, &src_view, mask_view, &out_view, w, h, format,
    );
    blit_region(encoder, &out_scratch, (0, 0), node_tex, (lx, ly), w, h);
    true
}

/// Resolve a node id to the facts an async node pass (content bounds,
/// histogram) needs to dispatch: the texture's view, its dimensions, and its
/// format. Free function over the map (not a method) so callers can keep the
/// returned view borrowed while mutably driving a disjoint pass field.
fn node_view_info(
    node_textures: &HashMap<LayerId, NodeSlot>,
    node_id: LayerId,
) -> Option<(&wgpu::TextureView, u32, u32, wgpu::TextureFormat)> {
    let tex = &node_textures.get(&node_id)?.texture;
    let extent = tex.layer_extent();
    Some((tex.view(), extent.width, extent.height, tex.format()))
}

/// Read-only handle to an entity's GPU pixel storage. Returned by
/// [`Compositor::pixel_data_for`] so callers that need to schedule a
/// readback (today: the save pipeline) can find the texture for any
/// pixel-bearing entity uniformly, without knowing whether it lives in
/// the unified `node_textures` pool or the selection's ping-pong pair.
pub struct PixelDataRef<'a> {
    pub texture: &'a wgpu::Texture,
    pub format: wgpu::TextureFormat,
    pub width: u32,
    pub height: u32,
}

impl PixelDataRef<'_> {
    /// The savable region as a rect, for handing to a readback. Always the
    /// whole texture — a `LayerRect` is a function-local translation type and
    /// never a struct field (see `tests/coord_invariants.rs`), so it is built
    /// here rather than stored.
    pub fn rect(&self) -> crate::coord::LayerRect {
        crate::coord::LayerRect::from_xywh(0, 0, self.width, self.height)
    }
}

/// Outcome of a layer-grow request — distinguishes a genuine reallocation
/// (callers must rebase stroke scratch / region store) from a no-op.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum GrowOutcome {
    /// New extent already contained — no reallocation performed.
    NoChange,
    /// Layer reallocated to the new chunked extent.
    Grown { new_extent: CanvasRect },
    /// Growth refused because the new extent would exceed `MAX_LAYER_DIM`.
    /// The stroke caller should clip its dab to current bounds.
    AtCap,
}

/// Timing helpers — compile to no-ops unless `cfg(feature = "profile")`.
#[cfg(feature = "profile")]
mod perf {
    pub fn time(label: &str) {
        log::trace!("[perf] {label} start");
    }
    pub fn time_end(label: &str) {
        log::trace!("[perf] {label} end");
    }
}
#[cfg(not(feature = "profile"))]
mod perf {
    #[inline(always)]
    pub fn time(_: &str) {}
    #[inline(always)]
    pub fn time_end(_: &str) {}
}

/// A pair of accumulator textures for ping-pong compositing within a group.
pub(super) struct AccumPair {
    pub(super) textures: [wgpu::Texture; 2],
    pub(super) views: [wgpu::TextureView; 2],
}

/// GPU state for a non-passthrough group (including root).
/// Every group that composites its children to an isolated buffer owns one.
pub(super) struct GroupState {
    /// Ping-pong accumulator pair for compositing children.
    pub(super) accum: AccumPair,
    /// Tracks which accumulator is the current "source" (last written).
    /// Between composites this names the half holding the group's finished
    /// output — see [`GroupState::output_view`].
    pub(super) current_accum: usize,
    /// Uniform buffer holding opacity, blend_mode, isolated for blending
    /// this group's result into its parent.
    pub(super) uniform_buf: wgpu::Buffer,
    /// What the last walk of this group saw, and the composite of its lower
    /// children captured for reuse. `None` until the first walk records one.
    pub(super) walk_cache: Option<crate::gpu::compose_walk::WalkCache>,
}

impl GroupState {
    /// The accumulator half holding this group's finished composite.
    ///
    /// A group's output is whichever half its walk last wrote, which is why
    /// consumers select a resource by this index rather than reading one
    /// fixed view: the walk flips halves an unpredictable number of times,
    /// so "the output" is an index, not a texture. Valid from the end of the
    /// group's `compose_group` until the next one — nothing outside
    /// `compose_group` writes an accumulator.
    pub(super) fn output_index(&self) -> usize {
        self.current_accum
    }

    pub(super) fn output_view(&self) -> &wgpu::TextureView {
        &self.accum.views[self.current_accum]
    }

    pub(super) fn output_texture(&self) -> &wgpu::Texture {
        &self.accum.textures[self.current_accum]
    }
}

/// Per-instance GPU scaffolding shared by every layer that participates in
/// the standard blend pipeline (raster + void). Both kinds need a blend
/// uniform buffer and the same CPU-side mirror of opacity/blend/isolated;
/// only the *source of pixels* differs and that's split out into
/// [`LayerContent`]. One pool keyed by [`LayerId`] replaces the previous
/// `raster_cache` + `void_layers` split, so the compositor's lookup paths
/// (blend arm, uniforms write, dispose) don't dispatch on layer kind.
pub(super) struct LayerCache {
    /// Uniform buffer holding opacity + blend_mode + isolated + geometry.
    pub(super) uniform_buf: wgpu::Buffer,
    /// CPU shadow of `uniform_buf`'s last written contents — the GPU buffer
    /// is write-only, so readers (the floating-preview mirror, projection
    /// sync, the extent refresh on texture swap) consult this instead of
    /// reading the buffer back. Invariant: every `queue.write_buffer` to
    /// `uniform_buf` writes the same value here.
    ///
    /// The blend mode is stored as the registry-resolved gpu_value: the
    /// compositor never branches on which mode it is — the shader does — so
    /// the raw shader integer is mirrored rather than a registration pointer.
    pub(super) last_uniforms: BlendUniforms,
    /// Where this layer's pixels come from. Raster pixels arrive via paint;
    /// procedural pixels are regenerated on demand by a [`Void`] trait
    /// object before each composite.
    pub(super) content: LayerContent,
}

/// GPU-side realization protocol for a single content-layer kind.
///
/// Each [`Layer`] variant implements this so the compositor's `ensure_layer`
/// walk doesn't need to match on which kind it's looking at — the variant
/// knows how to allocate its own per-instance resources. Adding a new layer
/// kind means implementing this trait once on the new variant; no consumer
/// edit is required.
pub trait LayerKindGpu {
    fn realize_in(&self, compositor: &mut Compositor, device: &wgpu::Device, queue: &wgpu::Queue);
}

impl LayerKindGpu for Layer {
    fn realize_in(&self, compositor: &mut Compositor, device: &wgpu::Device, queue: &wgpu::Queue) {
        match self {
            Layer::Raster(r) => r.realize_in(compositor, device, queue),
            Layer::Void(v) => v.realize_in(compositor, device, queue),
            // Effect layers hold no per-instance GPU resource here — their
            // instances are realized by `sync_effect_instances`. They are
            // excluded from the content walk (`Layer::is_blend_content`), so
            // this is never reached, but the arm keeps the match total.
            Layer::Filter(_) => {}
            Layer::Vector(v) => v.realize_in(compositor, device, queue),
            // The divider composites nothing and owns no GPU resource; like a
            // filter it is excluded by `Layer::is_blend_content`.
            Layer::Divider(_) => {}
        }
    }
}

impl LayerKindGpu for VectorLayer {
    fn realize_in(&self, compositor: &mut Compositor, device: &wgpu::Device, queue: &wgpu::Queue) {
        // Allocate the storage texture + blend cache. The scene itself is
        // pushed separately by the engine (it owns fonts + shaping); this only
        // guarantees the GPU slot exists.
        compositor.ensure_vector_layer(device, queue, self.id);
    }
}

impl LayerKindGpu for RasterLayer {
    fn realize_in(&self, compositor: &mut Compositor, device: &wgpu::Device, queue: &wgpu::Queue) {
        compositor.ensure_raster_layer(device, queue, self.id, self.pixels.bounds);
    }
}

impl LayerKindGpu for VoidLayer {
    fn realize_in(&self, compositor: &mut Compositor, device: &wgpu::Device, queue: &wgpu::Queue) {
        let void = compositor.create_void_box(device, &self.void_type, &self.params);
        compositor.ensure_void_layer(device, queue, self.id, void);
    }
}

/// Uniforms for raster layer compositing. The shader samples the layer
/// texture at its own UV space, so we pass the layer's pixel offset and
/// size in canvas coordinates plus the canvas size — the fragment shader
/// translates per-pixel from canvas UV to layer UV.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct BlendUniforms {
    pub(super) opacity: f32,
    pub(super) blend_mode: u32,
    pub(super) isolated: u32,
    pub(super) _pad1: f32,
    /// Layer's (offset_x, offset_y) in canvas coordinates.
    pub(super) layer_offset: [f32; 2],
    /// Layer texture dimensions in pixels.
    pub(super) layer_size: [f32; 2],
}

impl BlendUniforms {
    /// Blend uniforms describing a texture that occupies `extent` in canvas
    /// space — the one constructor behind every uniform-buffer write, so the
    /// offset/size packing lives in exactly one place.
    pub(super) fn for_extent(
        opacity: f32,
        blend_mode: u32,
        isolated: bool,
        extent: CanvasRect,
    ) -> Self {
        Self {
            opacity,
            blend_mode,
            isolated: isolated as u32,
            _pad1: 0.0,
            layer_offset: [extent.origin.x as f32, extent.origin.y as f32],
            layer_size: [extent.width as f32, extent.height as f32],
        }
    }
}

/// Shared canvas-window geometry (`composite.wgsl` group 2). Single source of
/// truth for `canvas_size` + `canvas_origin` across every composite draw —
/// owned by the document, written once per resize in
/// [`Compositor::set_canvas_rect`]. Pulling these out of the per-layer
/// [`BlendUniforms`] makes the post-resize stale-geometry squash unrepresentable:
/// there is exactly one copy, and it cannot be left behind when a layer that was
/// created before the resize composites afterward.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct CanvasUniform {
    /// Canvas dimensions in pixels.
    pub(super) canvas_size: [f32; 2],
    /// Plane-space offset of the canvas window (`Document::canvas_origin`).
    pub(super) canvas_origin: [f32; 2],
}

/// Uniforms for the shared in-place apply pass (`in_place_apply.wgsl`).
///
/// Carries the canvas-window + mask geometry inline so the pass samples the
/// host's mask in its own plane space (matching `apply_mask`) without the
/// pipeline needing the shared canvas bind group, plus the modulation an effect
/// layer contributes — its blend mode and opacity. A masked passthrough group
/// leaves those at Normal and 1.0, which is exactly the lerp this pass used to
/// be.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct ApplyUniforms {
    pub(super) canvas_origin: [f32; 2],
    pub(super) canvas_size: [f32; 2],
    pub(super) mask_offset: [f32; 2],
    pub(super) mask_size: [f32; 2],
    pub(super) isolated: u32,
    pub(super) blend_mode: u32,
    pub(super) opacity: f32,
    pub(super) _pad0: u32,
}

/// GPU state for a masked passthrough group — the one in-place host that still
/// needs a snapshot. Its "after" is produced by an arbitrary number of child
/// passes writing straight into the parent accumulator, so unlike an effect
/// layer it cannot be redirected into a scratch and must be captured before the
/// children run.
pub(super) struct MaskSnapshotState {
    /// Snapshot of the parent accumulator before the children are inlined —
    /// the "before" of the apply pass.
    pub(super) snapshot: wgpu::Texture,
    pub(super) snapshot_view: wgpu::TextureView,
    /// Uniform buffer for the in-place apply shader.
    pub(super) uniform_buf: wgpu::Buffer,
}

pub struct StagedNodeTexture {
    pub node_id: LayerId,
    texture: LayerTexture,
}

impl StagedNodeTexture {
    pub fn canvas_extent(&self) -> CanvasRect {
        self.texture.canvas_extent()
    }

    pub fn texture(&self) -> &wgpu::Texture {
        self.texture.texture()
    }

    pub fn view(&self) -> &wgpu::TextureView {
        self.texture.view()
    }
}

/// GPU realization of the document: textures, pipelines, and render caches,
/// always rebuildable from the document on the next frame.
///
/// # Per-node map lifecycle invariant
///
/// Several fields are `HashMap<LayerId, _>` keyed by document node id. Each
/// follows exactly one of three lifecycle disciplines — a new per-node map
/// must pick one and say which:
///
/// - **Disposed in [`Self::dispose_node_texture`]** when the node is
///   permanently removed: the node slot (`node_textures`, texture + mask
///   bind group), `layer_cache`, the vector scene (`vector.scenes`),
///   `projection_states` (host delete), and the node's `revisions` entry.
/// - **Swept against the document in its own sync phase**, so entries whose
///   document-side reason has lapsed are dropped or re-ensured each frame:
///   the projection stale-sweep and snapshot re-ensure in
///   [`Self::sync_projection_states`], and the effect-instance retain in
///   `sync_effect_instances`. `mask_snapshot_state` is additionally released
///   by its host-keyed dispose and cleared wholesale on canvas resize;
///   `group_state` is recreated wholesale on canvas resize and the bake's
///   transient entry removes itself before returning.
/// - **Evicted wherever the texture it references is replaced or
///   destroyed**: `blend_bind_groups` — the swap/dispose retains and the
///   canvas-resize clear. A cached bind group must never outlive a view it
///   names.
pub struct Compositor {
    /// Per-group GPU state. Every non-passthrough group (including root)
    /// owns a GroupState with its own accumulators and composite cache.
    /// Root's state lives at group_state[self.root_id].
    pub(super) group_state: HashMap<LayerId, GroupState>,

    /// Implicit root group id. Mirrored from the document at construction
    /// time so the compositor can address its own root's `GroupState` /
    /// composite cache without re-deriving it on every call. Stays valid for
    /// the compositor's lifetime — root id is fixed once allocated.
    pub(super) root_id: LayerId,

    /// One pool of per-node GPU slots, keyed by node id. Holds raster
    /// layer textures (Rgba8Unorm), mask filter textures (R8Unorm), and
    /// any future pixel-bearing filter kinds — `LayerTexture.format`
    /// distinguishes them. A mask node's slot also carries the bind group
    /// derived from its texture ([`NodeSlot::mask_bg`]). One lookup per
    /// access, no fan-out.
    pub(super) node_textures: HashMap<LayerId, NodeSlot>,

    /// Default mask bind group using the 1×1 white texture (pass-through
    /// fallback for hosts without a visible mask filter).
    pub(super) default_mask_bind_group: wgpu::BindGroup,

    /// Cached blend bind groups for `compose_children`. Key is
    /// `(parent_group, child_id, src_accum_idx)` — both ping-pong sides
    /// get their own entry per child because the source accum view flips
    /// every layer. Entries are invalidated in `dispose_node_texture` and
    /// `resize_node_texture` against the affected node id (either as
    /// parent or child); the floating-target case bypasses the cache so
    /// preview-state ephemera never leak in.
    pub(super) blend_bind_groups: HashMap<(LayerId, LayerId, u8), wgpu::BindGroup>,

    /// Pre-built GPU objects per content layer (raster + void). Keyed by
    /// the document's [`LayerId`] — both kinds share the same blend
    /// pipeline path, so collapsing them into one pool means the blend
    /// arm, uniforms write, and dispose all do exactly one lookup.
    pub(super) layer_cache: HashMap<LayerId, LayerCache>,

    pub(super) blend_pipelines: BlendPipelines,

    // --- Passthrough Group Mask (Photoshop-style snapshot-lerp) ---
    /// The shared in-place apply pass, one pipeline per target format a node
    /// can have. RGBA8 serves layer accumulators and raster nodes; R8 serves a
    /// mask node, which is what lets an effect declaring `R8Unorm` be applied
    /// destructively to a mask.
    pub(super) in_place_apply_pipelines: [(wgpu::TextureFormat, wgpu::RenderPipeline); 2],
    /// Per-group GPU state for passthrough groups with masks.
    pub(super) mask_snapshot_state: HashMap<LayerId, MaskSnapshotState>,

    // --- Leaf mask (de-fused projection + apply_mask) ---
    /// Pass that modulates a projection's alpha by a mask in the mask's own
    /// space (`apply_mask.wgsl`).
    pub(super) apply_mask_pipeline: crate::gpu::apply_mask::ApplyMaskPipeline,
    /// Pooled per-host projection state, allocated lazily for leaf layers with
    /// a visible mask and released on mask remove/hide, host delete, or canvas
    /// resize. Keyed by the host layer id.
    pub(super) projection_states: HashMap<LayerId, ProjectionState>,

    pub(super) present_pipeline: wgpu::RenderPipeline,
    /// Present pipeline targeting the accum format (Rgba8Unorm) for veil input.
    pub(super) present_to_effects_pipeline: wgpu::RenderPipeline,
    pub(super) _present_bind_group_layout: wgpu::BindGroupLayout,
    /// Present bind groups over the root group's accumulator halves; the
    /// root's [`GroupState::output_index`] picks one at draw time.
    pub(super) present_cache_bind_groups: [wgpu::BindGroup; 2],
    /// View transform uniform buffer for the present shader.
    pub(super) view_uniform_buf: wgpu::Buffer,

    /// Shared canvas-geometry uniform ([`CanvasUniform`]) — the single copy of
    /// `canvas_size` + `canvas_origin` bound to every composite draw (group 2).
    /// Written only by [`Self::set_canvas_rect`].
    pub(super) canvas_uniform_buf: wgpu::Buffer,
    /// Bind group wrapping `canvas_uniform_buf` for the blend pipeline's group 2.
    /// Stable across frames — only the buffer *contents* change on resize.
    pub(super) canvas_bind_group: wgpu::BindGroup,

    pub(super) sampler: wgpu::Sampler,

    /// Every source of truth the compositor's derived state can go stale
    /// against. Mutations bump a source; consumers compare their own stamp
    /// on read.
    pub(super) revisions: Revisions,
    /// The clock value the composite in `group_state`'s caches was built at.
    /// Compared against [`Revisions::latest_composite_input`] to decide
    /// whether a frame has anything to do.
    pub(super) composite_built: Tick,
    /// The clock value the last frame that actually reached the surface
    /// reflected. Compared against [`Revisions::latest_visual`].
    pub(super) presented: Tick,
    /// Composites actually encoded. Lets a test distinguish "produced the
    /// right pixels" from "produced them by recompositing when it should
    /// have skipped", which a pixel assertion alone cannot see.
    #[cfg(any(test, feature = "testing"))]
    pub(super) composite_runs: u64,
    /// Group walks that resumed from a captured prefix, and walks that found
    /// nothing changed at all. Anti-vacuity instruments: a reuse test that
    /// silently full-walks proves nothing, and only these can tell the two
    /// apart. They count branches, never time.
    #[cfg(any(test, feature = "testing"))]
    pub(super) walk_resumes: u64,
    #[cfg(any(test, feature = "testing"))]
    pub(super) walk_all_clean: u64,

    pub(super) canvas_width: u32,
    pub(super) canvas_height: u32,
    /// Plane-space offset of the canvas window, mirrored from
    /// `Document::canvas_origin`. Drives the selection-mask UV seam and the
    /// window→plane mapping in the layer composite shader. Updated together
    /// with `canvas_width`/`canvas_height` by `set_canvas_rect`.
    pub(super) canvas_origin: crate::coord::CanvasPoint,

    pub(super) screen_run: ScreenRun,

    /// Lazily-pipeline-cached registry of every void type built into the
    /// binary. Engine queries this for `void_types()` and `add_void_layer`
    /// goes through it to build the per-instance trait object.
    pub(super) void_registry: VoidRegistry,

    /// The one registry of every effect type built into the binary, shared by
    /// every consumer of an effect pipeline: effect *layers* (per-frame
    /// accumulator transform), the destructive apply path (one-shot document
    /// edit), the screen-space chain, and the picker previews.
    /// Compositor-owned because the cached pipelines are GPU resources, exactly
    /// like `void_registry`'s.
    pub(super) effect_registry: crate::gpu::effect::EffectRegistry,

    /// Per-effect-layer realized state — the instance, its cache, and the facts
    /// it was built against. Rebuilt by `sync_effect_instances` in the
    /// pre-compose phase whenever any of those facts drift; compose then merely
    /// encodes. Entries for removed effect layers are pruned there too.
    pub(super) effect_instances: HashMap<LayerId, EffectInstance>,

    /// How many times an effect instance has been built from scratch —
    /// pipeline lookup, `ScaledEffect::prepare`, fresh bind groups. Steady
    /// state is one per effect layer for the life of the document; anything
    /// that grows with the frame count means an instance is being rebuilt
    /// rather than reused, which is invisible except as lag.
    pub(super) effect_rebuilds: u64,

    /// Where a canvas-space effect writes its result, so the apply pass can
    /// read both the image before the effect and the image after it and still
    /// have somewhere to write.
    ///
    /// One for the whole space, not one per layer: the passes are sequential
    /// within a single encoder, so no two effects hold it at once. Sized with
    /// the accumulators and recreated with them.
    pub(super) canvas_apply_scratch: Option<(wgpu::Texture, wgpu::TextureView)>,

    /// Downscale/upscale pipelines for canvas-space effects that render below
    /// full resolution.
    pub(super) canvas_scaling_pipelines: crate::gpu::effect_scaling::ScalingPipelines,

    // --- Floating Content Transform ---
    pub(super) transform_pass: crate::gpu::transform::TransformPass,
    pub(super) transform_session: Option<crate::gpu::floating_preview::TransformGpuSession>,

    // --- Image rescale resampling ---
    pub(super) rescale_pass: crate::gpu::rescale::RescalePass,

    // --- Orthogonal (flip / 90° rotate) transforms ---
    pub(super) ortho_pass: crate::gpu::ortho_transform::OrthoTransformPass,

    // --- Selection (global) ---
    /// GPU realisation of the document's selection filter — ping-pong R8
    /// textures + brush/paint bind groups. `None` until the engine allocates
    /// the selection filter; once allocated, lives for the document's
    /// lifetime. Pixel metadata (active toggle, tight bounds, CPU cache)
    /// lives on `Document.selection.kind` (`SelectionFilter`).
    pub(super) selection_state: Option<crate::gpu::selection::SelectionState>,

    // --- Tool Overlay ---
    pub(super) tool_overlay: ToolOverlay,
    /// Cached view transform for overlay forward matrix computation.
    pub(super) cached_view_transform: ViewTransform,
    /// Workspace color drawn by the present shader outside the canvas
    /// rectangle. Stamped onto every transform on upload, so changing it
    /// only requires re-uploading the cached transform.
    pub(super) viewport_bg: [f32; 4],
    /// Pixel filter mode for the present shader's canvas-to-screen sample.
    /// 0 = linear (smooth), 1 = nearest (hard pixels), 2 = auto (nearest
    /// when zoom > 1, linear otherwise — decided in the shader from the
    /// matrix). Stamped onto `flags[0]` of the transform on upload.
    pub(super) pixel_filter: f32,

    // --- Content Bounds (GPU compute) ---
    pub(super) content_bounds: ContentBoundsPass,

    // --- Histogram (GPU compute) ---
    pub(super) histogram: HistogramPass,
    /// The filter layer whose input histogram is being computed (the Levels
    /// editor's selected filter), or `None` when no histogram is wanted.
    pub(super) histogram_target: Option<LayerId>,
    /// A node whose *own* texture is histogrammed on demand (the destructive
    /// Levels modal, which has no filter arm to bin). Pumped by
    /// [`pump_node_histogram`](Self::pump_node_histogram), not the compose walk.
    pub(super) node_histogram_target: Option<LayerId>,

    // --- Frame Scheduler ---
    /// Monotonic frame counter, incremented on each rAF tick.
    /// Systems fire when `frame_count % divisor == 0`.
    pub(super) frame_count: u64,
    /// Last wall-clock time for dt computation.
    pub(super) last_wall_time: f32,

    /// Reused buffer for the "ids of dirty procedural layers" pass in
    /// `encode_dirty_layer_content`. Cleared at the top of the pass and
    /// drained before returning, so the only retained allocation is the
    /// `Vec` capacity itself.
    pub(super) dirty_procedural_scratch: Vec<LayerId>,

    /// Vector-layer realization state: the shared Vello renderer plus the
    /// per-layer scenes ([`VectorSubsystem`]).
    pub(super) vector: VectorSubsystem,
}

impl Compositor {
    /// Create an accumulator texture at padded canvas dimensions.
    pub(super) fn make_accum_texture(
        device: &wgpu::Device,
        padded_w: u32,
        padded_h: u32,
        label: &str,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        create_texture_with_view(
            device,
            padded_w,
            padded_h,
            wgpu::TextureFormat::Rgba8Unorm,
            label,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
        )
    }

    /// Create a GroupState (accum pair + uniforms).
    pub(super) fn create_group_state(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        padded_w: u32,
        padded_h: u32,
        canvas_origin: crate::coord::CanvasPoint,
        group_id: LayerId,
    ) -> GroupState {
        let (a0, v0) =
            Self::make_accum_texture(device, padded_w, padded_h, &format!("accum-{group_id:?}-0"));
        let (a1, v1) =
            Self::make_accum_texture(device, padded_w, padded_h, &format!("accum-{group_id:?}-1"));

        let normal = crate::gpu::blend_mode::registry().default().gpu_value;
        // The group's window-sized accumulator occupies exactly the canvas
        // window in plane space, so describing it as a "layer" at
        // `layer_offset = canvas_origin`, `layer_size = canvas_size` makes the
        // shared-canvas plane round-trip in `composite.wgsl` collapse to an
        // identity sample.
        let uniforms = BlendUniforms::for_extent(
            1.0,
            normal,
            false,
            CanvasRect::from_xywh(canvas_origin.x, canvas_origin.y, padded_w, padded_h),
        );
        let uniform_buf =
            create_uniform_buffer::<BlendUniforms>(device, &format!("group-uniforms-{group_id:?}"));
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        GroupState {
            accum: AccumPair {
                textures: [a0, a1],
                views: [v0, v1],
            },
            current_accum: 0,
            uniform_buf,
            walk_cache: None,
        }
    }

    /// Build the present bind groups that sample the root group's output —
    /// one per accumulator half, selected at draw time by the root's
    /// [`GroupState::output_index`].
    ///
    /// A pair rather than one bind group over a fixed texture: a bind group
    /// is built against a specific view, and which half holds the composite
    /// depends on how many times the walk flipped. Building both once is the
    /// same trade `blend_bind_groups` already makes for children.
    fn make_present_cache_bind_groups(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        root_state: &GroupState,
        sampler: &wgpu::Sampler,
        view_uniform_buf: &wgpu::Buffer,
    ) -> [wgpu::BindGroup; 2] {
        std::array::from_fn(|half| {
            Self::make_present_cache_bind_group(
                device,
                layout,
                &root_state.accum.views[half],
                sampler,
                view_uniform_buf,
            )
        })
    }

    /// Build one present bind group over a given view.
    fn make_present_cache_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        cache_view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        view_uniform_buf: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("present-bg-cache"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(cache_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: view_uniform_buf.as_entire_binding(),
                },
            ],
        })
    }

    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        root_id: LayerId,
    ) -> Self {
        // Accumulator dimensions match layer textures exactly (no tile padding).
        let padded_w = width;
        let padded_h = height;

        let accum_format = wgpu::TextureFormat::Rgba8Unorm;

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("darkly-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let blend_pipelines = BlendPipelines::new(device, accum_format);

        let apply_mask_pipeline = crate::gpu::apply_mask::ApplyMaskPipeline::new(
            device,
            accum_format,
            &blend_pipelines.mask_bind_group_layout,
            &blend_pipelines.canvas_bind_group_layout,
        );

        // Create default 1x1 white mask texture (mask_alpha=1.0 = no effect)
        let (default_mask_tex, default_mask_view) = create_texture_with_view(
            device,
            1,
            1,
            wgpu::TextureFormat::R8Unorm,
            "default-mask-1x1",
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &default_mask_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255u8],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(1),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let default_mask_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("default-mask-bg"),
            layout: &blend_pipelines.mask_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&default_mask_view),
            }],
        });

        // --- In-place apply pipelines (effect layers, masked passthrough
        // groups, destructive region applies) ---
        // Reuses the blend BGL for group 0 (before, after, sampler, uniforms)
        // and the mask BGL for group 1 (mask texture). One per target format,
        // because a pipeline is compiled against exactly one.
        let in_place_apply_pipelines = {
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("in-place-apply-shader"),
                source: wgpu::ShaderSource::Wgsl(
                    crate::gpu::canvas_lib::with_canvas_lib(
                        &crate::gpu::blend_mode::build_in_place_apply_source(),
                    )
                    .into(),
                ),
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("in-place-apply-pipeline-layout"),
                bind_group_layouts: &[
                    Some(&blend_pipelines.bind_group_layout),
                    Some(&blend_pipelines.mask_bind_group_layout),
                ],
                immediate_size: 0,
            });
            let make = |format: wgpu::TextureFormat| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("in-place-apply-pipeline"),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("vs_main"),
                        buffers: &[],
                        compilation_options: Default::default(),
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some("fs_main"),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                        compilation_options: Default::default(),
                    }),
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleList,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    multiview_mask: None,
                    cache: None,
                })
            };
            [
                (accum_format, make(accum_format)),
                (
                    wgpu::TextureFormat::R8Unorm,
                    make(wgpu::TextureFormat::R8Unorm),
                ),
            ]
        };
        // View transform uniform buffer (present shader binding 2)
        let view_uniform_buf =
            create_uniform_buffer::<ViewTransform>(device, "view-transform-uniform");
        let identity = ViewTransform::identity();
        queue.write_buffer(&view_uniform_buf, 0, bytemuck::bytes_of(&identity));

        // Present pipeline: blit accumulator to surface
        let _present_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("present-bgl"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        let present_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("present-pipeline-layout"),
                bind_group_layouts: &[Some(&_present_bind_group_layout)],
                immediate_size: 0,
            });

        let present_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("present-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/present.wgsl").into()),
        });

        // One present pipeline per target format: the surface for display,
        // the accum format for handing the presented image to the veil chain.
        let make_present_pipeline = |label: &str, format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&present_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &present_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &present_shader,
                    entry_point: Some("fs_present"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let present_pipeline = make_present_pipeline("present-pipeline", surface_format);
        let present_to_effects_pipeline =
            make_present_pipeline("present-to-veil-pipeline", accum_format);

        // Create root GroupState (root is always a non-passthrough group)
        // A fresh document's canvas window is anchored at the plane origin.
        let canvas_origin = crate::coord::CanvasPoint::new(0, 0);
        let root_state =
            Self::create_group_state(device, queue, padded_w, padded_h, canvas_origin, root_id);

        // Shared canvas-geometry uniform (group 2) — the single copy of
        // canvas_size + canvas_origin for every composite draw.
        let canvas_uniform = CanvasUniform {
            canvas_size: [width as f32, height as f32],
            canvas_origin: [canvas_origin.x as f32, canvas_origin.y as f32],
        };
        let canvas_uniform_buf =
            create_uniform_buffer::<CanvasUniform>(device, "canvas-geometry-uniform");
        queue.write_buffer(&canvas_uniform_buf, 0, bytemuck::bytes_of(&canvas_uniform));
        let canvas_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("canvas-geometry-bg"),
            layout: &blend_pipelines.canvas_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: canvas_uniform_buf.as_entire_binding(),
            }],
        });

        // Present samples whichever root accumulator half the walk ends on.
        let present_cache_bind_groups = Self::make_present_cache_bind_groups(
            device,
            &_present_bind_group_layout,
            &root_state,
            &sampler,
            &view_uniform_buf,
        );

        let mut group_state = HashMap::new();
        group_state.insert(root_id, root_state);

        let screen_run = ScreenRun::new(device, sampler.clone(), surface_format, accum_format);

        let tool_overlay = ToolOverlay::new(device, queue, surface_format);

        let transform_pass = crate::gpu::transform::TransformPass::new(device, queue);
        let rescale_pass = crate::gpu::rescale::RescalePass::new(device);
        let ortho_pass = crate::gpu::ortho_transform::OrthoTransformPass::new(device);
        let content_bounds = ContentBoundsPass::new(device);
        let histogram = HistogramPass::new(device);

        let mut compositor = Compositor {
            group_state,
            root_id,
            node_textures: HashMap::new(),
            default_mask_bind_group,
            blend_bind_groups: HashMap::new(),
            layer_cache: HashMap::new(),
            blend_pipelines,

            in_place_apply_pipelines,
            mask_snapshot_state: HashMap::new(),
            apply_mask_pipeline,
            projection_states: HashMap::new(),
            present_pipeline,
            present_to_effects_pipeline,
            _present_bind_group_layout,
            present_cache_bind_groups,
            view_uniform_buf,
            canvas_uniform_buf,
            canvas_bind_group,
            sampler,
            revisions: Revisions::new(),
            composite_built: 0,
            presented: 0,
            #[cfg(any(test, feature = "testing"))]
            composite_runs: 0,
            #[cfg(any(test, feature = "testing"))]
            walk_resumes: 0,
            #[cfg(any(test, feature = "testing"))]
            walk_all_clean: 0,
            canvas_width: width,
            canvas_height: height,
            canvas_origin: crate::coord::CanvasPoint::new(0, 0),
            screen_run,
            void_registry: VoidRegistry::new(),
            effect_registry: crate::gpu::effect::EffectRegistry::new(),
            effect_instances: HashMap::new(),
            effect_rebuilds: 0,
            canvas_apply_scratch: None,
            canvas_scaling_pipelines: crate::gpu::effect_scaling::ScalingPipelines::new(
                device,
                accum_format,
                "canvas-effect",
            ),
            transform_pass,
            transform_session: None,
            rescale_pass,
            ortho_pass,
            selection_state: None,
            content_bounds,
            histogram,
            histogram_target: None,
            node_histogram_target: None,
            tool_overlay,
            cached_view_transform: identity,
            viewport_bg: DEFAULT_WORKSPACE_BG,
            // Auto until the engine pushes the persisted preference via
            // `set_pixel_filter` — config is session/host state the
            // compositor never reads itself.
            pixel_filter: 2.0,
            frame_count: 0,
            last_wall_time: 0.0,
            dirty_procedural_scratch: Vec::new(),
            vector: VectorSubsystem {
                renderer: None,
                scenes: HashMap::new(),
            },
        };
        // Nothing has been composited yet, and the frame gate deliberately
        // ignores the target bumps construction performs — without this the
        // first frame would compare clean and present a blank canvas.
        compositor.revisions.bump_document();
        compositor
    }

    /// Ensure GPU state exists for a content layer (raster or void),
    /// reading the kind off the document's [`Layer`] enum. Engine paths
    /// that walk the doc tree without knowing which kind each entry is
    /// (notably `sync_compositor_layers` after a load or undo) go through
    /// this rather than dispatching kind themselves — the compositor
    /// already knows about both kinds, so the dispatch lives here, once.
    ///
    /// Idempotent — both inner paths are no-ops when the layer is already
    /// allocated. Engine paths that *are* creating a layer of known kind
    /// (e.g. `add_raster_layer`, `add_void_layer`, paste, flatten) keep
    /// using the kind-specific entry points below; the caller already has
    /// the right inputs in hand.
    pub fn ensure_layer(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, layer: &Layer) {
        layer.realize_in(self, device, queue);
    }

    /// Create GPU texture + uniform buffer for a new raster layer.
    /// Called once when a layer is added, never in the render loop.
    /// `bounds` describes the layer's pixel-space extent in canvas
    /// coordinates — typically canvas-aligned and canvas-sized, but a
    /// paste of an oversized image may pre-allocate larger bounds.
    pub fn ensure_raster_layer(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layer_id: LayerId,
        bounds: crate::coord::CanvasRect,
    ) {
        if self.node_textures.contains_key(&layer_id) {
            return;
        }

        let layer_tex = LayerTexture::with_bounds(device, bounds);
        self.insert_content_layer(device, queue, layer_id, layer_tex, LayerContent::Raster);
        // A freshly-allocated layer still needs a thumbnail slot — without
        // this, an empty new layer renders as "no thumbnail" in the panel
        // until the user paints. Part of the "any write/alloc to a node
        // texture marks it dirty" invariant; see `mark_node_pixels_dirty`.
        self.mark_node_pixels_dirty(layer_id);
    }

    /// Shared allocation core behind every content-layer kind (raster, void,
    /// vector): build default blend uniforms for the texture's extent, create
    /// and write the uniform buffer, and insert the node-texture and
    /// layer-cache entries. Guards and dirty-marking stay with the per-kind
    /// entry points — raster marks node pixels (thumbnail invariant),
    /// void/vector mark the document — as do per-kind extras (procedural
    /// sidecar, vector scene).
    pub(super) fn insert_content_layer(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layer_id: LayerId,
        layer_tex: LayerTexture,
        content: LayerContent,
    ) {
        let bounds = layer_tex.canvas_extent();
        let normal = crate::gpu::blend_mode::registry().default().gpu_value;
        let uniforms = BlendUniforms::for_extent(1.0, normal, false, bounds);
        let uniform_buf =
            create_uniform_buffer::<BlendUniforms>(device, &format!("blend-uniforms-{layer_id:?}"));
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        self.node_textures.insert(
            layer_id,
            NodeSlot {
                texture: layer_tex,
                mask_bg: None,
            },
        );
        self.layer_cache.insert(
            layer_id,
            LayerCache {
                uniform_buf,
                last_uniforms: uniforms,
                content,
            },
        );
    }

    /// Resize a node's GPU texture (raster layer or mask filter) to a new
    /// canvas extent, copying old contents into the new texture at the offset
    /// that preserves their canvas-space anchor. Thin wrapper over
    /// [`realloc_node_texture`](Self::realloc_node_texture) with `copy_old =
    /// true`.
    ///
    /// **Lockstep growth across host + filters is the engine's job** — it
    /// owns the document and walks `host.filters` to call this helper for
    /// each non-locked sibling. The compositor is single-node here.
    pub fn resize_node_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        node_id: LayerId,
        new_extent: CanvasRect,
    ) {
        self.realloc_node_texture(device, queue, encoder, node_id, new_extent, true);
    }

    /// Reallocate a node's GPU texture (raster layer or mask filter) to a new
    /// canvas extent.
    ///
    /// **Pure realization.** A faithful reflection of the requested extent — it
    /// does not compute unions or chunk-align; the caller chooses `new_extent`.
    /// Format-agnostic: the existing texture's format drives reallocation. If
    /// the node is unknown or already at `new_extent`, this is a no-op.
    ///
    /// When `copy_old` is `true`, the old contents are
    /// `copy_texture_to_texture`'d into the new texture at the canvas-anchored
    /// offset; uncovered pixels start zeroed for RGBA (transparent) and
    /// white-filled for R8 (full reveal). When `copy_old` is `false`, the new
    /// texture is left at its allocation default (cleared) — used by undo
    /// restores that immediately upload the authoritative pixels themselves.
    pub fn realloc_node_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        node_id: LayerId,
        new_extent: CanvasRect,
        copy_old: bool,
    ) {
        let (current, format) = match self.node_textures.get(&node_id) {
            Some(s) => (s.texture.canvas_extent(), s.texture.format()),
            None => return,
        };
        if current == new_extent {
            return;
        }

        let new_tex = LayerTexture::new_for_format(device, queue, format, new_extent);

        if copy_old {
            let old_tex = &self
                .node_textures
                .get(&node_id)
                .expect("node_textures entry checked above")
                .texture;
            let copy_dst_x = (current.origin.x - new_extent.origin.x) as u32;
            let copy_dst_y = (current.origin.y - new_extent.origin.y) as u32;
            blit_region(
                encoder,
                old_tex.texture(),
                (0, 0),
                new_tex.texture(),
                (copy_dst_x, copy_dst_y),
                current.width,
                current.height,
            );
        }

        self.swap_node_texture(device, queue, node_id, new_tex);

        // Resize rewrites the texture; thumbnail must reflect the new
        // extent + transferred pixels.
        self.mark_node_pixels_dirty(node_id);
        self.mark_dirty();
    }

    /// Allocate and populate an unpublished replacement texture. No compositor
    /// mapping or document state changes until [`publish_staged_node_textures`].
    pub fn prepare_staged_node_texture(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        node_id: LayerId,
        new_extent: CanvasRect,
    ) -> Option<StagedNodeTexture> {
        let current = &self.node_textures.get(&node_id)?.texture;
        let texture = LayerTexture::new_for_format(device, queue, current.format(), new_extent);
        let overlap = current.canvas_extent().intersect(new_extent)?;
        let src = current.canvas_to_layer_rect(overlap)?;
        let dst_x = (overlap.x0() - new_extent.x0()) as u32;
        let dst_y = (overlap.y0() - new_extent.y0()) as u32;
        blit_region(
            encoder,
            current.texture(),
            (src.x0(), src.y0()),
            texture.texture(),
            (dst_x, dst_y),
            overlap.width,
            overlap.height,
        );
        Some(StagedNodeTexture { node_id, texture })
    }

    pub fn publish_staged_node_textures(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        staged: Vec<StagedNodeTexture>,
    ) {
        for target in staged {
            self.swap_node_texture(device, queue, target.node_id, target.texture);
            self.mark_node_pixels_dirty(target.node_id);
        }
        self.mark_dirty();
    }

    /// Replace a node's texture handle and rebuild the cached state that
    /// referenced the old view. Shared by every path that swaps a node texture
    /// out from under the compositor (resize/realloc, rescale).
    pub(super) fn swap_node_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        node_id: LayerId,
        new_tex: LayerTexture,
    ) {
        let had_mask_bg = self
            .node_textures
            .get(&node_id)
            .is_some_and(|s| s.mask_bg.is_some());
        self.node_textures.insert(
            node_id,
            NodeSlot {
                texture: new_tex,
                mask_bg: None,
            },
        );

        // A content layer's blend uniform bakes in the texture's canvas extent
        // (`layer_offset` / `layer_size`). The extent may have just changed, so
        // refresh it, keeping the shadowed blend props — otherwise the
        // composite samples the new texture through stale geometry (the
        // post-resize squash `BlendUniforms` is designed to make
        // unrepresentable). Masks have no layer_cache entry and are unaffected.
        if let Some(cache) = self.layer_cache.get_mut(&node_id) {
            let ext = self.node_textures[&node_id].texture.canvas_extent();
            let prev = cache.last_uniforms;
            let uniforms =
                BlendUniforms::for_extent(prev.opacity, prev.blend_mode, prev.isolated != 0, ext);
            queue.write_buffer(&cache.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
            cache.last_uniforms = uniforms;
        }

        // Any cached blend bind groups using this node (as parent or child)
        // reference the now-replaced texture view; drop them so the next
        // composite re-creates against the fresh handle.
        self.blend_bind_groups
            .retain(|(parent, child, _), _| *parent != node_id && *child != node_id);

        // If the old slot carried a mask bind group, rebuild it against the
        // freshly-allocated view. The blend stage holds no other reference.
        if had_mask_bg {
            let view = self.node_textures[&node_id].texture.view();
            let mask_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("mask-bg-{node_id:?}")),
                layout: &self.blend_pipelines.mask_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                }],
            });
            self.node_textures
                .get_mut(&node_id)
                .expect("slot inserted above")
                .mask_bg = Some(mask_bg);
        }
    }

    /// Resample each node's texture from its current extent into a new extent
    /// scaled about the canvas origin by `(sx, sy)` — the GPU half of image
    /// rescale. Replaces each node texture (rebuilding cached bind groups via
    /// [`swap_node_texture`](Self::swap_node_texture)) and marks pixels dirty.
    ///
    /// The engine owns the document side (extent bounds, undo snapshots) and
    /// reads the resulting extents back from `node_texture(id).canvas_extent()`.
    pub fn rescale_nodes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        node_ids: &[LayerId],
        sx: f32,
        sy: f32,
    ) {
        let origin = self.canvas_origin;
        for &id in node_ids {
            let (old_extent, format) = match self.node_textures.get(&id) {
                Some(s) => (s.texture.canvas_extent(), s.texture.format()),
                None => continue,
            };
            let new_extent = scaled_extent_about(old_extent, origin, sx, sy);
            let new_tex = {
                let src = &self
                    .node_textures
                    .get(&id)
                    .expect("node_textures entry checked above")
                    .texture;
                self.rescale_pass.resample_node(
                    device, queue, encoder, src, new_extent, origin, sx, sy, format,
                )
            };
            self.swap_node_texture(device, queue, id, new_tex);
            self.mark_node_pixels_dirty(id);
        }
        self.mark_dirty();
    }

    /// Orthogonally transform each node's texture about `frame` (the canvas
    /// window for canvas flip/rotate) — the exact, no-resample counterpart of
    /// [`rescale_nodes`](Self::rescale_nodes). Each node moves to
    /// [`ortho_extent_about`]'s computed extent (rotations also swap w/h);
    /// the texture is replaced via [`swap_node_texture`](Self::swap_node_texture).
    /// The engine owns the document side (extent bounds, undo snapshots) and
    /// reads results back from `node_texture(id).canvas_extent()`.
    pub fn ortho_transform_nodes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        node_ids: &[LayerId],
        frame: CanvasRect,
        xform: crate::gpu::ortho_transform::OrthoXform,
    ) {
        for &id in node_ids {
            let (old_extent, format) = match self.node_textures.get(&id) {
                Some(s) => (s.texture.canvas_extent(), s.texture.format()),
                None => continue,
            };
            let new_extent = ortho_extent_about(old_extent, frame, xform);
            let new_tex = {
                let src = &self
                    .node_textures
                    .get(&id)
                    .expect("node_textures entry checked above")
                    .texture;
                self.ortho_pass
                    .remap_node(device, queue, encoder, src, new_extent, xform, format)
            };
            self.swap_node_texture(device, queue, id, new_tex);
            self.mark_node_pixels_dirty(id);
        }
        self.mark_dirty();
    }

    /// Mirror (`FlipH`/`FlipV`) a node's `region` in place about that region's
    /// centre — the layer/selection flip primitive. Where `mask_view` (a
    /// region-sized R8) is selected the texel takes the mirror, elsewhere it
    /// passes through, so non-rectangular selections clip exactly; `None`
    /// mirrors the whole region. No extent change — `region` must already be
    /// clipped to the node extent by the caller (the document bbox center is
    /// the caller's to choose). Pixels are copied out, permuted, copied back.
    pub fn flip_node_region(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        node_id: LayerId,
        region: CanvasRect,
        xform: crate::gpu::ortho_transform::OrthoXform,
        mask_view: Option<&wgpu::TextureView>,
    ) {
        let ran = run_filter_region(
            &self.node_textures,
            device,
            queue,
            encoder,
            node_id,
            region,
            mask_view,
            |dev, q, enc, src, mask, out, w, h, fmt| match mask {
                Some(mv) => self
                    .ortho_pass
                    .render_mirror_masked(dev, q, enc, src, mv, out, w, h, xform, fmt),
                None => self
                    .ortho_pass
                    .render_remap(dev, q, enc, src, out, w, h, xform, fmt),
            },
        );
        if ran {
            self.mark_node_pixels_dirty(node_id);
            self.mark_dirty();
        }
    }

    /// Run an effect over a node's `region` in place — the destructive
    /// counterpart of [`flip_node_region`](Self::flip_node_region), riding the
    /// same copy-out → pass → copy-back plumbing (`run_filter_region`).
    ///
    /// Where `mask_view` (a region-sized R8 selection crop) is selected the
    /// texel takes the transformed value, elsewhere the original passes
    /// through; `None` transforms the whole region. That confinement is the
    /// shared in-place apply pass, exactly as on the layer path — which is why
    /// the effect itself never learns a mask exists, and why every effect is
    /// maskable without declaring anything.
    ///
    /// Node-generic: the effect is instantiated at the node's own format, so an
    /// effect declaring `R8Unorm` among its targets runs over a mask node for
    /// free. One that does not simply has no pipeline at that format and the
    /// call is a no-op.
    pub fn apply_effect_to_region(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        node_id: LayerId,
        region: CanvasRect,
        mask_view: Option<&wgpu::TextureView>,
        type_id: &str,
        params: &[crate::gpu::params::ParamValue],
    ) {
        let Some(format) = self.node_textures.get(&node_id).map(|s| s.texture.format()) else {
            return;
        };
        let Some(mut effect) = self
            .effect_registry
            .instance(type_id, params, device, format)
        else {
            return;
        };
        let Some(apply_pipeline) = self.in_place_apply_pipeline_for(format) else {
            return;
        };

        // Borrow-splitting: the pass closure captures these by shared reference
        // while `run_filter_region` holds `&self.node_textures`.
        let sampler = &self.sampler;
        let blend_bgl = &self.blend_pipelines.bind_group_layout;
        let mask_bgl = &self.blend_pipelines.mask_bind_group_layout;
        let default_mask_bg = &self.default_mask_bind_group;

        let ran = run_filter_region(
            &self.node_textures,
            device,
            queue,
            encoder,
            node_id,
            region,
            mask_view,
            |dev, q, enc, src, mask, out, w, h, fmt| {
                // Without a mask the effect writes the output directly; with
                // one it writes an intermediate the apply pass then confines.
                // The intermediate is region-sized and local to this call, so
                // `run_filter_region`'s own two scratches are untouched and the
                // flip path that shares it keeps its shape.
                let intermediate = mask.map(|_| create_ortho_scratch(dev, w, h, fmt));
                let intermediate_view = intermediate.as_ref().map(|(_, v)| v.clone());

                // The effect binds against `[src, out]` as its ping-pong pair
                // and always reads slot 0.
                let pair = [
                    src.clone(),
                    intermediate_view.clone().unwrap_or_else(|| out.clone()),
                ];
                let cache = effect.create_cache(dev, q, &pair, sampler, w, h);
                effect.encode(enc, &cache, 0, intermediate_view.as_ref().unwrap_or(out));

                let (Some(mask), Some(after)) = (mask, intermediate_view.as_ref()) else {
                    return;
                };

                // Region-local geometry: the scratch and the mask crop are the
                // same rect, so the shared shader's window → plane → mask hops
                // collapse to an identity sample.
                let uniforms = ApplyUniforms {
                    canvas_origin: [0.0, 0.0],
                    canvas_size: [w as f32, h as f32],
                    mask_offset: [0.0, 0.0],
                    mask_size: [w as f32, h as f32],
                    isolated: 0,
                    blend_mode: crate::gpu::blend_mode::registry().default().gpu_value,
                    opacity: 1.0,
                    _pad0: 0,
                };
                let uniform_buf =
                    create_uniform_buffer::<ApplyUniforms>(dev, "region-apply-uniform");
                q.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&uniforms));

                let bind_group = blend_bind_group(
                    dev,
                    "region-apply-bg",
                    blend_bgl,
                    src,
                    after,
                    sampler,
                    uniform_buf.as_entire_binding(),
                );
                let mask_bg = dev.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("region-apply-mask-bg"),
                    layout: mask_bgl,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(mask),
                    }],
                });
                let _ = default_mask_bg;

                let mut rpass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("region-apply"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: out,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                rpass.set_pipeline(apply_pipeline);
                rpass.set_bind_group(0, &bind_group, &[]);
                rpass.set_bind_group(1, &mask_bg, &[]);
                rpass.draw(0..3, 0..1);
            },
        );
        if ran {
            self.mark_node_pixels_dirty(node_id);
            self.mark_dirty();
        }
    }

    /// Copy a node's `region` (canvas coords) into a fresh region-sized texture —
    /// the pristine "before" for a live filter preview. Returns the snapshot and
    /// the clipped region actually captured, or `None` if the node has no texture
    /// or the region doesn't overlap it.
    pub fn snapshot_node_region(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        node_id: LayerId,
        region: CanvasRect,
    ) -> Option<(wgpu::Texture, CanvasRect)> {
        let tex = &self.node_textures.get(&node_id)?.texture;
        let extent = tex.canvas_extent();
        let region = extent.intersect(region)?;
        if region.width == 0 || region.height == 0 {
            return None;
        }
        let (snap, _) = create_texture_with_view(
            device,
            region.width,
            region.height,
            tex.format(),
            "filter-preview-snapshot",
            wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
        );
        let lx = (region.origin.x - extent.origin.x) as u32;
        let ly = (region.origin.y - extent.origin.y) as u32;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("filter-preview-save"),
        });
        blit_region(
            &mut encoder,
            tex.texture(),
            (lx, ly),
            &snap,
            (0, 0),
            region.width,
            region.height,
        );
        queue.submit(Some(encoder.finish()));
        Some((snap, region))
    }

    /// Copy a previously [snapshotted](Self::snapshot_node_region) region back
    /// into the node — undo a live preview so a fresh set of params (or a
    /// cancel) starts from the pristine pixels.
    pub fn restore_node_region(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        node_id: LayerId,
        region: CanvasRect,
        snapshot: &wgpu::Texture,
    ) {
        {
            let Some(tex) = self.node_textures.get(&node_id).map(|s| &s.texture) else {
                return;
            };
            let extent = tex.canvas_extent();
            let lx = (region.origin.x - extent.origin.x) as u32;
            let ly = (region.origin.y - extent.origin.y) as u32;
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("filter-preview-restore"),
            });
            blit_region(
                &mut encoder,
                snapshot,
                (0, 0),
                tex.texture(),
                (lx, ly),
                region.width,
                region.height,
            );
            queue.submit(Some(encoder.finish()));
        }
        self.mark_node_pixels_dirty(node_id);
        self.mark_dirty();
    }

    /// Ensure a non-passthrough group has GPU state allocated.
    /// Called when a group is created or switches from passthrough to normal.
    pub fn ensure_group_state(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        group_id: LayerId,
    ) {
        if self.group_state.contains_key(&group_id) {
            return;
        }
        self.revisions.bump_targets();
        let gs = Self::create_group_state(
            device,
            queue,
            self.canvas_width,
            self.canvas_height,
            self.canvas_origin,
            group_id,
        );
        self.group_state.insert(group_id, gs);
    }

    /// Move / resize the canvas window, recreating every window-sized GPU
    /// resource at the new dimensions and plane origin.
    ///
    /// Window-sized resources: every group's ping-pong accumulators + composite
    /// cache, the passthrough-mask snapshots, the present bind group, and the
    /// selection mask (re-realized at the moved window, preserving its plane
    /// anchor). Node textures (layers, masks) are plane-anchored and left
    /// untouched — crop/resize preserves off-window pixels. Pipelines are
    /// format- not dimension-dependent, so they are not rebuilt.
    ///
    /// Group blend uniforms reset to defaults here; `sync_compositor_layers`
    /// rewrites them before the next composite. Marks a full recomposite.
    pub fn set_canvas_rect(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        origin: crate::coord::CanvasPoint,
        width: u32,
        height: u32,
    ) {
        let old_origin = self.canvas_origin;
        self.canvas_width = width;
        self.canvas_height = height;
        self.canvas_origin = origin;

        // Update the single shared canvas-geometry uniform. This is the one
        // write that keeps every composite draw's canvas_size/canvas_origin in
        // step with the document — the per-layer uniforms no longer carry these
        // fields, so a layer created before this resize can no longer composite
        // through stale dimensions (the post-resize anisotropic-squash bug).
        let canvas_uniform = CanvasUniform {
            canvas_size: [width as f32, height as f32],
            canvas_origin: [origin.x as f32, origin.y as f32],
        };
        // Voids sample through a window-local uniform, so the shared canvas
        // uniform above is not enough — each one has to rewrite its own.
        self.resync_voids_to_canvas(device, queue);
        queue.write_buffer(
            &self.canvas_uniform_buf,
            0,
            bytemuck::bytes_of(&canvas_uniform),
        );

        // Recreate every group's accumulators + cache at the new size.
        let group_ids: Vec<LayerId> = self.group_state.keys().copied().collect();
        for gid in group_ids {
            self.revisions.bump_targets();
            let gs = Self::create_group_state(device, queue, width, height, origin, gid);
            self.group_state.insert(gid, gs);
        }

        // Passthrough-mask snapshots are parent-accumulator-sized and the
        // blend bind groups reference now-replaced accumulator views.
        self.mask_snapshot_state.clear();
        // Per-host projections are canvas-window-sized; drop them so the next
        // frame reallocates at the new dimensions.
        self.projection_states.clear();
        self.blend_bind_groups.clear();

        // Present samples the root accumulators — rebind to the fresh views.
        self.present_cache_bind_groups = Self::make_present_cache_bind_groups(
            device,
            &self._present_bind_group_layout,
            &self.group_state[&self.root_id],
            &self.sampler,
            &self.view_uniform_buf,
        );

        // Re-realize the window-sized selection mask at the moved window.
        if let Some(sel) = self.selection_state.as_mut() {
            sel.resize(
                device,
                queue,
                old_origin,
                CanvasRect::new(origin, width, height),
            );
        }

        // Canvas geometry is a document fact; the bump covers both the
        // recomposite and the re-present, since the present reflects it too.
        self.revisions.bump_document();
    }

    /// Update a group's blend uniforms (opacity, blend_mode).
    ///
    /// `blend_mode_gpu` is the registry-resolved gpu_value (i.e.
    /// `blend_props.blend_mode.gpu_value`). Engine callers fetch the
    /// integer at the call site so the compositor's per-frame paths stay
    /// pointer-indirection-free.
    pub fn update_group_uniforms(
        &mut self,
        queue: &wgpu::Queue,
        group_id: LayerId,
        opacity: f32,
        blend_mode_gpu: u32,
        isolated: bool,
    ) {
        if let Some(gs) = self.group_state.get(&group_id) {
            // The group's window-sized cache occupies the canvas window in the
            // plane, so `layer_offset = canvas_origin` / `layer_size =
            // canvas_size` makes the shared-canvas plane round-trip collapse to
            // an identity sample (see `create_group_state`).
            let uniforms = BlendUniforms::for_extent(
                opacity,
                blend_mode_gpu,
                isolated,
                CanvasRect::from_xywh(
                    self.canvas_origin.x,
                    self.canvas_origin.y,
                    self.canvas_width,
                    self.canvas_height,
                ),
            );
            queue.write_buffer(&gs.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
        }
        // The passthrough-mask lerp uniform (mask geometry + isolated) is
        // refreshed per-frame in `sync_projection_states`, where the mask's
        // current extent is available.
    }

    /// Mark that the document changed in a way the composite must reflect.
    ///
    /// Coarse by design: this is the "something about the document moved"
    /// source, and every consumer that depends on it recomposites. Narrowing
    /// a specific call site means bumping a more specific source, not adding
    /// an invalidation channel.
    pub fn mark_dirty(&mut self) {
        self.revisions.bump_document();
    }

    /// Mark that a node's pixels changed — a bump of that node's own
    /// revision, which every consumer of its pixels (thumbnails, content
    /// bounds, histograms, the composite) compares against on read.
    ///
    /// # Write-site invariant
    ///
    /// Every function that *takes a `LayerId` and either allocates or
    /// writes that node's GPU texture* must call this method before
    /// returning. The mark is the write-site's responsibility, **never**
    /// the caller's — otherwise the same bug (a freshly-written node with
    /// no thumbnail until a separate edit fires the mark) keeps coming
    /// back the next time someone adds a feature and forgets the call.
    ///
    /// Concretely this applies to:
    /// `ensure_raster_layer`, `ensure_node_texture`, `resize_node_texture`,
    /// `upload_node_pixels`, `bake_subtree_to_layer`, and the engine-level
    /// helpers `clone_node_pixels` / `clone_filter_pixels`. Higher-level
    /// engine ops (paint stroke end, fill, paste, …) that drive these
    /// through raw `wgpu::CommandEncoder` writes still need an explicit
    /// mark inside the public-facing function that takes the id — the
    /// invariant is "if your signature carries a LayerId, you mark it".
    pub fn mark_node_pixels_dirty(&mut self, node_id: LayerId) {
        self.revisions.bump_node_pixels(node_id);
    }

    /// Read-only access to the revision registry, for consumers that keep
    /// their own per-node cursors (thumbnail readbacks) or compare a cached
    /// artifact's stamp.
    pub fn revisions(&self) -> &Revisions {
        &self.revisions
    }

    /// Mark that something downstream of the composite changed — the view
    /// transform, the overlay, a screen-space effect's inputs. The composite
    /// itself stays valid, so only the present is owed.
    pub fn mark_needs_present(&mut self) {
        self.revisions.bump_present_inputs();
    }

    /// Whether the presented frame is behind any source it reflects.
    ///
    /// Nothing clears this: `finish_present` advances `presented` to the tick
    /// the frame was built from, and a dropped acquire (`Lost`/`Outdated`)
    /// returns before that — so a frame that never reached the surface stays
    /// owed without anyone having to remember to re-set a flag.
    pub fn needs_present(&self) -> bool {
        self.revisions.latest_visual() > self.presented
    }

    /// Treat the current state as presented without a real present. Headless
    /// tests never reach `finish_present` (no surface), so this gives them a
    /// deterministic starting point.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_clear_needs_present(&mut self) {
        self.presented = self.revisions.latest_visual();
    }

    /// Bump the `targets` source alone. Tests use it to pin that a target
    /// recreation schedules no frame by itself while still forcing effect
    /// instances to rebuild.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_bump_targets(&mut self) {
        self.revisions.bump_targets();
    }

    /// Composites actually encoded since construction.
    #[cfg(any(test, feature = "testing"))]
    pub fn composite_runs(&self) -> u64 {
        self.composite_runs
    }

    /// Group walks that resumed from a captured prefix since construction.
    #[cfg(any(test, feature = "testing"))]
    pub fn walk_resumes(&self) -> u64 {
        self.walk_resumes
    }

    /// Group walks that found nothing below them changed since construction.
    #[cfg(any(test, feature = "testing"))]
    pub fn walk_all_clean(&self) -> u64 {
        self.walk_all_clean
    }

    /// Force the next frame to composite and present from scratch, as if
    /// every source had just changed. The from-scratch half of the
    /// byte-equality tests.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_invalidate_all(&mut self) {
        self.revisions.bump_all_for_test();
    }

    // --- Content Bounds (GPU compute) ---

    /// Return cached content bounds for a layer: `[x, y, w, h]`.
    /// Returns `None` if bounds haven't been computed yet or were invalidated.
    pub fn content_bounds(&self, layer_id: LayerId) -> Option<[u32; 4]> {
        self.content_bounds.get(&self.revisions, layer_id)
    }

    /// Whether content bounds resolved against current state, including empty.
    pub fn content_bounds_resolved(&self, layer_id: LayerId) -> bool {
        self.content_bounds.is_resolved(&self.revisions, layer_id)
    }

    /// Request async content bounds computation for a layer.
    /// Results arrive on the next frame — retrieve via [`content_bounds`].
    /// Bounds are returned in **layer-local** pixel coords (top-left of the
    /// layer texture is `(0, 0)`). Translate to canvas coords with the
    /// layer's [`LayerTexture::layer_to_canvas_rect`].
    pub fn request_content_bounds(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        node_id: LayerId,
    ) {
        let Some((view, w, h, format)) = node_view_info(&self.node_textures, node_id) else {
            return;
        };
        self.content_bounds.request(
            device,
            queue,
            &self.revisions,
            view,
            w,
            h,
            format == wgpu::TextureFormat::R8Unorm,
            node_id,
        );
    }

    /// Poll pending content bounds computations. Call once per frame.
    /// Returns layer IDs whose bounds just became available.
    pub fn poll_content_bounds(&mut self, device: &wgpu::Device) -> Vec<LayerId> {
        self.content_bounds.poll(device, &self.revisions)
    }

    /// True if any content bounds computations are in flight.
    pub fn has_pending_content_bounds(&self) -> bool {
        self.content_bounds.has_pending()
    }

    /// True if a content bounds computation is in flight for a specific layer.
    pub fn is_content_bounds_pending(&self, layer_id: LayerId) -> bool {
        self.content_bounds.is_pending(&self.revisions, layer_id)
    }

    // --- Histogram (GPU compute) ---

    /// Select the filter layer whose input histogram is computed each compose
    /// (the Levels editor's target), or `None` to stop computing. Forces a
    /// recomposite so the histogram dispatches for the new target.
    pub fn set_histogram_target(&mut self, target: Option<LayerId>) {
        if self.histogram_target != target {
            if let Some(prev) = self.histogram_target {
                self.histogram.remove_layer(prev);
            }
            self.histogram_target = target;
            self.mark_dirty();
        }
    }

    /// Select a node whose *own* texture is histogrammed (the destructive
    /// Levels modal's backdrop — there is no filter arm in the tree to bin its
    /// input), or `None` to stop. Unlike [`set_histogram_target`], the binning is
    /// pumped directly off the node texture by [`pump_node_histogram`], not the
    /// compose walk.
    ///
    /// [`set_histogram_target`]: Self::set_histogram_target
    /// [`pump_node_histogram`]: Self::pump_node_histogram
    pub fn set_node_histogram_target(&mut self, target: Option<LayerId>) {
        if self.node_histogram_target != target {
            if let Some(prev) = self.node_histogram_target {
                self.histogram.remove_layer(prev);
            }
            self.node_histogram_target = target;
        }
    }

    /// Dispatch a histogram over the target node's own RGBA8 texture if one is
    /// selected and none is cached/pending. Self-contained (records + submits its
    /// own encoder), so it runs independently of the compose gating; `needs`
    /// guards re-dispatch, making a per-frame call cheap. The result lands in the
    /// same cache [`histogram`](Self::histogram) reads.
    pub fn pump_node_histogram(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let Some(id) = self.node_histogram_target else {
            return;
        };
        if !self.histogram.needs(&self.revisions, id) {
            return;
        }
        let Some((view, w, h, format)) = node_view_info(&self.node_textures, id) else {
            return;
        };
        // A per-channel colour histogram only makes sense for RGBA8 layers.
        if format != wgpu::TextureFormat::Rgba8Unorm {
            return;
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("node-histogram"),
        });
        self.histogram
            .dispatch(device, &mut encoder, &self.revisions, view, w, h, id);
        queue.submit(Some(encoder.finish()));
    }

    /// The cached 8×256 histogram (channel-major) for a layer, if available.
    pub fn histogram(&self, layer_id: LayerId) -> Option<&[u32]> {
        self.histogram.get(&self.revisions, layer_id)
    }

    /// Poll pending histogram computations. Call once per frame.
    pub fn poll_histogram(&mut self, device: &wgpu::Device) -> Vec<LayerId> {
        self.histogram.poll(device, &self.revisions)
    }

    /// True if any histogram computation is in flight.
    pub fn has_pending_histogram(&self) -> bool {
        self.histogram.has_pending()
    }

    // --- Paint Target Accessors ---

    /// Look up a node's GPU texture by id. Works uniformly for raster layers
    /// and mask filters — format and extent come from the texture's own
    /// metadata. Returns `None` for groups (no pixels) and unknown ids.
    pub fn node_texture(&self, node_id: LayerId) -> Option<&LayerTexture> {
        self.node_textures.get(&node_id).map(|s| &s.texture)
    }

    /// Return the GPU texture backing any entity's pixels — works uniformly
    /// for raster layers, mask filters, AND the selection filter.
    ///
    /// The selection's R8 texture lives in
    /// [`crate::gpu::selection::SelectionState`] (ping-pong pair + dedicated
    /// bind groups) rather than the unified `node_textures` HashMap;
    /// `pixel_data_for` hides that asymmetry so callers (save readback,
    /// future readers) don't need to know.
    pub fn pixel_data_for(&self, node_id: LayerId) -> Option<PixelDataRef<'_>> {
        // A void's *persistent* frame (camera void's last webcam frame, at its
        // native resolution) lives on the void's EffectCache, not in
        // `node_textures`. A void also has a canvas-sized `node_textures`
        // entry — its composited output for the blend — so this branch must
        // come FIRST: that texture is the wrong thing to save (wrong content,
        // wrong resolution), and only a void that declares a persistent frame
        // reaches here at all (procedural voids return `None` and fall
        // through). Without this ordering the save reads back the composited
        // output and the camera frame is lost on reload.
        if let Some(proc) = self.procedural_content(node_id) {
            if let Some((width, height)) = proc.void.persistent_frame_size() {
                if let Some(tex) = proc.cache.aux_textures.first() {
                    return Some(PixelDataRef {
                        texture: tex,
                        format: tex.format(),
                        width,
                        height,
                    });
                }
            }
        }
        if let Some(t) = self.node_textures.get(&node_id).map(|s| &s.texture) {
            let ext = t.layer_extent();
            return Some(PixelDataRef {
                texture: t.texture(),
                format: t.format(),
                width: ext.width,
                height: ext.height,
            });
        }
        if let Some(sel) = self.selection_state.as_ref() {
            if sel.filter_id == node_id {
                let frame = sel.canvas_frame();
                return Some(PixelDataRef {
                    texture: frame.texture,
                    format: wgpu::TextureFormat::R8Unorm,
                    width: frame.canvas_extent.width,
                    height: frame.canvas_extent.height,
                });
            }
        }
        None
    }

    /// Replace a node's entire texture contents with `bytes`, then mark
    /// the node's pixels dirty so the next render's
    /// `drain_dirty_thumbnail_readbacks` queues a fresh thumbnail.
    ///
    /// The single right way to upload pixels to a node — every paint
    /// site has historically had to remember to call
    /// `mark_node_pixels_dirty` after `queue.write_texture`. Centralising
    /// the pair makes the bug "load uploaded pixels but no thumbnails
    /// appeared until the first edit" impossible to express by
    /// construction: callers can't write without dirtying.
    ///
    /// `bytes` must exactly fill the texture (`width * height * bpp` of
    /// the texture's format). Returns `false` when the node has no
    /// texture (groups, unknown ids) or `bytes` is short — caller can
    /// log/ignore as appropriate. Production callers (paste, load)
    /// treat both as "silently skip"; the engine has already passed
    /// every validation gate by the time it reaches here.
    pub fn upload_node_pixels(
        &mut self,
        queue: &wgpu::Queue,
        node_id: LayerId,
        bytes: &[u8],
    ) -> bool {
        let Some(tex) = self.node_textures.get(&node_id).map(|s| &s.texture) else {
            return false;
        };
        let bpp = tex.format().block_copy_size(None).unwrap_or(1);
        let layer_extent = tex.layer_extent();
        let expected = (layer_extent.width * layer_extent.height * bpp) as usize;
        if bytes.len() < expected {
            return false;
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: tex.texture(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &bytes[..expected],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(layer_extent.width * bpp),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: layer_extent.width,
                height: layer_extent.height,
                depth_or_array_layers: 1,
            },
        );
        self.mark_node_pixels_dirty(node_id);
        true
    }

    /// Allocate or replace a node's GPU texture. Format-driven — `R8Unorm`
    /// allocates a mask-style (white-fill) texture; `Rgba8Unorm` allocates a
    /// raster-style (zero-fill) texture. Existing texture for the same id is
    /// replaced.
    pub fn ensure_node_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        node_id: LayerId,
        format: wgpu::TextureFormat,
        bounds: crate::coord::CanvasRect,
    ) {
        match format {
            wgpu::TextureFormat::R8Unorm => {
                let mask_tex = LayerTexture::new_mask_with_extent(device, queue, bounds);
                let mask_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(&format!("mask-bg-{node_id:?}")),
                    layout: &self.blend_pipelines.mask_bind_group_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(mask_tex.view()),
                    }],
                });
                self.node_textures.insert(
                    node_id,
                    NodeSlot {
                        texture: mask_tex,
                        mask_bg: Some(mask_bg),
                    },
                );
                // Fresh mask texture (typically all-white reveal); its
                // thumbnail must materialize without callers having to
                // remember a mark — see `mark_node_pixels_dirty` invariant.
                self.mark_node_pixels_dirty(node_id);
                // MaskSnapshotState is a per-host resource (the
                // snapshot is sized to the parent accumulator). It's not
                // owned by the mask texture itself, so creation lives behind
                // [`Self::ensure_mask_snapshot_state`] which the engine
                // calls when attaching a mask to a host. Keep the allocation
                // out of the texture-creation path so the keying is by host,
                // not by mask filter id.
            }
            wgpu::TextureFormat::Rgba8Unorm => {
                // ensure_raster_layer marks dirty itself.
                self.ensure_raster_layer(device, queue, node_id, bounds);
            }
            other => panic!("ensure_node_texture: unsupported format {other:?}"),
        }
    }

    /// Allocate the snapshot+uniform pair the in-place masked-host path needs,
    /// keyed by **host** id (the passthrough group or filter layer whose
    /// composited output gets snapshot-then-lerped against its mask).
    /// Idempotent. The mask texture itself lives in the shared node-texture
    /// pool keyed by mask filter id; this resource is a per-host concern, not
    /// per-filter — there's one snapshot buffer per host regardless of how many
    /// filters attach.
    pub fn ensure_mask_snapshot_state(&mut self, device: &wgpu::Device, host_id: LayerId) {
        if self.mask_snapshot_state.contains_key(&host_id) {
            return;
        }
        let (snapshot, snapshot_view) = Self::make_accum_texture(
            device,
            self.canvas_width,
            self.canvas_height,
            &format!("mask-snapshot-{host_id:?}"),
        );
        let uniform_buf = create_uniform_buffer::<ApplyUniforms>(
            device,
            &format!("mask-snapshot-lerp-uniforms-{host_id:?}"),
        );
        self.mask_snapshot_state.insert(
            host_id,
            MaskSnapshotState {
                snapshot,
                snapshot_view,
                uniform_buf,
            },
        );
    }

    /// Drop the in-place masked-host snapshot for a host id. Mirrors
    /// [`Self::ensure_mask_snapshot_state`].
    pub fn dispose_mask_snapshot_state(&mut self, host_id: LayerId) {
        self.mask_snapshot_state.remove(&host_id);
    }

    // --- Selection (global) ---

    /// Allocate the GPU realisation of the document's selection filter.
    /// Idempotent — returns immediately if already allocated. The selection
    /// filter id is stashed on the [`SelectionState`] so undo / region-store
    /// keying can resolve back to the document filter.
    pub fn ensure_selection_state(
        &mut self,
        device: &wgpu::Device,
        filter_id: LayerId,
        bgl: &wgpu::BindGroupLayout,
    ) {
        if self.selection_state.is_some() {
            return;
        }
        self.selection_state = Some(crate::gpu::selection::SelectionState::new(
            device,
            filter_id,
            self.canvas_width,
            self.canvas_height,
            bgl,
        ));
    }

    /// Read access to the global selection's GPU state. `None` until
    /// [`Self::ensure_selection_state`] is called.
    pub fn selection_state(&self) -> Option<&crate::gpu::selection::SelectionState> {
        self.selection_state.as_ref()
    }

    /// Mutable access to the global selection's GPU state — for the boolean
    /// op + invert pipelines that mutate the ping-pong textures.
    pub fn selection_state_mut(&mut self) -> Option<&mut crate::gpu::selection::SelectionState> {
        self.selection_state.as_mut()
    }

    /// Orthogonally transform the active selection mask alongside a canvas
    /// flip/rotate (no-op if there is no selection state). Drives the shared
    /// ortho pass over the selection's ping-pong textures.
    pub fn ortho_transform_selection(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        xform: crate::gpu::ortho_transform::OrthoXform,
    ) {
        let pass = &self.ortho_pass;
        if let Some(sel) = self.selection_state.as_mut() {
            sel.apply_ortho(device, queue, encoder, pass, xform);
        }
    }

    /// Drop all GPU state associated with a node id (texture, bind groups,
    /// dirty bits, layer cache including any procedural-content sidecar).
    /// Use when a node is permanently removed — e.g. layer delete or
    /// filter removal. Per-host passthrough state is owned by its host
    /// id, so it's not touched here.
    pub fn dispose_node_texture(&mut self, node_id: LayerId) {
        // The slot carries the texture and (for masks) its bind group; one
        // remove drops both.
        self.node_textures.remove(&node_id);
        // Blend bind groups that name this node as either the parent (a
        // group whose accum is gone) or the child (a layer or child-group
        // whose view is gone) point at a freed texture handle. Evict them
        // so the next composite rebuilds against current state.
        self.blend_bind_groups
            .retain(|(parent, child, _), _| *parent != node_id && *child != node_id);
        self.layer_cache.remove(&node_id);
        // Drop any vector-layer realization input; no-op for other kinds.
        self.vector.scenes.remove(&node_id);
        // A deleted host's projection is released immediately; a deleted mask
        // is caught by the next `sync_projection_states` stale sweep.
        self.projection_states.remove(&node_id);
        self.revisions.remove_node(node_id);
        self.mark_dirty();
    }

    /// Drop all GPU state for a layer when it's permanently removed
    /// (`Engine::remove_layer`) or when an auto-created paste-target is
    /// canceled (`cancel_floating`). Alias of [`Self::dispose_node_texture`]
    /// kept as a separate entry point because the engine's layer-removal
    /// path conceptually distinguishes "tree node gone" from "filter
    /// detached".
    pub fn dispose_layer(&mut self, layer_id: LayerId) {
        self.dispose_node_texture(layer_id);
    }

    /// Read-only access to the void registry — lets the engine answer
    /// `void_types()` / `void_param_defs()` queries without exposing a
    /// mutable handle.
    pub fn void_registry(&self) -> &VoidRegistry {
        &self.void_registry
    }

    /// Mutable access to the void registry. Engine callers go through this
    /// to instantiate a void (the registry lazy-caches the per-type
    /// pipeline, so creation needs `&mut`).
    pub fn void_registry_mut(&mut self) -> &mut VoidRegistry {
        &mut self.void_registry
    }

    /// Read-only access to the effect registry — lets the engine answer
    /// `effect_types()` without exposing a mutable handle.
    pub fn effect_registry(&self) -> &crate::gpu::effect::EffectRegistry {
        &self.effect_registry
    }

    /// Mutable access to the effect registry. Every path that instantiates an
    /// effect goes through this, since the registry lazily compiles the shared
    /// pipeline per `(type, target format)`.
    pub fn effect_registry_mut(&mut self) -> &mut crate::gpu::effect::EffectRegistry {
        &mut self.effect_registry
    }

    /// Canvas-content texture format used by every content layer (raster +
    /// void). Exposed so engine paths that need to construct a void via the
    /// registry before calling [`Self::ensure_void_layer`] can pass the same
    /// format the compositor would have used internally.
    pub fn canvas_content_format(&self) -> wgpu::TextureFormat {
        wgpu::TextureFormat::Rgba8Unorm
    }

    /// Total number of node textures (raster layers + mask filters)
    /// currently allocated. Test-only — used by leak-cycle regression tests
    /// to confirm `dispose_node_texture` reclaims state.
    pub fn test_node_texture_count(&self) -> usize {
        self.node_textures.len()
    }

    /// Number of `GroupState`s currently allocated (root plus every
    /// non-passthrough group). Test-only — the bake-leak regression test
    /// asserts merge/flatten leave no transient state behind.
    pub fn test_group_state_count(&self) -> usize {
        self.group_state.len()
    }

    /// Canvas width in pixels (unpadded).
    pub fn canvas_width(&self) -> u32 {
        self.canvas_width
    }

    /// Canvas height in pixels (unpadded).
    pub fn canvas_height(&self) -> u32 {
        self.canvas_height
    }

    /// The canvas window as a plane-space rect — `(canvas_origin, width,
    /// height)`. Mirrors `Document::canvas_rect()` on the compositor side.
    pub fn canvas_rect(&self) -> CanvasRect {
        CanvasRect::new(self.canvas_origin, self.canvas_width, self.canvas_height)
    }

    /// Update the view transform uniform buffer. The compositor owns the
    /// workspace background color and the pixel-filter mode, so it stamps
    /// them onto the uploaded copy rather than relying on every caller.
    pub fn update_view_transform(&mut self, queue: &wgpu::Queue, transform: &ViewTransform) {
        let mut t = *transform;
        t.bg = self.viewport_bg;
        t.flags[0] = self.pixel_filter;
        queue.write_buffer(&self.view_uniform_buf, 0, bytemuck::bytes_of(&t));
        self.cached_view_transform = t;
    }

    /// Set the workspace background color (the area shown outside the canvas
    /// rectangle in the present shader). Triggers a re-upload of the cached
    /// transform and a re-present so the color takes effect immediately.
    pub fn set_viewport_bg(&mut self, queue: &wgpu::Queue, bg: [f32; 4]) {
        if self.viewport_bg == bg {
            return;
        }
        self.viewport_bg = bg;
        let mut t = self.cached_view_transform;
        t.bg = bg;
        queue.write_buffer(&self.view_uniform_buf, 0, bytemuck::bytes_of(&t));
        self.cached_view_transform = t;
        self.revisions.bump_present_inputs();
    }

    /// Set the pixel filter mode used by the present shader: "linear",
    /// "nearest", or "auto" (anything else falls back to auto). Re-uploads
    /// the cached transform and forces a re-present so the change takes
    /// effect on the next frame.
    pub fn set_pixel_filter(&mut self, queue: &wgpu::Queue, mode: &str) {
        let new_mode = pixel_filter_from_str(mode);
        if (self.pixel_filter - new_mode).abs() < f32::EPSILON {
            return;
        }
        self.pixel_filter = new_mode;
        let mut t = self.cached_view_transform;
        t.flags[0] = new_mode;
        queue.write_buffer(&self.view_uniform_buf, 0, bytemuck::bytes_of(&t));
        self.cached_view_transform = t;
        self.revisions.bump_present_inputs();
    }

    /// Update a content layer's uniforms (called when opacity, blend mode,
    /// or isolated changes). Works uniformly for raster and procedural
    /// layers — both store their blend state in the same [`LayerCache`]
    /// and sample from canvas-positioned textures in `node_textures`.
    /// Reads the layer's bounds from its `LayerTexture` so callers don't
    /// need to thread them through; bounds-changing operations update the
    /// texture's stored offset/size directly via `resize_node_texture`.
    ///
    /// `blend_mode_gpu` is the registry-resolved gpu_value.
    pub fn update_layer_uniforms(
        &mut self,
        queue: &wgpu::Queue,
        layer_id: LayerId,
        opacity: f32,
        blend_mode_gpu: u32,
        isolated: bool,
    ) {
        let tex = match self.node_textures.get(&layer_id) {
            Some(s) => &s.texture,
            None => return,
        };
        let canvas_extent = tex.canvas_extent();
        let uniforms = BlendUniforms::for_extent(opacity, blend_mode_gpu, isolated, canvas_extent);
        let cache = match self.layer_cache.get_mut(&layer_id) {
            Some(c) => c,
            None => return,
        };
        queue.write_buffer(&cache.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
        cache.last_uniforms = uniforms;

        // Mirror into the floating preview's canvas-aligned uniform buffer
        // so the host's blend pass reads the same blend props (with canvas
        // dims/offset) when sampling the preview view. Voids never become
        // floating targets today, so this is a no-op for procedural layers;
        // keeping it on the shared path means the day they do, it just
        // works.
        self.write_preview_blend_uniforms_if_active(queue, layer_id);
    }

    /// The composited output texture: the root group's output accumulator
    /// half. Used by the color picker for readback.
    ///
    /// Stable between composites — accumulators are written only inside
    /// `compose_group` — and every consumer copies out of it at request time
    /// into an immediately submitted encoder, so queue ordering serializes
    /// that copy against any later composite.
    pub fn composited_texture(&self) -> &wgpu::Texture {
        self.group_state[&self.root_id].output_texture()
    }

    /// View over [`Self::composited_texture`] — lets callers wrap the
    /// root composite in a `GpuPaintTarget` (e.g. the sample-merged clone
    /// snapshot) without creating a fresh view per use.
    pub fn composited_view(&self) -> &wgpu::TextureView {
        self.group_state[&self.root_id].output_view()
    }

    /// The present bind group for the half the root's composite currently
    /// lives in.
    pub(super) fn present_cache_bind_group(&self) -> &wgpu::BindGroup {
        &self.present_cache_bind_groups[self.root_output_half()]
    }

    /// Whether a group has the accumulator its children compose into.
    /// Consulted through [`crate::layer::LayerNode::compose_ready`].
    pub fn has_group_state(&self, id: LayerId) -> bool {
        self.group_state.contains_key(&id)
    }

    /// Whether a node has a GPU texture to blend.
    pub fn has_node_texture(&self, id: LayerId) -> bool {
        self.node_textures.contains_key(&id)
    }

    /// Whether an effect layer's arm would draw: both a realized instance and
    /// the shared apply scratch exist. Mirrors `compose_effect_arm`'s own two
    /// early returns.
    pub fn effect_arm_ready(&self, id: LayerId) -> bool {
        self.effect_instances.contains_key(&id) && self.canvas_apply_scratch.is_some()
    }

    /// Which accumulator half holds the root group's composite.
    pub fn root_output_half(&self) -> usize {
        self.group_state[&self.root_id].output_index()
    }

    pub fn accum_format(&self) -> wgpu::TextureFormat {
        wgpu::TextureFormat::Rgba8Unorm
    }

    pub fn screen_run(&self) -> &ScreenRun {
        &self.screen_run
    }

    pub fn screen_run_mut(&mut self) -> &mut ScreenRun {
        &mut self.screen_run
    }

    /// Resize the screen-space run's textures. Replacing them invalidates every
    /// bind group pointing at them, which the `targets` bump is what rebuilds
    /// — the same source a canvas resize bumps, so neither space needs its own
    /// enumeration of invalidation triggers. The run's output is downstream of
    /// the composite, so a resize owes a present but no recomposite.
    pub fn resize_screen_run(&mut self, width: u32, height: u32) {
        if self.screen_run.resize(width, height) {
            self.revisions.bump_targets();
            self.revisions.bump_present_inputs();
        }
    }

    /// The registries a preview mechanism may need, borrow-split in one place
    /// so a caller does not have to reach for three `&mut self` accessors that
    /// cannot coexist.
    ///
    /// The compositor's own registries rather than a second set owned by the
    /// preview subsystem: a preview then shares the live pipeline cache and
    /// compiles no shader twice.
    pub fn preview_registries(&mut self) -> crate::gpu::preview::PreviewRegistries<'_> {
        crate::gpu::preview::PreviewRegistries {
            effects: &mut self.effect_registry,
            voids: &mut self.void_registry,
        }
    }

    /// Read-only access to the tool overlay. Callers do their own dispatch;
    /// the compositor stops being a switchboard.
    pub fn tool_overlay(&self) -> &ToolOverlay {
        &self.tool_overlay
    }

    /// Mutable access to the tool overlay. Callers that change overlay state
    /// must follow with `mark_needs_present()` themselves.
    pub fn tool_overlay_mut(&mut self) -> &mut ToolOverlay {
        &mut self.tool_overlay
    }

    /// Split-borrow accessor for the preview-render hot path: returns
    /// `(&mut tool_overlay, &selection_state)` so a caller can grow
    /// the preview mask through the overlay *and* keep a borrow of
    /// the active selection's brush bind group at the same time. The
    /// two fields are disjoint, but the borrow checker can't see
    /// through method calls — splitting at this granularity here
    /// makes the disjoint-field pattern usable from outside.
    pub fn split_overlay_and_selection(
        &mut self,
    ) -> (
        &mut ToolOverlay,
        Option<&crate::gpu::selection::SelectionState>,
    ) {
        (&mut self.tool_overlay, self.selection_state.as_ref())
    }

    /// Run the present pass, veil chain, and final blit to surface.
    /// Solid overlay primitives are drawn at the end of the final render
    /// pass (present or veil-blit) to avoid a separate LoadOp::Load pass.
    pub(super) fn present_and_screen_run(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        doc: &Document,
        surface_view: &wgpu::TextureView,
        isolated: Option<LayerId>,
    ) {
        // Membership, order and visibility all come from the document; the run
        // owns only the textures. An empty or wholly hidden run presents
        // straight to the surface, which is the common case.
        //
        // The run is consumed flattened: a group above the divider composites
        // nothing of its own, so the chain is its effect descendants in order.
        // `effective_visible` walks the parent chain, so hiding the group hides
        // everything it holds without this loop knowing groups exist.
        let run: Vec<LayerId> = doc
            .screen_space_effects()
            .into_iter()
            .filter(|id| doc.effective_visible(*id))
            .collect();

        // Synced here as well as before the compose walk, because the two
        // spaces are woken by different dirty flags: a viewport resize replaces
        // the run's textures without touching the canvas, so `render_offscreen`
        // returns early and never reaches the sync. Gated on the run having
        // members, so a document with no viewport effects — the common case —
        // does not pay for a second walk of every effect layer per frame.
        if !run.is_empty() {
            self.sync_effect_instances(device, queue, doc, isolated);
        }

        let members: Vec<LayerId> = run
            .into_iter()
            .filter(|id| self.effect_instances.contains_key(id))
            .collect();

        if members.is_empty() || self.screen_run.views().is_none() {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("present"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: surface_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            rpass.set_pipeline(&self.present_pipeline);
            rpass.set_bind_group(0, self.present_cache_bind_group(), &[]);
            rpass.draw(0..3, 0..1);
            // Draw solid overlay primitives in the same pass.
            self.tool_overlay.draw_solid(&mut rpass);
            return;
        }

        self.screen_run.encode_present_into_run(
            encoder,
            &self.present_to_effects_pipeline,
            self.present_cache_bind_group(),
        );

        let (Some(views), Some(scratch), Some(pipelines)) = (
            self.screen_run.views(),
            self.screen_run.scratch_view(),
            self.screen_run.scaling_pipelines(),
        ) else {
            return;
        };

        // Same two-step shape as the canvas arm: the effect writes into the
        // scratch, then the apply pass blends that back over the untouched
        // half carrying the layer's opacity and blend mode. No mask binding is
        // needed — a masked node cannot be above the divider.
        let (vw, vh) = self.screen_run.viewport_size();
        let full = (0, 0, vw, vh);
        let mut src = 0usize;
        for id in members {
            let inst = &self.effect_instances[&id];
            let dst = 1 - src;
            inst.scaled
                .encode(encoder, &*inst.effect, &inst.cache, pipelines, src, scratch);
            Self::encode_in_place_apply(
                &self.blend_pipelines,
                &self.in_place_apply_pipelines,
                &self.sampler,
                encoder,
                device,
                &views[src],
                scratch,
                inst.apply_uniform.as_entire_binding(),
                &views[dst],
                &self.default_mask_bind_group,
                full,
            );
            src = dst;
        }

        self.screen_run
            .blit_to_surface(encoder, surface_view, src, &self.tool_overlay);
    }

    /// Composite layer tree to offscreen target. GPU textures are authoritative —
    /// no CPU tile upload needed. Returns true if GPU work was submitted.
    ///
    /// `isolated` is the session isolation target (`engine.isolated_node`),
    /// passed per frame: the walk skips subtrees off the path between the
    /// root and the target. `None` composites everything.
    pub fn render_offscreen(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        doc: &mut Document,
        isolated: Option<LayerId>,
    ) -> bool {
        // Ahead of the dirty gate: a scale change is the one input that arrives
        // without anything marking the composite dirty, and this is the entry
        // point export, save, the recorder and the headless paths come through.
        self.sync_effect_scale();

        // Captured before the walk and committed after it. Only `targets` may
        // move in between — it is excluded from the gate precisely so a frame
        // creating its own group states cannot reschedule itself forever.
        let built_at = self.revisions.clock();
        let composite_input_at_capture = self.revisions.latest_composite_input();
        if composite_input_at_capture <= self.composite_built {
            return false;
        }

        let scissor = (0, 0, self.canvas_width, self.canvas_height);

        // Rasterize any dirty vector-layer scenes (Vello submits its own
        // command buffer) before building the composite encoder, so the blend
        // walk below samples up-to-date pixels.
        self.realize_dirty_vector_layers(device, queue);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("composite"),
        });

        // Regenerate any dirty void textures before the tree walk so the
        // downstream blend pass samples up-to-date pixels.
        self.encode_dirty_layer_content(&mut encoder);

        // Allocate + refresh per-host projection uniforms (needs `queue`)
        // before the compose walk, which only binds.
        self.sync_projection_states(device, queue, doc, isolated);

        let root_id = self.root_id;
        self.compose_group(&mut encoder, device, doc, root_id, scissor, isolated);

        queue.submit(std::iter::once(encoder.finish()));

        debug_assert_eq!(
            self.revisions.latest_composite_input(),
            composite_input_at_capture,
            "a composite must not bump its own inputs — only `targets` may move during the walk"
        );
        self.composite_built = built_at;
        #[cfg(any(test, feature = "testing"))]
        {
            self.composite_runs += 1;
        }
        true
    }

    /// The in-place apply pipeline compiled against `format`, or `None` for a
    /// format no node uses.
    fn in_place_apply_pipeline_for(
        &self,
        format: wgpu::TextureFormat,
    ) -> Option<&wgpu::RenderPipeline> {
        self.in_place_apply_pipelines
            .iter()
            .find(|(f, _)| *f == format)
            .map(|(_, p)| p)
    }

    /// Whether any rendering work is pending. A pending composite is by
    /// definition also a pending present, so the visual comparison covers
    /// both.
    fn has_pending_work(&self, _doc: &Document) -> bool {
        self.revisions.latest_visual() > self.presented
    }

    /// Record that a frame reflecting `frame_tick` reached the surface.
    fn finish_present(&mut self, frame_tick: Tick) {
        self.presented = frame_tick;
    }

    /// Upload dirty tiles, composite changed layers, present to a surface.
    /// Used by the WASM frontend.
    ///
    /// `isolated` is the session isolation target (`engine.isolated_node`),
    /// passed per frame — see [`Self::render_offscreen`].
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface: &wgpu::Surface,
        surface_config: &wgpu::SurfaceConfiguration,
        doc: &mut Document,
        isolated: Option<LayerId>,
    ) {
        perf::time("render-total");

        // Re-read the effect resolution scale. Needed here as well as in
        // `render_offscreen`, because this path returns below on
        // `!has_pending_work` without ever reaching it.
        self.sync_effect_scale();

        if !self.has_pending_work(doc) {
            perf::time_end("render-total");
            return;
        }

        perf::time("offscreen");
        self.render_offscreen(device, queue, doc, isolated);
        perf::time_end("offscreen");

        // Captured after the composite, not at frame entry: `render_offscreen`
        // runs `sync_effect_scale` a second time and a scale change is still
        // drifted there, so an earlier capture would stamp `presented` below
        // that bump and schedule a spurious extra frame. Only `targets` moves
        // during the walk, and it is not a visual source.
        let frame_tick = self.revisions.clock();

        // Acquire surface and present root composite → veils → surface.
        // wgpu 29 replaced `Result<SurfaceTexture, SurfaceError>` with the
        // `CurrentSurfaceTexture` enum. `Suboptimal` still yields a usable
        // texture; `Lost`/`Outdated` mean the swapchain must be reconfigured.
        let output = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(output)
            | wgpu::CurrentSurfaceTexture::Suboptimal(output) => output,
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                surface.configure(device, surface_config);
                perf::time_end("render-total");
                return;
            }
            other => {
                log::warn!("Surface unavailable: {other:?}");
                perf::time_end("render-total");
                return;
            }
        };
        let surface_view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        perf::time("present");
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("present"),
        });

        // Prepare overlay CPU-side work (upload, bind group) before render passes.
        if self.tool_overlay.has_content() {
            // The overlay draws plane-space (`FLAG_CANVAS_SPACE`) primitives, so
            // it needs the plane matrices, not the window-local present matrix.
            // Derive both from the cached present transform + the window origin.
            let vt = self.cached_view_transform;
            let (ox, oy) = (self.canvas_origin.x as f32, self.canvas_origin.y as f32);
            let plane_fwd = vt.plane_to_screen_matrix(ox, oy);
            let plane_inv = vt.screen_to_plane_matrix(ox, oy);
            let vw = self.screen_run.viewport_size().0;
            let vh = self.screen_run.viewport_size().1;
            self.tool_overlay
                .prepare(device, queue, &plane_fwd, &plane_inv, vw, vh);
        }

        // Present + screen-space run. Solid overlay primitives are drawn at
        // the end of the final pass (no separate LoadOp::Load pass needed).
        self.present_and_screen_run(&mut encoder, device, queue, doc, &surface_view, isolated);

        // Snapshot-sampling overlay primitives (invert + soft-contrast) need a
        // separate pass with a surface→snapshot copy. Hit by rect-select and
        // the brush-stamp preview.
        if self.tool_overlay.has_snapshot() {
            let vw = self.screen_run.viewport_size().0;
            let vh = self.screen_run.viewport_size().1;
            self.tool_overlay
                .encode_snapshot(&mut encoder, &output.texture, &surface_view, vw, vh);
        }

        queue.submit(std::iter::once(encoder.finish()));
        output.present();
        perf::time_end("present");

        self.finish_present(frame_tick);
        perf::time_end("render-total");
    }
}

#[cfg(test)]
mod ortho_extent_tests {
    use super::ortho_extent_about;
    use crate::coord::CanvasRect;
    use crate::gpu::ortho_transform::OrthoXform;

    // Odd, non-square canvas window so off-by-one in the index map or pivot
    // would surface. `frame` is the canvas; `e` a node extent inside it.
    fn frame() -> CanvasRect {
        CanvasRect::from_xywh(0, 0, 7, 5)
    }

    #[test]
    fn flip_h_moves_node_to_the_mirror_column() {
        // Node cols [1,4) (w=3) in a 7-wide frame → cols [3,6).
        let e = CanvasRect::from_xywh(1, 0, 3, 5);
        assert_eq!(
            ortho_extent_about(e, frame(), OrthoXform::FlipH),
            CanvasRect::from_xywh(3, 0, 3, 5)
        );
    }

    #[test]
    fn flip_v_moves_node_to_the_mirror_row() {
        let e = CanvasRect::from_xywh(0, 1, 7, 2);
        assert_eq!(
            ortho_extent_about(e, frame(), OrthoXform::FlipV),
            CanvasRect::from_xywh(0, 2, 7, 2)
        );
    }

    #[test]
    fn rot90_swaps_dims_and_recentres_the_frame() {
        // The whole canvas extent maps to the recentred, dimension-swapped frame
        // (GIMP offset rule: new_origin = old + (W-H)/2, (H-W)/2).
        let canvas = frame();
        let cw = ortho_extent_about(canvas, canvas, OrthoXform::Rot90Cw);
        assert_eq!(cw, CanvasRect::from_xywh((7 - 5) / 2, (5 - 7) / 2, 5, 7));
    }

    #[test]
    fn rot90_round_trips_to_identity() {
        let canvas = frame();
        let e = CanvasRect::from_xywh(1, 0, 3, 5);
        let cw = ortho_extent_about(e, canvas, OrthoXform::Rot90Cw);
        let rotated_frame = ortho_extent_about(canvas, canvas, OrthoXform::Rot90Cw);
        let back = ortho_extent_about(cw, rotated_frame, OrthoXform::Rot90Ccw);
        assert_eq!(back, e, "CW then CCW restores the node extent");
    }
}
