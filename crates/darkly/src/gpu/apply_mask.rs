//! The mask-apply pass: modulate a window-sized projection's alpha by a mask
//! sampled in the mask's own plane space (`apply_mask.wgsl`).
//!
//! This is the de-fused replacement for the fused mask sampling that used to
//! live inside the layer-blend pass. A masked host composites its content into
//! an isolated projection; this pass multiplies that projection's alpha by the
//! mask before it blends down onto the parent, so the mask never samples the
//! host layer's texture or geometry. See `gpu::compositor` projection path.

/// Per-pass uniform: the mask texture's plane geometry + the isolated-debug
/// flag. Matches `MaskUniform` in `apply_mask.wgsl`. A zero `mask_size` is the
/// "no footprint" sentinel (reveal everywhere).
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MaskUniform {
    pub mask_offset: [f32; 2],
    pub mask_size: [f32; 2],
    pub isolated: u32,
    pub _pad0: u32,
    pub _pad1: u32,
    pub _pad2: u32,
}

pub struct ApplyMaskPipeline {
    pipeline: wgpu::RenderPipeline,
    /// Group 0: projection texture + sampler + mask uniform.
    pub bind_group_layout: wgpu::BindGroupLayout,
}

impl ApplyMaskPipeline {
    /// Build the pipeline. Group 1 (mask texture) reuses the blend pipelines'
    /// `mask_bind_group_layout`; group 2 (canvas geometry) reuses their
    /// `canvas_bind_group_layout`: one shared layout per resource kind.
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        mask_bind_group_layout: &wgpu::BindGroupLayout,
        canvas_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("apply-mask-bgl"),
            entries: &[
                // binding 0: projection texture
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
                // binding 1: sampler
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // binding 2: mask uniform (offset/size/isolated)
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("apply-mask-pipeline-layout"),
            bind_group_layouts: &[
                Some(&bind_group_layout),
                Some(mask_bind_group_layout),
                Some(canvas_bind_group_layout),
            ],
            immediate_size: 0,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("apply-mask-shader"),
            source: wgpu::ShaderSource::Wgsl(
                crate::gpu::canvas_lib::with_canvas_lib(include_str!(
                    "../../shaders/apply_mask.wgsl"
                ))
                .into(),
            ),
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("apply-mask-pipeline"),
            layout: Some(&pipeline_layout),
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
        });

        ApplyMaskPipeline {
            pipeline,
            bind_group_layout,
        }
    }

    pub fn pipeline(&self) -> &wgpu::RenderPipeline {
        &self.pipeline
    }
}
