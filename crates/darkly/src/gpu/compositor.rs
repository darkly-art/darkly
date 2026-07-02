use crate::coord::CanvasRect;
use crate::document::Document;
use crate::gpu::atlas::LayerTexture;
use crate::gpu::blend::BlendPipelines;
use crate::gpu::content_bounds::ContentBoundsPass;
use crate::gpu::effect::EffectCache;
use crate::gpu::overlay::ToolOverlay;
use crate::gpu::params::ParamValue;
use crate::gpu::veil_chain::VeilChain;
use crate::gpu::view::{ViewTransform, DEFAULT_WORKSPACE_BG};
use crate::gpu::void::{Void, VoidRegistry};
use crate::layer::{FilterLayer, Layer, LayerId, RasterLayer, VoidLayer};
use smallvec::SmallVec;
use std::collections::{HashMap, HashSet};

/// Stack-side capacity for the per-frame `children_of(...)` snapshot used by
/// the composite walk. Typical documents have single-digit children per group;
/// wider groups (paste-many-layers, stress tests) spill to the heap without
/// ceremony.
type ChildIds = SmallVec<[LayerId; 8]>;

/// Convert a `display.pixelFilter` string value to the float code stamped
/// into `ViewTransform.flags[0]`. Unknown values fall back to auto.
fn pixel_filter_from_str(mode: &str) -> f32 {
    match mode {
        "linear" => 0.0,
        "nearest" => 1.0,
        _ => 2.0,
    }
}

