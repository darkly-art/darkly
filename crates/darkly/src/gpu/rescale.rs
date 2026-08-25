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
    /// `[old_size.x, old_size.y, is_r8, premul_io]`
    p2: [f32; 4],
}

/// Number of mip levels a `width × height` texture supports, i.e.
/// `floor(log2(max(w, h))) + 1` — level 0 plus one per halving down to 1×1.
pub fn levels_for(width: u32, height: u32) -> u32 {
    32 - width.max(height).max(1).leading_zeros()
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
            source: wgpu::ShaderSource::Wgsl(include_str!("../../shaders/rescale.wgsl").into()),
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
            self.halve_into(device, queue, encoder, &input_view, &out_view, is_r8, false);
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

    /// Box-reduce `src_view` to half size into `dst_view`. The single rung of
    /// the pyramid, target-agnostic: the caller owns both views, so the same
    /// pass drives standalone intermediates (the rescale chain) and views onto
    /// consecutive mip levels of one texture.
    ///
    /// `premul_io` selects the alpha convention. `false` is the straight-alpha
    /// layer/mask path (premultiply on load, un-premultiply on store); `true`
    /// means source and destination are both premultiplied and texels average
    /// as-is. Ignored when `is_r8` — single-channel masks never round-trip.
    #[allow(clippy::too_many_arguments)]
    pub fn halve_into(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        src_view: &wgpu::TextureView,
        dst_view: &wgpu::TextureView,
        is_r8: bool,
        premul_io: bool,
    ) {
        // A halve reads texel-space (`dst * 2`) and ignores every geometric
        // field, so only the two convention flags carry information.
        let params = Params {
            p0: [0.0, 0.0, 0.0, 0.0],
            p1: [1.0, 1.0, 0.0, 0.0],
            p2: [
                1.0,
                1.0,
                if is_r8 { 1.0 } else { 0.0 },
                if premul_io { 1.0 } else { 0.0 },
            ],
        };
        let pipeline = if is_r8 {
            &self.halve_r8
        } else {
            &self.halve_rgba
        };
        self.run_pass(
            device, queue, encoder, pipeline, src_view, dst_view, &params,
        );
    }

    /// Fill mip levels `1..levels` of `texture` by repeatedly box-reducing the
    /// level above. Level 0 must already hold the image.
    ///
    /// The texture must carry `RENDER_ATTACHMENT | TEXTURE_BINDING` and have
    /// been allocated with `mip_level_count >= levels` (see [`levels_for`]).
    /// `premul_io` matches [`Self::halve_into`]: pass `true` for a source
    /// stored in premultiplied alpha, which is the convention every sampled
    /// void source uses.
    pub fn generate_mip_chain(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        texture: &wgpu::Texture,
        levels: u32,
        premul_io: bool,
    ) {
        let is_r8 = texture.format() == wgpu::TextureFormat::R8Unorm;
        let level_view = |level: u32| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("rescale-mip-level"),
                base_mip_level: level,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        for level in 1..levels {
            let src_view = level_view(level - 1);
            let dst_view = level_view(level);
            self.halve_into(
                device, queue, encoder, &src_view, &dst_view, is_r8, premul_io,
            );
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::test_utils::{readback_texture, test_device};

    /// Allocate a mipped RGBA texture, upload `level0`, generate the chain,
    /// and read `level` back. Reading a non-zero mip needs a bounce through a
    /// fresh single-level texture — `readback_texture` always copies mip 0.
    fn chain_level(
        width: u32,
        height: u32,
        level0: &[u8],
        level: u32,
        premul_io: bool,
    ) -> (Vec<u8>, u32, u32) {
        let (device, queue) = test_device();
        let pass = RescalePass::new(&device);
        let levels = levels_for(width, height);
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mip-test-src"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            level0,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        let (lw, lh) = ((width >> level).max(1), (height >> level).max(1));
        let dst = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mip-test-dst"),
            size: wgpu::Extent3d {
                width: lw,
                height: lh,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let mut encoder = device.create_command_encoder(&Default::default());
        pass.generate_mip_chain(&device, &queue, &mut encoder, &tex, levels, premul_io);
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &dst,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: lw,
                height: lh,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);

        let bytes = readback_texture(
            &device,
            &queue,
            &dst,
            wgpu::TextureFormat::Rgba8Unorm,
            lw,
            lh,
        );
        (bytes, lw, lh)
    }

    #[test]
    fn levels_for_matches_floor_log2() {
        assert_eq!(levels_for(1, 1), 1);
        assert_eq!(levels_for(2, 2), 2);
        assert_eq!(levels_for(1024, 768), 11);
        assert_eq!(levels_for(1000, 1), 10);
        assert_eq!(levels_for(4096, 4096), 13);
        // Degenerate input must not panic or produce a zero-level chain.
        assert_eq!(levels_for(0, 0), 1);
    }

    /// A uniform image must survive halving exactly — any premultiply /
    /// un-premultiply asymmetry in the chain shows up here first.
    #[test]
    fn two_by_two_solid_halves_to_one_texel() {
        let level0: Vec<u8> = std::iter::repeat_n([64u8, 64, 64, 255], 4)
            .flatten()
            .collect();
        let (out, w, h) = chain_level(2, 2, &level0, 1, true);
        assert_eq!((w, h), (1, 1));
        assert_eq!(
            out[..4],
            [64, 64, 64, 255],
            "averaging four identical premultiplied texels must reproduce them exactly",
        );
    }

    /// The 4→2 reduction samples the exact 4-texel corner, so a per-texel
    /// checker averages to the arithmetic mean of its block. Values are
    /// premultiplied going in and must stay premultiplied coming out.
    #[test]
    fn four_by_four_checker_averages_exactly() {
        let mut level0 = Vec::new();
        for y in 0..4 {
            for x in 0..4 {
                if (x + y) % 2 == 0 {
                    level0.extend_from_slice(&[0, 0, 0, 0]);
                } else {
                    level0.extend_from_slice(&[200, 100, 50, 255]);
                }
            }
        }
        let (out, w, h) = chain_level(4, 4, &level0, 1, true);
        assert_eq!((w, h), (2, 2));
        // Each 2×2 block holds two transparent and two opaque texels.
        let expect = [100i32, 50, 25, 128];
        for (i, px) in out.chunks_exact(4).enumerate() {
            for (c, (&got, &want)) in px.iter().zip(expect.iter()).enumerate() {
                assert!(
                    (i32::from(got) - want).abs() <= 1,
                    "texel {i} channel {c}: got {got}, want {want} (±1 for 8-bit rounding)",
                );
            }
        }
    }

    /// `premul_io` is the only behavioural difference from the straight-alpha
    /// rescale path, so the same input must produce a different, predictable
    /// result under each setting. With the flag off the average is
    /// un-premultiplied on store, restoring the opaque texel's full colour.
    #[test]
    fn premul_io_flag_controls_the_alpha_round_trip() {
        let mut level0 = Vec::new();
        for y in 0..4 {
            for x in 0..4 {
                if (x + y) % 2 == 0 {
                    level0.extend_from_slice(&[0, 0, 0, 0]);
                } else {
                    level0.extend_from_slice(&[200, 100, 50, 255]);
                }
            }
        }
        let (straight, ..) = chain_level(4, 4, &level0, 1, false);
        let expect = [200i32, 100, 50, 128];
        for (c, (&got, &want)) in straight[..4].iter().zip(expect.iter()).enumerate() {
            assert!(
                (i32::from(got) - want).abs() <= 1,
                "channel {c}: got {got}, want {want} — with premul_io off the \
                 averaged colour is divided back out by the averaged alpha",
            );
        }
    }
}
