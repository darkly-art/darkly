//! Pre-built GPU pipelines for the brush system.
//!
//! Three pipelines:
//! - **Circle**: renders an SDF circle mask to a dab texture (REPLACE blend).
//! - **Stamp**: renders a brush tip texture to a dab texture with transforms.
//! - **Composite**: composites a dab texture onto the canvas with correct
//!   straight-alpha Porter-Duff source-over (REPLACE blend, shader-side composite).
//!
//! The composite pass reads a copy of the canvas region (captured before
//! compositing) so the shader can do manual Porter-Duff blending.  This avoids
//! the premultiplied-stored-as-straight bug that hardware alpha blending causes
//! on straight-alpha layer textures (see compositing-lessons-learned.md #2).
//!
//! Separate from `PaintPipelines` — different concerns (dab generation +
//! dab compositing vs. SDF circle painting + gradient fill).

use std::cell::Cell;
use std::num::NonZeroU64;

/// Uniform data for the circle mask generation shader.
///
/// Carries the algorithm choice (sine harmonic / 1D Perlin / Gielis
/// superformula), all per-algorithm shape parameters, and the CPU-computed
/// centroid offset that anchors the rendered shape's geometric centroid at
/// the texture centre — see [`crate::brush::nodes::circle`].
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CircleUniforms {
    pub softness: f32,    // 0-1 fraction of base radius
    pub algorithm: u32,   // 0 = sine harmonic, 1 = perlin/value-noise, 2 = superformula
    pub amplitude: f32,   // bump amplitude (sine, perlin) — fraction of base radius
    pub frequency: f32,   // bump count (sine.n, perlin.f, superformula.m)
    pub phase: f32,       // rotation in radians applied before r(θ) sample
    pub persistence: f32, // perlin: per-octave amplitude falloff
    pub seed: f32,        // perlin: rng seed
    pub octaves: u32,     // perlin: stacked frequency count
    pub n1: f32,          // superformula: overall sharpness
    pub n2: f32,          // superformula: bump rise
    pub n3: f32,          // superformula: bump fall
    pub base_radius: f32, // shrink factor so r_max stays inside the viewport
    pub centroid_x: f32,  // shape centroid in viewport-radius units
    pub centroid_y: f32,
    pub _pad: [f32; 2], // pad to 16-byte alignment
}

/// Uniform data for the stamp dab generation shader.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct StampUniforms {
    pub dab_width: f32,   // dab viewport width in pixels
    pub dab_height: f32,  // dab viewport height in pixels
    pub opacity: f32,     // dab opacity (0-1)
    pub rotation: f32,    // dab rotation in radians
    pub color: [f32; 4],  // RGBA paint color (straight alpha)
    pub mirror_x: f32,    // 1.0 = flip horizontally
    pub mirror_y: f32,    // 1.0 = flip vertically
    pub application: u32, // BrushTipApplication as u32
    pub ratio: f32,       // user-controlled aspect ratio squeeze (1.0 = none)
}

/// Uniform data for the texture overlay shader.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TexOverlayUniforms {
    pub dab_width: f32,
    pub dab_height: f32,
    pub position_x: f32,
    pub position_y: f32,
    pub pattern_width: f32,
    pub pattern_height: f32,
    pub scale: f32,
    pub strength: f32,
    pub blend_mode: u32,
    pub _pad: [f32; 3],
}

/// Uniform data for the blit shader (preview mask blit).
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BlitUniforms {
    /// UV corner (0..1) inside the source texture where sampling starts.
    pub uv_min: [f32; 2],
    /// UV corner (0..1) inside the source texture where sampling ends.
    pub uv_max: [f32; 2],
}

/// Uniform data for the liquify warp shader.
///
/// The shader samples `scratch read mirror` (a copy of the stroke scratch) at a
/// displaced UV inside a circular brush disc and writes the warped sample
/// back to the scratch. Everything is canvas-space; the shader converts to
/// UVs via `canvas_size` and `copy_origin`.
///
/// Per-dab displacement magnitude is decided on the CPU (strength × radius)
/// and passed as `displacement`; the shader just multiplies by a unit
/// direction vector and the radial falloff. Pen speed never enters the
/// equation — a slow drag produces the same per-dab warp as a fast flick.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LiquifyUniforms {
    /// Top-left of the render-pass quad in canvas pixels (clamped to the
    /// **layer's** canvas extent so paste-extent / grown layers can warp
    /// off-canvas pixels).
    pub rect_origin: [f32; 2],
    /// Width and height of the render-pass quad in canvas pixels.
    pub rect_size: [f32; 2],
    /// Layer's canvas-space offset (= GpuPaintTarget.offset_x/y). Vertex
    /// stage subtracts this from canvas_pos before the NDC divide so the
    /// quad maps onto the layer-sized scratch render target correctly.
    pub target_offset: [f32; 2],
    /// Layer pixel dimensions (= GpuPaintTarget.width/height). Used by
    /// the vertex stage as the NDC denominator.
    pub target_size: [f32; 2],
    /// Document canvas dimensions (fragment-stage selection UV only —
    /// the selection texture is canvas-sized).
    pub canvas_size: [f32; 2],
    /// Layer-local origin of the scratch read mirror region (matches the
    /// `ensure_canvas_copy` source origin). The fragment shader floors
    /// this before dividing to recover the texel coordinate, same
    /// floor-then-ceil pattern as `composite.wgsl`.
    pub copy_origin: [f32; 2],
    /// Brush centre in canvas pixels.
    pub center: [f32; 2],
    /// Unit direction vector (cos θ, sin θ). Pixels sampled from
    /// `canvas_pos − direction × displacement × falloff`.
    pub direction: [f32; 2],
    /// Displacement magnitude in canvas pixels at the brush centre
    /// (where falloff = 1). Computed as `radius × K × strength`.
    pub displacement: f32,
    /// Brush radius in canvas pixels.
    pub radius: f32,
    /// Waveshape knob (0–1). 0 = saw, 0.5 = sine, 1 = square.
    pub softness: f32,
    pub _pad: f32,
}

