//! Central brush pipeline registry, plumbing pipelines, and shared infra.
//!
//! Each terminal-mode brush node (stamp, liquify, watercolor, …) declares
//! its own GPU pipeline alongside its node `register()` — see
//! [`crate::brush::nodes`].  Their `BrushPipelineRegistration`s are
//! harvested at [`BrushPipelines::new`] time and stored in a typed map.
//! Pipelines that are not tied to any one node — `blit`, `mask_blit`,
//! `scratch_blit_r8` — are format-bridging plumbing and live directly on
//! [`BrushPipelines`].
//!
//! ## Per-mode pipeline contract
//!
//! Each per-mode pipeline:
//!
//! - is a `struct` implementing [`BrushPipelineEntry`];
//! - is built by a `fn build(ctx: &BuildContext) -> Self` constructor;
//! - exposes its own typed `write_uniforms` / `pipeline` / `uniform_bind_group`
//!   methods — uniform struct shapes vary, so dispatch is type-owned;
//! - returns its dynamic-uniform-ring (if any) from `ring()` so the
//!   registry can iterate all rings for frame reset / overflow checks.
//!
//! Look-up is by `(id, type)`:
//!
//! ```ignore
//! let liq = pipelines.get::<LiquifyPipeline>("liquify");
//! pass.set_pipeline(liq.pipeline());
//! ```

use std::any::Any;
use std::cell::Cell;
use std::collections::HashMap;
use std::num::NonZeroU64;

// ── Uniforms shared by the plumbing pipelines ────────────────────────────

/// Uniform data for the blit shader (preview mask blit).
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BlitUniforms {
    /// UV corner (0..1) inside the source texture where sampling starts.
    pub uv_min: [f32; 2],
    /// UV corner (0..1) inside the source texture where sampling ends.
    pub uv_max: [f32; 2],
}

// ── Dynamic uniform ring ─────────────────────────────────────────────────

/// Ring buffer for dynamic uniform offsets.
///
/// Instead of a single uniform buffer that must be submitted between dabs,
/// each dab writes to a unique offset.  All render passes can go into one
/// command encoder and be submitted once.
///
/// Uses `Cell` for `next_index` so `write()` can take `&self` — the ring is
/// never shared across threads.
pub const UNIFORM_RING_CAPACITY: u32 = 256;

pub struct DynamicUniformRing {
    pub buffer: wgpu::Buffer,
    aligned_stride: u64,
    capacity: u32,
    next_index: Cell<u32>,
}

impl DynamicUniformRing {
    pub fn new(device: &wgpu::Device, label: &str, uniform_size: u64, min_alignment: u32) -> Self {
        let aligned_stride = align_up(uniform_size, min_alignment as u64);
        let capacity = UNIFORM_RING_CAPACITY;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: aligned_stride * capacity as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            buffer,
            aligned_stride,
            capacity,
            next_index: Cell::new(0),
        }
    }

    /// Write uniform data to the next slot.  Returns the byte offset for
    /// `set_bind_group`'s dynamic offset array.
    pub fn write(&self, queue: &wgpu::Queue, data: &[u8]) -> u32 {
        let idx = self.next_index.get();
        debug_assert!(idx < self.capacity, "DynamicUniformRing overflow");
        let offset = idx as u64 * self.aligned_stride;
        queue.write_buffer(&self.buffer, offset, data);
        self.next_index.set(idx + 1);
        offset as u32
    }

    /// Reset to slot 0 for the next frame.
    pub fn reset(&self) {
        self.next_index.set(0);
    }

    pub fn nearly_full(&self) -> bool {
        // Leave headroom for a few more writes after the check (one dab
        // can use up to 3 ring slots across different pipelines).
        self.next_index.get() >= self.capacity - 4
    }

    /// Binding size for the bind group entry (one slot, not the whole buffer).
    pub fn binding_size(&self) -> NonZeroU64 {
        NonZeroU64::new(self.aligned_stride).unwrap()
    }
}

pub fn align_up(value: u64, alignment: u64) -> u64 {
    (value + alignment - 1) & !(alignment - 1)
}

// ── Per-mode pipeline contract ───────────────────────────────────────────

