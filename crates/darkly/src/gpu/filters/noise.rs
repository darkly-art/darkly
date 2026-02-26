use crate::gpu::filter::{Filter, FilterLayerCache, FilterPipeline, FilterRegistration};
use std::sync::Arc;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsValue;

pub fn register() -> FilterRegistration {
    FilterRegistration {
        type_id: "noise",
        create_pipeline,
        #[cfg(target_arch = "wasm32")]
        from_js: |js, shared| Box::new(Noise::from_js(js, shared)),
    }
}

#[derive(Clone, Debug)]
pub struct Noise {
    pub amount: f32,
    pub resolution: u32,
    shared: Arc<FilterPipeline>,
}

impl Noise {
    pub fn new(amount: f32, resolution: u32, shared: Arc<FilterPipeline>) -> Self {
        Noise {
            amount,
            resolution,
            shared,
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn from_js(js: JsValue, shared: Arc<FilterPipeline>) -> Self {
        let amount = js_sys::Reflect::get(&js, &"amount".into())
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.5) as f32;
        let resolution = js_sys::Reflect::get(&js, &"resolution".into())
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0) as u32;
        Noise::new(amount, resolution, shared)
    }

    fn generate_noise_texture(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        canvas_width: u32,
        canvas_height: u32,
        resolution: u32,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let res = resolution.max(1);
        let noise_w = (canvas_width + res - 1) / res;
        let noise_h = (canvas_height + res - 1) / res;

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("noise-texture"),
            size: wgpu::Extent3d {
                width: noise_w,
                height: noise_h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Simple xorshift PRNG for noise data
        let pixel_count = (noise_w * noise_h) as usize;
        let mut data = vec![0u8; pixel_count];
        let mut state: u32 = 0xDEAD_BEEF;
        for byte in data.iter_mut() {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *byte = (state & 0xFF) as u8;
        }

        // Pad rows to 256-byte alignment as required by WebGPU
        let bytes_per_row = noise_w;
        let aligned_bytes_per_row = (bytes_per_row + 255) & !255;

        if aligned_bytes_per_row == bytes_per_row {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(noise_h),
                },
                wgpu::Extent3d {
                    width: noise_w,
                    height: noise_h,
                    depth_or_array_layers: 1,
                },
            );
        } else {
            let mut padded = vec![0u8; (aligned_bytes_per_row * noise_h) as usize];
            for y in 0..noise_h as usize {
                let src_start = y * noise_w as usize;
                let dst_start = y * aligned_bytes_per_row as usize;
                padded[dst_start..dst_start + noise_w as usize]
                    .copy_from_slice(&data[src_start..src_start + noise_w as usize]);
            }
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &padded,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(aligned_bytes_per_row),
                    rows_per_image: Some(noise_h),
                },
                wgpu::Extent3d {
                    width: noise_w,
                    height: noise_h,
                    depth_or_array_layers: 1,
                },
            );
        }

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        (texture, view)
    }
}

/// GPU uniforms for the noise shader, 16-byte aligned.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct NoiseUniforms {
    amount: f32,
    resolution: f32,
    _pad0: f32,
    _pad1: f32,
}

impl Filter for Noise {
    fn type_id(&self) -> &'static str {
        "noise"
    }

    fn clone_boxed(&self) -> Box<dyn Filter> {
        Box::new(self.clone())
    }

    fn pass_count(&self) -> u32 {
        1
    }

    fn pipeline(&self) -> &wgpu::RenderPipeline {
        &self.shared.pipeline
    }

    fn bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.shared.bind_group_layout
    }

    fn create_cache(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        accum_views: &[wgpu::TextureView; 2],
        sampler: &wgpu::Sampler,
        canvas_width: u32,
        canvas_height: u32,
    ) -> FilterLayerCache {
        let uniforms = NoiseUniforms {
            amount: self.amount,
            resolution: self.resolution as f32,
            _pad0: 0.0,
            _pad1: 0.0,
        };

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("noise-uniforms"),
            size: std::mem::size_of::<NoiseUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        let (noise_tex, noise_view) = Self::generate_noise_texture(
            device,
            queue,
            canvas_width,
            canvas_height,
            self.resolution,
        );

        let bind_groups: [wgpu::BindGroup; 2] = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("noise-bg-{i}")),
                layout: self.bind_group_layout(),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&accum_views[i]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&noise_view),
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
        });

        FilterLayerCache {
            uniform_bufs: vec![uniform_buf],
            bind_groups: vec![bind_groups],
            aux_textures: vec![noise_tex],
            aux_views: vec![noise_view],
        }
    }
}

/// Create the shared pipeline + bind group layout for noise filters.
pub fn create_pipeline(device: &wgpu::Device, format: wgpu::TextureFormat) -> FilterPipeline {
    let bind_group_layout =
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("noise-bgl"),
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
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
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
        label: Some("noise-pipeline-layout"),
        bind_group_layouts: &[&bind_group_layout],
        push_constant_ranges: &[],
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("noise-shader"),
        source: wgpu::ShaderSource::Wgsl(
            include_str!("../../../../../shaders/filters/noise.wgsl").into(),
        ),
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("noise-pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_noise"),
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

    FilterPipeline {
        pipeline,
        bind_group_layout,
    }
}