/// Uniform data for the watercolor pickup shader.
///
/// Drives one render pass per watercolor dab that averages canvas_copy
/// under the brush footprint (alpha-weighted RGB, unweighted alpha) into
/// a 1×1 RGBA8 pickup texture. Each dab is independent — no cross-dab
/// carry; every dab samples the canvas afresh.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WatercolorPickupUniforms {
    pub center: [f32; 2],              // brush centre in canvas pixels
    pub copy_origin: [f32; 2], // top-left of the valid scratch-mirror region (canvas pixels)
    pub scratch_mirror_size: [f32; 2], // scratch mirror texture dimensions
    pub half_extent: [f32; 2], // half the dab footprint (canvas pixels) per axis
}

/// Uniform data for the watercolor compositing shader.
///
/// Same shape as `CompositeUniforms` minus the per-dab `blend_mode` and
/// `fg_premultiplied` knobs (watercolor is always source-over with a
/// premultiplied dab), plus `paint_color` and `deposit` — the two new
/// quantities the watercolor blend reads on top of the standard composite
/// inputs.
///
/// `paint_color` is first because vec4 needs 16-byte alignment in WGSL.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WatercolorCompositeUniforms {
    pub paint_color: [f32; 4], // straight-alpha paint color (rgb used; alpha via dab.a)
    pub origin: [f32; 2],      // quad top-left in canvas pixels
    pub size: [f32; 2],        // quad size in canvas pixels
    pub target_offset: [f32; 2], // canvas-space offset of render target's (0,0) pixel
    pub target_size: [f32; 2], // render target pixel dimensions (vertex NDC)
    pub canvas_size: [f32; 2], // document canvas dimensions (fragment selection UV)
    pub uv_min: [f32; 2],      // min UV in dab texture (nonzero when clipped at top/left)
    pub uv_max: [f32; 2],      // max UV in dab texture
    pub deposit: f32,          // paint↔pickup mix ratio (0 = pure pickup, 1 = pure paint)
    pub wetness: f32,          // smudge intensity (0 = dry brush, 1 = full smudge)
    pub stroke_opacity: f32,   // per-stroke opacity cap (1.0 = no cap)
    pub apply_selection: u32,  // 1 = modulate fg by selection, 0 = ignore (commit pass)
}

/// Uniform data for the dab compositing shader.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CompositeUniforms {
    pub origin: [f32; 2],        // quad top-left in canvas pixels
    pub size: [f32; 2],          // quad size in canvas pixels
    pub target_offset: [f32; 2], // canvas-space offset of render target's (0,0) pixel
    pub target_size: [f32; 2],   // render target pixel dimensions (vertex NDC)
    pub canvas_size: [f32; 2],   // document canvas dimensions (fragment selection UV)
    pub uv_min: [f32; 2],        // min UV in dab texture (nonzero when clipped at top/left)
    pub uv_max: [f32; 2],        // max UV in dab texture
    pub blend_mode: u32,         // 0 = source-over, 1 = erase (destination-out)
    pub fg_premultiplied: u32,   // 1 = dab input is premultiplied, 0 = straight alpha
    pub stroke_opacity: f32,     // stroke-level opacity cap (1.0 = no cap)
    pub apply_selection: u32,    // 1 = modulate by selection, 0 = ignore (commit pass)
}

/// Ring buffer for dynamic uniform offsets.
///
/// Instead of a single uniform buffer that must be submitted between dabs,
/// each dab writes to a unique offset.  All render passes can go into one
/// command encoder and be submitted once.
///
/// Uses `Cell` for `next_index` so `write()` can take `&self` — the ring is
/// never shared across threads.
const UNIFORM_RING_CAPACITY: u32 = 256;

pub struct DynamicUniformRing {
    buffer: wgpu::Buffer,
    aligned_stride: u64,
    capacity: u32,
    next_index: Cell<u32>,
}

