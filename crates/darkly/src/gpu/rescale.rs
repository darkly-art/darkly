//! Image-rescale GPU resampling pass.
//!
//! Resamples a single layer/mask texture from its old canvas extent into a
//! freshly-allocated texture at a new extent, scaled about the document's
//! canvas origin. Upscales bilinearly; downscales through a box pyramid
//! (`fs_halve` chained until within 2x of the target, then one `fs_resample`)
//! so large reductions stay anti-aliased. RGBA is resampled in premultiplied
//! alpha; R8 masks straight. See `shaders/rescale.wgsl`.
//!
//! The pass is owned by the [`Compositor`](super::compositor::Compositor),
//! which drives it per node in `rescale_nodes`.

use crate::coord::{CanvasPoint, CanvasRect};
use crate::gpu::atlas::LayerTexture;

/// Resample uniform, packed into vec4 rows so the WGSL uniform layout is
/// unambiguous (16-byte-aligned members; the same discipline as
/// `TransformBlendUniforms`).
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    /// `[new_origin.x, new_origin.y, canvas_origin.x, canvas_origin.y]`
    p0: [f32; 4],
    /// `[inv_scale.x, inv_scale.y, old_origin.x, old_origin.y]`
    p1: [f32; 4],
    /// `[old_size.x, old_size.y, is_r8, _pad]`
    p2: [f32; 4],
}

/// Render pipelines + bind-group layout for the rescale shader. Holds one
/// pipeline per (entry point × target format) combination.
pub struct RescalePass {
    resample_rgba: wgpu::RenderPipeline,
    resample_r8: wgpu::RenderPipeline,
    halve_rgba: wgpu::RenderPipeline,
    halve_r8: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
}

impl std::fmt::Debug for RescalePass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RescalePass").finish_non_exhaustive()
    }
}

impl RescalePass {
    pub fn new(device: &wgpu::Device) -> Self {
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rescale-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        // Sampled with textureLoad — no hardware filtering.
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
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

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rescale-layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rescale-shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/rescale.wgsl").into(),
            ),
        });

        let make = |entry: &str, format: wgpu::TextureFormat, label: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
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

        let rgba = wgpu::TextureFormat::Rgba8Unorm;
        let r8 = wgpu::TextureFormat::R8Unorm;
        RescalePass {
            resample_rgba: make("fs_resample", rgba, "rescale-resample-rgba"),
            resample_r8: make("fs_resample", r8, "rescale-resample-r8"),
            halve_rgba: make("fs_halve", rgba, "rescale-halve-rgba"),
            halve_r8: make("fs_halve", r8, "rescale-halve-r8"),
            bgl,
        }
    }

    /// Resample `src` (at its old extent) into a fresh `LayerTexture` at
    /// `new_extent`, with content scaled about `canvas_origin` by `(sx, sy)`
    /// (the document scale = new_dim / old_dim). `format` selects the RGBA
    /// (premultiplied) or R8 (straight) path.
    pub fn resample_node(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        src: &LayerTexture,
        new_extent: CanvasRect,
        canvas_origin: CanvasPoint,
        sx: f32,
        sy: f32,
        format: wgpu::TextureFormat,
    ) -> LayerTexture {
        let is_r8 = format == wgpu::TextureFormat::R8Unorm;
        let old_extent = src.canvas_extent();
        let target_w = new_extent.width.max(1);
        let target_h = new_extent.height.max(1);

        // Box-pyramid downscale: halve while both axes stay at least 2x the
        // target, so the final resample only ever upsamples or shrinks <2x.
        // Each intermediate holds the same old canvas extent at lower res.
        let halve_params = Params {
            p0: [0.0, 0.0, 0.0, 0.0],
            p1: [1.0, 1.0, 0.0, 0.0],
            p2: [1.0, 1.0, if is_r8 { 1.0 } else { 0.0 }, 0.0],
        };
        let mut chain: Vec<wgpu::Texture> = Vec::new();
        let mut cur_w = src.layer_extent().width;
        let mut cur_h = src.layer_extent().height;
        while cur_w >= 2 * target_w && cur_h >= 2 * target_h && cur_w > 1 && cur_h > 1 {
            let hw = (cur_w / 2).max(1);
            let hh = (cur_h / 2).max(1);
            let input_view = match chain.last() {
                Some(t) => t.create_view(&wgpu::TextureViewDescriptor::default()),
                None => src
                    .texture()
                    .create_view(&wgpu::TextureViewDescriptor::default()),
            };
            let out_tex = create_intermediate(device, hw, hh, format);
            let out_view = out_tex.create_view(&wgpu::TextureViewDescriptor::default());
            let pipeline = if is_r8 {
                &self.halve_r8
            } else {
                &self.halve_rgba
            };
            self.run_pass(
                device,
                queue,
                encoder,
                pipeline,
                &input_view,
                &out_view,
                &halve_params,
            );
            chain.push(out_tex);
            cur_w = hw;
            cur_h = hh;
        }

        let dest = if is_r8 {
            LayerTexture::new_mask_with_extent(device, queue, new_extent)
        } else {
            LayerTexture::with_bounds(device, new_extent)
        };

        let params = Params {
            p0: [
                new_extent.origin.x as f32,
                new_extent.origin.y as f32,
                canvas_origin.x as f32,
                canvas_origin.y as f32,
            ],
            p1: [
                1.0 / sx,
                1.0 / sy,
                old_extent.origin.x as f32,
                old_extent.origin.y as f32,
            ],
            p2: [
                old_extent.width as f32,
                old_extent.height as f32,
                if is_r8 { 1.0 } else { 0.0 },
                0.0,
            ],
        };
        let final_input_view = match chain.last() {
            Some(t) => t.create_view(&wgpu::TextureViewDescriptor::default()),
            None => src
                .texture()
                .create_view(&wgpu::TextureViewDescriptor::default()),
        };
        let pipeline = if is_r8 {
            &self.resample_r8
        } else {
            &self.resample_rgba
        };
        self.run_pass(
            device,
            queue,
            encoder,
            pipeline,
            &final_input_view,
            dest.view(),
            &params,
        );
        dest
    }

    #[allow(clippy::too_many_arguments)]
    fn run_pass(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::RenderPipeline,
        input_view: &wgpu::TextureView,
        output_view: &wgpu::TextureView,
        params: &Params,
    ) {
        let ubuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rescale-params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&ubuf, 0, bytemuck::bytes_of(params));

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rescale-bg"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(input_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: ubuf.as_entire_binding(),
                },
            ],
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("rescale-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn create_intermediate(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("rescale-intermediate"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}
