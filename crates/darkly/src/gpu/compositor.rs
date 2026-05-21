use crate::coord::CanvasRect;
use crate::document::Document;
use crate::gpu::atlas::LayerTexture;
use crate::gpu::blend::BlendPipelines;
use crate::gpu::content_bounds::ContentBoundsPass;
use crate::gpu::effect::EffectCache;
use crate::gpu::overlay::{OverlayPrimitive, ToolOverlay};
use crate::gpu::params::ParamValue;
use crate::gpu::veil_chain::VeilChain;
use crate::gpu::view::{ViewTransform, DEFAULT_WORKSPACE_BG};
use crate::gpu::void::{Void, VoidRegistry};
use crate::layer::{Layer, LayerId, LayerNode};
use std::collections::{HashMap, HashSet};

/// Maximum allowed layer texture dimension in either axis. Strokes that
/// would push the layer past this are clipped to current bounds.
pub const MAX_LAYER_DIM: u32 = 16384;

/// Layer-growth quantum. Bounds are rounded outward to multiples of this so
/// repeated cross-stroke growth amortizes — a typical stroke triggers 0–3
/// reallocations regardless of dab count.
pub const LAYER_GROWTH_CHUNK: u32 = 256;

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
struct LayerCache {
    /// Uniform buffer holding opacity + blend_mode + isolated.
    uniform_buf: wgpu::Buffer,
    /// CPU-side cache of the blend properties last written to `uniform_buf`.
    /// Kept here so the floating-preview path can mirror them into its own
    /// canvas-aligned uniform buffer without re-reading the GPU buffer.
    opacity: f32,
    /// Cached gpu_value for the layer's blend mode. The compositor never
    /// branches on which mode this is — the shader does — so we mirror the
    /// raw shader integer rather than carry a registration pointer through
    /// every per-frame access.
    blend_mode: u32,
    isolated: bool,
    /// Where this layer's pixels come from. Raster pixels arrive via paint;
    /// procedural pixels are regenerated on demand by a [`Void`] trait
    /// object before each composite.
    content: LayerContent,
}

/// How a layer's pixels reach `node_textures[id]`.
///
/// - `Raster`: pixels arrive via paint / paste / fill — `node_textures[id]`
///   is authoritative and the compositor doesn't regenerate it.
/// - `Procedural`: pixels are GPU-regenerable from `(void_type, params)`.
///   The compositor calls [`Void::encode`] before the next composite when
///   `dirty` is set (first allocation, param edit, animation tick, …).
enum LayerContent {
    Raster,
    Procedural(ProceduralContent),
}

/// Per-instance procedural-content state.
///
/// `dirty` is the regenerate gate: flipped true when the void is first
/// allocated, when params change, when external input lands, or when an
/// animated tick writes a fresh time uniform. The render path clears it
/// after re-rendering once via [`Compositor::encode_dirty_layer_content`].
struct ProceduralContent {
    /// The procedural-content trait object. Owned here (one per layer)
    /// because animation mutates its `time` field.
    void: Box<dyn Void>,
    /// Per-instance GPU resources for the void's own pipeline (uniform
    /// buffer + bind groups built off the registry's shared pipeline).
    cache: EffectCache,
    /// True when the procedural texture is stale and needs to be
    /// re-rendered before the next blend.
    dirty: bool,
}

/// Uniforms for raster layer compositing. The shader samples the layer
/// texture at its own UV space, so we pass the layer's pixel offset and
/// size in canvas coordinates plus the canvas size — the fragment shader
/// translates per-pixel from canvas UV to layer UV.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct BlendUniforms {
    opacity: f32,
    blend_mode: u32,
    isolated: u32,
    _pad1: f32,
    /// Layer's (offset_x, offset_y) in canvas coordinates.
    layer_offset: [f32; 2],
    /// Layer texture dimensions in pixels.
    layer_size: [f32; 2],
    /// Canvas dimensions in pixels.
    canvas_size: [f32; 2],
    _pad2: [f32; 2],
}

/// GPU state for a passthrough group that has a mask (Photoshop-style).
/// Stores a snapshot texture for the parent accumulator and a uniform buffer
/// for the lerp pass.
struct PassthroughMaskState {
    /// Snapshot of the parent accumulator before compositing this group's children.
    snapshot: wgpu::Texture,
    snapshot_view: wgpu::TextureView,
    /// Uniform buffer for the mask lerp shader (isolated flag).
    uniform_buf: wgpu::Buffer,
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
    /// layer textures (Rgba8Unorm), mask modifier textures (R8Unorm), and
    /// any future pixel-bearing modifier kinds — `LayerTexture.format`
    /// distinguishes them. One lookup per access, no fan-out.
    node_textures: HashMap<LayerId, LayerTexture>,

    /// Default mask bind group using the 1×1 white texture (pass-through
    /// fallback for hosts without a visible mask modifier).
    default_mask_bind_group: wgpu::BindGroup,

    /// Cached "use my texture as a mask" bind group, keyed by mask modifier
    /// id. Built when a mask modifier is allocated; consumed by the blend
    /// pipeline at composite time. Visibility gating happens in the render
    /// loop (which falls back to `default_mask_bind_group` for hidden masks).
    mask_bind_groups: HashMap<LayerId, wgpu::BindGroup>,

    /// Pre-built GPU objects per content layer (raster + void). Keyed by
    /// the document's [`LayerId`] — both kinds share the same blend
    /// pipeline path, so collapsing them into one pool means the blend
    /// arm, uniforms write, and dispose all do exactly one lookup.
    layer_cache: HashMap<LayerId, LayerCache>,

    blend_pipelines: BlendPipelines,

    // --- Passthrough Group Mask (Photoshop-style snapshot-lerp) ---
    mask_lerp_pipeline: wgpu::RenderPipeline,
    /// Per-group GPU state for passthrough groups with masks.
    passthrough_mask_state: HashMap<LayerId, PassthroughMaskState>,