impl DynamicUniformRing {
    fn new(device: &wgpu::Device, label: &str, uniform_size: u64, min_alignment: u32) -> Self {
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

    fn nearly_full(&self) -> bool {
        // Leave headroom for a few more writes after the check (one dab
        // can use up to 3 ring slots across different pipelines).
        self.next_index.get() >= self.capacity - 4
    }

    /// Binding size for the bind group entry (one slot, not the whole buffer).
    fn binding_size(&self) -> NonZeroU64 {
        NonZeroU64::new(self.aligned_stride).unwrap()
    }
}

fn align_up(value: u64, alignment: u64) -> u64 {
    (value + alignment - 1) & !(alignment - 1)
}

/// Pre-built render pipelines for the brush system.
pub struct BrushPipelines {
    circle_pipeline: wgpu::RenderPipeline,
    stamp_pipeline: wgpu::RenderPipeline,
    tex_overlay_pipeline: wgpu::RenderPipeline,
    /// Composite pipeline targeting `Rgba8Unorm` (per-dab into stroke scratch,
    /// and stroke→layer commit when the destination layer is RGBA8).
    composite_pipeline_rgba: wgpu::RenderPipeline,
    /// Composite pipeline targeting `R8Unorm` — used for stroke→mask commits.
    /// Same WGSL as the RGBA variant; the GPU writes only the `.r` channel
    /// of the fragment output to the R8 target. This keeps brush logic format-
    /// agnostic — terminals look up the pipeline by destination format and
    /// never branch on R8 vs RGBA8.
    composite_pipeline_r8: wgpu::RenderPipeline,
    /// "Mask blit" pipeline: samples a single-channel R8 source and broadcasts
    /// `(r, r, r, 1)` into an RGBA8 destination. Used by
    /// `GpuPaintTarget::save_pre_stroke_snapshot` to bridge R8 mask sources
    /// into the brush stack's RGBA8 pre-stroke snapshot. Replaces the
    /// `copy_texture_to_texture(R8 → RGBA8)` that wgpu rejects as a format
    /// mismatch.
    mask_blit_pipeline: wgpu::RenderPipeline,
    /// "Scratch blit" pipeline: passes RGBA8 scratch through to an R8 mask
    /// destination. Used by `GpuPaintTarget::commit_scratch_blit` to bridge
    /// liquify-style direct scratch→layer commits when the destination is a
    /// mask. The fragment writes `vec4<f32>` and the GPU drops G/B/A.
    scratch_blit_r8_pipeline: wgpu::RenderPipeline,

    circle_uniform_ring: DynamicUniformRing,
    pub(crate) circle_uniform_bind_group: wgpu::BindGroup,

    stamp_uniform_ring: DynamicUniformRing,
    pub(crate) stamp_uniform_bind_group: wgpu::BindGroup,

    tex_overlay_uniform_ring: DynamicUniformRing,
    pub(crate) tex_overlay_uniform_bind_group: wgpu::BindGroup,

    composite_uniform_ring: DynamicUniformRing,
    pub(crate) composite_uniform_bind_group: wgpu::BindGroup,

    blit_pipeline: wgpu::RenderPipeline,
    blit_uniform_ring: DynamicUniformRing,
    pub(crate) blit_uniform_bind_group: wgpu::BindGroup,

    liquify_pipeline: wgpu::RenderPipeline,
    liquify_uniform_ring: DynamicUniformRing,
    pub(crate) liquify_uniform_bind_group: wgpu::BindGroup,

    /// Pickup pass: alpha-weighted average of canvas_copy under the brush
    /// footprint, written to a 1×1 RGBA8 pickup texture. The composite
    /// pass samples this single texel so every fragment of the dab reads
    /// the same colour. Each dab is independent.
    watercolor_pickup_pipeline: wgpu::RenderPipeline,
    watercolor_pickup_uniform_ring: DynamicUniformRing,
    pub(crate) watercolor_pickup_uniform_bind_group: wgpu::BindGroup,

    /// 1×1 RGBA8 pickup texture. Allocated once at engine startup and
    /// reused per dab — the pickup pass overwrites the single texel.
    _watercolor_pickup_texture: wgpu::Texture,
    /// Sampled-side view of the 1×1 pickup texture, embedded by every
    /// `Scratch` in its `watercolor_sources_bind_group` at binding 2.
    /// Static across strokes — only the read-mirror side of the bind
    /// group needs rebuilding when the mirror grows.
    watercolor_pickup_view: wgpu::TextureView,
    /// Render-attachment view of the pickup texture. The pickup pass
    /// writes one fragment here per dab.
    watercolor_pickup_attachment_view: wgpu::TextureView,

    /// Watercolor composite pipeline. Always targets RGBA8 stroke scratch —
    /// stroke→layer commits go through the standard `composite_pipeline`,
    /// so no R8 variant is needed.
    watercolor_composite_pipeline: wgpu::RenderPipeline,
    watercolor_composite_uniform_ring: DynamicUniformRing,
    pub(crate) watercolor_composite_uniform_bind_group: wgpu::BindGroup,