/// Borrowed view of all shared brush infra a per-mode pipeline can read
/// while it builds itself.  Constructed once by [`BrushPipelines::new`]
/// and passed to every `BrushPipelineRegistration::build`.
pub struct BuildContext<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    /// `group(0)` layout — single dynamic-offset uniform buffer.  Every
    /// per-mode pipeline binds its dab uniforms here.
    pub uniform_bgl: &'a wgpu::BindGroupLayout,
    /// Texture + linear sampler — bound where composites need to modulate
    /// fragment output by the selection mask.
    pub selection_bgl: &'a wgpu::BindGroupLayout,
    /// Texture + linear sampler — bound where shaders sample the per-dab
    /// scratch read mirror snapshot (composite, smudge,
    /// liquify, watercolor atlas). After the
    /// `dab_pool` deletion this BGL is the single shape for every
    /// `texture_2d<f32> + sampler` binding in the brush stack — the
    /// scratch's write bind group also lives on it.
    pub canvas_copy_bgl: &'a wgpu::BindGroupLayout,
    pub canvas_copy_sampler: &'a wgpu::Sampler,
    pub min_uniform_align: u32,
}

impl<'a> BuildContext<'a> {
    /// Allocate the standard `(ring, bind_group)` pair every pipeline uses
    /// to feed its dynamic-offset uniform buffer.  Concentrates the
    /// `DynamicUniformRing::new` + `uniform_bgl` create_bind_group dance in
    /// one place so per-mode `build()` functions don't repeat it.
    pub fn make_uniform_ring<U>(
        &self,
        label_ring: &str,
        label_bg: &str,
    ) -> (DynamicUniformRing, wgpu::BindGroup) {
        let ring = DynamicUniformRing::new(
            self.device,
            label_ring,
            std::mem::size_of::<U>() as u64,
            self.min_uniform_align,
        );
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label_bg),
            layout: self.uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &ring.buffer,
                    offset: 0,
                    size: Some(ring.binding_size()),
                }),
            }],
        });
        (ring, bg)
    }
}

/// One per-mode brush pipeline, type-erased so the registry can store many
/// kinds in a single map.  Consumers downcast via [`BrushPipelines::get`].
///
/// Not `Sync`: per-mode pipelines own a [`DynamicUniformRing`] backed by
/// a `Cell<u32>` write cursor (intentional — see `DynamicUniformRing`'s
/// doc).  The brush engine is single-threaded.
pub trait BrushPipelineEntry: Any {
    fn as_any(&self) -> &dyn Any;
    /// The pipeline's dynamic-offset uniform ring, if it has one.  The
    /// registry iterates every entry's ring on frame reset and on overflow
    /// checks; pipelines without per-dab uniforms (`mask_blit`, ...) return
    /// `None`.
    ///
    /// Most pipelines have at most one ring; entries that own multiple
    /// rings (e.g. a terminal that runs both a pickup and a composite
    /// pass with separate uniform layouts) override [`Self::rings`]
    /// instead.
    fn ring(&self) -> Option<&DynamicUniformRing> {
        None
    }
    /// All dynamic-offset uniform rings the registry must coordinate
    /// (reset, overflow-check) for this entry. Default returns the
    /// single [`Self::ring`] wrapped in a `Vec`; override directly when
    /// the entry holds more than one ring.
    fn rings(&self) -> Vec<&DynamicUniformRing> {
        self.ring().into_iter().collect()
    }
}

/// What a brush node module declares to plug a GPU pipeline into the
/// central registry.  See [`crate::brush::node::BrushNodeRegistration`].
#[derive(Clone)]
pub struct BrushPipelineRegistration {
    /// Key used by [`BrushPipelines::get`] and as a debug label.
    pub id: &'static str,
    /// One-shot constructor.  Called once at engine init.
    pub build: fn(&BuildContext) -> Box<dyn BrushPipelineEntry>,
}

/// Brush pipelines that aren't owned by any single node — the
/// commit composite blit, future plumbing — funnel through here so
/// [`BrushPipelines::new`] has a single uniform input alongside
/// `nodes::registrations()`. Adding a future plumbing pipeline means
/// dropping its registration into this list; the harvest loop picks
/// it up automatically.
pub fn plumbing_registrations() -> Vec<BrushPipelineRegistration> {
    vec![crate::brush::composite_pipeline::composite_pipeline_registration()]
}