    present_pipeline: wgpu::RenderPipeline,
    /// Present pipeline targeting the accum format (Rgba8Unorm) for veil input.
    present_to_veil_pipeline: wgpu::RenderPipeline,
    _present_bind_group_layout: wgpu::BindGroupLayout,
    /// Present bind group that reads from root's composite_cache.
    present_cache_bind_group: wgpu::BindGroup,
    /// View transform uniform buffer for the present shader.
    view_uniform_buf: wgpu::Buffer,

    sampler: wgpu::Sampler,

    /// Dirty gate — false means nothing changed, skip compositing.
    needs_composite: bool,
    /// When only the view transform changes, skip compositing and only re-present.
    needs_present: bool,

    /// Layers whose pixel content was modified since the last drain.
    /// Drained by the engine each frame to auto-queue thumbnail readbacks
    /// — anything in here had its layer texture written, so the panel's
    /// thumbnail is stale until a fresh readback lands.
    /// Node ids whose textures were modified since the last drain. Single
    /// pool keyed by node id; raster layers and mask modifiers go in the
    /// same set, and the engine's drain pumps thumbnail readbacks for both
    /// uniformly.
    dirty_node_pixels: HashSet<LayerId>,

    canvas_width: u32,
    canvas_height: u32,
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

    // --- Floating Content Transform ---
    transform_pass: crate::gpu::transform::TransformPass,

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
    /// GPU realisation of the document's selection modifier — ping-pong R8
    /// textures + brush/paint bind groups. `None` until the engine allocates
    /// the selection modifier; once allocated, lives for the document's
    /// lifetime. Pixel metadata (active toggle, tight bounds, CPU cache)
    /// lives on `Document.selection.kind` (`SelectionModifier`).
    selection_state: Option<crate::gpu::selection::SelectionState>,

    // --- Tool Overlay ---
    tool_overlay: ToolOverlay,
    /// Cached view transform for overlay forward matrix computation.
    cached_view_transform: ViewTransform,
    /// Workspace color drawn by the present shader outside the canvas
    /// rectangle. Stamped onto every transform on upload, so changing it
    /// only requires re-uploading the cached transform.
    viewport_bg: [f32; 4],

    // --- Content Bounds (GPU compute) ---
    content_bounds: ContentBoundsPass,