/// Read the current `display.pixelFilter` config value. Used at compositor
/// startup so a fresh session picks up the persisted preference.
fn pixel_filter_from_config() -> f32 {
    pixel_filter_from_str(&crate::config::get_str("display.pixelFilter"))
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
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ortho-scratch"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
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
    node_textures: &HashMap<LayerId, LayerTexture>,
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
        Some(t) => (t.canvas_extent(), t.format()),
        None => return false,
    };
    let region = match extent.intersect(region) {
        Some(r) if r.width > 0 && r.height > 0 => r,
        _ => return false,
    };
    let (w, h) = (region.width, region.height);
    let lx = (region.origin.x - extent.origin.x) as u32;
    let ly = (region.origin.y - extent.origin.y) as u32;

    let src_scratch = create_ortho_scratch(device, w, h, format);
    let out_scratch = create_ortho_scratch(device, w, h, format);
    let node_tex = node_textures[&node_id].texture();

    encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: node_tex,
            mip_level: 0,
            origin: wgpu::Origin3d { x: lx, y: ly, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyTextureInfo {
            texture: &src_scratch,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );

    let src_view = src_scratch.create_view(&wgpu::TextureViewDescriptor::default());
    let out_view = out_scratch.create_view(&wgpu::TextureViewDescriptor::default());
    run_pass(
        device, queue, encoder, &src_view, mask_view, &out_view, w, h, format,
    );

    encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &out_scratch,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyTextureInfo {
            texture: node_tex,
            mip_level: 0,
            origin: wgpu::Origin3d { x: lx, y: ly, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    true
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
struct AccumPair {
    textures: [wgpu::Texture; 2],
    views: [wgpu::TextureView; 2],
}

/// GPU state for a non-passthrough group (including root).
/// Every group that composites its children to an isolated buffer owns one.
struct GroupState {
    /// Ping-pong accumulator pair for compositing children.
    accum: AccumPair,
    /// Tracks which accumulator is the current "source" (last written).
    current_accum: usize,
    /// Cached final composite result of this group's children.
    composite_cache: wgpu::Texture,
    composite_cache_view: wgpu::TextureView,
    /// Child index through which the cache is valid.
    /// None = cache is empty, must composite from scratch.
    cache_valid_through: Option<usize>,
    /// Uniform buffer holding opacity, blend_mode, isolated for blending
    /// this group's result into its parent.
    uniform_buf: wgpu::Buffer,
}

/// Per-instance GPU scaffolding shared by every layer that participates in
/// the standard blend pipeline (raster + void). Both kinds need a blend
/// uniform buffer and the same CPU-side mirror of opacity/blend/isolated;
/// only the *source of pixels* differs and that's split out into
/// [`LayerContent`]. One pool keyed by [`LayerId`] replaces the previous
/// `raster_cache` + `void_layers` split, so the compositor's lookup paths
/// (blend arm, uniforms write, dispose) don't dispatch on layer kind.
pub(super) struct LayerCache {
    /// Uniform buffer holding opacity + blend_mode + isolated.
    pub(super) uniform_buf: wgpu::Buffer,
    /// CPU-side cache of the blend properties last written to `uniform_buf`.
    /// Kept here so the floating-preview path can mirror them into its own
    /// canvas-aligned uniform buffer without re-reading the GPU buffer.
    pub(super) opacity: f32,
    /// Cached gpu_value for the layer's blend mode. The compositor never
    /// branches on which mode this is — the shader does — so we mirror the
    /// raw shader integer rather than carry a registration pointer through
    /// every per-frame access.
    pub(super) blend_mode: u32,
    pub(super) isolated: bool,
    /// Where this layer's pixels come from. Raster pixels arrive via paint;
    /// procedural pixels are regenerated on demand by a [`Void`] trait
    /// object before each composite.
    content: LayerContent,
}

/// Carrier passed to [`LayerNode::compose_into`] so the dispatch hop can
/// reach the compositor and the per-walk parameters without exploding the
/// compositor's private surface. Built once per child by `compose_children`
/// and discarded after the call.
pub struct CompositionContext<'a> {
    pub(super) compositor: &'a mut Compositor,
    pub(super) encoder: &'a mut wgpu::CommandEncoder,
    pub(super) device: &'a wgpu::Device,
    pub(super) doc: &'a Document,
    pub(super) parent_group: LayerId,
    pub(super) scissor: (u32, u32, u32, u32),
}

impl<'a> CompositionContext<'a> {
    /// Dispatch into the compositor's per-variant compose arm. Mirrors the
    /// [`LayerKindGpu::realize_in`] split — the arm bodies live on
    /// [`Compositor`] (where they touch its private fields), and the
    /// dispatch is owned by the variant via [`LayerNode::compose_into`].
    pub(crate) fn compose_layer(&mut self, layer: &Layer) {
        // A filter layer transforms the running group accumulator in place
        // (everything composited below it) rather than blending a texture in,
        // so it takes a separate arm from the raster/void blend path.
        if let Layer::Filter(f) = layer {
            self.compositor.compose_filter_arm(
                self.encoder,
                self.device,
                self.doc,
                self.parent_group,
                f,
                self.scissor,
            );
            return;
        }
        self.compositor.compose_layer_arm(
            self.encoder,
            self.device,
            self.doc,
            self.parent_group,
            layer,
            self.scissor,
        );
    }

    pub(crate) fn compose_group(&mut self, group: &crate::layer::LayerGroup) {
        self.compositor.compose_group_arm(
            self.encoder,
            self.device,
            self.doc,
            self.parent_group,
            group,
            self.scissor,
        );
    }
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
            // Filter layers have no per-instance GPU resource — the shared
            // filter pipeline is resolved lazily in `compose_filter_arm`. They
            // are excluded from the content walk (`Layer::is_blend_content`),
            // so this is never reached, but the arm keeps the match total.
            Layer::Filter(_) => {}
        }
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

/// How a layer's pixels reach `node_textures[id]`.
///
/// - `Raster`: pixels arrive via paint / paste / fill — `node_textures[id]`
///   is authoritative and the compositor doesn't regenerate it.
/// - `Procedural`: pixels are GPU-regenerable. The compositor calls
///   [`Void::encode`] before the next composite when the void's own dirty
///   flag (owned on the trait object) reports stale.
enum LayerContent {
    Raster,
    Procedural(ProceduralContent),
}

/// Per-instance procedural-content state. The "needs re-encode" flag lives
/// on the [`Void`] itself ([`Void::take_dirty`]) — the compositor neither
/// stores nor reconciles it.
struct ProceduralContent {
    /// The procedural-content trait object. Owned here (one per layer)
    /// because animation mutates its `time` field.
    void: Box<dyn Void>,
    /// Per-instance GPU resources for the void's own pipeline (uniform
    /// buffer + bind groups built off the registry's shared pipeline).
    cache: EffectCache,
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

/// Uniforms for the passthrough-group mask lerp (`mask_lerp.wgsl`). Carries the
/// canvas-window + mask geometry inline so the lerp samples the group mask in
/// its own plane space (matching `apply_mask`) without the pipeline needing the
/// shared canvas bind group.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct LerpUniforms {
    canvas_origin: [f32; 2],
    canvas_size: [f32; 2],
    mask_offset: [f32; 2],
    mask_size: [f32; 2],
    isolated: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

/// GPU state for an in-place host (passthrough group or filter layer) that has
/// a visible mask (Photoshop-style adjustment-layer mask). Stores a snapshot
/// texture for the parent accumulator and a uniform buffer for the lerp pass.
struct MaskSnapshotState {
    /// Snapshot of the parent accumulator before the host transforms it
    /// (children inlined, or filter pipeline run) — the "before" of the lerp.
    snapshot: wgpu::Texture,
    snapshot_view: wgpu::TextureView,
    /// Uniform buffer for the mask lerp shader (isolated flag).
    uniform_buf: wgpu::Buffer,
}

/// Lean per-host projection for a leaf layer (raster / void) that carries a
/// visible mask filter. The host's content composites into this isolated
/// window-sized buffer; the mask modulates it (`apply_mask`); the finished
/// projection blends down onto the parent — so the mask never samples the host
/// layer's texture or geometry. This is the de-fused replacement for the fused
/// mask that used to live inside the layer-blend pass.
///
/// Leaner than a [`GroupState`]: just a ping-pong pair (content → masked) and
/// the three uniform buffers the three passes need. No composite cache, no
/// child caching — a leaf has exactly one piece of content.
struct ProjectionState {
    /// `[0]` receives the composited host content; `[1]` receives the
    /// mask-modulated result. (Two textures, not a true ping-pong loop.)
    accum: AccumPair,
    /// Compose-content-into-projection uniform: opacity 1, Normal blend, the
    /// host layer texture's own offset/size (so the content samples cleanly).
    content_uniform_buf: wgpu::Buffer,
    /// Down-composite uniform: the host's opacity + blend mode, canvas-window
    /// geometry (the projection occupies exactly the canvas window in plane).
    down_uniform_buf: wgpu::Buffer,
    /// `apply_mask` uniform: the mask texture's plane offset/size + isolated.
    mask_uniform_buf: wgpu::Buffer,
    /// Dimensions this state was allocated at; rebuilt when the canvas resizes.
    padded_w: u32,
    padded_h: u32,
}

pub struct Compositor {
    /// Per-group GPU state. Every non-passthrough group (including root)
    /// owns a GroupState with its own accumulators and composite cache.
    /// Root's state lives at group_state[self.root_id].
    group_state: HashMap<LayerId, GroupState>,

    /// Implicit root group id. Mirrored from the document at construction
    /// time so the compositor can address its own root's `GroupState` /
    /// composite cache without re-deriving it on every call. Stays valid for
    /// the compositor's lifetime — root id is fixed once allocated.
    root_id: LayerId,

    /// One pool of per-node GPU textures, keyed by node id. Holds raster
    /// layer textures (Rgba8Unorm), mask filter textures (R8Unorm), and
    /// any future pixel-bearing filter kinds — `LayerTexture.format`
    /// distinguishes them. One lookup per access, no fan-out.
    pub(super) node_textures: HashMap<LayerId, LayerTexture>,

    /// Default mask bind group using the 1×1 white texture (pass-through
    /// fallback for hosts without a visible mask filter).
    default_mask_bind_group: wgpu::BindGroup,

    /// Cached "use my texture as a mask" bind group, keyed by mask filter
    /// id. Built when a mask filter is allocated; consumed by the blend
    /// pipeline at composite time. Visibility gating happens in the render
    /// loop (which falls back to `default_mask_bind_group` for hidden masks).
    mask_bind_groups: HashMap<LayerId, wgpu::BindGroup>,

    /// Cached blend bind groups for `compose_children`. Key is
    /// `(parent_group, child_id, src_accum_idx)` — both ping-pong sides
    /// get their own entry per child because the source accum view flips
    /// every layer. Entries are invalidated in `dispose_node_texture` and
    /// `resize_node_texture` against the affected node id (either as
    /// parent or child); the floating-target case bypasses the cache so
    /// preview-state ephemera never leak in.
    blend_bind_groups: HashMap<(LayerId, LayerId, u8), wgpu::BindGroup>,

    /// Pre-built GPU objects per content layer (raster + void). Keyed by
    /// the document's [`LayerId`] — both kinds share the same blend
    /// pipeline path, so collapsing them into one pool means the blend
    /// arm, uniforms write, and dispose all do exactly one lookup.
    pub(super) layer_cache: HashMap<LayerId, LayerCache>,

    pub(super) blend_pipelines: BlendPipelines,

    // --- Passthrough Group Mask (Photoshop-style snapshot-lerp) ---
    mask_lerp_pipeline: wgpu::RenderPipeline,
    /// Per-group GPU state for passthrough groups with masks.
    mask_snapshot_state: HashMap<LayerId, MaskSnapshotState>,

    // --- Leaf mask (de-fused projection + apply_mask) ---
    /// Pass that modulates a projection's alpha by a mask in the mask's own
    /// space (`apply_mask.wgsl`).
    apply_mask_pipeline: crate::gpu::apply_mask::ApplyMaskPipeline,
    /// Pooled per-host projection state, allocated lazily for leaf layers with
    /// a visible mask and released on mask remove/hide, host delete, or canvas
    /// resize. Keyed by the host layer id.
    projection_states: HashMap<LayerId, ProjectionState>,

    present_pipeline: wgpu::RenderPipeline,
    /// Present pipeline targeting the accum format (Rgba8Unorm) for veil input.
    present_to_veil_pipeline: wgpu::RenderPipeline,
    _present_bind_group_layout: wgpu::BindGroupLayout,
    /// Present bind group that reads from root's composite_cache.
    present_cache_bind_group: wgpu::BindGroup,
    /// View transform uniform buffer for the present shader.
    view_uniform_buf: wgpu::Buffer,

    /// Shared canvas-geometry uniform ([`CanvasUniform`]) — the single copy of
    /// `canvas_size` + `canvas_origin` bound to every composite draw (group 2).
    /// Written only by [`Self::set_canvas_rect`].
    canvas_uniform_buf: wgpu::Buffer,
    /// Bind group wrapping `canvas_uniform_buf` for the blend pipeline's group 2.
    /// Stable across frames — only the buffer *contents* change on resize.
    canvas_bind_group: wgpu::BindGroup,

    pub(super) sampler: wgpu::Sampler,

    /// Dirty gate — false means nothing changed, skip compositing.
    needs_composite: bool,
    /// When only the view transform changes, skip compositing and only re-present.
    needs_present: bool,

    /// Layers whose pixel content was modified since the last drain.
    /// Drained by the engine each frame to auto-queue thumbnail readbacks
    /// — anything in here had its layer texture written, so the panel's
    /// thumbnail is stale until a fresh readback lands.
    /// Node ids whose textures were modified since the last drain. Single
    /// pool keyed by node id; raster layers and mask filters go in the
    /// same set, and the engine's drain pumps thumbnail readbacks for both
    /// uniformly.
    dirty_node_pixels: HashSet<LayerId>,

    pub(super) canvas_width: u32,
    pub(super) canvas_height: u32,
    /// Plane-space offset of the canvas window, mirrored from
    /// `Document::canvas_origin`. Drives the selection-mask UV seam and the
    /// window→plane mapping in the layer composite shader. Updated together
    /// with `canvas_width`/`canvas_height` by `set_canvas_rect`.
    pub(super) canvas_origin: crate::coord::CanvasPoint,
    /// Padded (tile-aligned) render target dimensions — used for shader UV
    /// computations in the transform pass, which must match the actual
    /// accumulator texture size.
    padded_width: u32,
    padded_height: u32,

    veil_chain: VeilChain,

    /// Lazily-pipeline-cached registry of every void type built into the
    /// binary. Engine queries this for `void_types()` and `add_void_layer`
    /// goes through it to build the per-instance trait object.
    void_registry: VoidRegistry,

    /// Lazily-pipeline-cached registry of every filter type built into the
    /// binary (invert, …). Shared by both consumers of a filter pipeline: the
    /// destructive `apply_filter` path (one-shot document edit) and filter
    /// *layers* (per-frame accumulator transform in `compose_filter_arm`).
    /// Compositor-owned because the cached `MaskedFilterPipeline` is a GPU
    /// resource, exactly like `void_registry`'s pipelines.
    filter_pipeline_registry: crate::gpu::filter::FilterPipelineRegistry,

    /// Per-filter-layer GPU state for *parametric* filters (curves, …). The
    /// stored `Vec<ParamValue>` is the change-detection fingerprint — when a
    /// layer's params differ from it, the effect rebuilds its resources (the
    /// LUT) into the paired [`EffectCache`] during the pre-compose ensure phase
    /// (`sync_projection_states`). Compose then merely binds the cache. Entries
    /// for removed filter layers are pruned there too. Parameter-free filters
    /// (invert) never populate this — their `ensure` is a no-op.
    filter_caches: HashMap<LayerId, (Vec<ParamValue>, crate::gpu::effect::EffectCache)>,

    // --- Floating Content Transform ---
    pub(super) transform_pass: crate::gpu::transform::TransformPass,

    // --- Image rescale resampling ---
    rescale_pass: crate::gpu::rescale::RescalePass,

    // --- Orthogonal (flip / 90° rotate) transforms ---
    ortho_pass: crate::gpu::ortho_transform::OrthoTransformPass,

    // --- Isolation (session state) ---
    /// When `Some(id)`, the render walk descends only into nodes on the
    /// path between the root and `id` (ancestors + self + descendants).
    /// Off-path subtrees are skipped entirely without touching their
    /// `visible` document state — eye icons stay independent.
    ///
    /// Mirrored from `engine.isolated_node` via `set_isolated_node`. The
    /// per-host `isolated` uniform (sample mask as grayscale) is driven
    /// off the same field by `sync_compositor_layers`.
    isolated_node: Option<LayerId>,

    // --- Selection (global) ---
    /// GPU realisation of the document's selection filter — ping-pong R8
    /// textures + brush/paint bind groups. `None` until the engine allocates
    /// the selection filter; once allocated, lives for the document's
    /// lifetime. Pixel metadata (active toggle, tight bounds, CPU cache)
    /// lives on `Document.selection.kind` (`SelectionFilter`).
    selection_state: Option<crate::gpu::selection::SelectionState>,

    // --- Tool Overlay ---
    tool_overlay: ToolOverlay,
    /// Cached view transform for overlay forward matrix computation.
    cached_view_transform: ViewTransform,
    /// Workspace color drawn by the present shader outside the canvas
    /// rectangle. Stamped onto every transform on upload, so changing it
    /// only requires re-uploading the cached transform.
    viewport_bg: [f32; 4],
    /// Pixel filter mode for the present shader's canvas-to-screen sample.
    /// 0 = linear (smooth), 1 = nearest (hard pixels), 2 = auto (nearest
    /// when zoom > 1, linear otherwise — decided in the shader from the
    /// matrix). Stamped onto `flags[0]` of the transform on upload.
    pixel_filter: f32,

    // --- Content Bounds (GPU compute) ---
    content_bounds: ContentBoundsPass,

    // --- Frame Scheduler ---
    /// Monotonic frame counter, incremented on each rAF tick.
    /// Systems fire when `frame_count % divisor == 0`.
    frame_count: u64,
    /// Last wall-clock time for dt computation.
    last_wall_time: f32,

    /// Reused buffer for the "ids of dirty procedural layers" pass in
    /// `encode_dirty_layer_content`. Cleared at the top of the pass and
    /// drained before returning, so the only retained allocation is the
    /// `Vec` capacity itself.
    dirty_procedural_scratch: Vec<LayerId>,
}

impl Compositor {
    /// Create an accumulator texture at padded canvas dimensions.
    fn make_accum_texture(
        device: &wgpu::Device,
        padded_w: u32,
        padded_h: u32,
        label: &str,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: padded_w,
                height: padded_h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        (tex, view)
    }

    /// Create a GroupState (accum pair + composite cache + uniforms).
    fn create_group_state(
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
        let (cache, cache_view) =
            Self::make_accum_texture(device, padded_w, padded_h, &format!("cache-{group_id:?}"));

        let canvas = [padded_w as f32, padded_h as f32];
        let normal = crate::gpu::blend_mode::registry().default().gpu_value;
        // The group's window-sized cache occupies exactly the canvas window in
        // plane space, so describing it as a "layer" at `layer_offset =
        // canvas_origin`, `layer_size = canvas_size` makes the shared-canvas
        // plane round-trip in `composite.wgsl` collapse to an identity sample.
        let uniforms = BlendUniforms {
            opacity: 1.0,
            blend_mode: normal,
            isolated: 0,
            _pad1: 0.0,
            layer_offset: [canvas_origin.x as f32, canvas_origin.y as f32],
            layer_size: canvas,
        };
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!("group-uniforms-{group_id:?}")),
            size: std::mem::size_of::<BlendUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        GroupState {
            accum: AccumPair {
                textures: [a0, a1],
                views: [v0, v1],
            },
            current_accum: 0,
            composite_cache: cache,
            composite_cache_view: cache_view,
            cache_valid_through: None,
            uniform_buf,
        }
    }

    /// Build the present bind group that samples the root composite cache.
    /// Shared by `new` and `set_canvas_rect` so the binding layout lives in
    /// exactly one place.
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
        let default_mask_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("default-mask-1x1"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
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
        let default_mask_view =
            default_mask_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let default_mask_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("default-mask-bg"),
            layout: &blend_pipelines.mask_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&default_mask_view),
            }],
        });

        // --- Mask lerp pipeline (passthrough groups with masks) ---
        // Reuses the blend BGL for group 0 (before, after, sampler, uniforms)
        // and the mask BGL for group 1 (mask texture).
        let mask_lerp_pipeline = {
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("mask-lerp-shader"),
                source: wgpu::ShaderSource::Wgsl(
                    crate::gpu::canvas_lib::with_canvas_lib(include_str!(
                        "../../shaders/mask_lerp.wgsl"
                    ))
                    .into(),
                ),
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("mask-lerp-pipeline-layout"),
                bind_group_layouts: &[
                    Some(&blend_pipelines.bind_group_layout),
                    Some(&blend_pipelines.mask_bind_group_layout),
                ],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("mask-lerp-pipeline"),
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
                        format: accum_format,
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
        // View transform uniform buffer (present shader binding 2)
        let view_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("view-transform-uniform"),
            size: std::mem::size_of::<ViewTransform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
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

        let present_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("present-pipeline"),
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
                    format: surface_format,
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
        });

        let accum_format = wgpu::TextureFormat::Rgba8Unorm;
        let present_to_veil_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("present-to-veil-pipeline"),
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
                        format: accum_format,
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
            });

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
        let canvas_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("canvas-geometry-uniform"),
            size: std::mem::size_of::<CanvasUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&canvas_uniform_buf, 0, bytemuck::bytes_of(&canvas_uniform));
        let canvas_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("canvas-geometry-bg"),
            layout: &blend_pipelines.canvas_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: canvas_uniform_buf.as_entire_binding(),
            }],
        });

        // Present bind group reads from root's composite cache
        let present_cache_bind_group = Self::make_present_cache_bind_group(
            device,
            &_present_bind_group_layout,
            &root_state.composite_cache_view,
            &sampler,
            &view_uniform_buf,
        );

        let mut group_state = HashMap::new();
        group_state.insert(root_id, root_state);

        let veil_chain = VeilChain::new(device, sampler.clone(), surface_format, accum_format);

        let tool_overlay = ToolOverlay::new(device, queue, surface_format);

        let transform_pass = crate::gpu::transform::TransformPass::new(device);
        let rescale_pass = crate::gpu::rescale::RescalePass::new(device);
        let ortho_pass = crate::gpu::ortho_transform::OrthoTransformPass::new(device);
        let content_bounds = ContentBoundsPass::new(device);

        Compositor {
            group_state,
            root_id,
            node_textures: HashMap::new(),
            default_mask_bind_group,
            mask_bind_groups: HashMap::new(),
            blend_bind_groups: HashMap::new(),
            layer_cache: HashMap::new(),
            blend_pipelines,
            mask_lerp_pipeline,
            mask_snapshot_state: HashMap::new(),
            apply_mask_pipeline,
            projection_states: HashMap::new(),
            present_pipeline,
            present_to_veil_pipeline,
            _present_bind_group_layout,
            present_cache_bind_group,
            view_uniform_buf,
            canvas_uniform_buf,
            canvas_bind_group,
            sampler,
            needs_composite: true,
            needs_present: false,
            dirty_node_pixels: HashSet::new(),
            canvas_width: width,
            canvas_height: height,
            canvas_origin: crate::coord::CanvasPoint::new(0, 0),
            padded_width: padded_w,
            padded_height: padded_h,
            veil_chain,
            void_registry: VoidRegistry::new(),
            filter_pipeline_registry: crate::gpu::filter::FilterPipelineRegistry::new(),
            filter_caches: HashMap::new(),
            transform_pass,
            rescale_pass,
            ortho_pass,
            isolated_node: None,
            selection_state: None,
            content_bounds,
            tool_overlay,
            cached_view_transform: identity,
            viewport_bg: DEFAULT_WORKSPACE_BG,
            pixel_filter: pixel_filter_from_config(),
            frame_count: 0,
            last_wall_time: 0.0,
            dirty_procedural_scratch: Vec::new(),
        }
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

    /// Construct a `Box<dyn Void>` via the compositor-owned registry without
    /// exposing the registry itself. Used by [`VoidLayer`]'s
    /// [`LayerKindGpu::realize_in`] so the registry stays private to the
    /// compositor.
    fn create_void_box(
        &mut self,
        device: &wgpu::Device,
        type_id: &str,
        params: &[ParamValue],
    ) -> Box<dyn Void> {
        let format = self.canvas_content_format();
        self.void_registry
            .create_void(type_id, params, device, format)
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

        let normal = crate::gpu::blend_mode::registry().default().gpu_value;
        let uniforms = BlendUniforms {
            opacity: 1.0,
            blend_mode: normal,
            isolated: 0,
            _pad1: 0.0,
            layer_offset: [bounds.origin.x as f32, bounds.origin.y as f32],
            layer_size: [bounds.width as f32, bounds.height as f32],
        };

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!("blend-uniforms-{layer_id:?}")),
            size: std::mem::size_of::<BlendUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        self.layer_cache.insert(
            layer_id,
            LayerCache {
                uniform_buf,
                opacity: 1.0,
                blend_mode: normal,
                isolated: false,
                content: LayerContent::Raster,
            },
        );
        self.node_textures.insert(layer_id, layer_tex);
        // A freshly-allocated layer still needs a thumbnail slot — without
        // this, an empty new layer renders as "no thumbnail" in the panel
        // until the user paints. Part of the "any write/alloc to a node
        // texture marks it dirty" invariant; see `mark_node_pixels_dirty`.
        self.mark_node_pixels_dirty(layer_id);
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
            Some(t) => (t.canvas_extent(), t.format()),
            None => return,
        };
        if current == new_extent {
            return;
        }

        let new_tex = match format {
            wgpu::TextureFormat::R8Unorm => {
                LayerTexture::new_mask_with_extent(device, queue, new_extent)
            }
            wgpu::TextureFormat::Rgba8Unorm => LayerTexture::with_bounds(device, new_extent),
            other => panic!("realloc_node_texture: unsupported format {other:?}"),
        };

        if copy_old {
            let old_tex = self
                .node_textures
                .get(&node_id)
                .expect("node_textures entry checked above");
            let copy_dst_x = (current.origin.x - new_extent.origin.x) as u32;
            let copy_dst_y = (current.origin.y - new_extent.origin.y) as u32;
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: old_tex.texture(),
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: new_tex.texture(),
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: copy_dst_x,
                        y: copy_dst_y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: current.width,
                    height: current.height,
                    depth_or_array_layers: 1,
                },
            );
        }

        self.swap_node_texture(device, queue, node_id, new_tex);

        // Resize rewrites the texture; thumbnail must reflect the new
        // extent + transferred pixels.
        self.mark_node_pixels_dirty(node_id);
        self.mark_dirty();
    }

    /// Replace a node's texture handle and rebuild the cached state that
    /// referenced the old view. Shared by every path that swaps a node texture
    /// out from under the compositor (resize/realloc, rescale).
    fn swap_node_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        node_id: LayerId,
        new_tex: LayerTexture,
    ) {
        self.node_textures.insert(node_id, new_tex);

        // A content layer's blend uniform bakes in the texture's canvas extent
        // (`layer_offset` / `layer_size`). The extent may have just changed, so
        // refresh it from the cached blend props — otherwise the composite
        // samples the new texture through stale geometry (the post-resize
        // squash `BlendUniforms` is designed to make unrepresentable). Masks
        // have no layer_cache entry and are unaffected.
        if let Some(cache) = self.layer_cache.get(&node_id) {
            let ext = self.node_textures[&node_id].canvas_extent();
            let uniforms = BlendUniforms {
                opacity: cache.opacity,
                blend_mode: cache.blend_mode,
                isolated: cache.isolated as u32,
                _pad1: 0.0,
                layer_offset: [ext.x0() as f32, ext.y0() as f32],
                layer_size: [ext.width as f32, ext.height as f32],
            };
            queue.write_buffer(&cache.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
        }

        // Any cached blend bind groups using this node (as parent or child)
        // reference the now-replaced texture view; drop them so the next
        // composite re-creates against the fresh handle.
        self.blend_bind_groups
            .retain(|(parent, child, _), _| *parent != node_id && *child != node_id);

        // If this node has a cached mask bind group, rebuild it against the
        // freshly-allocated view. The blend stage holds no other reference.
        if self.mask_bind_groups.contains_key(&node_id) {
            let view = self.node_textures[&node_id].view();
            let mask_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("mask-bg-{node_id:?}")),
                layout: &self.blend_pipelines.mask_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                }],
            });
            self.mask_bind_groups.insert(node_id, mask_bg);
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
                Some(t) => (t.canvas_extent(), t.format()),
                None => continue,
            };
            let new_extent = scaled_extent_about(old_extent, origin, sx, sy);
            let new_tex = {
                let src = self
                    .node_textures
                    .get(&id)
                    .expect("node_textures entry checked above");
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
                Some(t) => (t.canvas_extent(), t.format()),
                None => continue,
            };
            let new_extent = ortho_extent_about(old_extent, frame, xform);
            let new_tex = {
                let src = self
                    .node_textures
                    .get(&id)
                    .expect("node_textures entry checked above");
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

    /// Run a `MaskedFilterPipeline` over a node's `region` in place — the
    /// destructive-filter counterpart of
    /// [`flip_node_region`](Self::flip_node_region), riding the same copy-out →
    /// pass → copy-back plumbing (`run_filter_region`). Where `mask_view` (a
    /// region-sized R8) is selected the texel takes the filtered value,
    /// elsewhere it passes through; `None` filters the whole region. Node-
    /// generic — the pipeline's format dispatch handles RGBA8 layers and R8
    /// masks alike, so masks invert for free.
    pub fn filter_node_region(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        node_id: LayerId,
        region: CanvasRect,
        mask_view: Option<&wgpu::TextureView>,
        pipeline: &dyn crate::gpu::filter::FilterEffect,
    ) {
        // The destructive path serves only parameter-free filters (parametric
        // ones are excluded upstream in `apply_filter`), so a throwaway empty
        // cache is all the effect's `render` needs — invert ignores it.
        let cache = crate::gpu::effect::EffectCache::empty();
        let ran = run_filter_region(
            &self.node_textures,
            device,
            queue,
            encoder,
            node_id,
            region,
            mask_view,
            |dev, _q, enc, src, mask, out, _w, _h, fmt| {
                pipeline.render(dev, enc, src, mask, out, fmt, &cache)
            },
        );
        if ran {
            self.mark_node_pixels_dirty(node_id);
            self.mark_dirty();
        }
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
        let gs = Self::create_group_state(
            device,
            queue,
            self.padded_width,
            self.padded_height,
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
        // Accumulators match canvas dimensions exactly (no tile padding).
        self.padded_width = width;
        self.padded_height = height;

        // Update the single shared canvas-geometry uniform. This is the one
        // write that keeps every composite draw's canvas_size/canvas_origin in
        // step with the document — the per-layer uniforms no longer carry these
        // fields, so a layer created before this resize can no longer composite
        // through stale dimensions (the post-resize anisotropic-squash bug).
        let canvas_uniform = CanvasUniform {
            canvas_size: [width as f32, height as f32],
            canvas_origin: [origin.x as f32, origin.y as f32],
        };
        queue.write_buffer(
            &self.canvas_uniform_buf,
            0,
            bytemuck::bytes_of(&canvas_uniform),
        );

        // Recreate every group's accumulators + cache at the new size.
        let group_ids: Vec<LayerId> = self.group_state.keys().copied().collect();
        for gid in group_ids {
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

        // Present samples the root composite cache — rebind to the fresh view.
        self.present_cache_bind_group = Self::make_present_cache_bind_group(
            device,
            &self._present_bind_group_layout,
            &self.group_state[&self.root_id].composite_cache_view,
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

        self.needs_composite = true;
        self.needs_present = true;
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
            let canvas = [self.canvas_width as f32, self.canvas_height as f32];
            let uniforms = BlendUniforms {
                opacity,
                blend_mode: blend_mode_gpu,
                isolated: isolated as u32,
                _pad1: 0.0,
                layer_offset: [self.canvas_origin.x as f32, self.canvas_origin.y as f32],
                layer_size: canvas,
            };
            queue.write_buffer(&gs.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
        }
        // The passthrough-mask lerp uniform (mask geometry + isolated) is
        // refreshed per-frame in `sync_projection_states`, where the mask's
        // current extent is available.
    }

    /// Set the session-level isolation target. The render walk filters off-
    /// path subtrees on the next composite. Pass `None` to clear isolation.
    /// Engine state (`engine.isolated_node`) is the originator; this mirror
    /// drives the renderer.
    pub fn set_isolated_node(&mut self, id: Option<LayerId>) {
        self.isolated_node = id;
        self.mark_dirty();
    }

    /// True if the renderer should descend into / render `id` under the
    /// current isolation target. When no target is set, every id qualifies.
    /// Otherwise the path is `ancestors(target) ∪ {target} ∪ descendants(target)` —
    /// ancestors so the walk reaches the target, descendants so an isolated
    /// group renders its contents. Filters naturally fall in via their
    /// host (which is the filter's `parent_of`); they have no children, so
    /// isolating a filter limits the visible canvas to the host plus the
    /// filter itself, which the host's blend pass then renders as
    /// grayscale via `sync_compositor_layers` setting `isolated=true`.
    fn is_in_isolation_path(&self, doc: &Document, id: LayerId) -> bool {
        let Some(target) = self.isolated_node else {
            return true;
        };
        if id == target {
            return true;
        }
        // Is `id` an ancestor of the target?
        let mut cur = doc.parent_of(target);
        while let Some(p) = cur {
            if p == id {
                return true;
            }
            cur = doc.parent_of(p);
        }
        // Is `id` a descendant of the target?
        let mut cur = doc.parent_of(id);
        while let Some(p) = cur {
            if p == target {
                return true;
            }
            cur = doc.parent_of(p);
        }
        false
    }

    /// Mark that recompositing is needed.
    pub fn mark_dirty(&mut self) {
        self.needs_composite = true;
        // Invalidate all group caches
        for gs in self.group_state.values_mut() {
            gs.cache_valid_through = None;
        }
        // Invalidate all layer content bounds — pixels may have changed.
        self.content_bounds.invalidate_all();
    }

    /// Mark that a node's pixels changed. Records the node id in the
    /// per-frame dirty set the engine drains to auto-queue thumbnail
    /// readbacks, then implies `mark_dirty()`.
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
        self.dirty_node_pixels.insert(node_id);
        self.mark_dirty();
    }

    /// Drain and return the set of node ids whose pixels were dirtied since
    /// the last call. Engine calls this each `render()` to auto-queue
    /// thumbnail readbacks; resolves layer-vs-filter through the document.
    pub fn drain_dirty_pixels(&mut self) -> Vec<LayerId> {
        self.dirty_node_pixels.drain().collect()
    }

    /// Mark that only the present pass needs to re-run (view transform changed).
    /// Skips compositing when there are no dirty tiles or layer changes.
    pub fn mark_needs_present(&mut self) {
        self.needs_present = true;
    }

    // --- Content Bounds (GPU compute) ---

    /// Return cached content bounds for a layer: `[x, y, w, h]`.
    /// Returns `None` if bounds haven't been computed yet or were invalidated.
    pub fn content_bounds(&self, layer_id: LayerId) -> Option<[u32; 4]> {
        self.content_bounds.get(layer_id)
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
        let Some(tex) = self.node_textures.get(&node_id) else {
            return;
        };
        let r_channel = tex.format() == wgpu::TextureFormat::R8Unorm;
        let extent = tex.layer_extent();
        self.content_bounds.request(
            device,
            queue,
            tex.view(),
            extent.width,
            extent.height,
            r_channel,
            node_id,
        );
    }

    /// Poll pending content bounds computations. Call once per frame.
    /// Returns layer IDs whose bounds just became available.
    pub fn poll_content_bounds(&mut self, device: &wgpu::Device) -> Vec<LayerId> {
        self.content_bounds.poll(device)
    }

    /// True if any content bounds computations are in flight.
    pub fn has_pending_content_bounds(&self) -> bool {
        self.content_bounds.has_pending()
    }

    /// True if a content bounds computation is in flight for a specific layer.
    pub fn is_content_bounds_pending(&self, layer_id: LayerId) -> bool {
        self.content_bounds.is_pending(layer_id)
    }

    // --- Paint Target Accessors ---

    /// Look up a node's GPU texture by id. Works uniformly for raster layers
    /// and mask filters — format and extent come from the texture's own
    /// metadata. Returns `None` for groups (no pixels) and unknown ids.
    pub fn node_texture(&self, node_id: LayerId) -> Option<&LayerTexture> {
        self.node_textures.get(&node_id)
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
            if let Some((w, h)) = proc.void.persistent_frame_size() {
                if let Some(tex) = proc.cache.aux_textures.first() {
                    return Some(PixelDataRef {
                        texture: tex,
                        format: tex.format(),
                        width: w,
                        height: h,
                    });
                }
            }
        }
        if let Some(t) = self.node_textures.get(&node_id) {
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

    /// Current persistent frame size of a void layer, if the void declares
    /// one. Used by the engine after `upload_void_external_image` to keep
    /// the doc's [`crate::layer::VoidLayer::frame`] in sync with the
    /// GPU-side texture so save sees the right dimensions.
    pub fn void_persistent_frame_size(&self, layer_id: LayerId) -> Option<(u32, u32)> {
        self.procedural_content(layer_id)
            .and_then(|p| p.void.persistent_frame_size())
    }

    /// Restore a saved void frame at document load. Wraps
    /// [`crate::gpu::void::Void::restore_persistent_pixels`] — sized for
    /// the saved dimensions, the void rebuilds its bind group around the
    /// new texture and writes the bytes via `queue.write_texture`. Marks
    /// the void dirty so the next composite re-renders it.
    pub fn restore_void_pixels(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layer_id: LayerId,
        width: u32,
        height: u32,
        bytes: &[u8],
    ) {
        let Some(proc) = self.procedural_content_mut(layer_id) else {
            return;
        };
        proc.void
            .restore_persistent_pixels(device, queue, &mut proc.cache, width, height, bytes);
        self.mark_dirty();
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
        let Some(tex) = self.node_textures.get(&node_id) else {
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
                self.node_textures.insert(node_id, mask_tex);
                self.mask_bind_groups.insert(node_id, mask_bg);
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
            self.padded_width,
            self.padded_height,
            &format!("mask-snapshot-{host_id:?}"),
        );
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!("mask-snapshot-lerp-uniforms-{host_id:?}")),
            size: std::mem::size_of::<LerpUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
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

    /// Borrow a layer's procedural-content sidecar, if any. Returns `None`
    /// for raster layers (no sidecar) and unknown ids. Centralises the
    /// "is this a procedural layer?" lookup so the rest of the compositor
    /// never pattern-matches on [`LayerContent`] directly.
    fn procedural_content(&self, layer_id: LayerId) -> Option<&ProceduralContent> {
        match self.layer_cache.get(&layer_id).map(|c| &c.content) {
            Some(LayerContent::Procedural(p)) => Some(p),
            _ => None,
        }
    }

    /// Mutable counterpart to [`Self::procedural_content`].
    fn procedural_content_mut(&mut self, layer_id: LayerId) -> Option<&mut ProceduralContent> {
        match self.layer_cache.get_mut(&layer_id).map(|c| &mut c.content) {
            Some(LayerContent::Procedural(p)) => Some(p),
            _ => None,
        }
    }

    /// Drop all GPU state associated with a node id (texture, bind groups,
    /// dirty bits, layer cache including any procedural-content sidecar).
    /// Use when a node is permanently removed — e.g. layer delete or
    /// filter removal. Per-host passthrough state is owned by its host
    /// id, so it's not touched here.
    pub fn dispose_node_texture(&mut self, node_id: LayerId) {
        self.node_textures.remove(&node_id);
        self.mask_bind_groups.remove(&node_id);
        // Blend bind groups that name this node as either the parent (a
        // group whose accum is gone) or the child (a layer or child-group
        // whose view is gone) point at a freed texture handle. Evict them
        // so the next composite rebuilds against current state.
        self.blend_bind_groups
            .retain(|(parent, child, _), _| *parent != node_id && *child != node_id);
        self.layer_cache.remove(&node_id);
        // A deleted host's projection is released immediately; a deleted mask
        // is caught by the next `sync_projection_states` stale sweep.
        self.projection_states.remove(&node_id);
        self.dirty_node_pixels.remove(&node_id);
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

    /// Read-only access to the filter pipeline registry — lets the engine
    /// answer `filter_types()` without exposing a mutable handle.
    pub fn filter_pipeline_registry(&self) -> &crate::gpu::filter::FilterPipelineRegistry {
        &self.filter_pipeline_registry
    }

    /// Mutable access to the filter pipeline registry. The destructive
    /// `apply_filter` path goes through this to resolve (and lazily cache) the
    /// shared `MaskedFilterPipeline` for a filter type.
    pub fn filter_pipeline_registry_mut(
        &mut self,
    ) -> &mut crate::gpu::filter::FilterPipelineRegistry {
        &mut self.filter_pipeline_registry
    }

    /// Canvas-content texture format used by every content layer (raster +
    /// void). Exposed so engine paths that need to construct a void via the
    /// registry before calling [`Self::ensure_void_layer`] can pass the same
    /// format the compositor would have used internally.
    pub fn canvas_content_format(&self) -> wgpu::TextureFormat {
        wgpu::TextureFormat::Rgba8Unorm
    }

    /// Allocate the per-instance GPU state for a new void layer:
    /// procedural texture in [`Self::node_textures`] (canvas-sized
    /// [`wgpu::TextureFormat::Rgba8Unorm`], matching the raster path so the
    /// compositor's blend pipeline can sample it without any kind-specific
    /// branch) and a [`LayerCache`] holding the blend uniforms plus a
    /// [`LayerContent::Procedural`] sidecar with the trait object and its
    /// `EffectCache`. Idempotent — calling twice for the same id is a no-op.
    ///
    /// The caller constructs `void` via the engine's void registry. The
    /// compositor takes the trait object as-is and stops bookkeeping the
    /// `(type_id, params)` pair: ownership of those facts already lives on
    /// the `Void` itself.
    pub fn ensure_void_layer(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layer_id: LayerId,
        void: Box<dyn Void>,
    ) {
        if self.layer_cache.contains_key(&layer_id) {
            return;
        }
        // Canvas-sized texture so the procedural output composites against
        // the same coordinate system as raster layers.
        let bounds = self.canvas_rect();
        let layer_tex = LayerTexture::with_bounds(device, bounds);

        let cache = void.create_cache(
            device,
            queue,
            layer_tex.view(),
            &self.sampler,
            self.canvas_width,
            self.canvas_height,
        );

        // Blend uniforms — same layout raster layers use; the shader doesn't
        // care which kind sourced the texture it samples.
        let normal = crate::gpu::blend_mode::registry().default().gpu_value;
        let uniforms = BlendUniforms {
            opacity: 1.0,
            blend_mode: normal,
            isolated: 0,
            _pad1: 0.0,
            layer_offset: [bounds.origin.x as f32, bounds.origin.y as f32],
            layer_size: [bounds.width as f32, bounds.height as f32],
        };
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!("blend-uniforms-{layer_id:?}")),
            size: std::mem::size_of::<BlendUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        self.node_textures.insert(layer_id, layer_tex);
        self.layer_cache.insert(
            layer_id,
            LayerCache {
                uniform_buf,
                opacity: 1.0,
                blend_mode: normal,
                isolated: false,
                content: LayerContent::Procedural(ProceduralContent { void, cache }),
            },
        );
        self.mark_dirty();
    }

    /// Update a void's procedural inputs in place. The void mutates its
    /// own fields and rewrites the uniform buffer; the existing
    /// `EffectCache` (including any aux textures the void was using to
    /// hold stateful pixel data — e.g. the camera void's last received
    /// frame) is preserved untouched. The blend uniforms (opacity / mode
    /// / isolated) are also untouched — only the procedural side changes.
    pub fn update_void_layer_params(
        &mut self,
        queue: &wgpu::Queue,
        layer_id: LayerId,
        params: &[ParamValue],
    ) {
        let Some(proc) = self.procedural_content_mut(layer_id) else {
            return;
        };
        proc.void.update_params(queue, &proc.cache, params);
        self.mark_dirty();
    }

    /// Apply a void's user transform in place. Sibling of
    /// [`Self::update_void_layer_params`]: delegates to [`Void::set_transform`],
    /// which rewrites the uniform without rebuilding (preserving any aux
    /// textures, e.g. the camera's live frame). No-op for non-procedural or
    /// non-transform-aware voids.
    pub fn update_void_layer_transform(
        &mut self,
        queue: &wgpu::Queue,
        layer_id: LayerId,
        transform: &crate::transform::Transform,
    ) {
        let Some(proc) = self.procedural_content_mut(layer_id) else {
            return;
        };
        proc.void.set_transform(queue, &proc.cache, transform);
        self.mark_dirty();
    }

    /// The void's active-pixel bbox in WINDOW-LOCAL coords, via
    /// [`Void::content_extent`]. Canvas-filling for most voids; the cover-fit
    /// rect for the camera. `None` if `layer_id` isn't a realized void.
    pub fn void_content_extent(&self, layer_id: LayerId) -> Option<(f32, f32, f32, f32)> {
        let proc = self.procedural_content(layer_id)?;
        Some(
            proc.void
                .content_extent(self.canvas_width, self.canvas_height),
        )
    }

    /// Push a fresh external image frame (webcam, screenshare, …) into a
    /// void's input texture. Delegates to the void's
    /// [`Void::upload_external_image`], which handles texture allocation,
    /// bind-group rebuild on dimension changes, and the actual texel copy.
    /// Flags the void's destination texture dirty so the next compositor
    /// frame re-renders it.
    pub fn upload_void_external_image(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layer_id: LayerId,
        source: crate::gpu::void::ExternalImageSource,
    ) {
        let Some(proc) = self.procedural_content_mut(layer_id) else {
            return;
        };
        if !proc.void.wants_external_input() {
            return;
        }
        proc.void
            .upload_external_image(device, queue, &mut proc.cache, source);
        self.mark_dirty();
    }

    /// True when any allocated layer with procedural content reports
    /// `needs_animation()` AND is effectively visible in `doc`. Folded into
    /// the compositor's overall `needs_animation()` so the rAF loop keeps
    /// ticking while animated voids exist — but a hidden layer's animation
    /// contribution is dropped at the same point the compositor's tree walk
    /// would drop the layer's output (see `compose_children`'s
    /// `node.visible()` skip).
    fn any_animated_layer(&self, doc: &Document) -> bool {
        self.layer_cache.iter().any(|(id, c)| match &c.content {
            LayerContent::Procedural(p) => p.void.needs_animation() && doc.effective_visible(*id),
            LayerContent::Raster => false,
        })
    }

    /// Advance every effectively-visible animated layer's procedural
    /// content by `dt`. Called by `update_animations` at the cadence set by
    /// `animation.void_divisor`. Visibility is queried the same way the
    /// main composite walk queries it — no precomputed "hidden" set; the
    /// doc is the authoritative tree.
    fn tick_animated_layers(&mut self, queue: &wgpu::Queue, dt: f32, doc: &Document) {
        for (id, entry) in self.layer_cache.iter_mut() {
            let LayerContent::Procedural(proc) = &mut entry.content else {
                continue;
            };
            if !proc.void.needs_animation() {
                continue;
            }
            if !doc.effective_visible(*id) {
                continue;
            }
            proc.void.update_time(queue, &proc.cache, dt);
        }
    }

    /// Re-render every dirty procedural layer's texture. Runs at the top of
    /// the compositor's encode pass so the subsequent blend in
    /// `compose_children` samples up-to-date pixels. Raster layers are
    /// inherently "never dirty" — their pixels arrived through paint and
    /// `node_textures[id]` is authoritative.
    ///
    /// The dirty bit is the void's own, returned through
    /// [`Void::take_dirty`]: state-changing methods on the trait
    /// (`update_params`, `update_time`, `upload_external_image`) mark it,
    /// and `take_dirty` returns-and-clears so a void encodes at most once
    /// per state change.
    fn encode_dirty_layer_content(&mut self, encoder: &mut wgpu::CommandEncoder) {
        // Two-phase: collect ids of procedural layers whose void reports
        // dirty (consuming the flag), then drop the mutable borrow and
        // re-acquire per-entry. Keeps the loop body short and avoids
        // borrowing `self.layer_cache` and `self.node_textures` at the same
        // time. The scratch buffer is owned by `self` so the per-frame Vec
        // churn vanishes.
        self.dirty_procedural_scratch.clear();
        self.dirty_procedural_scratch
            .extend(self.layer_cache.iter_mut().filter_map(|(id, c)| {
                if let LayerContent::Procedural(p) = &mut c.content {
                    if p.void.take_dirty() {
                        return Some(*id);
                    }
                }
                None
            }));
        // Index-iterate so the loop body can re-borrow `self.layer_cache`
        // mutably; the LayerIds are `Copy`. The scratch retains its
        // capacity across frames so the per-frame `Vec` churn vanishes.
        let count = self.dirty_procedural_scratch.len();
        for i in 0..count {
            let id = self.dirty_procedural_scratch[i];
            let dst_view = match self.node_textures.get(&id) {
                Some(t) => t.view(),
                None => continue,
            };
            // Inline the LayerContent match instead of going through
            // `procedural_content_mut(&mut self, ..)` so the borrow checker
            // sees that `node_textures` and `layer_cache` are disjoint
            // fields — without that, dst_view and the procedural sidecar
            // can't both be live at once.
            let Some(proc) = self
                .layer_cache
                .get_mut(&id)
                .and_then(|c| match &mut c.content {
                    LayerContent::Procedural(p) => Some(p),
                    LayerContent::Raster => None,
                })
            else {
                continue;
            };
            proc.void.encode(encoder, &proc.cache, dst_view);
        }
        // Leave the field empty at exit — capacity is retained but no
        // potentially-stale LayerIds live past dispose.
        self.dirty_procedural_scratch.clear();
    }

    /// Total number of node textures (raster layers + mask filters)
    /// currently allocated. Test-only — used by leak-cycle regression tests
    /// to confirm `dispose_node_texture` reclaims state.
    pub fn test_node_texture_count(&self) -> usize {
        self.node_textures.len()
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

    /// Master rAF tick counter. Advances exactly once per `update_animations`
    /// call (i.e. once per `engine.render`), starting at 0. This is the same
    /// counter every divisor-throttled subsystem inside the compositor checks
    /// (`veil_divisor`, `overlay_divisor`, `void_divisor` — see
    /// [`Self::update_animations`]), so any JS-side throttle that uses
    /// `frame_count % divisor == 0` automatically aligns with all of them.
    /// Exposed so the WASM bridge can hand it to the frontend (e.g. the
    /// camera void's upload throttle).
    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }

    /// Unified frame scheduler. Called once per rAF tick.
    ///
    /// Systems fire at fractional rates of the master clock (rAF rate):
    /// - Veils: every `veil_divisor`-th frame (default 2 = 50% = 30fps at 60hz)
    /// - Overlay: every `overlay_divisor`-th frame (default 4 = 25% = 15fps at 60hz)
    ///
    /// Integer divisors guarantee alignment — a divisor-4 tick always coincides
    /// with a divisor-2 tick, so systems never force extra frame renders.
    ///
    /// `doc` is borrowed to consult layer visibility — animation work for an
    /// effectively-hidden layer (self or any ancestor hidden) is skipped at
    /// exactly the point the compositor's tree walk would drop the layer's
    /// composited output.
    pub fn update_animations(&mut self, queue: &wgpu::Queue, wall_time: f32, doc: &Document) {
        let dt = if self.last_wall_time > 0.0 {
            (wall_time - self.last_wall_time).max(0.0)
        } else {
            0.0
        };
        self.last_wall_time = wall_time;
        self.frame_count += 1;

        if dt == 0.0 {
            return;
        }

        let veil_divisor = crate::config::get_i64("animation.veil_divisor") as u64;
        let overlay_divisor = crate::config::get_i64("animation.overlay_divisor") as u64;
        let void_divisor = crate::config::get_i64("animation.void_divisor") as u64;

        let veil_fires = veil_divisor > 0
            && self.veil_chain.needs_animation()
            && self.frame_count.is_multiple_of(veil_divisor);

        let overlay_fires = overlay_divisor > 0
            && self.tool_overlay.needs_animation()
            && self.frame_count.is_multiple_of(overlay_divisor);

        // Each animated subsystem advances on its own integer divisor of
        // the master rAF clock; integer divisors guarantee no subsystem
        // forces a frame another subsystem wouldn't already produce. See
        // `docs/lessons-learned/gpu-lessons-learned.md` master-clock
        // principle.
        let void_fires = void_divisor > 0
            && self.any_animated_layer(doc)
            && self.frame_count.is_multiple_of(void_divisor);

        if veil_fires {
            self.veil_chain
                .update_veils(queue, dt * veil_divisor as f32);
        }

        if overlay_fires {
            self.tool_overlay.advance_time(dt * overlay_divisor as f32);
        }

        if void_fires {
            self.tick_animated_layers(queue, dt * void_divisor as f32, doc);
            // Re-render needed: voids are document-side content, so they
            // require a full composite, not just a re-present.
            self.needs_composite = true;
        }

        if veil_fires || overlay_fires {
            self.needs_present = true;
        }
    }

    /// Returns true if any animations need continuous frames (veils, overlay,
    /// or any effectively-visible animated layer). `doc` is consulted for
    /// per-layer visibility — same contract as [`Self::update_animations`].
    pub fn needs_animation(&self, doc: &Document) -> bool {
        self.tool_overlay.needs_animation()
            || self.veil_chain.needs_animation()
            || self.any_animated_layer(doc)
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
        self.needs_present = true;
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
        self.needs_present = true;
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
            Some(t) => t,
            None => return,
        };
        let canvas_extent = tex.canvas_extent();
        let uniforms = BlendUniforms {
            opacity,
            blend_mode: blend_mode_gpu,
            isolated: isolated as u32,
            _pad1: 0.0,
            layer_offset: [canvas_extent.x0() as f32, canvas_extent.y0() as f32],
            layer_size: [canvas_extent.width as f32, canvas_extent.height as f32],
        };
        let cache = match self.layer_cache.get_mut(&layer_id) {
            Some(c) => c,
            None => return,
        };
        queue.write_buffer(&cache.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
        cache.opacity = opacity;
        cache.blend_mode = blend_mode_gpu;
        cache.isolated = isolated;

        // Mirror into the floating preview's canvas-aligned uniform buffer
        // so the host's blend pass reads the same blend props (with canvas
        // dims/offset) when sampling the preview view. Voids never become
        // floating targets today, so this is a no-op for procedural layers;
        // keeping it on the shared path means the day they do, it just
        // works.
        self.write_preview_blend_uniforms_if_active(queue, layer_id);
    }

    /// Get the composited output texture (root group's composite cache).
    /// Used by the color picker for readback.
    pub fn composited_texture(&self) -> &wgpu::Texture {
        &self.group_state[&self.root_id].composite_cache
    }

    pub fn accum_format(&self) -> wgpu::TextureFormat {
        wgpu::TextureFormat::Rgba8Unorm
    }

    pub fn veil_chain(&self) -> &VeilChain {
        &self.veil_chain
    }

    pub fn veil_chain_mut(&mut self) -> &mut VeilChain {
        &mut self.veil_chain
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

    /// Effective mask bind group for a host raster/group during compositing
    /// — substitutes the preview-mask bind group when one of the host's
    /// filters is the floating target. Fall-through resolves the live
    /// mask through the existing `mask_bind_group` lookup.
    pub(crate) fn effective_mask_bind_group(
        &self,
        doc: &Document,
        host_id: LayerId,
    ) -> &wgpu::BindGroup {
        Self::effective_mask_bind_group_fields(
            &self.mask_bind_groups,
            &self.default_mask_bind_group,
            &self.transform_pass,
            doc,
            host_id,
        )
    }

    /// Field-explicit variant of [`Self::effective_mask_bind_group`] so a
    /// caller can hold a disjoint `&mut self.blend_bind_groups` borrow
    /// across this lookup. The method-form takes `&self` whole; the
    /// borrow checker can't split that.
    fn effective_mask_bind_group_fields<'a>(
        mask_bind_groups: &'a HashMap<LayerId, wgpu::BindGroup>,
        default_mask_bind_group: &'a wgpu::BindGroup,
        transform_pass: &'a crate::gpu::transform::TransformPass,
        doc: &Document,
        host_id: LayerId,
    ) -> &'a wgpu::BindGroup {
        let live_or_default = doc
            .mask_filter(host_id)
            .filter(|m| m.common.visible)
            .and_then(|m| mask_bind_groups.get(&m.id))
            .unwrap_or(default_mask_bind_group);

        if let Some(state) = transform_pass.active.as_ref() {
            if doc.parent_of(state.target_layer) == Some(host_id) {
                if let Some(bg) = state.preview_mask_bind_group.as_ref() {
                    return bg;
                }
            }
        }
        live_or_default
    }

    /// Run the present pass, veil chain, and final blit to surface.
    /// Solid overlay primitives are drawn at the end of the final render
    /// pass (present or veil-blit) to avoid a separate LoadOp::Load pass.
    fn present_and_veils(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        surface_view: &wgpu::TextureView,
    ) {
        if !self.veil_chain.has_visible() {
            // No veils — present directly to surface.
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
            rpass.set_bind_group(0, &self.present_cache_bind_group, &[]);
            rpass.draw(0..3, 0..1);
            // Draw solid overlay primitives in the same pass.
            self.tool_overlay.draw_solid(&mut rpass);
            return;
        }

        self.veil_chain.encode(
            encoder,
            surface_view,
            &self.present_to_veil_pipeline,
            &self.present_cache_bind_group,
            &self.tool_overlay,
        );
    }

    /// Composite a flat list of source node ids into a target raster layer's
    /// texture, on the GPU, in one submit. Used by Merge Down and Flatten
    /// Image: both operations consume some sources, allocate a destination
    /// raster, and need the destination to hold the baked composite of the
    /// sources under their normal blend modes.
    ///
    /// `source_ids` is bottom-to-top order. Each source may be a raster, a
    /// non-passthrough group (its `composite_cache` must already be current),
    /// or a passthrough group (children inlined). The destination's GPU
    /// texture must already exist and be canvas-sized (the engine allocates
    /// it via `ensure_raster_layer` before calling).
    ///
    /// The bake runs through a transient `GroupState` keyed by slotmap's
    /// null `LayerId` so it doesn't collide with any real group. After
    /// composing, the final accum is `copy_texture_to_texture`'d into the
    /// destination's GPU texture — no CPU readback.
    pub fn bake_subtree_to_layer(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        doc: &mut Document,
        source_ids: &[LayerId],
        dest_layer_id: LayerId,
    ) {
        if !self.node_textures.contains_key(&dest_layer_id) {
            debug_assert!(false, "bake_subtree_to_layer: dest texture missing");
            return;
        }

        // Session isolation should not filter the bake — it represents
        // "what would these layers look like, composited as-is". Save and
        // restore around the compose walk.
        let saved_isolation = self.isolated_node.take();

        // Sentinel parent id — slotmap's null key never collides with a
        // minted LayerId, so we can stash a transient GroupState here.
        let bake_parent = LayerId::from_ffi(0);
        if !self.group_state.contains_key(&bake_parent) {
            let gs = Self::create_group_state(
                device,
                queue,
                self.padded_width,
                self.padded_height,
                self.canvas_origin,
                bake_parent,
            );
            self.group_state.insert(bake_parent, gs);
        }

        let scissor = (0u32, 0u32, self.canvas_width, self.canvas_height);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("bake-subtree"),
        });

        // Clear the bake accum so the composite starts from transparent.
        {
            let gs = self.group_state.get_mut(&bake_parent).unwrap();
            gs.current_accum = 0;
            gs.cache_valid_through = None;
            let _rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear-bake-accum"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &gs.accum.views[0],
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        }

        // Refresh per-host projection uniforms so masked leaves in the baked
        // subtree composite through the same projection path the live render
        // uses (isolation already cleared above → masks bake non-isolated).
        self.sync_projection_states(device, queue, doc);

        // Composite the sources into the bake accum. `compose_children`
        // handles rasters, groups (recursing through `compose_group` which
        // updates each group's own composite_cache), and passthrough groups.
        self.compose_children(&mut encoder, device, doc, bake_parent, source_ids, scissor);

        // Copy the final accum into the destination layer's texture.
        let gs = self
            .group_state
            .get(&bake_parent)
            .expect("bake group state allocated above");
        let src_accum = gs.current_accum;
        let dest_tex = self
            .node_textures
            .get(&dest_layer_id)
            .expect("dest texture presence checked above");
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &gs.accum.textures[src_accum],
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: dest_tex.texture(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: self.canvas_width,
                height: self.canvas_height,
                depth_or_array_layers: 1,
            },
        );

        queue.submit(std::iter::once(encoder.finish()));

        self.isolated_node = saved_isolation;
        self.mark_node_pixels_dirty(dest_layer_id);
        self.mark_dirty();
    }

    /// Composite layer tree to offscreen target. GPU textures are authoritative —
    /// no CPU tile upload needed. Returns true if GPU work was submitted.
    pub fn render_offscreen(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        doc: &mut Document,
    ) -> bool {
        if !self.needs_composite {
            return false;
        }

        let scissor = (0, 0, self.canvas_width, self.canvas_height);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("composite"),
        });

        // Regenerate any dirty void textures before the tree walk so the
        // downstream blend pass samples up-to-date pixels.
        self.encode_dirty_layer_content(&mut encoder);

        // Allocate + refresh per-host projection uniforms (needs `queue`)
        // before the compose walk, which only binds.
        self.sync_projection_states(device, queue, doc);

        let root_id = self.root_id;
        self.compose_group(&mut encoder, device, doc, root_id, scissor);

        queue.submit(std::iter::once(encoder.finish()));

        self.needs_composite = false;
        true
    }

    /// Run the present pass (`present.wgsl` via the current `view_uniform_buf`)
    /// into a `target_w × target_h` offscreen RGBA8 texture and return its
    /// bytes. Assumes the composite cache and view uniform are already current.
    #[cfg(any(test, feature = "testing"))]
    fn present_into_target(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_w: u32,
        target_h: u32,
    ) -> Vec<u8> {
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("test-present-target"),
            size: wgpu::Extent3d {
                width: target_w,
                height: target_h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("test-present"),
        });
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("test-present-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            rpass.set_pipeline(&self.present_to_veil_pipeline);
            rpass.set_bind_group(0, &self.present_cache_bind_group, &[]);
            rpass.draw(0..3, 0..1);
        }
        queue.submit(std::iter::once(encoder.finish()));

        crate::gpu::test_utils::readback_texture(
            device,
            queue,
            &target,
            wgpu::TextureFormat::Rgba8Unorm,
            target_w,
            target_h,
        )
    }

    /// Run the present pass into a canvas-sized offscreen RGBA8 texture and
    /// return its bytes. For tests: the production present pass writes to the
    /// surface (un-readable), but the present shader is exactly where bugs
    /// like premultiplied-alpha mishandling live, so test coverage of that
    /// stage requires a parallel sink.
    ///
    /// Forces an identity 1:1 view transform so screen pixels map to canvas
    /// pixels and the OOB branch is inactive across the whole target. Use
    /// [`Self::test_present_to_viewport`] instead when the screen↔canvas
    /// mapping itself is under test (resize / squash / offset bugs).
    #[cfg(any(test, feature = "testing"))]
    pub fn test_present_to_canvas(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        doc: &mut Document,
    ) -> Vec<u8> {
        self.render_offscreen(device, queue, doc);

        let cw = self.canvas_width;
        let ch = self.canvas_height;
        let identity = ViewTransform::from_pan_zoom_rotate(
            0.0, 0.0, 1.0, 0.0, false, cw as f32, ch as f32, cw as f32, ch as f32,
        );
        self.update_view_transform(queue, &identity);

        self.present_into_target(device, queue, cw, ch)
    }

    /// Run the present pass through the **production** cached `view_uniform_buf`
    /// (the matrix `rebuild_view_transform` last uploaded) into a
    /// `viewport_w × viewport_h` target — i.e. exactly what the surface would
    /// show, minus the surface. Unlike [`Self::test_present_to_canvas`] this
    /// does NOT force identity, so it exercises the real screen↔canvas mapping
    /// where view-transform / resize bugs (anisotropic squash, offset) live.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_present_to_viewport(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        doc: &mut Document,
        viewport_w: u32,
        viewport_h: u32,
    ) -> Vec<u8> {
        self.render_offscreen(device, queue, doc);
        self.present_into_target(device, queue, viewport_w, viewport_h)
    }

    /// Create a dynamic blend bind group for compositing a layer into a group.
    fn create_blend_bind_group(
        &self,
        device: &wgpu::Device,
        bg_view: &wgpu::TextureView,
        layer_view: &wgpu::TextureView,
        uniform_buf: &wgpu::Buffer,
        label: &str,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.blend_pipelines.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(bg_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(layer_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: uniform_buf.as_entire_binding(),
                },
            ],
        })
    }

    /// Cached entry point for `create_blend_bind_group`. The key
    /// `(parent_group, child, src_accum_idx)` uniquely identifies the bg+layer
    /// view pair for a given composite — view handles and uniform buffers
    /// are stable across frames, so caching by key avoids the per-frame
    /// allocator round-trip. Caller is responsible for bypassing the cache
    /// when the inputs are not stable (e.g. floating-target preview swap).
    ///
    /// Takes the cache field directly rather than `&mut self` so the caller
    /// can keep other immutable field borrows live across this call. Returns
    /// a borrow into the cache, valid until the cache is mutated again.
    fn get_or_create_blend_bind_group<'a>(
        blend_bind_groups: &'a mut HashMap<(LayerId, LayerId, u8), wgpu::BindGroup>,
        bgl: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        device: &wgpu::Device,
        key: (LayerId, LayerId, u8),
        bg_view: &wgpu::TextureView,
        layer_view: &wgpu::TextureView,
        uniform_buf: &wgpu::Buffer,
        label: &str,
    ) -> &'a wgpu::BindGroup {
        blend_bind_groups.entry(key).or_insert_with(|| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(bg_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(layer_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: uniform_buf.as_entire_binding(),
                    },
                ],
            })
        })
    }

    /// Recursively composite a group's children into its GroupState.
    ///
    /// For passthrough groups, children are inlined into the parent's accum
    /// (same as the old flat loop). For normal groups, children composite
    /// into the group's own accum pair, then the result is blended into the
    /// parent.
    fn compose_group(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        doc: &Document,
        group_id: LayerId,
        scissor: (u32, u32, u32, u32),
    ) {
        let (scissor_x, scissor_y, scissor_w, scissor_h) = scissor;

        // Reset group's accum state for a fresh composite.
        {
            let gs = self
                .group_state
                .get_mut(&group_id)
                .expect("GroupState missing");
            gs.current_accum = 0;
            gs.cache_valid_through = None;
            let _rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear-accum"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &gs.accum.views[0],
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        }

        // Inline children into this group's accumulators. Clone the child
        // ids so the borrow on `doc` doesn't outlive the call into
        // `compose_children`, which itself re-borrows `doc`. `SmallVec`
        // absorbs the typical single-digit-children case on the stack.
        let children: ChildIds = ChildIds::from_slice(doc.children_of(group_id));
        self.compose_children(encoder, device, doc, group_id, &children, scissor);

        // Copy final accum to this group's composite cache.
        let gs = self.group_state.get(&group_id).expect("GroupState missing");
        let src_accum = gs.current_accum;
        let origin = wgpu::Origin3d {
            x: scissor_x,
            y: scissor_y,
            z: 0,
        };
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &gs.accum.textures[src_accum],
                mip_level: 0,
                origin,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &gs.composite_cache,
                mip_level: 0,
                origin,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: scissor_w,
                height: scissor_h,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Composite a list of children into the parent group's accumulators.
    /// Handles passthrough groups by recursing with the same parent group_id.
    ///
    /// Per-child dispatch goes through [`LayerNode::compose_into`] so each
    /// node variant is responsible for its own compose behaviour — this
    /// walk only owns the per-child visibility + isolation filters that are
    /// orthogonal to node kind.
    fn compose_children(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        doc: &Document,
        parent_group: LayerId,
        children: &[LayerId],
        scissor: (u32, u32, u32, u32),
    ) {
        for &child_id in children {
            let node = match doc.find_node(child_id) {
                Some(n) => n,
                None => continue,
            };
            if !node.visible() {
                continue;
            }
            // Isolation filter: skip children whose subtree doesn't touch
            // the isolation target. `node.visible()` and isolation are
            // orthogonal — the document's eye state is never inspected
            // beyond this `visible()` check, and isolation never mutates it.
            if !self.is_in_isolation_path(doc, child_id) {
                continue;
            }
            let mut ctx = CompositionContext {
                compositor: self,
                encoder,
                device,
                doc,
                parent_group,
                scissor,
            };
            node.compose_into(&mut ctx);
        }
    }

    /// The mask id a leaf host should route through the de-fused projection
    /// path, or `None` to keep the fast (fused) path. A host qualifies whenever
    /// it carries a *visible* mask filter — including while a transform
    /// preview is active on the host or its mask, in which case the projection
    /// swaps in the preview content/mask (see
    /// [`Self::compose_layer_through_projection`]).
    fn host_active_mask_for_projection(&self, doc: &Document, host_id: LayerId) -> Option<LayerId> {
        doc.mask_filter(host_id)
            .filter(|m| m.common.visible)
            .map(|m| m.id)
    }

    /// During an active transform, the roles a masked host's preview can play:
    /// `(layer_transforming, mask_transforming)`. At most one is true — the
    /// layer or the mask is being dragged, not both. Drives whether the
    /// projection samples the preview layer / preview mask vs. the live ones.
    fn projection_transform_roles(&self, host_id: LayerId, mask_id: LayerId) -> (bool, bool) {
        match self.transform_pass.active.as_ref() {
            Some(s) => (s.target_layer == host_id, s.target_layer == mask_id),
            None => (false, false),
        }
    }

    /// Allocate a [`ProjectionState`] for `host_id` at the given dimensions,
    /// reusing an existing one if it already matches (pooling). Released by
    /// [`Self::dispose_projection_state`] on mask remove/hide / host delete and
    /// cleared wholesale on canvas resize.
    fn ensure_projection_state(
        &mut self,
        device: &wgpu::Device,
        host_id: LayerId,
        padded_w: u32,
        padded_h: u32,
    ) {
        let fits = self
            .projection_states
            .get(&host_id)
            .is_some_and(|ps| ps.padded_w == padded_w && ps.padded_h == padded_h);
        if fits {
            return;
        }
        let ps = Self::create_projection_state(device, padded_w, padded_h, host_id);
        self.projection_states.insert(host_id, ps);
    }

    fn create_projection_state(
        device: &wgpu::Device,
        padded_w: u32,
        padded_h: u32,
        host_id: LayerId,
    ) -> ProjectionState {
        let (a0, v0) =
            Self::make_accum_texture(device, padded_w, padded_h, &format!("proj-{host_id:?}-0"));
        let (a1, v1) =
            Self::make_accum_texture(device, padded_w, padded_h, &format!("proj-{host_id:?}-1"));
        let mk = |label: &str, size: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let blend_sz = std::mem::size_of::<BlendUniforms>() as u64;
        let mask_sz = std::mem::size_of::<crate::gpu::apply_mask::MaskUniform>() as u64;
        ProjectionState {
            accum: AccumPair {
                textures: [a0, a1],
                views: [v0, v1],
            },
            content_uniform_buf: mk("proj-content-uniform", blend_sz),
            down_uniform_buf: mk("proj-down-uniform", blend_sz),
            mask_uniform_buf: mk("proj-mask-uniform", mask_sz),
            padded_w,
            padded_h,
        }
    }

    /// Release a host's pooled projection state. Called on mask remove/hide and
    /// host delete; idempotent.
    pub fn dispose_projection_state(&mut self, host_id: LayerId) {
        self.projection_states.remove(&host_id);
    }

    /// Pre-walk pass (has `queue`): ensure a projection state exists and its
    /// three uniform buffers are current for every leaf host that needs one,
    /// and drop states whose host no longer qualifies (mask hidden/removed).
    /// The compose walk that follows only binds — it never allocates or writes.
    fn sync_projection_states(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        doc: &Document,
    ) {
        let padded_w = self.padded_width;
        let padded_h = self.padded_height;

        // Drop projections whose host no longer needs one (mask hidden/removed
        // or now transform-previewing) — the "released on mask remove/hide"
        // requirement, plus the host-delete case (the layer_cache entry is
        // gone so it never re-qualifies below).
        let stale: Vec<LayerId> = self
            .projection_states
            .keys()
            .copied()
            .filter(|h| self.host_active_mask_for_projection(doc, *h).is_none())
            .collect();
        for h in stale {
            self.projection_states.remove(&h);
        }

        let normal = crate::gpu::blend_mode::registry().default().gpu_value;
        let canvas_size = [padded_w as f32, padded_h as f32];
        let host_ids: Vec<LayerId> = self.layer_cache.keys().copied().collect();
        for host_id in host_ids {
            let mask_id = match self.host_active_mask_for_projection(doc, host_id) {
                Some(m) => m,
                None => continue,
            };
            let layer_ext = match self.node_textures.get(&host_id) {
                Some(t) => t.canvas_extent(),
                None => continue,
            };
            let mask_ext = match self.node_textures.get(&mask_id) {
                Some(t) => t.canvas_extent(),
                None => continue,
            };
            let (opacity, blend_mode) = {
                let c = &self.layer_cache[&host_id];
                (c.opacity, c.blend_mode)
            };
            let isolated = self.isolated_node == Some(mask_id);
            let (layer_transforming, mask_transforming) =
                self.projection_transform_roles(host_id, mask_id);
            let canvas_origin = [self.canvas_origin.x as f32, self.canvas_origin.y as f32];

            self.ensure_projection_state(device, host_id, padded_w, padded_h);

            // Compose-content uniform: straight host content (opacity 1, Normal).
            // When the *layer* is transform-previewing, the content comes from
            // the canvas-aligned preview texture, so it samples at the canvas
            // window; otherwise it samples the live layer in its own frame.
            let (content_offset, content_size) = if layer_transforming {
                (canvas_origin, canvas_size)
            } else {
                (
                    [layer_ext.x0() as f32, layer_ext.y0() as f32],
                    [layer_ext.width as f32, layer_ext.height as f32],
                )
            };
            let content = BlendUniforms {
                opacity: 1.0,
                blend_mode: normal,
                isolated: 0,
                _pad1: 0.0,
                layer_offset: content_offset,
                layer_size: content_size,
            };
            // Down-composite uniform: the host's opacity + blend mode, canvas-
            // window geometry (the projection fills exactly the canvas window).
            let down = BlendUniforms {
                opacity,
                blend_mode,
                isolated: 0,
                _pad1: 0.0,
                layer_offset: canvas_origin,
                layer_size: canvas_size,
            };
            // Mask uniform: when the *mask* is transform-previewing, the preview
            // mask is a canvas-aligned R8 texture, so it samples at the canvas
            // window; otherwise the live mask samples in its own frame.
            let (mask_offset, mask_size) = if mask_transforming {
                (canvas_origin, canvas_size)
            } else {
                (
                    [mask_ext.x0() as f32, mask_ext.y0() as f32],
                    [mask_ext.width as f32, mask_ext.height as f32],
                )
            };
            let mu = crate::gpu::apply_mask::MaskUniform {
                mask_offset,
                mask_size,
                isolated: isolated as u32,
                _pad0: 0,
                _pad1: 0,
                _pad2: 0,
            };
            let ps = &self.projection_states[&host_id];
            queue.write_buffer(&ps.content_uniform_buf, 0, bytemuck::bytes_of(&content));
            queue.write_buffer(&ps.down_uniform_buf, 0, bytemuck::bytes_of(&down));
            queue.write_buffer(&ps.mask_uniform_buf, 0, bytemuck::bytes_of(&mu));
        }

        // (Re-)ensure a mask snapshot exists for every in-place host (passthrough
        // group or filter layer) with a visible mask. The snapshot is
        // parent-accumulator-sized, so `set_canvas_rect` drops it on a
        // crop/resize; without this re-ensure the mask would silently disable
        // after a crop (compose falls back to the unmasked in-place path).
        for host_id in doc.masked_in_place_hosts() {
            self.ensure_mask_snapshot_state(device, host_id);
        }

        // Refresh the in-place-host mask lerp uniforms (canvas + mask
        // geometry + isolated) so a mask that grew independently of the
        // canvas window samples in its own space — same `sample_mask_window`
        // path as the leaf projection.
        let canvas_origin = [self.canvas_origin.x as f32, self.canvas_origin.y as f32];
        let host_ids: Vec<LayerId> = self.mask_snapshot_state.keys().copied().collect();
        for host_id in host_ids {
            let mask_id = match doc.mask_filter(host_id).filter(|m| m.common.visible) {
                Some(m) => m.id,
                None => continue,
            };
            // A mask being transform-previewed is canvas-aligned; otherwise it
            // samples in its live extent.
            let mask_transforming = self
                .transform_pass
                .active
                .as_ref()
                .is_some_and(|s| s.target_layer == mask_id);
            let (mask_offset, mask_size) = if mask_transforming {
                (canvas_origin, canvas_size)
            } else {
                match self.node_textures.get(&mask_id).map(|t| t.canvas_extent()) {
                    Some(e) => (
                        [e.x0() as f32, e.y0() as f32],
                        [e.width as f32, e.height as f32],
                    ),
                    None => continue,
                }
            };
            let lu = LerpUniforms {
                canvas_origin,
                canvas_size,
                mask_offset,
                mask_size,
                isolated: (self.isolated_node == Some(mask_id)) as u32,
                _pad0: 0,
                _pad1: 0,
                _pad2: 0,
            };
            let pms = &self.mask_snapshot_state[&host_id];
            queue.write_buffer(&pms.uniform_buf, 0, bytemuck::bytes_of(&lu));
        }

        // Build/refresh parametric-filter GPU resources (the curves LUT). This
        // is the one place with both `device` and `queue`; the compose walk that
        // follows only *binds* the cache. Walk every filter layer — the loops
        // above (`layer_cache.keys()`, `masked_in_place_hosts()`) miss an
        // unmasked filter layer. Rebuild only when the param fingerprint changes,
        // and prune caches for filter layers that no longer exist.
        let filters: Vec<(LayerId, String, Vec<ParamValue>)> = doc
            .all_filter_layers()
            .iter()
            .map(|f| (f.id, f.pipeline.clone(), f.params.clone()))
            .collect();
        let live: HashSet<LayerId> = filters.iter().map(|(id, ..)| *id).collect();
        self.filter_caches.retain(|id, _| live.contains(id));
        for (id, pipeline_id, params) in filters {
            let unchanged = self
                .filter_caches
                .get(&id)
                .is_some_and(|(stored, _)| *stored == params);
            if unchanged {
                continue;
            }
            let Some(effect) = self.filter_pipeline_registry.pipeline(&pipeline_id, device) else {
                continue;
            };
            let entry = self
                .filter_caches
                .entry(id)
                .or_insert_with(|| (Vec::new(), crate::gpu::effect::EffectCache::empty()));
            effect.ensure(device, queue, &params, &mut entry.1);
            entry.0 = params;
        }
    }

    /// De-fused leaf-mask compose: host content → projection, mask modulates
    /// the projection (`apply_mask`), projection blends down onto the parent.
    /// The mask samples only `(projection, mask)` in its own space — never the
    /// host layer texture or geometry. Uniforms are pre-written by
    /// [`Self::sync_projection_states`]; this only encodes the three passes.
    fn compose_layer_through_projection(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        parent_group: LayerId,
        host_id: LayerId,
        mask_id: LayerId,
        scissor: (u32, u32, u32, u32),
    ) {
        let (sx, sy, sw, sh) = scissor;

        // Advance the parent ping-pong up front so the rest can borrow `&self`.
        let (parent_src, parent_dst) = {
            let gsp = self.group_state.get_mut(&parent_group).unwrap();
            let src = gsp.current_accum;
            let dst = 1 - src;
            gsp.current_accum = dst;
            (src, dst)
        };

        let ps = match self.projection_states.get(&host_id) {
            Some(p) => p,
            None => return,
        };

        // Transform-preview swap: sample the (canvas-aligned) preview layer when
        // the layer is being dragged, and the preview mask when the mask is.
        // Geometry for these is set to match in `sync_projection_states`.
        let (layer_transforming, mask_transforming) =
            self.projection_transform_roles(host_id, mask_id);
        let preview = self.transform_pass.active.as_ref();

        let layer_view = if layer_transforming {
            match preview {
                Some(s) => &s.preview_view,
                None => return,
            }
        } else {
            match self.node_textures.get(&host_id) {
                Some(t) => t.view(),
                None => return,
            }
        };
        let live_mask_bg = self
            .mask_bind_groups
            .get(&mask_id)
            .unwrap_or(&self.default_mask_bind_group);
        let mask_bg = if mask_transforming {
            preview
                .and_then(|s| s.preview_mask_bind_group.as_ref())
                .unwrap_or(live_mask_bg)
        } else {
            live_mask_bg
        };

        // Build the (non-hot) bind groups fresh — masked leaves are rare.
        // Pass 1 reads the cleared accum[1] as a transparent background.
        let content_bg = self.create_blend_bind_group(
            device,
            &ps.accum.views[1],
            layer_view,
            &ps.content_uniform_buf,
            "proj-content",
        );
        let apply_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("proj-apply-mask-bg"),
            layout: &self.apply_mask_pipeline.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&ps.accum.views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: ps.mask_uniform_buf.as_entire_binding(),
                },
            ],
        });
        let down_bg = self.create_blend_bind_group(
            device,
            &self.group_state[&parent_group].accum.views[parent_src],
            &ps.accum.views[1],
            &ps.down_uniform_buf,
            "proj-down",
        );

        // Pass 0: clear accum[1] (the transparent background for pass 1).
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("proj-clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &ps.accum.views[1],
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });

        // Pass 1: composite straight host content into accum[0].
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("proj-content"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &ps.accum.views[0],
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            rpass.set_scissor_rect(sx, sy, sw, sh);
            rpass.set_pipeline(self.blend_pipelines.pipeline());
            rpass.set_bind_group(0, &content_bg, &[]);
            rpass.set_bind_group(1, &self.default_mask_bind_group, &[]);
            rpass.set_bind_group(2, &self.canvas_bind_group, &[]);
            rpass.draw(0..3, 0..1);
        }

        // Pass 2: modulate the projection's alpha by the mask → accum[1].
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("proj-apply-mask"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &ps.accum.views[1],
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            rpass.set_scissor_rect(sx, sy, sw, sh);
            rpass.set_pipeline(self.apply_mask_pipeline.pipeline());
            rpass.set_bind_group(0, &apply_bg, &[]);
            rpass.set_bind_group(1, mask_bg, &[]);
            rpass.set_bind_group(2, &self.canvas_bind_group, &[]);
            rpass.draw(0..3, 0..1);
        }

        // Pass 3: blend the masked projection down onto the parent accum.
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("proj-down"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.group_state[&parent_group].accum.views[parent_dst],
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            rpass.set_scissor_rect(sx, sy, sw, sh);
            rpass.set_pipeline(self.blend_pipelines.pipeline());
            rpass.set_bind_group(0, &down_bg, &[]);
            rpass.set_bind_group(1, &self.default_mask_bind_group, &[]);
            rpass.set_bind_group(2, &self.canvas_bind_group, &[]);
            rpass.draw(0..3, 0..1);
        }
    }

    /// Composite a content layer (raster or void) into its parent group's
    /// ping-pong accumulators. One blend arm for both raster and procedural
    /// content — the procedural texture lives in `node_textures` keyed by
    /// layer id (allocated by `ensure_void_layer` and refreshed by
    /// `encode_dirty_layer_content` before the tree walk), and the blend
    /// uniforms are the same `BlendUniforms` shape in the unified
    /// `layer_cache` — so neither lookup branches on kind here.
    fn compose_layer_arm(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        doc: &Document,
        parent_group: LayerId,
        layer: &Layer,
        scissor: (u32, u32, u32, u32),
    ) {
        let (scissor_x, scissor_y, scissor_w, scissor_h) = scissor;
        let layer_id = layer.id();

        // De-fused leaf mask: a host carrying a visible mask composites through
        // its own projection (content → apply_mask → blend down) so the mask
        // never samples the host's texture or geometry. The fused path below
        // runs only for unmasked hosts (default white mask) and the transform-
        // preview detour (still fused until B3).
        if let Some(mask_id) = self.host_active_mask_for_projection(doc, layer_id) {
            self.compose_layer_through_projection(
                encoder,
                device,
                parent_group,
                layer_id,
                mask_id,
                scissor,
            );
            return;
        }

        // Effective view + uniforms: when this layer is the floating
        // target, swap the live texture view for the (canvas-aligned)
        // preview view AND swap the live's layer-aligned blend uniforms
        // for the preview's canvas-aligned ones — both halves must move
        // together or the shader maps fragments to the wrong region.
        // Voids never become floating targets today, so the detour
        // collapses to the live path for them; if voids ever do, the same
        // code path will Just Work.
        let active_floating = self
            .transform_pass
            .active
            .as_ref()
            .filter(|s| s.target_layer == layer_id);
        let layer_view = match active_floating {
            Some(s) => &s.preview_view,
            None => match self.node_textures.get(&layer_id) {
                Some(t) => t.view(),
                None => return,
            },
        };
        let uniform_buf_ptr = match active_floating {
            Some(s) => &s.preview_blend_uniform_buf,
            None => match self.layer_cache.get(&layer_id) {
                Some(c) => &c.uniform_buf,
                None => return,
            },
        };

        // Ping-pong: read from current accum, write to the other.
        let gs = self.group_state.get_mut(&parent_group).unwrap();
        let src = gs.current_accum;
        let dst = 1 - src;
        gs.current_accum = dst;

        // Floating target swaps the bg/layer view per-frame to the
        // (canvas-aligned) preview texture; skip the cache so preview-state
        // ephemera never leak in. The non-floating path uses stable
        // view+uniform handles so the cache key `(parent, child, src)` is
        // sufficient.
        let fresh_bind_group: Option<wgpu::BindGroup>;
        let cached_bind_group: Option<&wgpu::BindGroup>;
        if active_floating.is_some() {
            fresh_bind_group = Some(self.create_blend_bind_group(
                device,
                &self.group_state[&parent_group].accum.views[src],
                layer_view,
                uniform_buf_ptr,
                "blend-layer",
            ));
            cached_bind_group = None;
        } else {
            let bg_view = &self.group_state[&parent_group].accum.views[src];
            let bgl = &self.blend_pipelines.bind_group_layout;
            let sampler = &self.sampler;
            cached_bind_group = Some(Self::get_or_create_blend_bind_group(
                &mut self.blend_bind_groups,
                bgl,
                sampler,
                device,
                (parent_group, layer_id, src as u8),
                bg_view,
                layer_view,
                uniform_buf_ptr,
                "blend-layer",
            ));
            fresh_bind_group = None;
        }
        let bind_group: &wgpu::BindGroup = cached_bind_group
            .unwrap_or_else(|| fresh_bind_group.as_ref().expect("one branch sets it"));

        let gs = &self.group_state[&parent_group];
        let mask_bg = Self::effective_mask_bind_group_fields(
            &self.mask_bind_groups,
            &self.default_mask_bind_group,
            &self.transform_pass,
            doc,
            layer_id,
        );
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("blend-layer"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &gs.accum.views[dst],
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        rpass.set_scissor_rect(scissor_x, scissor_y, scissor_w, scissor_h);
        rpass.set_pipeline(self.blend_pipelines.pipeline());
        rpass.set_bind_group(0, bind_group, &[]);
        rpass.set_bind_group(1, mask_bg, &[]);
        rpass.set_bind_group(2, &self.canvas_bind_group, &[]);
        rpass.draw(0..3, 0..1);
    }

    /// Compose a filter layer: transform the running group accumulator in
    /// place rather than blending a layer in. Because the child walk composites
    /// bottom-to-top, `gs.current_accum` already holds the composite of
    /// everything below this filter (lower siblings + everything beneath the
    /// group, since a passthrough group inlines into its non-passthrough
    /// ancestor's accumulator). Using the ping-pong: read the current accum,
    /// write the filtered result to the other half, advance `current_accum`.
    /// No third texture — the filter pipeline samples 1:1 and overwrites every
    /// texel of the destination.
    ///
    /// A visible mask confines where the filter applies (a masked adjustment
    /// layer): snapshot the accumulator (before), run the filter (after), then
    /// lerp `mix(before, after, mask)` so the filtered result shows inside the
    /// mask and the original passes through outside. Same snapshot+lerp
    /// scaffolding a masked passthrough group uses
    /// ([`Self::snapshot_parent_accum`] / [`Self::lerp_parent_accum_with_mask`]).
    fn compose_filter_arm(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        doc: &Document,
        parent_group: LayerId,
        filter: &FilterLayer,
        scissor: (u32, u32, u32, u32),
    ) {
        // Resolve the shared, lazily-cached pipeline. An unknown pipeline id
        // (e.g. a save referencing a filter this binary doesn't ship) composes
        // as a no-op rather than erroring mid-frame.
        let pipeline = match self
            .filter_pipeline_registry
            .pipeline(&filter.pipeline, device)
        {
            Some(p) => p,
            None => return,
        };

        // A visible mask confines where the filter applies. The snapshot state
        // is ensure-driven per frame (see `masked_in_place_hosts`); if it's not
        // yet present, fall back to the unmasked path this frame.
        let masked = doc
            .mask_filter(filter.id)
            .map(|m| m.common.visible)
            .unwrap_or(false)
            && self.mask_snapshot_state.contains_key(&filter.id);

        if masked {
            self.snapshot_parent_accum(encoder, parent_group, filter.id, scissor);
        }

        // Ping-pong advance: src = last-written accum, dst = the other half.
        let (src, dst) = {
            let gs = match self.group_state.get_mut(&parent_group) {
                Some(g) => g,
                None => return,
            };
            let src = gs.current_accum;
            let dst = 1 - src;
            gs.current_accum = dst;
            (src, dst)
        };

        {
            let gs = &self.group_state[&parent_group];
            // The LUT (for parametric filters) is built in `sync_projection_states`
            // before this walk, keyed by `filter.id`. Parameter-free filters have
            // an empty cache; an absent entry (should not happen post-sync) falls
            // back to a transient empty one.
            let empty;
            let cache = match self.filter_caches.get(&filter.id) {
                Some((_, c)) => c,
                None => {
                    empty = crate::gpu::effect::EffectCache::empty();
                    &empty
                }
            };
            pipeline.render(
                device,
                encoder,
                &gs.accum.views[src],
                None,
                &gs.accum.views[dst],
                wgpu::TextureFormat::Rgba8Unorm,
                cache,
            );
        }

        if masked {
            self.lerp_parent_accum_with_mask(
                encoder,
                device,
                doc,
                parent_group,
                filter.id,
                scissor,
            );
        }
    }

    /// Snapshot the current parent accumulator into the host's mask-snapshot
    /// texture (the "before" image of the lerp). Step 1 of every in-place
    /// masked composite — shared by the masked passthrough group and the masked
    /// filter layer. The snapshot state must already exist (ensure-driven per
    /// frame); the caller checks before invoking.
    fn snapshot_parent_accum(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        parent_group: LayerId,
        host_id: LayerId,
        scissor: (u32, u32, u32, u32),
    ) {
        let (scissor_x, scissor_y, scissor_w, scissor_h) = scissor;
        let gs = self
            .group_state
            .get(&parent_group)
            .expect("parent GroupState missing");
        let before_idx = gs.current_accum;
        let origin = wgpu::Origin3d {
            x: scissor_x,
            y: scissor_y,
            z: 0,
        };
        let copy_size = wgpu::Extent3d {
            width: scissor_w,
            height: scissor_h,
            depth_or_array_layers: 1,
        };
        let pms = &self.mask_snapshot_state[&host_id];
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &gs.accum.textures[before_idx],
                mip_level: 0,
                origin,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &pms.snapshot,
                mip_level: 0,
                origin,
                aspect: wgpu::TextureAspect::All,
            },
            copy_size,
        );
    }

    /// Lerp the host's snapshot against the (now-transformed) parent
    /// accumulator using the host's mask, writing `mix(before, after, mask)`
    /// into the next ping-pong half. Step 3 of every in-place masked composite —
    /// shared by the masked passthrough group and the masked filter layer. The
    /// mask samples in its own plane space via `sample_mask_window`; the
    /// effective mask honours an in-flight transform preview.
    fn lerp_parent_accum_with_mask(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        doc: &Document,
        parent_group: LayerId,
        host_id: LayerId,
        scissor: (u32, u32, u32, u32),
    ) {
        let (scissor_x, scissor_y, scissor_w, scissor_h) = scissor;

        let gs = self.group_state.get_mut(&parent_group).unwrap();
        let after_idx = gs.current_accum;
        let dst = 1 - after_idx;
        gs.current_accum = dst;

        let pms = &self.mask_snapshot_state[&host_id];
        // Lerp bind group (group 0): before=snapshot, after=current_accum,
        // sampler, uniforms.
        let lerp_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mask-lerp-bg"),
            layout: &self.blend_pipelines.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&pms.snapshot_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(
                        &self.group_state[&parent_group].accum.views[after_idx],
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: pms.uniform_buf.as_entire_binding(),
                },
            ],
        });

        let gs = &self.group_state[&parent_group];
        // Effective mask: live by default, preview-mask when the floating
        // target is this host's mask filter.
        let mask_bg = self.effective_mask_bind_group(doc, host_id);
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("mask-lerp"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &gs.accum.views[dst],
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        rpass.set_scissor_rect(scissor_x, scissor_y, scissor_w, scissor_h);
        rpass.set_pipeline(&self.mask_lerp_pipeline);
        rpass.set_bind_group(0, &lerp_bind_group, &[]);
        rpass.set_bind_group(1, mask_bg, &[]);
        rpass.draw(0..3, 0..1);
    }

    /// Composite a child group into its parent's ping-pong accumulators.
    /// Passthrough groups inline their children into the parent (with the
    /// Photoshop-style snapshot+lerp detour when a visible mask is
    /// attached); normal groups composite into their own isolated buffer
    /// first and then blend the result into the parent.
    fn compose_group_arm(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        doc: &Document,
        parent_group: LayerId,
        group: &crate::layer::LayerGroup,
        scissor: (u32, u32, u32, u32),
    ) {
        let (scissor_x, scissor_y, scissor_w, scissor_h) = scissor;
        let group_id = group.id;

        if group.passthrough {
            // Structural detection: a passthrough group with a visible mask
            // filter triggers Photoshop-style snapshot+lerp; otherwise
            // it's pure passthrough.
            let has_active_mask = doc
                .mask_filter(group_id)
                .map(|m| m.common.visible)
                .unwrap_or(false);

            if has_active_mask {
                self.compose_passthrough_masked(
                    encoder,
                    device,
                    doc,
                    parent_group,
                    group_id,
                    scissor,
                );
            } else {
                // Pure passthrough — inline children into parent.
                let inner: ChildIds = ChildIds::from_slice(doc.children_of(group_id));
                self.compose_children(encoder, device, doc, parent_group, &inner, scissor);
            }
            return;
        }

        // Normal group: composite into its own isolated buffer, then blend
        // the result into the parent.
        if !self.group_state.contains_key(&group_id) {
            return;
        }
        self.compose_group(encoder, device, doc, group_id, scissor);

        // Blend group's composite cache into parent's accumulators.
        let gs_parent = self.group_state.get_mut(&parent_group).unwrap();
        let src = gs_parent.current_accum;
        let dst = 1 - src;
        gs_parent.current_accum = dst;

        // Split-borrow into the cache: bg/layer views and uniform buffer
        // live in distinct fields from `blend_bind_groups`, so we can hold
        // the mutable borrow of the cache and the immutable borrows of the
        // views together. Groups never become floating targets themselves,
        // so the cache always applies here (a filter-as-floating-target
        // only swaps mask_bg via `effective_mask_bind_group`).
        let bg_view = &self.group_state[&parent_group].accum.views[src];
        let gs_child = &self.group_state[&group_id];
        let child_view = &gs_child.composite_cache_view;
        let child_uniform = &gs_child.uniform_buf;
        let bgl = &self.blend_pipelines.bind_group_layout;
        let sampler = &self.sampler;
        let bind_group = Self::get_or_create_blend_bind_group(
            &mut self.blend_bind_groups,
            bgl,
            sampler,
            device,
            (parent_group, group_id, src as u8),
            bg_view,
            child_view,
            child_uniform,
            "blend-group",
        );

        let gs_parent = &self.group_state[&parent_group];
        let child_mask_bg = Self::effective_mask_bind_group_fields(
            &self.mask_bind_groups,
            &self.default_mask_bind_group,
            &self.transform_pass,
            doc,
            group_id,
        );
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("blend-group"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &gs_parent.accum.views[dst],
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        rpass.set_scissor_rect(scissor_x, scissor_y, scissor_w, scissor_h);
        rpass.set_pipeline(self.blend_pipelines.pipeline());
        rpass.set_bind_group(0, bind_group, &[]);
        rpass.set_bind_group(1, child_mask_bg, &[]);
        rpass.set_bind_group(2, &self.canvas_bind_group, &[]);
        rpass.draw(0..3, 0..1);
    }

    /// Composite a passthrough group whose mask is active.
    ///
    /// Snapshots the parent accumulator, composites children (passthrough),
    /// then lerps between the snapshot and the result using the group mask —
    /// the same snapshot+lerp scaffolding [`Self::compose_filter_arm`] uses for
    /// a masked filter layer, with the children-inline transform sandwiched
    /// between the two shared halves.
    fn compose_passthrough_masked(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        doc: &Document,
        parent_group: LayerId,
        group_id: LayerId,
        scissor: (u32, u32, u32, u32),
    ) {
        // MaskSnapshotState must exist (ensure-driven per frame). If it isn't
        // ready, inline children without the mask this frame.
        if !self.mask_snapshot_state.contains_key(&group_id) {
            let inner: ChildIds = ChildIds::from_slice(doc.children_of(group_id));
            self.compose_children(encoder, device, doc, parent_group, &inner, scissor);
            return;
        }

        self.snapshot_parent_accum(encoder, parent_group, group_id, scissor);

        let inner: ChildIds = ChildIds::from_slice(doc.children_of(group_id));
        self.compose_children(encoder, device, doc, parent_group, &inner, scissor);

        self.lerp_parent_accum_with_mask(encoder, device, doc, parent_group, group_id, scissor);
    }

    /// Whether any rendering work is pending (composite, present, veils).
    fn has_pending_work(&self, _doc: &Document) -> bool {
        self.needs_composite || self.needs_present || self.veil_chain.needs_present()
    }

    /// Clear present-related dirty flags after a frame.
    fn finish_present(&mut self) {
        self.needs_present = false;
        self.veil_chain.clear_needs_present();
    }

    /// Upload dirty tiles, composite changed layers, present to a surface.
    /// Used by the WASM frontend.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface: &wgpu::Surface,
        surface_config: &wgpu::SurfaceConfiguration,
        doc: &mut Document,
    ) {
        perf::time("render-total");

        // Re-read `rendering.veil_scale` and rebuild per-veil resources if it
        // changed. This sets needs_present when applicable, so config changes
        // wake up an otherwise-idle app.
        self.veil_chain.sync_resolution_scale(device, queue);

        if !self.has_pending_work(doc) {
            perf::time_end("render-total");
            return;
        }

        perf::time("offscreen");
        self.render_offscreen(device, queue, doc);
        perf::time_end("offscreen");

        // Acquire surface and present composite_cache → veils → surface.
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
            let vw = self.veil_chain.viewport_size().0;
            let vh = self.veil_chain.viewport_size().1;
            self.tool_overlay
                .prepare(device, queue, &plane_fwd, &plane_inv, vw, vh);
        }

        // Present + veils. Solid overlay primitives are drawn at the end
        // of the final pass (no separate LoadOp::Load pass needed).
        self.present_and_veils(&mut encoder, &surface_view);

        // Snapshot-sampling overlay primitives (invert + soft-contrast) need a
        // separate pass with a surface→snapshot copy. Hit by rect-select and
        // the brush-stamp preview.
        if self.tool_overlay.has_snapshot() {
            let vw = self.veil_chain.viewport_size().0;
            let vh = self.veil_chain.viewport_size().1;
            self.tool_overlay
                .encode_snapshot(&mut encoder, &output.texture, &surface_view, vw, vh);
        }

        queue.submit(std::iter::once(encoder.finish()));
        output.present();
        perf::time_end("present");

        self.finish_present();
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