// ── BrushPipelines: shared infra + plumbing + per-mode registry ──────────

/// Central brush GPU pipeline owner.
///
/// Holds the bind-group layouts and samplers every brush composite
/// shares, the three plumbing pipelines (`blit`, `mask_blit`,
/// `scratch_blit_r8`) that have no owning node, and a typed map of
/// per-mode pipelines harvested from every brush node's
/// [`BrushNodeRegistration::pipelines`](crate::brush::node::BrushNodeRegistration).
///
/// Constructed once at engine init.  See
/// [`crate::engine::DarklyEngine`](crate::engine::DarklyEngine) for the
/// owner.
pub struct BrushPipelines {
    // ── Shared bind-group layouts ────────────────────────────────────
    // `uniform_bgl` is stored alongside the others so per-brush
    // compiled pipelines (built lazily after `new()`) can rebuild
    // their dynamic-uniform bind group against the same layout the
    // shared infra was set up with.  The three BGLs below have
    // external consumers (`Scratch::new`, format-bridging blit-source
    // bind groups, the composite that wants a shared layout for both
    // R8 and RGBA8 variants).
    uniform_bgl: wgpu::BindGroupLayout,
    selection_bgl: wgpu::BindGroupLayout,
    canvas_copy_bgl: wgpu::BindGroupLayout,

    // ── Shared samplers / default bind groups ────────────────────────
    canvas_copy_sampler: wgpu::Sampler,
    /// 1×1 white selection (= fully selected).  Bound when no selection
    /// is active.  `pub` because hot-path call sites take its address
    /// directly via `unwrap_or(&self.brush_pipelines.default_selection_bind_group)`.
    pub default_selection_bind_group: wgpu::BindGroup,

    // ── Plumbing pipelines (no owning node) ──────────────────────────
    blit_pipeline: wgpu::RenderPipeline,
    blit_uniform_ring: DynamicUniformRing,
    /// `pub` for the same direct-address reason as `default_selection_bind_group`.
    pub blit_uniform_bind_group: wgpu::BindGroup,
    mask_blit_pipeline: wgpu::RenderPipeline,
    scratch_blit_r8_pipeline: wgpu::RenderPipeline,

    // ── Per-mode pipelines (modular, looked up by id) ────────────────
    entries: HashMap<&'static str, Box<dyn BrushPipelineEntry>>,

    // ── Shared compiled-brush preview pipeline cache ─────────────────
    /// One cache for every compiled terminal's hover-cursor preview.
    /// Pipelines built lazily, keyed by the brush's `topology_hash`.
    /// See [`PreviewPipelineCache`].
    preview_pipeline_cache: PreviewPipelineCache,
}

impl BrushPipelines {
    /// Build all brush pipelines.
    ///
    /// No canvas dimensions: the read-mirror texture brush composite
    /// shaders sample from lives on [`Scratch`](crate::brush::scratch::Scratch)
    /// (per-stroke, lazy-grown to dab footprint), so engine-init no
    /// longer needs to know the canvas size.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let min_uniform_align = device.limits().min_uniform_buffer_offset_alignment;