    /// 1x1 white selection texture — bound when no selection is active.
    pub(crate) default_selection_bind_group: wgpu::BindGroup,
    pub(crate) selection_bgl: wgpu::BindGroupLayout,

    /// BGL for the per-dab read mirror (texture+sampler).  Bound by every
    /// brush composite pipeline (`composite.wgsl`,
    /// `watercolor_composite.wgsl`, `liquify.wgsl`).  The actual texture
    /// and bind group live on `Scratch` (stroke-scoped, lazy-grown to dab
    /// footprint) — `BrushPipelines` only holds the size-agnostic layout
    /// + sampler that all `Scratch` instances reuse.
    canvas_copy_bgl: wgpu::BindGroupLayout,
    /// BGL for the combined watercolor sources bind group (read mirror +
    /// sampler at 0/1, pickup at 2).  Same story as `canvas_copy_bgl` —
    /// layout lives here, the bind group lives on `Scratch`.
    watercolor_sources_bgl: wgpu::BindGroupLayout,
    /// Linear sampler shared by every `Scratch`'s read-mirror bind group
    /// and the format-bridging blits (`mask_blit`, `scratch_blit_r8`).
    /// Linear because liquify reads the read mirror at displaced sub-
    /// pixel UVs and would otherwise produce blocky warp output.
    canvas_copy_sampler: wgpu::Sampler,
}

impl BrushPipelines {
    /// Create brush pipelines.
    ///
    /// `dab_bgl` is the dab texture bind group layout from `DabTexturePool`.
    /// No canvas dimensions: the read-mirror texture that brush composite
    /// shaders sample from now lives on `Scratch` (per-stroke, lazy-grown
    /// to dab footprint), so engine-init no longer needs to know the canvas
    /// size.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        dab_bgl: &wgpu::BindGroupLayout,
    ) -> Self {
        // --- Shaders ---
        let circle_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brush-circle"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/brush/circle.wgsl").into(),
            ),
        });

        let stamp_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brush-stamp"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/brush/stamp.wgsl").into(),
            ),
        });

        let tex_overlay_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brush-tex-overlay"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/brush/texture_overlay.wgsl").into(),
            ),
        });

        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brush-composite"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("../../../../shaders/source_over.wgsl"),
                    "\n",
                    include_str!("../../../../shaders/brush/composite.wgsl"),
                )
                .into(),
            ),
        });

        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brush-blit"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/brush/blit.wgsl").into(),
            ),
        });

        let liquify_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brush-liquify"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/brush/liquify.wgsl").into(),
            ),
        });

        let watercolor_pickup_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brush-watercolor-pickup"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/brush/watercolor_pickup.wgsl").into(),
            ),
        });

        let watercolor_composite_shader =
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("brush-watercolor-composite"),
                source: wgpu::ShaderSource::Wgsl(
                    concat!(
                        include_str!("../../../../shaders/source_over.wgsl"),
                        "\n",
                        include_str!("../../../../shaders/brush/watercolor_composite.wgsl"),
                    )
                    .into(),
                ),
            });

        let mask_blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brush-mask-blit"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/brush/mask_blit.wgsl").into(),
            ),
        });

        // --- Bind group layouts ---
        let min_align = device.limits().min_uniform_buffer_offset_alignment;

        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("brush-uniform-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
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

        // Canvas copy bind group layout (texture + sampler, same structure as dab).
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

        // --- Pipeline layouts ---
        // Circle: group(0) = uniforms only (renders to dab texture).
        let circle_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("brush-circle-layout"),
            bind_group_layouts: &[&uniform_bgl],
            immediate_size: 0,
        });

        // Stamp: group(0) = uniforms, group(1) = brush tip texture+sampler.
        let stamp_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("brush-stamp-layout"),
            bind_group_layouts: &[&uniform_bgl, dab_bgl],
            immediate_size: 0,
        });

        // Texture overlay: group(0) = uniforms, group(1) = dab texture, group(2) = pattern texture.
        let tex_overlay_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("brush-tex-overlay-layout"),
            bind_group_layouts: &[&uniform_bgl, dab_bgl, dab_bgl],
            immediate_size: 0,
        });

        // Composite: group(0) = uniforms, group(1) = dab texture, group(2) = selection,
        //            group(3) = canvas copy (for shader-side Porter-Duff).
        let composite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("brush-composite-layout"),
            bind_group_layouts: &[&uniform_bgl, dab_bgl, &selection_bgl, &canvas_copy_bgl],
            immediate_size: 0,
        });

        // Blit: group(0) = uniforms, group(1) = source texture+sampler.
        let blit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("brush-blit-layout"),
            bind_group_layouts: &[&uniform_bgl, dab_bgl],
            immediate_size: 0,
        });

        // Liquify: group(0) = uniforms, group(1) = selection mask,
        //          group(2) = canvas copy (sampled at displaced UV — linear).
        // No dab texture — the warp reads from canvas_copy and writes to the
        // scratch render target, producing the new canvas state directly.
        let liquify_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("brush-liquify-layout"),
            bind_group_layouts: &[&uniform_bgl, &selection_bgl, &canvas_copy_bgl],
            immediate_size: 0,
        });

        // Watercolor sources BGL: canvas_copy (texture+sampler at 0/1) plus
        // a 1×1 carried-pickup texture at 2 (no sampler — shader uses
        // `textureLoad`). Same shape works for both passes: the pickup pass
        // reads `prev_carried` from slot 2; the composite reads
        // `curr_carried` from slot 2. Packed into one BGL because WebGPU
        // caps `max_bind_groups` at 4.
        let watercolor_sources_bgl =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("brush-watercolor-sources-bgl"),
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
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                ],
            });

        // Watercolor pickup: group(0) = uniforms, group(1) = canvas copy.
        // Renders to a 1×1 RGBA8 pickup texture; one fragment computes
        // the alpha-weighted average of canvas_copy across the footprint.
        let watercolor_pickup_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("brush-watercolor-pickup-layout"),
                bind_group_layouts: &[&uniform_bgl, &canvas_copy_bgl],
                immediate_size: 0,
            });

        // Watercolor composite: group(0) = uniforms, group(1) = dab,
        //                       group(2) = selection,
        //                       group(3) = sources (canvas_copy + pickup).
        let watercolor_composite_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("brush-watercolor-composite-layout"),
                bind_group_layouts: &[
                    &uniform_bgl,
                    dab_bgl,
                    &selection_bgl,
                    &watercolor_sources_bgl,
                ],
                immediate_size: 0,
            });

        // Mask blit: group(0) = source texture+sampler. No uniforms — the
        // shader is a fullscreen triangle that always covers the whole
        // destination viewport.
        let mask_blit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("brush-mask-blit-layout"),
            bind_group_layouts: &[&canvas_copy_bgl],
            immediate_size: 0,
        });

        // --- Dynamic uniform rings ---
        let circle_uniform_ring = DynamicUniformRing::new(
            device,
            "brush-circle-uniforms",
            std::mem::size_of::<CircleUniforms>() as u64,
            min_align,
        );
        let circle_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brush-circle-uniform-bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &circle_uniform_ring.buffer,
                    offset: 0,
                    size: Some(circle_uniform_ring.binding_size()),
                }),
            }],
        });

        let stamp_uniform_ring = DynamicUniformRing::new(
            device,
            "brush-stamp-uniforms",
            std::mem::size_of::<StampUniforms>() as u64,
            min_align,
        );
        let stamp_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brush-stamp-uniform-bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &stamp_uniform_ring.buffer,
                    offset: 0,
                    size: Some(stamp_uniform_ring.binding_size()),
                }),
            }],
        });

        let tex_overlay_uniform_ring = DynamicUniformRing::new(
            device,
            "brush-tex-overlay-uniforms",
            std::mem::size_of::<TexOverlayUniforms>() as u64,
            min_align,
        );
        let tex_overlay_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brush-tex-overlay-uniform-bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &tex_overlay_uniform_ring.buffer,
                    offset: 0,
                    size: Some(tex_overlay_uniform_ring.binding_size()),
                }),
            }],
        });

        let composite_uniform_ring = DynamicUniformRing::new(
            device,
            "brush-composite-uniforms",
            std::mem::size_of::<CompositeUniforms>() as u64,
            min_align,
        );
        let composite_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brush-composite-uniform-bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &composite_uniform_ring.buffer,
                    offset: 0,
                    size: Some(composite_uniform_ring.binding_size()),
                }),
            }],
        });

        let blit_uniform_ring = DynamicUniformRing::new(
            device,
            "brush-blit-uniforms",
            std::mem::size_of::<BlitUniforms>() as u64,
            min_align,
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

        let liquify_uniform_ring = DynamicUniformRing::new(
            device,
            "brush-liquify-uniforms",
            std::mem::size_of::<LiquifyUniforms>() as u64,
            min_align,
        );
        let liquify_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brush-liquify-uniform-bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &liquify_uniform_ring.buffer,
                    offset: 0,
                    size: Some(liquify_uniform_ring.binding_size()),
                }),
            }],
        });

        let watercolor_pickup_uniform_ring = DynamicUniformRing::new(
            device,
            "brush-watercolor-pickup-uniforms",
            std::mem::size_of::<WatercolorPickupUniforms>() as u64,
            min_align,
        );
        let watercolor_pickup_uniform_bind_group =
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("brush-watercolor-pickup-uniform-bg"),
                layout: &uniform_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &watercolor_pickup_uniform_ring.buffer,
                        offset: 0,
                        size: Some(watercolor_pickup_uniform_ring.binding_size()),
                    }),
                }],
            });

        let watercolor_composite_uniform_ring = DynamicUniformRing::new(
            device,
            "brush-watercolor-composite-uniforms",
            std::mem::size_of::<WatercolorCompositeUniforms>() as u64,
            min_align,
        );
        let watercolor_composite_uniform_bind_group =
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("brush-watercolor-composite-uniform-bg"),
                layout: &uniform_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &watercolor_composite_uniform_ring.buffer,
                        offset: 0,
                        size: Some(watercolor_composite_uniform_ring.binding_size()),
                    }),
                }],
            });

        // --- Default selection (1x1 white = fully selected) ---
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

        // Linear sampler shared by every `Scratch`'s read-mirror bind
        // group and the format-bridging blits.  Linear because liquify
        // reads at displaced sub-pixel UVs.
        let canvas_copy_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("brush-canvas-copy-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // --- Watercolor pickup texture (1×1 RGBA8) ---
        // Each dab is independent — the pickup pass overwrites this single
        // texel with the alpha-weighted canvas average of the brush's
        // footprint, and the composite pass reads it. No cross-dab carry,
        // no ping-pong.
        let watercolor_pickup_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("brush-watercolor-pickup"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let watercolor_pickup_view =
            watercolor_pickup_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let watercolor_pickup_attachment_view =
            watercolor_pickup_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // --- Pipelines ---

        // Circle: REPLACE blend — we clear the dab texture and write the SDF mask.
        let circle_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("brush-circle"),
            layout: Some(&circle_layout),
            vertex: wgpu::VertexState {
                module: &circle_shader,
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
                module: &circle_shader,
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

        // Stamp: REPLACE blend — clear dab texture and stamp the tip image.
        let stamp_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("brush-stamp"),
            layout: Some(&stamp_layout),
            vertex: wgpu::VertexState {
                module: &stamp_shader,
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
                module: &stamp_shader,
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

        // Texture overlay: REPLACE blend — reads dab + pattern, writes textured dab.
        let tex_overlay_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("brush-tex-overlay"),
            layout: Some(&tex_overlay_layout),
            vertex: wgpu::VertexState {
                module: &tex_overlay_shader,
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
                module: &tex_overlay_shader,
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

        // Composite: REPLACE blend — the shader does Porter-Duff source-over
        // manually by reading the canvas copy, producing correct straight-alpha output.
        // Built once per supported destination format (Rgba8Unorm and R8Unorm).
        // Identical WGSL; the GPU silently writes only `.r` to R8 targets.
        let make_composite_pipeline = |format: wgpu::TextureFormat, label: &'static str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&composite_layout),
                vertex: wgpu::VertexState {
                    module: &composite_shader,
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
                    module: &composite_shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let composite_pipeline_rgba =
            make_composite_pipeline(wgpu::TextureFormat::Rgba8Unorm, "brush-composite-rgba");
        let composite_pipeline_r8 =
            make_composite_pipeline(wgpu::TextureFormat::R8Unorm, "brush-composite-r8");

        // Blit: stretch a UV sub-rect of the source across the target viewport.
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

        // Mask blit: R8 source → RGBA8 destination, broadcasting `.r` to all
        // channels. Used by `GpuPaintTarget::save_pre_stroke_snapshot` when the
        // paint target is an R8 mask, replacing the format-mismatched
        // `copy_texture_to_texture` call in `StrokeBuffer::save_pre_stroke`.
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

        // Scratch-blit-R8: RGBA8 source (the brush stack's stroke scratch) →
        // R8 destination (a mask). Used by `GpuPaintTarget::commit_scratch_blit`
        // for liquify-style direct scratch→layer commits when the destination
        // is a mask. The shader returns `vec4<f32>`; the GPU writes only `.r`
        // to the R8 target.
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

        // Liquify: REPLACE blend — the shader reads canvas_copy at a displaced
        // UV and writes the result straight into the scratch render target.
        // No alpha blending needed; each fragment either outputs a warped
        // sample or (outside the disc) discards, leaving the scratch's prior
        // content (which LoadOp::Load preserved).
        let liquify_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("brush-liquify"),
            layout: Some(&liquify_layout),
            vertex: wgpu::VertexState {
                module: &liquify_shader,
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
                module: &liquify_shader,
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

        // Watercolor pickup: REPLACE blend; targets a 1×1 RGBA8 texture.
        let watercolor_pickup_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("brush-watercolor-pickup"),
                layout: Some(&watercolor_pickup_layout),
                vertex: wgpu::VertexState {
                    module: &watercolor_pickup_shader,
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
                    module: &watercolor_pickup_shader,
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

        // Watercolor composite: REPLACE blend (shader-side Porter-Duff,
        // identical pattern to the standard composite). Always targets
        // RGBA8 stroke scratch — stroke→layer commits go through the shared
        // composite pipeline, so no R8 variant is needed here.
        let watercolor_composite_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("brush-watercolor-composite"),
                layout: Some(&watercolor_composite_layout),
                vertex: wgpu::VertexState {
                    module: &watercolor_composite_shader,
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
                    module: &watercolor_composite_shader,
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

        Self {
            circle_pipeline,
            stamp_pipeline,
            tex_overlay_pipeline,
            composite_pipeline_rgba,
            composite_pipeline_r8,
            mask_blit_pipeline,
            scratch_blit_r8_pipeline,
            circle_uniform_ring,
            circle_uniform_bind_group,
            stamp_uniform_ring,
            stamp_uniform_bind_group,
            tex_overlay_uniform_ring,
            tex_overlay_uniform_bind_group,
            composite_uniform_ring,
            composite_uniform_bind_group,
            blit_pipeline,
            blit_uniform_ring,
            blit_uniform_bind_group,
            liquify_pipeline,
            liquify_uniform_ring,
            liquify_uniform_bind_group,
            watercolor_pickup_pipeline,
            watercolor_pickup_uniform_ring,
            watercolor_pickup_uniform_bind_group,
            _watercolor_pickup_texture: watercolor_pickup_texture,
            watercolor_pickup_view,
            watercolor_pickup_attachment_view,
            watercolor_composite_pipeline,
            watercolor_composite_uniform_ring,
            watercolor_composite_uniform_bind_group,
            default_selection_bind_group,
            selection_bgl,
            canvas_copy_bgl,
            watercolor_sources_bgl,
            canvas_copy_sampler,
        }
    }

    /// Build a one-shot bind group over a single source texture view, using
    /// the canvas-copy BGL (texture + linear sampler). For format-bridging
    /// blits invoked from `GpuPaintTarget` (`mask_blit`, `scratch_blit_r8`).
    /// One bind group allocation per stroke — not per dab.
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

    pub fn circle_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.circle_pipeline
    }

    pub fn stamp_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.stamp_pipeline
    }

    pub fn tex_overlay_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.tex_overlay_pipeline
    }

    /// Look up the brush composite pipeline for a destination format.
    ///
    /// Stroke scratch composites (per-dab into the RGBA8 stroke scratch) hit
    /// the RGBA variant. Stroke→layer commits hit the variant matching the
    /// layer's storage format — RGBA8 for raster layers, R8 for masks. Both
    /// pipelines are built from the same WGSL; the GPU silently writes only
    /// `.r` to R8 targets, so the brush stack stays format-agnostic.
    ///
    /// Used by `GpuPaintTarget::commit_brush_dab` (the format-aware brush
    /// commit). Not for direct call by terminals.
    pub fn composite_pipeline(&self, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        match format {
            wgpu::TextureFormat::R8Unorm => &self.composite_pipeline_r8,
            _ => &self.composite_pipeline_rgba,
        }
    }

    /// R8 → RGBA8 broadcast pipeline. Source bind group: single texture+sampler
    /// using `canvas_copy_bind_group_layout`. Used by
    /// `GpuPaintTarget::save_pre_stroke_snapshot` to populate the brush's
    /// RGBA8 pre-stroke snapshot from an R8 mask source.
    pub fn mask_blit_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.mask_blit_pipeline
    }

    /// RGBA8 → R8 passthrough pipeline. Source bind group: single
    /// texture+sampler using `canvas_copy_bind_group_layout`. Used by
    /// `GpuPaintTarget::commit_scratch_blit` for direct scratch→mask commits
    /// (liquify-style terminals that don't go through the composite path).
    pub fn scratch_blit_r8_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.scratch_blit_r8_pipeline
    }

    pub fn blit_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.blit_pipeline
    }

    pub fn liquify_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.liquify_pipeline
    }

    /// Watercolor pickup pipeline. Reads canvas_copy + prev_carried, writes
    /// the wetness-blended result to a 1×1 carried-pickup texture. Bound
    /// resources: group(0) = pickup uniforms, group(1) = canvas_copy.
    pub fn watercolor_pickup_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.watercolor_pickup_pipeline
    }

    /// Render-attachment view of the 1×1 pickup texture. The pickup pass
    /// writes one fragment per dab; the composite reads it via
    /// `watercolor_sources_bind_group`.
    pub fn watercolor_pickup_attachment_view(&self) -> &wgpu::TextureView {
        &self.watercolor_pickup_attachment_view
    }

    /// Watercolor composite pipeline. Targets RGBA8 stroke scratch.
    /// Bound resources: group(0) = composite uniforms, group(1) = dab,
    /// group(2) = selection, group(3) = sources (canvas_copy + pickup).
    pub fn watercolor_composite_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.watercolor_composite_pipeline
    }

    pub fn selection_bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.selection_bgl
    }

    /// The 1×1 white selection bind group — bound when no selection is active.
    /// Exposed for out-of-crate tests that construct a `BrushGpuContext`
    /// manually and need a default selection mask.
    pub fn default_selection_bind_group(&self) -> &wgpu::BindGroup {
        &self.default_selection_bind_group
    }

    /// BGL used by the per-dab read-mirror bind group on every `Scratch`.
    /// Brush composite pipelines bind a `Scratch::read_mirror_bind_group()`
    /// against this layout.
    pub fn canvas_copy_bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.canvas_copy_bgl
    }

    /// BGL used by the watercolor sources bind group on every `Scratch`
    /// (read mirror + sampler + pickup texture).
    pub fn watercolor_sources_bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.watercolor_sources_bgl
    }

    /// Linear sampler shared by every `Scratch`'s read-mirror bind group.
    pub fn canvas_copy_sampler(&self) -> &wgpu::Sampler {
        &self.canvas_copy_sampler
    }

    /// Sampled-side view of the 1×1 watercolor pickup texture.  Embedded
    /// by `Scratch` in its `watercolor_sources_bind_group` at binding 2.
    pub fn watercolor_pickup_view(&self) -> &wgpu::TextureView {
        &self.watercolor_pickup_view
    }

    /// Write circle mask uniforms to the next ring slot.
    /// Returns the dynamic byte offset for `set_bind_group`.
    pub fn write_circle_uniforms(&self, queue: &wgpu::Queue, uniforms: &CircleUniforms) -> u32 {
        self.circle_uniform_ring
            .write(queue, bytemuck::bytes_of(uniforms))
    }

    /// Write stamp dab uniforms to the next ring slot.
    /// Returns the dynamic byte offset for `set_bind_group`.
    pub fn write_stamp_uniforms(&self, queue: &wgpu::Queue, uniforms: &StampUniforms) -> u32 {
        self.stamp_uniform_ring
            .write(queue, bytemuck::bytes_of(uniforms))
    }

    /// Write texture overlay uniforms to the next ring slot.
    /// Returns the dynamic byte offset for `set_bind_group`.
    pub fn write_tex_overlay_uniforms(
        &self,
        queue: &wgpu::Queue,
        uniforms: &TexOverlayUniforms,
    ) -> u32 {
        self.tex_overlay_uniform_ring
            .write(queue, bytemuck::bytes_of(uniforms))
    }

    /// Write composite uniforms to the next ring slot.
    /// Returns the dynamic byte offset for `set_bind_group`.
    pub fn write_composite_uniforms(
        &self,
        queue: &wgpu::Queue,
        uniforms: &CompositeUniforms,
    ) -> u32 {
        self.composite_uniform_ring
            .write(queue, bytemuck::bytes_of(uniforms))
    }

    /// Write blit uniforms to the next ring slot.
    /// Returns the dynamic byte offset for `set_bind_group`.
    pub fn write_blit_uniforms(&self, queue: &wgpu::Queue, uniforms: &BlitUniforms) -> u32 {
        self.blit_uniform_ring
            .write(queue, bytemuck::bytes_of(uniforms))
    }

    /// Write liquify uniforms to the next ring slot.
    /// Returns the dynamic byte offset for `set_bind_group`.
    pub fn write_liquify_uniforms(&self, queue: &wgpu::Queue, uniforms: &LiquifyUniforms) -> u32 {
        self.liquify_uniform_ring
            .write(queue, bytemuck::bytes_of(uniforms))
    }

    /// Write watercolor pickup uniforms to the next ring slot.
    /// Returns the dynamic byte offset for `set_bind_group`.
    pub fn write_watercolor_pickup_uniforms(
        &self,
        queue: &wgpu::Queue,
        uniforms: &WatercolorPickupUniforms,
    ) -> u32 {
        self.watercolor_pickup_uniform_ring
            .write(queue, bytemuck::bytes_of(uniforms))
    }

    /// Write watercolor composite uniforms to the next ring slot.
    /// Returns the dynamic byte offset for `set_bind_group`.
    pub fn write_watercolor_composite_uniforms(
        &self,
        queue: &wgpu::Queue,
        uniforms: &WatercolorCompositeUniforms,
    ) -> u32 {
        self.watercolor_composite_uniform_ring
            .write(queue, bytemuck::bytes_of(uniforms))
    }

    /// True if any ring is close to capacity.  The caller should flush
    /// the current encoder, reset rings, and create a fresh encoder.
    pub fn rings_nearly_full(&self) -> bool {
        self.circle_uniform_ring.nearly_full()
            || self.stamp_uniform_ring.nearly_full()
            || self.tex_overlay_uniform_ring.nearly_full()
            || self.composite_uniform_ring.nearly_full()
            || self.blit_uniform_ring.nearly_full()
            || self.liquify_uniform_ring.nearly_full()
            || self.watercolor_pickup_uniform_ring.nearly_full()
            || self.watercolor_composite_uniform_ring.nearly_full()
    }

    /// Reset all uniform rings for a new frame.
    pub fn reset_uniform_rings(&self) {
        self.circle_uniform_ring.reset();
        self.stamp_uniform_ring.reset();
        self.tex_overlay_uniform_ring.reset();
        self.composite_uniform_ring.reset();
        self.blit_uniform_ring.reset();
        self.liquify_uniform_ring.reset();
        self.watercolor_pickup_uniform_ring.reset();
        self.watercolor_composite_uniform_ring.reset();
    }
}
