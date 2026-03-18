//! Pre-built GPU pipelines for the brush system.
//!
//! Two pipelines:
//! - **Procedural**: renders SDF circle/gaussian to a dab texture (REPLACE blend).
//! - **Composite**: composites a dab texture onto the canvas (alpha-over blend).
//!
//! Separate from `PaintPipelines` — different concerns (dab generation +
//! dab compositing vs. SDF circle painting + gradient fill).

/// Uniform data for the procedural dab generation shader.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DabUniforms {
    pub dab_size: f32,       // actual dab diameter in pixels
    pub radius: f32,         // SDF circle radius
    pub softness: f32,       // edge softness in pixels
    pub opacity: f32,        // dab opacity (0-1)
    pub color: [f32; 4],     // RGBA paint color (straight alpha)
}

/// Uniform data for the dab compositing shader.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CompositeUniforms {
    pub origin: [f32; 2],      // quad top-left in canvas pixels
    pub size: [f32; 2],        // quad size in canvas pixels
    pub canvas_size: [f32; 2], // canvas dimensions
    pub uv_max: [f32; 2],     // max UV in dab texture (dab_diameter / tex_size)
}

/// Pre-built render pipelines for the brush system.
pub struct BrushPipelines {
    procedural_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,

    procedural_uniform_buf: wgpu::Buffer,
    pub(crate) procedural_uniform_bind_group: wgpu::BindGroup,

    composite_uniform_buf: wgpu::Buffer,
    pub(crate) composite_uniform_bind_group: wgpu::BindGroup,

    /// 1x1 white selection texture — bound when no selection is active.
    pub(crate) default_selection_bind_group: wgpu::BindGroup,
    pub(crate) selection_bgl: wgpu::BindGroupLayout,
}

impl BrushPipelines {
    /// Create brush pipelines.
    ///
    /// `dab_bgl` is the dab texture bind group layout from `DabTexturePool`.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        dab_bgl: &wgpu::BindGroupLayout,
    ) -> Self {
        // --- Shaders ---
        let procedural_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brush-procedural"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/brush/procedural.wgsl").into(),
            ),
        });

        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brush-composite"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/brush/composite.wgsl").into(),
            ),
        });

        // --- Bind group layouts ---
        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("brush-uniform-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
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

        // --- Pipeline layouts ---
        // Procedural: group(0) = uniforms only (renders to dab texture).
        let procedural_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("brush-procedural-layout"),
            bind_group_layouts: &[&uniform_bgl],
            immediate_size: 0,
        });

        // Composite: group(0) = uniforms, group(1) = dab texture, group(2) = selection.
        let composite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("brush-composite-layout"),
            bind_group_layouts: &[&uniform_bgl, dab_bgl, &selection_bgl],
            immediate_size: 0,
        });

        // --- Uniform buffers ---
        let procedural_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("brush-procedural-uniforms"),
            size: std::mem::size_of::<DabUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let procedural_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brush-procedural-uniform-bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: procedural_uniform_buf.as_entire_binding(),
            }],
        });

        let composite_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("brush-composite-uniforms"),
            size: std::mem::size_of::<CompositeUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let composite_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brush-composite-uniform-bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: composite_uniform_buf.as_entire_binding(),
            }],
        });

        // --- Default selection (1x1 white = fully selected) ---
        let sel_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("brush-default-selection"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
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
            wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
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

        // --- Pipelines ---

        // Procedural: REPLACE blend — we clear the dab texture and write fresh pixels.
        let procedural_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("brush-procedural"),
            layout: Some(&procedural_layout),
            vertex: wgpu::VertexState {
                module: &procedural_shader,
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
                module: &procedural_shader,
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

        // Composite: source-over alpha blending (same as paint composite).
        let blend_composite = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };

        let composite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("brush-composite"),
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
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(blend_composite),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        Self {
            procedural_pipeline,
            composite_pipeline,
            procedural_uniform_buf,
            procedural_uniform_bind_group,
            composite_uniform_buf,
            composite_uniform_bind_group,
            default_selection_bind_group,
            selection_bgl,
        }
    }

    pub fn procedural_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.procedural_pipeline
    }

    pub fn composite_pipeline(&self) -> &wgpu::RenderPipeline {
        &self.composite_pipeline
    }

    pub fn selection_bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.selection_bgl
    }

    /// Write procedural dab uniforms to the GPU buffer.
    pub fn write_dab_uniforms(&self, queue: &wgpu::Queue, uniforms: &DabUniforms) {
        queue.write_buffer(&self.procedural_uniform_buf, 0, bytemuck::bytes_of(uniforms));
    }

    /// Write composite uniforms to the GPU buffer.
    pub fn write_composite_uniforms(&self, queue: &wgpu::Queue, uniforms: &CompositeUniforms) {
        queue.write_buffer(&self.composite_uniform_buf, 0, bytemuck::bytes_of(uniforms));
    }
}