        // ── Bind group layouts ──────────────────────────────────────
        // Shared layouts are visible in vertex + fragment AND compute so the
        // dab-batching terminals can reuse them for their
        // uniform and selection bindings without rebuilding parallel BGLs.
        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("brush-uniform-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT | wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let selection_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("brush-selection-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        // Canvas copy: texture + sampler (same structure as a dab texture
        // bind, but distinct BGL so composites can advertise the slot in
        // their layouts independently of the dab pool's layout).
        let canvas_copy_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("brush-canvas-copy-bgl"),
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
            ],
        });

        // ── Default selection (1×1 white = fully selected) ─────────
        let sel_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("brush-default-selection"),
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
                texture: &sel_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255u8],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(1),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let sel_view = sel_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sel_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("brush-selection-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let default_selection_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brush-default-selection-bg"),
            layout: &selection_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&sel_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sel_sampler),
                },
            ],
        });

        // Linear sampler shared by every Scratch's read-mirror bind group
        // and the format-bridging blits.  Linear because liquify reads at
        // displaced sub-pixel UVs.
        let canvas_copy_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("brush-canvas-copy-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // ── Plumbing pipelines (no owning node) ────────────────────

        // Blit: stretch a UV sub-rect of the source across the target viewport.
        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brush-blit"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/brush/blit.wgsl").into(),
            ),
        });
        // `canvas_copy_bgl` is the canonical `texture_2d<f32> + sampler`
        // layout — same shape the old dab-pool BGL had, used here for
        // the blit's source texture binding.
        let blit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("brush-blit-layout"),
            bind_group_layouts: &[&uniform_bgl, &canvas_copy_bgl],
            immediate_size: 0,
        });
        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("brush-blit"),
            layout: Some(&blit_layout),
            vertex: wgpu::VertexState {
                module: &blit_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &blit_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let blit_uniform_ring = DynamicUniformRing::new(
            device,
            "brush-blit-uniforms",
            std::mem::size_of::<BlitUniforms>() as u64,
            min_uniform_align,
        );
        let blit_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brush-blit-uniform-bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &blit_uniform_ring.buffer,
                    offset: 0,
                    size: Some(blit_uniform_ring.binding_size()),
                }),
            }],
        });

        // Mask blit (R8 → RGBA8 broadcast) and Scratch blit R8 (RGBA8 →
        // R8 passthrough) share `mask_blit.wgsl` and a no-uniforms layout
        // — just the source texture at group(0).
        let mask_blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brush-mask-blit"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/brush/mask_blit.wgsl").into(),
            ),
        });
        let mask_blit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("brush-mask-blit-layout"),
            bind_group_layouts: &[&canvas_copy_bgl],
            immediate_size: 0,
        });
        let mask_blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("brush-mask-blit"),
            layout: Some(&mask_blit_layout),
            vertex: wgpu::VertexState {
                module: &mask_blit_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &mask_blit_shader,
                entry_point: Some("fs_broadcast_r"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let scratch_blit_r8_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("brush-scratch-blit-r8"),
                layout: Some(&mask_blit_layout),
                vertex: wgpu::VertexState {
                    module: &mask_blit_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &mask_blit_shader,
                    entry_point: Some("fs_passthrough"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::R8Unorm,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            });

        // ── Per-mode pipelines: harvested from node registrations ──
        let build_ctx = BuildContext {
            device,
            queue,
            uniform_bgl: &uniform_bgl,
            selection_bgl: &selection_bgl,
            canvas_copy_bgl: &canvas_copy_bgl,
            canvas_copy_sampler: &canvas_copy_sampler,
            min_uniform_align,
        };
        let mut entries: HashMap<&'static str, Box<dyn BrushPipelineEntry>> = HashMap::new();
        let pipeline_regs = crate::brush::nodes::registrations()
            .into_iter()
            .flat_map(|node_reg| node_reg.pipelines)
            .chain(plumbing_registrations());
        for pl_reg in pipeline_regs {
            let id = pl_reg.id;
            let prev = entries.insert(id, (pl_reg.build)(&build_ctx));
            debug_assert!(prev.is_none(), "duplicate brush pipeline id: {id}");
        }

        // Shared compiled-brush preview pipeline cache. Owns its own
        // dabs-storage BGL (single-element binding); reuses the
        // already-built `uniform_bgl` at render time via the
        // build_preview_pipeline path.
        let preview_pipeline_cache = PreviewPipelineCache::new(device);

        Self {
            uniform_bgl,
            selection_bgl,
            canvas_copy_bgl,
            canvas_copy_sampler,
            default_selection_bind_group,
            blit_pipeline,
            blit_uniform_ring,
            blit_uniform_bind_group,
            mask_blit_pipeline,
            scratch_blit_r8_pipeline,
            entries,
            preview_pipeline_cache,
        }
    }

    /// BGL used by every per-mode pipeline's dynamic-offset uniform
    /// buffer (group 0). Exposed so per-brush compiled pipelines
    /// built lazily after `BrushPipelines::new` can bind their own
    /// uniform ring against the same layout. See
    /// [`crate::brush::nodes::paint`].
    pub fn uniform_bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.uniform_bgl
    }

    /// Look up a per-mode pipeline by id.  Panics if the id is not
    /// registered or the type doesn't match — both are programming
    /// errors discovered at the first paint.
    pub fn get<P: BrushPipelineEntry>(&self, id: &'static str) -> &P {
        self.entries
            .get(id)
            .unwrap_or_else(|| panic!("brush pipeline not registered: {id}"))
            .as_any()
            .downcast_ref::<P>()
            .unwrap_or_else(|| panic!("brush pipeline {id} downcast failed"))
    }

    // ── Plumbing pipeline accessors ──────────────────────────────────

    pub fn blit_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.blit_pipeline
    }

    /// Write blit uniforms to the next ring slot.  Returns the dynamic
    /// byte offset for `set_bind_group`.
    pub fn write_blit_uniforms(&self, queue: &wgpu::Queue, uniforms: &BlitUniforms) -> u32 {
        self.blit_uniform_ring
            .write(queue, bytemuck::bytes_of(uniforms))
    }

    /// R8 → RGBA8 broadcast pipeline.  Source bind group: single
    /// texture+sampler using `canvas_copy_bgl`.  Used by
    /// `GpuPaintTarget::save_pre_stroke_snapshot` to populate the brush's
    /// RGBA8 pre-stroke snapshot from an R8 mask source.
    pub fn mask_blit_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.mask_blit_pipeline
    }

    /// RGBA8 → R8 passthrough pipeline.  Source bind group: single
    /// texture+sampler using `canvas_copy_bgl`.  Used by
    /// `GpuPaintTarget::commit_scratch_blit` for direct scratch→mask
    /// commits (liquify-style terminals that don't go through the
    /// composite path).
    pub fn scratch_blit_r8_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.scratch_blit_r8_pipeline
    }

    /// Build a one-shot bind group over a single source texture view,
    /// using the canvas-copy BGL (texture + linear sampler).  For
    /// format-bridging blits invoked from `GpuPaintTarget` (`mask_blit`,
    /// `scratch_blit_r8`).  One bind group allocation per stroke — not
    /// per dab.
    pub fn create_blit_source_bind_group(
        &self,
        device: &wgpu::Device,
        source_view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brush-blit-source-bg"),
            layout: &self.canvas_copy_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.canvas_copy_sampler),
                },
            ],
        })
    }

    // ── Shared infra accessors (BGLs and sampler) ───────────────────

    pub fn selection_bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.selection_bgl
    }

    /// BGL used by the per-dab read-mirror bind group on every `Scratch`.
    /// Brush composite pipelines bind a `Scratch::read_mirror_bind_group()`
    /// against this layout.
    pub fn canvas_copy_bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.canvas_copy_bgl
    }

    /// Linear sampler shared by every `Scratch`'s read-mirror bind group.
    pub fn canvas_copy_sampler(&self) -> &wgpu::Sampler {
        &self.canvas_copy_sampler
    }

    /// The 1×1 white selection bind group — bound when no selection is
    /// active.  Exposed for out-of-crate tests that construct a
    /// `BrushGpuContext` manually and need a default selection mask.
    pub fn default_selection_bind_group(&self) -> &wgpu::BindGroup {
        &self.default_selection_bind_group
    }

    // ── Ring coordination ───────────────────────────────────────────

    /// True if any ring is close to capacity.  The caller should flush
    /// the current encoder, reset rings, and create a fresh encoder.
    pub fn rings_nearly_full(&self) -> bool {
        if self.blit_uniform_ring.nearly_full() {
            return true;
        }
        self.entries
            .values()
            .flat_map(|e| e.rings())
            .any(|r| r.nearly_full())
    }

    /// Reset all uniform rings for a new frame.
    pub fn reset_uniform_rings(&self) {
        self.blit_uniform_ring.reset();
        for r in self.entries.values().flat_map(|e| e.rings()) {
            r.reset();
        }
    }

    /// Shared cache of compiled-brush *preview* pipelines, keyed by
    /// `topology_hash`. All four compiled terminals (paint, watercolor,
    /// smudge, liquify) route their hover-cursor preview through this
    /// single cache — preview pipelines look identical across terminals
    /// (single-quad vertex stage, no `@group(2)` selection, no
    /// `@group(3)` terminal bindings, REPLACE blend, `Rgba8Unorm`
    /// target). Their shape depends only on the brush's
    /// `uniform_layout` / `dab_layout`, not on which terminal compiled
    /// the brush.
    pub fn preview_cache(&self) -> &PreviewPipelineCache {
        &self.preview_pipeline_cache
    }

    /// Render the brush's hover-cursor preview via the shared
    /// [`PreviewPipelineCache`]. Builds the per-brush preview pipeline
    /// lazily on first invocation per `compiled.topology_hash`; later
    /// invocations reuse.
    ///
    /// `uniform_bytes` must contain a fully packed `IntrinsicUniforms`
    /// header followed by any node-contributed uniforms in the order
    /// declared by `compiled.uniform_layout`; `dab_bytes` must contain
    /// the intrinsic dab header (`pos`, `bbox_target_px`,
    /// `inv_radius_target_px`) followed by node-contributed dab fields.
    /// Both packers (`pack_intrinsic_dab_header`, `pack_dab_record`)
    /// are shared with the stroke path.
    #[allow(clippy::too_many_arguments)]
    pub fn render_preview(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        compiled: &crate::brush::wgsl_compile::CompiledBrush,
        target_view: &wgpu::TextureView,
        target_size: (u32, u32),
        uniform_bytes: &[u8],
        dab_bytes: &[u8],
    ) {
        let min_align = device.limits().min_uniform_buffer_offset_alignment;
        self.preview_pipeline_cache.with_pipeline(
            device,
            &self.uniform_bgl,
            min_align,
            compiled,
            |pp| {
                pp.render(
                    queue,
                    encoder,
                    target_view,
                    target_size,
                    uniform_bytes,
                    dab_bytes,
                )
            },
        );
    }
}

