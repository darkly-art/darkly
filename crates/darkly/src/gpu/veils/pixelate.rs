use crate::gpu::effect::{create_blit_pipeline, EffectCache, EffectPipeline};
use crate::gpu::veil::{ParamDef, ParamValue, Veil, VeilRegistration};
use std::sync::Arc;

const PARAMS: &[ParamDef] = &[
    ParamDef::Int {
        name: "scale",
        min: 1,
        max: 6,
        default: 2,
    },
    ParamDef::Bool {
        name: "soft",
        default: false,
    },
];

pub fn register() -> VeilRegistration {
    VeilRegistration {
        type_id: "pixelate",
        display_name: "Pixelate",
        params: PARAMS,
        create_pipeline: create_pixelate_pipeline,
        from_params: |params, shared| {
            let scale = match params.first() {
                Some(ParamValue::Int(v)) => *v as u32,
                _ => 2,
            };
            let soft = match params.get(1) {
                Some(ParamValue::Bool(v)) => *v,
                _ => true,
            };
            Box::new(Pixelate::new(scale, soft, shared))
        },
    }
}

#[derive(Clone, Debug)]
pub struct Pixelate {
    /// Downscale factor (e.g. 4 = quarter resolution each axis).
    pub scale: u32,
    /// When true, upscale uses linear filtering (soft/blurry).
    /// When false, uses nearest-neighbor (hard pixel edges).
    pub soft: bool,
    shared: Arc<EffectPipeline>,
}

impl Pixelate {
    pub fn new(scale: u32, soft: bool, shared: Arc<EffectPipeline>) -> Self {
        Pixelate {
            scale: scale.max(1),
            soft,
            shared,
        }
    }

    fn bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.shared.bind_group_layout
    }

    /// The scale value IS the number of 2x halving passes.
    /// Each halving doubles the pixel size: 1 → 2px, 2 → 4px, 3 → 8px, etc.
    fn num_halvings(&self) -> u32 {
        self.scale
    }
}

impl Veil for Pixelate {
    fn type_id(&self) -> &'static str {
        "pixelate"
    }

    fn clone_boxed(&self) -> Box<dyn Veil> {
        Box::new(self.clone())
    }

    fn param_values(&self) -> Vec<ParamValue> {
        vec![
            ParamValue::Int(self.scale as i32),
            ParamValue::Bool(self.soft),
        ]
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
        let n = self.num_halvings();
        let layout = self.bind_group_layout();
        let tex_usage =
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;

        // Create intermediate textures at each halving level.
        let mut aux_textures = Vec::with_capacity(n as usize);
        let mut aux_views = Vec::with_capacity(n as usize);
        let mut w = viewport_width;
        let mut h = viewport_height;

        for i in 0..n {
            w = (w / 2).max(1);
            h = (h / 2).max(1);
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(&format!("pixelate-aux-{i}")),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: tex_usage,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            aux_views.push(view);
            aux_textures.push(tex);
        }

        // Build bind groups for each pass.
        // Passes 0..n are halving passes (downscale), pass n is the upscale.
        let mut bind_groups = Vec::with_capacity(n as usize + 1);

        // Pass 0: reads from ping_pong[src_idx], writes to aux[0].
        // Needs two bind groups for the two possible ping-pong sources.
        if n > 0 {
            bind_groups.push(std::array::from_fn(|i| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(&format!("pixelate-half-0-{i}")),
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
                    ],
                })
            }));
        }

        // Passes 1..n: each reads from aux[i-1], writes to aux[i].
        // Source is fixed, so both ping-pong variants are identical.
        for i in 1..n as usize {
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("pixelate-half-{i}")),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&aux_views[i - 1]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
            });
            bind_groups.push([bg.clone(), bg]);
        }

        // Upscale pass: reads from the final (smallest) aux texture.
        // Uses linear or nearest sampler depending on `soft`.
        let nearest_sampler;
        let upscale_sampler = if self.soft {
            sampler
        } else {
            nearest_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("pixelate-nearest"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Nearest,
                min_filter: wgpu::FilterMode::Nearest,
                ..Default::default()
            });
            &nearest_sampler
        };

        // Source for upscale: the smallest aux texture, or ping_pong if n==0.
        let upscale_bind_groups: [wgpu::BindGroup; 2] = if n > 0 {
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("pixelate-up"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(aux_views.last().unwrap()),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(upscale_sampler),
                    },
                ],
            });
            [bg.clone(), bg]
        } else {
            // scale ~1.0, no downscaling — just blit through with chosen sampler.
            std::array::from_fn(|i| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(&format!("pixelate-up-{i}")),
                    layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&ping_pong_views[i]),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(upscale_sampler),
                        },
                    ],
                })
            })
        };
        bind_groups.push(upscale_bind_groups);

        EffectCache {
            uniform_bufs: vec![],
            bind_groups,
            aux_textures,
            aux_views,
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
        let n = cache.aux_views.len(); // number of halving passes

        // Halving passes: each renders to a progressively smaller aux texture.
        // Bilinear min_filter correctly averages 2x2 source texels per output pixel.
        for i in 0..n {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pixelate-half"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &cache.aux_views[i],
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
            // First pass selects ping-pong source; subsequent passes use [0].
            let bg_src = if i == 0 { src_idx } else { 0 };
            rpass.set_bind_group(0, &cache.bind_groups[i][bg_src], &[]);
            rpass.draw(0..3, 0..1);
        }

        // Upscale pass: render from smallest aux (or ping-pong if n==0)
        // to full-size destination.
        let upscale_idx = n; // last bind_group entry
        let upscale_src = if n == 0 { src_idx } else { 0 };
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
            rpass.set_bind_group(0, &cache.bind_groups[upscale_idx][upscale_src], &[]);
            rpass.draw(0..3, 0..1);
        }
    }
}

fn create_pixelate_pipeline(device: &wgpu::Device, format: wgpu::TextureFormat) -> EffectPipeline {
    // Pixelate uses the shared blit shader — the effect comes from
    // iteratively halving to a small texture (proper 2x2 averaging),
    // then upscaling with linear or nearest filtering.
    create_blit_pipeline(device, format, "pixelate")
}