    // --- Frame Scheduler ---
    /// Monotonic frame counter, incremented on each rAF tick.
    /// Systems fire when `frame_count % divisor == 0`.
    frame_count: u64,
    /// Last wall-clock time for dt computation.
    last_wall_time: f32,
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
        let uniforms = BlendUniforms {
            opacity: 1.0,
            blend_mode: normal,
            isolated: 0,
            _pad1: 0.0,
            layer_offset: [0.0, 0.0],
            layer_size: canvas,
            canvas_size: canvas,
            _pad2: [0.0, 0.0],
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
                    include_str!("../../../../shaders/mask_lerp.wgsl").into(),
                ),
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("mask-lerp-pipeline-layout"),
                bind_group_layouts: &[
                    &blend_pipelines.bind_group_layout,
                    &blend_pipelines.mask_bind_group_layout,
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
                bind_group_layouts: &[&_present_bind_group_layout],
                immediate_size: 0,
            });

        let present_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("present-shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/present.wgsl").into(),
            ),
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
        let root_state = Self::create_group_state(device, queue, padded_w, padded_h, root_id);

        // Present bind group reads from root's composite cache
        let present_cache_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("present-bg-cache"),
            layout: &_present_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&root_state.composite_cache_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: view_uniform_buf.as_entire_binding(),
                },
            ],
        });

        let mut group_state = HashMap::new();
        group_state.insert(root_id, root_state);

        let veil_chain = VeilChain::new(device, sampler.clone(), surface_format, accum_format);

        let tool_overlay = ToolOverlay::new(device, queue, surface_format);

        let transform_pass = crate::gpu::transform::TransformPass::new(device);
        let content_bounds = ContentBoundsPass::new(device);

        Compositor {
            group_state,
            root_id,
            node_textures: HashMap::new(),
            default_mask_bind_group,
            mask_bind_groups: HashMap::new(),
            layer_cache: HashMap::new(),
            blend_pipelines,
            mask_lerp_pipeline,
            passthrough_mask_state: HashMap::new(),
            present_pipeline,
            present_to_veil_pipeline,
            _present_bind_group_layout,
            present_cache_bind_group,
            view_uniform_buf,
            sampler,
            needs_composite: true,
            needs_present: false,
            dirty_node_pixels: HashSet::new(),
            canvas_width: width,
            canvas_height: height,
            padded_width: padded_w,
            padded_height: padded_h,
            veil_chain,
            void_registry: VoidRegistry::new(),
            transform_pass,
            isolated_node: None,
            selection_state: None,
            content_bounds,
            tool_overlay,
            cached_view_transform: identity,
            viewport_bg: DEFAULT_WORKSPACE_BG,
            frame_count: 0,
            last_wall_time: 0.0,
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
        match layer {
            Layer::Raster(r) => self.ensure_raster_layer(device, queue, r.id, r.pixels.bounds),
            Layer::Void(v) => self.ensure_void_layer(device, queue, v.id, &v.void_type, &v.params),
        }
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
            canvas_size: [self.canvas_width as f32, self.canvas_height as f32],
            _pad2: [0.0, 0.0],
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

    /// Resize a layer's GPU texture to the given canvas-space extent.
    ///
    /// **Pure realization.** This method is a faithful reflection of the
    /// requested extent — it does not compute unions or chunk-align. The
    /// caller (engine-level `grow_layer`) is responsible for choosing
    /// Resize a node's GPU texture (raster layer or mask modifier) to a new
    /// canvas extent. Format-agnostic — the existing texture's format drives
    /// reallocation. If the node is unknown or already at `new_extent`, this
    /// is a no-op. Otherwise the texture is reallocated and old contents are
    /// `copy_texture_to_texture`'d into the new texture at the offset that
    /// preserves their canvas-space anchor; new pixels start zeroed for RGBA
    /// (transparent) and white-filled for R8 (full reveal).
    ///
    /// **Lockstep growth across host + modifiers is the engine's job** — it
    /// owns the document and walks `host.modifiers` to call this helper for
    /// each non-locked sibling. The compositor is single-node here.
    pub fn resize_node_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        node_id: LayerId,
        new_extent: CanvasRect,
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
            other => panic!("resize_node_texture: unsupported format {other:?}"),
        };

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

        self.node_textures.insert(node_id, new_tex);

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

        // Resize rewrites the texture; thumbnail must reflect the new
        // extent + transferred pixels.
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
        let gs = Self::create_group_state(
            device,
            queue,
            self.padded_width,
            self.padded_height,
            group_id,
        );
        self.group_state.insert(group_id, gs);
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
            // Groups composite a canvas-sized cache against canvas — the
            // layer-offset translation collapses to identity.
            let canvas = [self.canvas_width as f32, self.canvas_height as f32];
            let uniforms = BlendUniforms {
                opacity,
                blend_mode: blend_mode_gpu,
                isolated: isolated as u32,
                _pad1: 0.0,
                layer_offset: [0.0, 0.0],
                layer_size: canvas,
                canvas_size: canvas,
                _pad2: [0.0, 0.0],
            };
            queue.write_buffer(&gs.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
        }
        // Also update passthrough mask state uniform (isolated only).
        if let Some(pms) = self.passthrough_mask_state.get(&group_id) {
            let val = isolated as u32;
            queue.write_buffer(&pms.uniform_buf, 0, bytemuck::bytes_of(&val));
        }
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
    /// group renders its contents. Modifiers naturally fall in via their
    /// host (which is the modifier's `parent_of`); they have no children, so
    /// isolating a modifier limits the visible canvas to the host plus the
    /// modifier itself, which the host's blend pass then renders as
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
    /// helpers `clone_node_pixels` / `clone_modifier_pixels`. Higher-level
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
    /// thumbnail readbacks; resolves layer-vs-modifier through the document.
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
        let Some(tex) = self
            .node_textures
            .get(&node_id)
            .or_else(|| self.node_textures.get(&node_id))
        else {
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
    /// and mask modifiers — format and extent come from the texture's own
    /// metadata. Returns `None` for groups (no pixels) and unknown ids.
    pub fn node_texture(&self, node_id: LayerId) -> Option<&LayerTexture> {
        self.node_textures.get(&node_id)
    }

    /// Return the GPU texture backing any entity's pixels — works uniformly
    /// for raster layers, mask modifiers, AND the selection modifier.
    ///
    /// The selection's R8 texture lives in
    /// [`crate::gpu::selection::SelectionState`] (ping-pong pair + dedicated
    /// bind groups) rather than the unified `node_textures` HashMap;
    /// `pixel_data_for` hides that asymmetry so callers (save readback,
    /// future readers) don't need to know.
    pub fn pixel_data_for(&self, node_id: LayerId) -> Option<PixelDataRef<'_>> {
        if let Some(t) = self.node_textures.get(&node_id) {
            let ext = t.layer_extent();
            return Some(PixelDataRef {
                texture: t.texture(),
                format: t.format(),
                width: ext.width,
                height: ext.height,
            });
        }
        // Persistent void textures (camera void's last frame, etc.) live
        // on the void's own EffectCache rather than in `node_textures` —
        // they're not canvas-sized and don't participate in the standard
        // raster blend at that resolution. Surface them here so the save
        // flow's `queue_pixel_readback` sees a uniform lookup, no kind
        // discrimination at the call site.
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
        if let Some(sel) = self.selection_state.as_ref() {
            if sel.modifier_id == node_id {
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
        proc.dirty = true;
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
                // PassthroughMaskState is a per-host-group resource (the
                // snapshot is sized to the parent accumulator). It's not
                // owned by the mask texture itself, so creation lives behind
                // [`Self::ensure_passthrough_mask_state`] which the engine
                // calls when attaching a mask to a host. Keep the allocation
                // out of the texture-creation path so the keying is by host,
                // not by mask modifier id.
            }
            wgpu::TextureFormat::Rgba8Unorm => {
                // ensure_raster_layer marks dirty itself.
                self.ensure_raster_layer(device, queue, node_id, bounds);
            }
            other => panic!("ensure_node_texture: unsupported format {other:?}"),
        }
    }

    /// Allocate the snapshot+uniform pair the passthrough-group mask path
    /// needs, keyed by **host** id (the group whose composited output gets
    /// snapshot-then-lerped against its mask). Idempotent. The mask texture
    /// itself lives in the shared node-texture pool keyed by mask modifier
    /// id; this resource is a per-host concern, not per-modifier — there's
    /// one snapshot buffer per group regardless of how many modifiers attach.
    pub fn ensure_passthrough_mask_state(&mut self, device: &wgpu::Device, host_id: LayerId) {
        if self.passthrough_mask_state.contains_key(&host_id) {
            return;
        }
        let (snapshot, snapshot_view) = Self::make_accum_texture(
            device,
            self.padded_width,
            self.padded_height,
            &format!("pt-snapshot-{host_id:?}"),
        );
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!("pt-lerp-uniforms-{host_id:?}")),
            size: 4,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.passthrough_mask_state.insert(
            host_id,
            PassthroughMaskState {
                snapshot,
                snapshot_view,
                uniform_buf,
            },
        );
    }

    /// Drop the passthrough-group mask snapshot for a host id. Mirrors
    /// [`Self::ensure_passthrough_mask_state`].
    pub fn dispose_passthrough_mask_state(&mut self, host_id: LayerId) {
        self.passthrough_mask_state.remove(&host_id);
    }

    // --- Selection (global) ---

    /// Allocate the GPU realisation of the document's selection modifier.
    /// Idempotent — returns immediately if already allocated. The selection
    /// modifier id is stashed on the [`SelectionState`] so undo / region-store
    /// keying can resolve back to the document modifier.
    pub fn ensure_selection_state(
        &mut self,
        device: &wgpu::Device,
        modifier_id: LayerId,
        brush_bgl: &wgpu::BindGroupLayout,
        paint_bgl: &wgpu::BindGroupLayout,
    ) {
        if self.selection_state.is_some() {
            return;
        }
        self.selection_state = Some(crate::gpu::selection::SelectionState::new(
            device,
            modifier_id,
            self.canvas_width,
            self.canvas_height,
            brush_bgl,
            paint_bgl,
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
    /// modifier removal. Per-host passthrough state is owned by its host
    /// id, so it's not touched here.
    pub fn dispose_node_texture(&mut self, node_id: LayerId) {
        self.node_textures.remove(&node_id);
        self.mask_bind_groups.remove(&node_id);
        self.layer_cache.remove(&node_id);
        self.dirty_node_pixels.remove(&node_id);
        self.mark_dirty();
    }

    /// Drop all GPU state for a layer when it's permanently removed
    /// (`Engine::remove_layer`) or when an auto-created paste-target is
    /// canceled (`cancel_floating`). Alias of [`Self::dispose_node_texture`]
    /// kept as a separate entry point because the engine's layer-removal
    /// path conceptually distinguishes "tree node gone" from "modifier
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

    /// Allocate the per-instance GPU state for a new void layer:
    /// procedural texture in [`Self::node_textures`] (canvas-sized
    /// [`wgpu::TextureFormat::Rgba8Unorm`], matching the raster path so the
    /// compositor's blend pipeline can sample it without any kind-specific
    /// branch) and a [`LayerCache`] holding the blend uniforms plus a
    /// [`LayerContent::Procedural`] sidecar with the trait object and its
    /// `EffectCache`. Idempotent — calling twice for the same id is a no-op.
    pub fn ensure_void_layer(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layer_id: LayerId,
        void_type: &str,
        params: &[ParamValue],
    ) {
        if self.layer_cache.contains_key(&layer_id) {
            return;
        }
        // Canvas-sized texture so the procedural output composites against
        // the same coordinate system as raster layers.
        let bounds = CanvasRect::from_xywh(0, 0, self.canvas_width, self.canvas_height);
        let layer_tex = LayerTexture::with_bounds(device, bounds);

        // Build the per-instance trait object via the registry. The registry
        // lazy-builds the shared pipeline on the first call for a given type.
        let format = layer_tex.format();
        let void = self
            .void_registry
            .create_void(void_type, params, device, format);
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
            canvas_size: [self.canvas_width as f32, self.canvas_height as f32],
            _pad2: [0.0, 0.0],
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
                content: LayerContent::Procedural(ProceduralContent {
                    void,
                    cache,
                    dirty: true,
                }),
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
    ///
    /// `void_type` is unused here but kept in the signature so the engine
    /// API matches what callers had to thread through anyway. It used to
    /// be required when this method rebuilt the void from scratch via the
    /// registry; that path was abandoned because it dropped the camera
    /// void's aux texture on every param edit.
    pub fn update_void_layer_params(
        &mut self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        layer_id: LayerId,
        _void_type: &str,
        params: &[ParamValue],
    ) {
        let Some(proc) = self.procedural_content_mut(layer_id) else {
            return;
        };
        proc.void.update_params(queue, &proc.cache, params);
        proc.dirty = true;
        self.mark_dirty();
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
        proc.dirty = true;
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
            proc.dirty = true;
        }
    }

    /// Re-render every dirty procedural layer's texture. Runs at the top of
    /// the compositor's encode pass so the subsequent blend in
    /// `compose_children` samples up-to-date pixels. Raster layers are
    /// inherently "never dirty" — their pixels arrived through paint and
    /// `node_textures[id]` is authoritative.
    fn encode_dirty_layer_content(&mut self, encoder: &mut wgpu::CommandEncoder) {
        // Two-phase: collect ids of dirty procedural layers, then drop the
        // mutable borrow and re-acquire per-entry. Keeps the loop body
        // short and avoids borrowing `self.layer_cache` and
        // `self.node_textures` at the same time.
        let dirty: Vec<LayerId> = self
            .layer_cache
            .iter()
            .filter_map(|(id, c)| match &c.content {
                LayerContent::Procedural(p) if p.dirty => Some(*id),
                _ => None,
            })
            .collect();
        for id in dirty {
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
            proc.dirty = false;
        }
    }

    /// Total number of node textures (raster layers + mask modifiers)
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

        // Voids piggyback on the same master clock as veils and the tool
        // overlay — using a parallel integer divisor keeps tick alignment so
        // an animated void never forces a frame the other systems wouldn't
        // already produce, matching the `gpu-lessons-learned.md` master-
        // clock principle.
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
    /// workspace background color, so it stamps it onto the uploaded copy
    /// rather than relying on every caller to set it.
    pub fn update_view_transform(&mut self, queue: &wgpu::Queue, transform: &ViewTransform) {
        let mut t = *transform;
        t.bg = self.viewport_bg;
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

    /// Update a content layer's uniforms (called when opacity, blend mode,
    /// or isolated changes). Convenience wrapper that pins `isolated` to
    /// false — bulk sync paths that compute isolation already use the full
    /// variant directly.
    pub fn update_layer_uniforms(
        &mut self,
        queue: &wgpu::Queue,
        layer_id: LayerId,
        opacity: f32,
        blend_mode_gpu: u32,
    ) {
        self.update_layer_uniforms_full(queue, layer_id, opacity, blend_mode_gpu, false);
    }

    /// Update a content layer's uniforms including the isolated flag. Works
    /// uniformly for raster and procedural layers — both store their blend
    /// state in the same [`LayerCache`] and sample from canvas-positioned
    /// textures in `node_textures`. Reads the layer's bounds from its
    /// `LayerTexture` so callers don't need to thread them through;
    /// bounds-changing operations update the texture's stored offset/size
    /// directly via `resize_node_texture`.
    ///
    /// `blend_mode_gpu` is the registry-resolved gpu_value.
    pub fn update_layer_uniforms_full(
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
            canvas_size: [self.canvas_width as f32, self.canvas_height as f32],
            _pad2: [0.0, 0.0],
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

    /// Write the floating preview's canvas-aligned blend uniforms using the
    /// given layer's cached blend props. No-op when there is no active
    /// floating, or when the active floating's target is not `layer_id`.
    /// Called from both `update_layer_uniforms_full` (on prop change) and
    /// the floating setup paths (to seed the buffer at session start).
    fn write_preview_blend_uniforms_if_active(&self, queue: &wgpu::Queue, layer_id: LayerId) {
        let state = match self.transform_pass.active.as_ref() {
            Some(s) if s.target_layer == layer_id => s,
            _ => return,
        };
        let cache = match self.layer_cache.get(&layer_id) {
            Some(c) => c,
            None => return,
        };
        let uniforms = BlendUniforms {
            opacity: cache.opacity,
            blend_mode: cache.blend_mode,
            isolated: cache.isolated as u32,
            _pad1: 0.0,
            layer_offset: [0.0, 0.0],
            layer_size: [self.canvas_width as f32, self.canvas_height as f32],
            canvas_size: [self.canvas_width as f32, self.canvas_height as f32],
            _pad2: [0.0, 0.0],
        };
        queue.write_buffer(
            &state.preview_blend_uniform_buf,
            0,
            bytemuck::bytes_of(&uniforms),
        );
    }

    /// Look up the resolved mask bind group for a modifier id, falling back to
    /// the default (1x1 white = no masking) when no real mask is active.
    fn mask_bind_group(&self, layer_id: LayerId) -> &wgpu::BindGroup {
        self.mask_bind_groups
            .get(&layer_id)
            .unwrap_or(&self.default_mask_bind_group)
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

    // --- Tool Overlay ---

    /// Replace the current overlay primitives.
    pub fn set_overlay_primitives(&mut self, prims: Vec<OverlayPrimitive>) {
        self.tool_overlay.set_primitives(prims);
        self.needs_present = true;
    }

    /// Clear all overlay primitives.
    pub fn clear_overlay(&mut self) {
        self.tool_overlay.clear_primitives();
        self.needs_present = true;
    }

    /// Advance overlay animation time.
    pub fn update_overlay_time(&mut self, dt: f32) {
        self.tool_overlay.advance_time(dt);
    }

    /// Upload a mask texture for KIND_MASKED_STAMP overlay primitives.
    pub fn set_overlay_mask(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) {
        self.tool_overlay
            .set_mask_texture(device, queue, width, height, rgba);
        self.needs_present = true;
    }

    /// Drop the uploaded mask (fall back to 1×1 white).
    pub fn clear_overlay_mask(&mut self) {
        self.tool_overlay.clear_mask_texture();
        self.needs_present = true;
    }

    /// Ensure the overlay's preview-mask texture exists at the given size;
    /// returns a view for a brush node to render into.
    pub fn ensure_overlay_preview_mask(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> &wgpu::TextureView {
        self.tool_overlay.ensure_preview_mask(device, width, height)
    }

    /// Route the preview-mask texture as the active overlay mask binding.
    pub fn use_overlay_preview_mask(&mut self) {
        self.tool_overlay.use_preview_mask_as_mask();
        self.needs_present = true;
    }

    /// Unbind the preview mask (falls back to 1×1 white default).
    pub fn clear_overlay_preview_mask(&mut self) {
        self.tool_overlay.clear_preview_mask();
        self.needs_present = true;
    }

    /// Borrow the overlay's preview-mask Texture (None if never allocated).
    pub fn overlay_preview_mask_texture(&self) -> Option<&wgpu::Texture> {
        self.tool_overlay.preview_mask_texture()
    }

    /// Immutable access to the tool overlay for test assertions.
    pub fn tool_overlay_ref(&self) -> &ToolOverlay {
        &self.tool_overlay
    }

    /// CPU-side hit test on overlay primitives.
    pub fn overlay_hit_test(&self, screen_x: f32, screen_y: f32) -> Option<usize> {
        self.tool_overlay.hit_test(screen_x, screen_y)
    }

    // --- Floating Content (Transform) ---
    //
    // The floating preview is a *derived view* of the target node's texture:
    // when a transform is active, the compositor maintains a per-target
    // preview texture rebuilt on every matrix update, holding "what the
    // target would look like if commit ran right now". The render walk's
    // `effective_node_view` and `effective_mask_bind_group` accessors swap
    // the live view / mask bind group for the preview equivalents when the
    // floating target is encountered, so the host's normal blend pass
    // renders the preview without any extra render pass — and isolation,
    // grouping, and other branch-free render concerns compose with it
    // automatically.
    //
    // The compositor exposes primitives (set/clear floating content, update
    // uniforms + preview, commit to live target). The engine drives them
    // by calling `update_floating_preview` after each matrix change and on
    // setup_transform.

    /// Allocate the per-target preview texture (and, when target is R8, a
    /// mask-shape bind group sampling it) plus the canvas-aligned blend
    /// uniform buffer the host's blend pass reads when this layer is the
    /// floating target.
    ///
    /// Preview is canvas-sized (not live-sized) so a translate that moves
    /// content past the live layer's bounding box still has room on the
    /// preview to write — clipped at canvas bounds, which is all the
    /// viewport renders anyway. Commit's `grow_node_to_fit` separately
    /// expands the live layer so off-canvas pixels survive the commit.
    fn allocate_preview_resources(
        &self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> (
        wgpu::Texture,
        wgpu::TextureView,
        Option<wgpu::BindGroup>,
        wgpu::Buffer,
    ) {
        let preview = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("floating-preview"),
            size: wgpu::Extent3d {
                width: self.canvas_width.max(1),
                height: self.canvas_height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            // COPY_SRC needed because `render_commit` runs
            // `copy_for_compositing` against the render target before its
            // shader pass — when the target is the preview texture, that
            // copy reads from preview.
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = preview.create_view(&wgpu::TextureViewDescriptor::default());
        let mask_bg = if format == wgpu::TextureFormat::R8Unorm {
            Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("floating-preview-mask-bg"),
                layout: &self.blend_pipelines.mask_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                }],
            }))
        } else {
            None
        };
        let blend_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("floating-preview-blend-uniforms"),
            size: std::mem::size_of::<BlendUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        (preview, view, mask_bg, blend_uniform_buf)
    }

    /// Look up the target's format + dimensions. Falls back to canvas-sized
    /// RGBA8 when the node texture hasn't been allocated yet (paste-as-
    /// floating creates the layer before its texture).
    fn target_format_and_dims(&self, target_layer: LayerId) -> (wgpu::TextureFormat, u32, u32) {
        match self.node_textures.get(&target_layer) {
            Some(t) => {
                let ext = t.layer_extent();
                (t.format(), ext.width, ext.height)
            }
            None => (
                wgpu::TextureFormat::Rgba8Unorm,
                self.canvas_width,
                self.canvas_height,
            ),
        }
    }

    /// Set up floating content for GPU preview from straight-alpha RGBA
    /// pixel data (used by the paste paths). The target's preview texture
    /// is allocated alongside.
    pub fn set_floating_content(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rgba_data: &[u8],
        _source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
        target_layer: LayerId,
    ) {
        let (target_format, _tw, _th) = self.target_format_and_dims(target_layer);
        let (preview_texture, preview_view, preview_mask_bg, preview_blend_uniform_buf) =
            self.allocate_preview_resources(device, target_format);
        self.transform_pass.set_floating_content(
            device,
            queue,
            &self.sampler,
            rgba_data,
            source_width,
            source_height,
            target_layer,
            target_format,
            preview_texture,
            preview_view,
            preview_mask_bg,
            preview_blend_uniform_buf,
        );
        // Seed the preview's blend uniforms from the live layer's cached
        // props now that the floating session is active.
        self.write_preview_blend_uniforms_if_active(queue, target_layer);
        self.mark_dirty();
    }

    /// Set floating content by copying directly from a node's GPU texture.
    /// GPU→GPU copy — no CPU tiles involved. Format dispatch (R8 mask vs RGBA
    /// layer) is driven by the texture's own format from the unified node pool.
    pub fn set_floating_content_from_gpu(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
        target_layer: LayerId,
    ) {
        let layer = match self.node_textures.get(&target_layer) {
            Some(t) => t,
            None => return,
        };
        let target_format = layer.format();
        let (preview_texture, preview_view, preview_mask_bg, preview_blend_uniform_buf) =
            self.allocate_preview_resources(device, target_format);
        // Re-borrow `layer` after `allocate_preview_resources` — the helper
        // doesn't take `&mut self`, but rust-analyzer prefers the explicit
        // re-fetch over keeping the borrow live across the helper call.
        let layer = self
            .node_textures
            .get(&target_layer)
            .expect("layer present after preview allocation");
        self.transform_pass.set_floating_content_from_gpu(
            device,
            queue,
            encoder,
            &self.sampler,
            layer,
            source_origin,
            source_width,
            source_height,
            target_layer,
            target_format,
            preview_texture,
            preview_view,
            preview_mask_bg,
            preview_blend_uniform_buf,
        );
        // Seed the preview's blend uniforms from the live layer's cached
        // props now that the floating session is active.
        self.write_preview_blend_uniforms_if_active(queue, target_layer);
        self.mark_dirty();
    }

    /// Update the floating preview: write the current matrix uniforms, copy
    /// live target → preview texture, apply the engine-side `clear_shape`
    /// (None for paste mode, `Some` for transform mode), then run the
    /// commit shader into the preview. The resulting preview texture is
    /// what the host's blend pass reads through `effective_node_view` /
    /// `effective_mask_bind_group` for the rest of the frame.
    ///
    /// Called by the engine on `setup_transform` (initial preview) and
    /// `update_floating_matrix` (per drag tick).
    #[allow(clippy::too_many_arguments)]
    pub fn update_floating_preview(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        paint_pipelines: &crate::gpu::paint_target::PaintPipelines,
        matrix: &crate::gpu::transform::Affine2D,
        source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
        clear_shape: Option<&crate::gpu::transform::ClearShape>,
    ) {
        let Some(state) = self.transform_pass.active.as_ref() else {
            return;
        };
        let live = match self.node_textures.get(&state.target_layer) {
            Some(t) => t,
            None => return,
        };

        // The preview is canvas-aligned: the transform shader writes the
        // moved source content using target_offset=(0,0), target_size=canvas,
        // so any pixel that lands within the canvas survives — including
        // ones that fell outside the live texture's bounding box.
        self.transform_pass.update_uniforms(
            queue,
            matrix,
            source_origin,
            source_width,
            source_height,
            (0, 0),
            self.canvas_width,
            self.canvas_height,
            self.canvas_width,
            self.canvas_height,
        );

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("floating-preview-build"),
        });

        // 0. Reset the whole preview to transparent. The copy below only
        //    repaints the canvas portion live actually covers, and the
        //    commit shader discards outside the transformed source bounds —
        //    so canvas pixels outside live's extent would otherwise retain
        //    previous-frame transform writes (ghost trails).
        crate::gpu::clear_view_transparent(
            &mut encoder,
            &state.preview_view,
            "floating-preview-clear",
        );

        // 1. Copy live → preview so non-source-rect pixels are preserved.
        //    Live texture sits at `(live.offset_x, live.offset_y)` in canvas
        //    space; clip the copy to the on-canvas portion (the preview is
        //    canvas-sized at origin 0,0). Off-canvas pixels are not in the
        //    preview — the viewport never renders them anyway, and commit's
        //    `grow_node_to_fit` preserves them on the live texture.
        let canvas_rect =
            crate::coord::CanvasRect::from_xywh(0, 0, self.canvas_width, self.canvas_height);
        let live_canvas_extent = live.canvas_extent();
        if let Some(visible) = live_canvas_extent.intersect(canvas_rect) {
            // Translate the visible canvas slice into the live texture's
            // layer-local coordinate frame for the GPU copy origin.
            let visible_layer = live
                .canvas_to_layer_rect(visible)
                .expect("intersect with live's extent yields a layer-local rect");
            let dst_x = visible.x0() as u32;
            let dst_y = visible.y0() as u32;
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: live.texture(),
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: visible_layer.x0(),
                        y: visible_layer.y0(),
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &state.preview_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: dst_x,
                        y: dst_y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: visible.width,
                    height: visible.height,
                    depth_or_array_layers: 1,
                },
            );
        }

        // 2. Apply the source-rect clear (transform mode only — paste mode
        //    leaves the preview as a copy of the live target so the commit
        //    shader composites over the existing pixels). The preview is
        //    canvas-aligned, so the paint target reports canvas dims/offset.
        if let Some(cs) = clear_shape {
            let preview_target = crate::gpu::paint_target::GpuPaintTarget::from_canvas_texture(
                &state.preview_texture,
                &state.preview_view,
                state.target_format,
                self.canvas_width,
                self.canvas_height,
            );
            match cs {
                crate::gpu::transform::ClearShape::Rect(rect) => {
                    preview_target.clear_rect(&mut encoder, paint_pipelines, queue, *rect);
                }
                crate::gpu::transform::ClearShape::Selection { mask_bind_group } => {
                    preview_target.erase_with_selection(
                        &mut encoder,
                        paint_pipelines,
                        queue,
                        mask_bind_group,
                    );
                }
            }
        }

        // 3. Run the commit shader into the preview at the current matrix.
        self.transform_pass.render_commit(
            device,
            &mut encoder,
            &state.preview_texture,
            &state.preview_view,
        );

        queue.submit(std::iter::once(encoder.finish()));
    }

    /// Render the transform directly into the live target texture.
    /// Used by `commit_floating()` after the engine has applied the
    /// `clear_shape` to the live target.
    pub fn commit_floating_to_texture(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        matrix: &crate::gpu::transform::Affine2D,
        source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
    ) {
        let Some(state) = self.transform_pass.active.as_ref() else {
            return;
        };
        let live = match self.node_textures.get(&state.target_layer) {
            Some(t) => t,
            None => return,
        };

        let live_extent = live.canvas_extent();
        self.transform_pass.update_uniforms(
            queue,
            matrix,
            source_origin,
            source_width,
            source_height,
            (live_extent.x0(), live_extent.y0()),
            live_extent.width,
            live_extent.height,
            self.canvas_width,
            self.canvas_height,
        );

        self.transform_pass
            .render_commit(device, encoder, live.texture(), live.view());
    }

    /// Remove floating content GPU state.
    pub fn clear_floating_content(&mut self) {
        self.transform_pass.clear();
        self.mark_dirty();
    }

    /// Effective mask bind group for a host raster/group during compositing
    /// — substitutes the preview-mask bind group when one of the host's
    /// modifiers is the floating target. Fall-through resolves the live
    /// mask through the existing `mask_bind_group` lookup.
    pub(crate) fn effective_mask_bind_group(
        &self,
        doc: &Document,
        host_id: LayerId,
    ) -> &wgpu::BindGroup {
        let live_or_default = doc
            .mask_modifier(host_id)
            .filter(|m| m.common.visible)
            .map(|m| self.mask_bind_group(m.id))
            .unwrap_or(&self.default_mask_bind_group);

        if let Some(state) = self.transform_pass.active.as_ref() {
            if doc.parent_of(state.target_layer) == Some(host_id) {
                if let Some(bg) = state.preview_mask_bind_group.as_ref() {
                    return bg;
                }
            }
        }
        live_or_default
    }

    /// Get a reference to the transform source texture and its view.
    /// Returns None if no floating content is active.
    pub fn transform_source_texture(&self) -> Option<(&wgpu::Texture, &wgpu::TextureView)> {
        self.transform_pass
            .active
            .as_ref()
            .map(|s| (&s.source_texture, &s.source_view))
    }

    /// Check if floating content is active.
    pub fn has_floating_content(&self) -> bool {
        self.transform_pass.active.is_some()
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

        let root_id = self.root_id;
        self.compose_group(&mut encoder, device, doc, root_id, scissor);

        queue.submit(std::iter::once(encoder.finish()));

        self.needs_composite = false;
        true
    }

    /// Run the present pass into a canvas-sized offscreen RGBA8 texture and
    /// return its bytes. For tests: the production present pass writes to the
    /// surface (un-readable), but the present shader is exactly where bugs
    /// like premultiplied-alpha mishandling live, so test coverage of that
    /// stage requires a parallel sink.
    ///
    /// Forces an identity 1:1 view transform so screen pixels map to canvas
    /// pixels and the OOB branch is inactive across the whole target.
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

        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("test-present-target"),
            size: wgpu::Extent3d {
                width: cw,
                height: ch,
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
            cw,
            ch,
        )
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
        // `compose_children`, which itself re-borrows `doc`.
        let children: Vec<LayerId> = doc.children_of(group_id).to_vec();
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
    fn compose_children(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        doc: &Document,
        parent_group: LayerId,
        children: &[LayerId],
        scissor: (u32, u32, u32, u32),
    ) {
        let (scissor_x, scissor_y, scissor_w, scissor_h) = scissor;

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
            match node {
                LayerNode::Layer(layer) => {
                    // One blend arm for both raster and procedural content.
                    // The procedural texture lives in `node_textures` keyed
                    // by layer id (allocated by `ensure_void_layer` and
                    // refreshed by `encode_dirty_layer_content` before the
                    // tree walk), and the blend uniforms are the same
                    // `BlendUniforms` shape in the unified `layer_cache` —
                    // so neither lookup branches on kind here.
                    let layer_id = layer.id();

                    // Effective view + uniforms: when this layer is the
                    // floating target, swap the live texture view for the
                    // (canvas-aligned) preview view AND swap the live's
                    // layer-aligned blend uniforms for the preview's
                    // canvas-aligned ones — both halves must move together
                    // or the shader maps fragments to the wrong region.
                    // Voids never become floating targets today, so the
                    // detour collapses to the live path for them; if voids
                    // ever do, the same code path will Just Work.
                    let active_floating = self
                        .transform_pass
                        .active
                        .as_ref()
                        .filter(|s| s.target_layer == layer_id);
                    let layer_view = match active_floating {
                        Some(s) => &s.preview_view,
                        None => match self.node_textures.get(&layer_id) {
                            Some(t) => t.view(),
                            None => continue,
                        },
                    };
                    let uniform_buf_ptr = match active_floating {
                        Some(s) => &s.preview_blend_uniform_buf,
                        None => match self.layer_cache.get(&layer_id) {
                            Some(c) => &c.uniform_buf,
                            None => continue,
                        },
                    };

                    // Ping-pong: read from current accum, write to the other.
                    let gs = self.group_state.get_mut(&parent_group).unwrap();
                    let src = gs.current_accum;
                    let dst = 1 - src;
                    gs.current_accum = dst;

                    let bind_group = self.create_blend_bind_group(
                        device,
                        &self.group_state[&parent_group].accum.views[src],
                        layer_view,
                        uniform_buf_ptr,
                        "blend-layer",
                    );

                    {
                        let gs = &self.group_state[&parent_group];
                        // Effective mask bind group: live mask by default,
                        // preview-mask bind group when one of this layer's
                        // modifiers is the floating target.
                        let mask_bg = self.effective_mask_bind_group(doc, layer_id);
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
                        rpass.set_bind_group(0, &bind_group, &[]);
                        rpass.set_bind_group(1, mask_bg, &[]);
                        rpass.draw(0..3, 0..1);
                    }
                }

                LayerNode::Group(g) => {
                    let group_id = g.id;
                    if g.passthrough {
                        // Structural detection: a passthrough group with a
                        // visible mask modifier triggers Photoshop-style
                        // snapshot+lerp; otherwise it's pure passthrough.
                        let has_active_mask = doc
                            .mask_modifier(group_id)
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
                            let inner: Vec<LayerId> = doc.children_of(group_id).to_vec();
                            self.compose_children(
                                encoder,
                                device,
                                doc,
                                parent_group,
                                &inner,
                                scissor,
                            );
                        }
                    } else {
                        // Normal group: composite into its own isolated buffer,
                        // then blend the result into the parent.
                        if !self.group_state.contains_key(&group_id) {
                            continue;
                        }
                        self.compose_group(encoder, device, doc, group_id, scissor);

                        // Blend group's composite cache into parent's accumulators.
                        let gs_parent = self.group_state.get_mut(&parent_group).unwrap();
                        let src = gs_parent.current_accum;
                        let dst = 1 - src;
                        gs_parent.current_accum = dst;

                        let gs_child = &self.group_state[&group_id];
                        let bind_group = self.create_blend_bind_group(
                            device,
                            &self.group_state[&parent_group].accum.views[src],
                            &gs_child.composite_cache_view,
                            &gs_child.uniform_buf,
                            "blend-group",
                        );

                        let gs_parent = &self.group_state[&parent_group];
                        // Same effective-mask routing as the raster path —
                        // when the floating target is one of this group's
                        // modifiers, sample the preview-mask bind group
                        // instead of the live one.
                        let child_mask_bg = self.effective_mask_bind_group(doc, group_id);
                        {
                            let mut rpass =
                                encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
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
                            rpass.set_bind_group(0, &bind_group, &[]);
                            rpass.set_bind_group(1, child_mask_bg, &[]);
                            rpass.draw(0..3, 0..1);
                        }
                    }
                }
            }
        }
    }

    /// Composite a passthrough group whose mask is active.
    ///
    /// Snapshots the parent accumulator, composites children (passthrough),
    /// then lerps between the snapshot and the result using the group mask.
    fn compose_passthrough_masked(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        doc: &Document,
        parent_group: LayerId,
        group_id: LayerId,
        scissor: (u32, u32, u32, u32),
    ) {
        let (scissor_x, scissor_y, scissor_w, scissor_h) = scissor;

        // PassthroughMaskState must exist (created when the mask was added).
        if !self.passthrough_mask_state.contains_key(&group_id) {
            // Fallback: just inline children without mask.
            let inner: Vec<LayerId> = doc.children_of(group_id).to_vec();
            self.compose_children(encoder, device, doc, parent_group, &inner, scissor);
            return;
        }

        // 1. Copy current parent accum (the "before" state) into the snapshot.
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
        let pms = &self.passthrough_mask_state[&group_id];
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.group_state[&parent_group].accum.textures[before_idx],
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

        // 2. Composite children into parent accumulators (passthrough).
        let inner: Vec<LayerId> = doc.children_of(group_id).to_vec();
        self.compose_children(encoder, device, doc, parent_group, &inner, scissor);

        // 3. Lerp pass: mix(snapshot, current_accum, mask).
        //    Write the lerp result into the ping-pong "other" accumulator.
        let gs = self.group_state.get_mut(&parent_group).unwrap();
        let after_idx = gs.current_accum;
        let dst = 1 - after_idx;
        gs.current_accum = dst;

        let pms = &self.passthrough_mask_state[&group_id];

        // Create lerp bind group (group 0): before=snapshot, after=current_accum, sampler, uniforms.
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

        {
            let gs = &self.group_state[&parent_group];
            // Effective mask: live by default, preview-mask when the
            // floating target is this passthrough group's mask modifier.
            let group_mask_bg = self.effective_mask_bind_group(doc, group_id);
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
            rpass.set_bind_group(1, group_mask_bg, &[]);
            rpass.draw(0..3, 0..1);
        }
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

    /// Composite layers if needed, then present to an arbitrary texture view.
    ///
    /// This is the backend-agnostic rendering entry point. Any frontend
    /// (WASM surface, native window, CEF hole-punch, headless test) can
    /// provide a `TextureView` and get the composited + veiled result.
    pub fn render_to_view(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_view: &wgpu::TextureView,
        doc: &mut Document,
    ) {
        if !self.has_pending_work(doc) {
            return;
        }

        self.render_offscreen(device, queue, doc);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("present-to-view"),
        });
        self.present_and_veils(&mut encoder, target_view);
        queue.submit(std::iter::once(encoder.finish()));

        self.finish_present();
    }

    /// Upload dirty tiles, composite changed layers, present to a surface.
    ///
    /// Convenience wrapper around `render_to_view` that handles surface
    /// acquisition and presentation. Used by the WASM frontend.
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
        let output = match surface.get_current_texture() {
            Ok(output) => output,
            Err(wgpu::SurfaceError::Lost) => {
                surface.configure(device, surface_config);
                perf::time_end("render-total");
                return;
            }
            Err(wgpu::SurfaceError::OutOfMemory) => {
                log::error!("Out of GPU memory");
                perf::time_end("render-total");
                return;
            }
            Err(e) => {
                log::warn!("Surface error: {e:?}");
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
            let vt = self.cached_view_transform;
            let vw = self.veil_chain.viewport_size().0;
            let vh = self.veil_chain.viewport_size().1;
            self.tool_overlay.prepare(device, queue, &vt, vw, vh);
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