// ── Compiled-brush preview pipeline cache ─────────────────────────────────

/// One built per-brush preview pipeline. Owns its own uniform ring
/// (per-brush, not shared) plus a single-dab storage buffer + bind
/// group. The terminal's `render_preview` writes one dab record and
/// one uniform block per hover refresh; the ring's leftover capacity
/// is irrelevant because every preview reset reset()s before writing.
pub struct PreviewPipeline {
    pipeline: wgpu::RenderPipeline,
    uniform_ring: DynamicUniformRing,
    uniform_bind_group: wgpu::BindGroup,
    dabs_buffer: wgpu::Buffer,
    dabs_bind_group: wgpu::BindGroup,
    /// Total uniform-block size for this brush — intrinsic header +
    /// node-contributed uniforms, rounded to the ring's alignment.
    uniform_size: usize,
}

impl PreviewPipeline {
    /// Render-pass driver. Resets the ring, writes the uniform block
    /// and the single dab record, encodes one render pass that
    /// clear-loads the target view and draws the preview's single
    /// quad. Caller is responsible for packing
    /// `uniform_bytes` (intrinsic + node uniforms) and `dab_bytes`
    /// (intrinsic header + node-contributed dab record).
    pub fn render(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target_view: &wgpu::TextureView,
        target_size: (u32, u32),
        uniform_bytes: &[u8],
        dab_bytes: &[u8],
    ) {
        // Pad uniform bytes up to the ring's binding size — the dynamic
        // offset bind group's `binding_size` is the ring stride, so
        // writes shorter than the stride leave the tail of the ring
        // slot whatever it had before. Zeroing keeps reads of node-
        // contributed fields deterministic when the brush has no
        // uniforms.
        let mut bytes = Vec::from(uniform_bytes);
        if bytes.len() < self.uniform_size {
            bytes.resize(self.uniform_size, 0);
        }
        self.uniform_ring.reset();
        let uniform_offset = self.uniform_ring.write(queue, &bytes);
        queue.write_buffer(&self.dabs_buffer, 0, dab_bytes);

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("brush-preview"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_viewport(
            0.0,
            0.0,
            target_size.0 as f32,
            target_size.1 as f32,
            0.0,
            1.0,
        );
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.uniform_bind_group, &[uniform_offset]);
        pass.set_bind_group(1, &self.dabs_bind_group, &[]);
        pass.draw(0..6, 0..1);
    }
}

