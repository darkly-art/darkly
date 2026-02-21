use wgpu::util::DeviceExt;

use darkly_core::layer::FilterParams;

/// Uniform for blur shader — must match BlurParams in blur.wgsl
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct BlurUniforms {
    radius: f32,
    direction_x: f32,
    direction_y: f32,
    _pad: f32,
}

/// Cached bind groups for a single blur direction pair (src → temp → dst).
struct BlurPassCache {
    /// H pass bind group: reads src, writes temp
    h_bind_group: wgpu::BindGroup,
    /// V pass bind group: reads temp, writes dst
    v_bind_group: wgpu::BindGroup,
}

/// Manages filter shader pipelines.
/// Blur uniform buffers and bind groups are created once at init (P1),
/// never in the render loop.
pub struct FilterPipelines {
    blur_pipeline: wgpu::RenderPipeline,
    pub bind_group_layout: wgpu::BindGroupLayout,

    /// Cached uniform buffers: [0] = H direction, [1] = V direction.
    /// Usage includes COPY_DST so radius can be updated via write_buffer.
    blur_uniform_bufs: [wgpu::Buffer; 2],
    /// Cached blur bind groups per ping-pong config:
    /// [0] = src=accum[0], dst=accum[1] (temp=accum[2])
    /// [1] = src=accum[1], dst=accum[0] (temp=accum[2])
    blur_pass_cache: [BlurPassCache; 2],
    cached_radius: f32,
}

impl FilterPipelines {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        accum_views: &[wgpu::TextureView; 3],
        sampler: &wgpu::Sampler,
    ) -> Self {
        let fullscreen_src = include_str!("../../../shaders/fullscreen.wgsl");
        let blur_src = include_str!("../../../shaders/filters/blur.wgsl");

        let vs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fullscreen_vs_filter"),
            source: wgpu::ShaderSource::Wgsl(fullscreen_src.into()),
        });

        let fs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blur_fs"),
            source: wgpu::ShaderSource::Wgsl(blur_src.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blur_bgl"),
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blur_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let blur_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blur_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &vs_module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &fs_module,
                entry_point: Some("fs_blur"),
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
            multiview: None,
            cache: None,
        });

        // Create cached uniform buffers with COPY_DST for write_buffer updates (P1)
        let h_uniforms = BlurUniforms {
            radius: 0.0,
            direction_x: 1.0,
            direction_y: 0.0,
            _pad: 0.0,
        };
        let v_uniforms = BlurUniforms {
            radius: 0.0,
            direction_x: 0.0,
            direction_y: 1.0,
            _pad: 0.0,
        };

        let h_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("blur_h_uniforms"),
            contents: bytemuck::bytes_of(&h_uniforms),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let v_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("blur_v_uniforms"),
            contents: bytemuck::bytes_of(&v_uniforms),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // Pre-build bind groups for both ping-pong configurations.
        // temp is always accum[2].
        let blur_pass_cache = std::array::from_fn(|src| {
            let dst = if src == 0 { 1 } else { 0 };

            // H pass: reads accum[src], writes to accum[2] (temp)
            let h_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("blur_h_bg_src{src}")),
                layout: &bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&accum_views[src]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: h_buf.as_entire_binding(),
                    },
                ],
            });

            // V pass: reads accum[2] (temp), writes to accum[dst]
            let v_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("blur_v_bg_dst{dst}")),
                layout: &bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&accum_views[2]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: v_buf.as_entire_binding(),
                    },
                ],
            });

            BlurPassCache { h_bind_group, v_bind_group }
        });

        FilterPipelines {
            blur_pipeline,
            bind_group_layout,
            blur_uniform_bufs: [h_buf, v_buf],
            blur_pass_cache,
            cached_radius: 0.0,
        }
    }

    /// Run a two-pass separable blur with cached bind groups (P1).
    /// `src_accum` is the index of the current accumulator (0 or 1).
    pub fn run_blur_indexed(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        src_accum: usize,
        temp: &wgpu::TextureView,
        output: &wgpu::TextureView,
        params: &FilterParams,
    ) {
        // Update radius via write_buffer if it changed (P1 — no allocation)
        if (params.radius - self.cached_radius).abs() > f32::EPSILON {
            let h_uniforms = BlurUniforms {
                radius: params.radius,
                direction_x: 1.0,
                direction_y: 0.0,
                _pad: 0.0,
            };
            let v_uniforms = BlurUniforms {
                radius: params.radius,
                direction_x: 0.0,
                direction_y: 1.0,
                _pad: 0.0,
            };
            queue.write_buffer(&self.blur_uniform_bufs[0], 0, bytemuck::bytes_of(&h_uniforms));
            queue.write_buffer(&self.blur_uniform_bufs[1], 0, bytemuck::bytes_of(&v_uniforms));
            self.cached_radius = params.radius;
        }

        self.run_blur_passes(encoder, src_accum, temp, output);
    }

    fn run_blur_passes(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        src_accum: usize,
        temp: &wgpu::TextureView,
        output: &wgpu::TextureView,
    ) {
        let cache = &self.blur_pass_cache[src_accum];

        // H pass: src → temp
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("blur_h_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: temp,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });

            pass.set_pipeline(&self.blur_pipeline);
            pass.set_bind_group(0, &cache.h_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        // V pass: temp → output
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("blur_v_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: output,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });

            pass.set_pipeline(&self.blur_pipeline);
            pass.set_bind_group(0, &cache.v_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}
