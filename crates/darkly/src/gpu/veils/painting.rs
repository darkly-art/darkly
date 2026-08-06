// User-facing "Painting" veil. The underlying algorithm is the
// generalized Kuwahara filter — see shader header for prior-art credit.

use crate::gpu::effect::{create_effect_pipeline, Binding, EffectCache, EffectPipeline};
use crate::gpu::preview::{swing, PreviewAnim};
use crate::gpu::veil::{ParamDef, ParamValue, Veil, VeilRegistration};
use crate::units::UnitType;
use std::sync::Arc;

const PARAMS: &[ParamDef] = &[
    ParamDef::int("kernel_size", 1, 7, 6)
        .with_label("Brush Size")
        .with_description("Width of the region each output pixel is averaged from — larger reads as broader strokes.")
        .with_unit(UnitType::Pixels),
    ParamDef::float("sharpness", 1.0, 18.0, 8.0)
        .with_label("Sharpness")
        .with_description("How crisply one painted region ends and the next begins."),
    ParamDef::float("hardness", 1.0, 200.0, 100.0)
        .with_label("Hardness")
        .with_description("How strongly the strongest-oriented region wins, flattening detail into flat patches."),
];

pub fn register() -> VeilRegistration {
    VeilRegistration {
        type_id: "painting",
        display_name: "Painting",
        description: "Smooth the view into painterly, brush-like daubs.",
        params: PARAMS,
        preview: Some(PreviewAnim::LOOPING),
        create_pipeline: create_painting_pipeline,
        from_params: |params, shared| {
            let kernel_size = match params.first() {
                Some(ParamValue::Int(v)) => *v,
                _ => 6,
            };
            let sharpness = match params.get(1) {
                Some(ParamValue::Float(v)) => *v,
                _ => 8.0,
            };
            let hardness = match params.get(2) {
                Some(ParamValue::Float(v)) => *v,
                _ => 100.0,
            };
            Box::new(Painting::new(kernel_size, sharpness, hardness, shared))
        },
    }
}

/// GPU uniforms for the Painting shader.
/// Layout must match the WGSL `Params` struct exactly.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct PaintingUniforms {
    kernel_size: i32,
    sharpness: f32,
    hardness: f32,
    _pad: f32,
    resolution_x: f32,
    resolution_y: f32,
}

#[derive(Clone, Debug)]
pub struct Painting {
    pub kernel_size: i32,
    pub sharpness: f32,
    pub hardness: f32,
    /// Render resolution, kept from `create_cache` so
    /// [`uniforms`](Self::uniforms) rebuilds the whole struct from state.
    resolution: (f32, f32),
    shared: Arc<EffectPipeline>,
}

impl Painting {
    pub fn new(
        kernel_size: i32,
        sharpness: f32,
        hardness: f32,
        shared: Arc<EffectPipeline>,
    ) -> Self {
        Painting {
            kernel_size: kernel_size.max(1),
            sharpness,
            hardness,
            resolution: (0.0, 0.0),
            shared,
        }
    }

    fn uniforms(&self) -> PaintingUniforms {
        PaintingUniforms {
            kernel_size: self.kernel_size,
            sharpness: self.sharpness,
            hardness: self.hardness,
            _pad: 0.0,
            resolution_x: self.resolution.0,
            resolution_y: self.resolution.1,
        }
    }
}

impl Veil for Painting {
    fn type_id(&self) -> &'static str {
        "painting"
    }

    fn clone_boxed(&self) -> Box<dyn Veil> {
        Box::new(self.clone())
    }

    fn perf_scale_factor(&self) -> f32 {
        // O(kernel²) samples per pixel — at default kernel_size=6 that's 169
        // texture taps. Painterly output is inherently smooth/blurry, so the
        // bilinear upscale is visually free.
        0.7
    }

    fn param_values(&self) -> Vec<ParamValue> {
        vec![
            ParamValue::Int(self.kernel_size),
            ParamValue::Float(self.sharpness),
            ParamValue::Float(self.hardness),
        ]
    }

    /// The brush widens from a single texel to the full Kuwahara window and
    /// back, so each quantised step of the control is plainly visible.
    /// `kernel_size` sets the sampling radius inside one pass — `O(kernel²)`
    /// samples — so the ramp averages a radius of 4 against the shipped default
    /// of 6 and is *cheaper* per frame than a default-parameter render.
    fn preview_at(&mut self, queue: &wgpu::Queue, cache: &EffectCache, t: f32) -> bool {
        self.kernel_size = (1.0 + 6.0 * swing(t)).round() as i32;
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
            label: Some("painting-uniforms"),
            size: std::mem::size_of::<PaintingUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&self.uniforms()));

        let layout = &self.shared.bind_group_layout;
        let bind_groups: [wgpu::BindGroup; 2] = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("painting-bg-{i}")),
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
            label: Some("painting"),
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

fn create_painting_pipeline(device: &wgpu::Device, format: wgpu::TextureFormat) -> EffectPipeline {
    create_effect_pipeline(
        device,
        format,
        "painting",
        &[Binding::Texture, Binding::Sampler, Binding::Uniform],
        include_str!("../../../shaders/veils/painting.wgsl"),
        "fs_painting",
    )
}