/// Shared cache of preview pipelines for compiled brushes — see
/// [`BrushPipelines::preview_cache`].
///
/// Pipelines are built on demand. The cache key is `topology_hash`;
/// two brushes that compile to identical dab/uniform layouts share
/// one entry. The pipeline shape is derived entirely from
/// `CompiledBrush::preview_wgsl`, `dab_layout`, and `uniform_layout`
/// — independent of which terminal compiled the brush.
pub struct PreviewPipelineCache {
    pipelines: std::cell::RefCell<HashMap<u64, PreviewPipeline>>,
    dabs_bgl: wgpu::BindGroupLayout,
}

impl PreviewPipelineCache {
    fn new(device: &wgpu::Device) -> Self {
        let dabs_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("brush-preview-dabs-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        Self {
            pipelines: std::cell::RefCell::new(HashMap::new()),
            dabs_bgl,
        }
    }

    /// Build (or look up) the preview pipeline for `compiled`. Invoke a
    /// closure with it. The cache is built lazily on first call for a
    /// given `topology_hash`; later calls reuse. Per-brush double
    /// compile cost (stroke + preview WGSL) lives at brush *load*
    /// time, not preview time.
    pub fn with_pipeline<R>(
        &self,
        device: &wgpu::Device,
        uniform_bgl: &wgpu::BindGroupLayout,
        min_uniform_align: u32,
        compiled: &crate::brush::wgsl_compile::CompiledBrush,
        f: impl FnOnce(&PreviewPipeline) -> R,
    ) -> R {
        let key = compiled.topology_hash;
        let mut pipelines = self.pipelines.borrow_mut();
        let entry = pipelines.entry(key).or_insert_with(|| {
            build_preview_pipeline(
                device,
                uniform_bgl,
                &self.dabs_bgl,
                min_uniform_align,
                compiled,
            )
        });
        f(entry)
    }
}

