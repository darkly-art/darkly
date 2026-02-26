use crate::gpu::effect::{create_blit_pipeline, EffectCache, EffectPipeline};
use crate::gpu::veil::{Veil, VeilRegistration};
use std::sync::Arc;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsValue;

pub fn register() -> VeilRegistration {
    VeilRegistration {
        type_id: "pixelate",
        create_pipeline: create_pixelate_pipeline,
        #[cfg(target_arch = "wasm32")]
        from_js: |js, shared| Box::new(Pixelate::from_js(js, shared)),
    }
}

#[derive(Clone, Debug)]
pub struct Pixelate {
    /// Downscale factor (e.g. 4 = quarter resolution each axis).
    pub factor: u32,
    shared: Arc<EffectPipeline>,
}

impl Pixelate {
    pub fn new(factor: u32, shared: Arc<EffectPipeline>) -> Self {
        Pixelate {
            factor: factor.max(1),
            shared,
        }
    }

    fn bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.shared.bind_group_layout
    }

    #[cfg(target_arch = "wasm32")]
    pub fn from_js(js: JsValue, shared: Arc<EffectPipeline>) -> Self {
        let factor = js_sys::Reflect::get(&js, &"factor".into())
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(4.0) as u32;
        Pixelate::new(factor, shared)
    }
}

impl Veil for Pixelate {
    fn type_id(&self) -> &'static str {
        "pixelate"
    }

    fn clone_boxed(&self) -> Box<dyn Veil> {
        Box::new(self.clone())
    }

    fn create_cache(
        &self,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        ping_pong_views: &[wgpu::TextureView; 2],
        sampler: &wgpu::Sampler,
        viewport_width: u32,
        viewport_height: u32,
    ) -> EffectCache {
        let aux_w = (viewport_width / self.factor).max(1);
        let aux_h = (viewport_height / self.factor).max(1);

        // Small intermediate texture for the downscaled result.
        let aux_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pixelate-aux"),
            size: wgpu::Extent3d {
                width: aux_w,
                height: aux_h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let aux_view = aux_tex.create_view(&wgpu::TextureViewDescriptor::default());

        // Pass 0 (downscale): reads from ping_pong[src], writes to aux.
        // Two bind groups for the two possible source textures.
        let downscale_bind_groups: [wgpu::BindGroup; 2] = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("pixelate-down-{i}")),
                layout: self.bind_group_layout(),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&ping_pong_views[i]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
            })
        });

        // Pass 1 (upscale): reads from aux, writes to dst_view.
        // Source is always the aux texture, so both ping-pong variants are identical.
        let upscale_bind_groups: [wgpu::BindGroup; 2] = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("pixelate-up-{i}")),
                layout: self.bind_group_layout(),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&aux_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
            })
        });

        EffectCache {
            uniform_bufs: vec![],
            bind_groups: vec![downscale_bind_groups, upscale_bind_groups],
            aux_textures: vec![aux_tex],
            aux_views: vec![aux_view],
        }
    }

    fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        cache: &EffectCache,
        src_idx: usize,
        dst_view: &wgpu::TextureView,
    ) {
        // Pass 0: downscale — render to small aux texture.
        // Linear min_filter averages source texels into the smaller target.
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pixelate-downscale"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &cache.aux_views[0],
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

        // Pass 1: upscale — read small aux, write to full-size destination.
        // Linear mag_filter interpolates between the coarse texels → soft blur.
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pixelate-upscale"),
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
            rpass.set_bind_group(0, &cache.bind_groups[1][0], &[]);
            rpass.draw(0..3, 0..1);
        }
    }
}

fn create_pixelate_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
) -> EffectPipeline {
    // Pixelate uses the shared blit shader — the blur comes from
    // rendering to a small texture (downscale) then sampling it
    // at full resolution with linear filtering (upscale).
    create_blit_pipeline(device, format, "pixelate")
}
