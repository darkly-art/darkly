use crate::gpu::effect::{create_effect_pipeline, Binding, EffectCache, EffectPipeline};
use crate::gpu::preview::{swing, PreviewAnim};
use crate::gpu::veil::{ParamDef, ParamValue, Veil, VeilRegistration};
use std::sync::Arc;

const PARAMS: &[ParamDef] = &[
    // User-facing 0..1 slider. The shader multiplies by 0.03 to derive the
    // blur radius as a fraction of sqrt(canvas area), so user 1.0 = 3% of
    // sqrt(area) (≈ 30 px on a 1024² canvas) and the default user 1/3
    // ≈ 0.01 of sqrt(area) (≈ 10 px on 1024²).
    ParamDef::float("radius", 0.0, 1.0, 1.0 / 3.0)
        .with_label("Radius")
        .with_description("Size of the defocus circle: how far out of focus the image sits."),
    ParamDef::float("threshold", 0.01, 1.0, 0.1)
        .with_label("Threshold")
        .with_description("How bright a pixel must be before it blooms into a bokeh highlight."),
];

pub fn register() -> VeilRegistration {
    VeilRegistration {
        type_id: "lens_blur",
        display_name: "Lens Blur",
        description: "Defocus the view with a soft camera-lens blur.",
        params: PARAMS,
        preview: Some(PreviewAnim::LOOPING),
        create_pipeline: create_lens_blur_pipeline,
        from_params: |params, shared| {
            let radius = match params.first() {
                Some(ParamValue::Float(v)) => *v,
                _ => 1.0 / 3.0,
            };
            let threshold = match params.get(1) {
                Some(ParamValue::Float(v)) => *v,
                _ => 0.1,
            };
            Box::new(LensBlur::new(radius, threshold, shared))
        },
    }
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct LensBlurUniforms {
    radius: f32,
    threshold: f32,
    resolution_x: f32,
    resolution_y: f32,
}

#[derive(Clone, Debug)]
pub struct LensBlur {
    pub radius: f32,
    pub threshold: f32,
    /// Render resolution, kept from `create_cache` so
    /// [`uniforms`](Self::uniforms) rebuilds the whole struct from state.
    resolution: (f32, f32),
    shared: Arc<EffectPipeline>,
}

impl LensBlur {
    pub fn new(radius: f32, threshold: f32, shared: Arc<EffectPipeline>) -> Self {
        LensBlur {
            radius: radius.max(0.0),
            threshold: threshold.max(0.01),
            resolution: (0.0, 0.0),
            shared,
        }
    }

    fn uniforms(&self) -> LensBlurUniforms {
        LensBlurUniforms {
            radius: self.radius,
            threshold: self.threshold,
            resolution_x: self.resolution.0,
            resolution_y: self.resolution.1,
        }
    }
}

impl Veil for LensBlur {
    fn type_id(&self) -> &'static str {
        "lens_blur"
    }

    fn clone_boxed(&self) -> Box<dyn Veil> {
        Box::new(self.clone())
    }

    fn param_values(&self) -> Vec<ParamValue> {
        vec![
            ParamValue::Float(self.radius),
            ParamValue::Float(self.threshold),
        ]
    }

    /// Focus pulls all the way out and back in. `radius` is a sample-footprint
    /// parameter within a single pass rather than a pass count, so sweeping its
    /// full band averages *below* the shipped default in cost.
    fn preview_at(&mut self, queue: &wgpu::Queue, cache: &EffectCache, t: f32) -> bool {
        self.radius = swing(t);
        cache.write_uniform(queue, 0, bytemuck::bytes_of(&self.uniforms()));
        true
    }

    fn create_cache(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        ping_pong_views: &[wgpu::TextureView; 2],
        sampler: &wgpu::Sampler,
        render_width: u32,
        render_height: u32,
    ) -> EffectCache {
        self.resolution = (render_width as f32, render_height as f32);
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lens-blur-uniforms"),
            size: std::mem::size_of::<LensBlurUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&self.uniforms()));

        let layout = &self.shared.bind_group_layout;
        let bind_groups: [wgpu::BindGroup; 2] = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("lens-blur-bg-{i}")),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&ping_pong_views[i]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: uniform_buf.as_entire_binding(),
                    },
                ],
            })
        });

        EffectCache {
            uniform_bufs: vec![uniform_buf],
            bind_groups: vec![bind_groups],
            aux_textures: vec![],
            aux_views: vec![],
            aux_pipelines: vec![],
        }
    }

    fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        cache: &EffectCache,
        src_idx: usize,
        dst_view: &wgpu::TextureView,
    ) {
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("lens-blur"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        rpass.set_pipeline(&self.shared.pipeline);
        rpass.set_bind_group(0, &cache.bind_groups[0][src_idx], &[]);
        rpass.draw(0..3, 0..1);
    }
}

fn create_lens_blur_pipeline(device: &wgpu::Device, format: wgpu::TextureFormat) -> EffectPipeline {
    create_effect_pipeline(
        device,
        format,
        "lens-blur",
        &[Binding::Texture, Binding::Sampler, Binding::Uniform],
        include_str!("../../../shaders/veils/lens_blur.wgsl"),
        "fs_lens_blur",
    )
}