fn build_preview_pipeline(
    device: &wgpu::Device,
    uniform_bgl: &wgpu::BindGroupLayout,
    dabs_bgl: &wgpu::BindGroupLayout,
    min_uniform_align: u32,
    compiled: &crate::brush::wgsl_compile::CompiledBrush,
) -> PreviewPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("brush-preview-shader"),
        source: wgpu::ShaderSource::Wgsl(compiled.preview_wgsl.clone().into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("brush-preview-layout"),
        bind_group_layouts: &[uniform_bgl, dabs_bgl],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("brush-preview"),
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
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: Some(wgpu::BlendState::REPLACE),
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

    let uniform_size = (crate::brush::wgsl_compile::INTRINSIC_UNIFORMS_SIZE
        + compiled.uniform_size)
        .max(crate::brush::wgsl_compile::INTRINSIC_UNIFORMS_SIZE);
    let uniform_ring = DynamicUniformRing::new(
        device,
        "brush-preview-uniforms",
        uniform_size as u64,
        min_uniform_align,
    );
    let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("brush-preview-uniform-bg"),
        layout: uniform_bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &uniform_ring.buffer,
                offset: 0,
                size: Some(uniform_ring.binding_size()),
            }),
        }],
    });

    // Single-dab storage. The preview draws one quad with one record;
    // the buffer's only sized to that record. `max(16)` mirrors the
    // safety floor every stroke pipeline uses (zero-sized buffers
    // aren't valid).
    let dab_record_size = compiled.dab_record_size.max(16);
    let dabs_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("brush-preview-dabs-buffer"),
        size: dab_record_size as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let dabs_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("brush-preview-dabs-bg"),
        layout: dabs_bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: dabs_buffer.as_entire_binding(),
        }],
    });

    PreviewPipeline {
        pipeline,
        uniform_ring,
        uniform_bind_group,
        dabs_buffer,
        dabs_bind_group,
        uniform_size: align_up(uniform_size as u64, min_uniform_align as u64) as usize,
    }
}
