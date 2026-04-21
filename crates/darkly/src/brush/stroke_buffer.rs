//! Stroke buffer — separate texture for dab rendering during a stroke,
//! enabling mid-stroke rewind and re-rendering for the stabilizer.
//!
//! Two textures:
//! - **stroke texture**: dabs render here instead of directly to the layer.
//! - **pre-stroke texture**: snapshot of the layer before the stroke started,
//!   used to restore both the stroke buffer and the layer on rewind.
//!
//! The composite step writes the final result to the layer each frame:
//! source-over blend of the stroke buffer onto the pre-stroke snapshot.

/// Manages the stroke-in-progress scratch and pre-stroke snapshot textures.
///
/// `StrokeBuffer` owns the raw GPU resources; the stroke *semantics* (how
/// the scratch is initialised, how it lands on the layer) belong to the
/// active terminal node's lifecycle hooks (`begin_stroke` / `commit`). This
/// keeps the engine free of terminal-type branching — swapping in a warp or
/// smudge terminal doesn't require editing this file.
pub struct StrokeBuffer {
    /// Dabs render into this texture (instead of directly to the layer).
    stroke_texture: wgpu::Texture,
    stroke_view: wgpu::TextureView,

    /// Snapshot of the layer before the stroke started.
    pre_stroke_texture: wgpu::Texture,
    #[allow(dead_code)] // Kept alive for bind group references.
    pre_stroke_view: wgpu::TextureView,

    /// Bind group for the stroke texture, compatible with the dab texture BGL
    /// so the existing composite pipeline can read it as the foreground.
    stroke_bind_group: wgpu::BindGroup,

    /// Bind group for the pre-stroke texture, compatible with the canvas copy BGL
    /// so the existing composite pipeline can read it as the background.
    pre_stroke_bind_group: wgpu::BindGroup,

    width: u32,
    height: u32,
}

impl StrokeBuffer {
    /// Create a new stroke buffer matching the given canvas dimensions.
    ///
    /// `dab_bgl` must be the bind group layout from `DabTexturePool` (texture+sampler).
    /// `canvas_copy_bgl` must be the canvas copy bind group layout from `BrushPipelines`.
    pub fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        dab_bgl: &wgpu::BindGroupLayout,
        canvas_copy_bgl: &wgpu::BindGroupLayout,
    ) -> Self {
        let stroke_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("stroke-buffer"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let stroke_view = stroke_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let pre_stroke_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pre-stroke-snapshot"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let pre_stroke_view = pre_stroke_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("stroke-buffer-sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let stroke_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("stroke-buffer-bg"),
            layout: dab_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&stroke_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let pre_stroke_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pre-stroke-bg"),
            layout: canvas_copy_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&pre_stroke_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        Self {
            stroke_texture,
            stroke_view,
            pre_stroke_texture,
            pre_stroke_view,
            stroke_bind_group,
            pre_stroke_bind_group,
            width,
            height,
        }
    }

    /// The texture view dabs render into.
    pub fn stroke_view(&self) -> &wgpu::TextureView {
        &self.stroke_view
    }

    /// The stroke texture (for use as copy source/dest).
    pub fn stroke_texture(&self) -> &wgpu::Texture {
        &self.stroke_texture
    }

    /// Bind group over the stroke texture using the dab BGL — the composite
    /// pipeline binds this as the foreground at commit time.
    pub fn stroke_bind_group(&self) -> &wgpu::BindGroup {
        &self.stroke_bind_group
    }

    /// The pre-stroke snapshot texture.
    pub fn pre_stroke_texture(&self) -> &wgpu::Texture {
        &self.pre_stroke_texture
    }

    /// Bind group over the pre-stroke snapshot using the canvas-copy BGL —
    /// the composite pipeline binds this as the background at commit time.
    pub fn pre_stroke_bind_group(&self) -> &wgpu::BindGroup {
        &self.pre_stroke_bind_group
    }

    /// Save a snapshot of the layer texture before the stroke starts.
    pub fn save_pre_stroke(&self, encoder: &mut wgpu::CommandEncoder, layer_texture: &wgpu::Texture) {
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: layer_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.pre_stroke_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
    }
}
