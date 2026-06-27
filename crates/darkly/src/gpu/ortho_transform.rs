//! Orthogonal (90°-multiple) transform GPU pass — exact pixel permutation.
//!
//! Flips and 90° rotations relabel texels without resampling, so this pass
//! `textureLoad`s each destination texel from its inverse-mapped source index
//! (no sampler, no filtering, no premultiply) — output is bit-identical to the
//! input up to the permutation. Contrast `rescale.rs`, which genuinely
//! resamples. One primitive serves both whole-document ops (canvas flip/rotate,
//! per node + the selection mask) and layer/selection flip (a masked in-place
//! mirror), so exactness is true by construction in both.
//!
//! Owned by the [`Compositor`](super::compositor::Compositor), which drives it
//! in `ortho_transform_nodes` and `flip_node_region`. See
//! `shaders/ortho_transform.wgsl`.

use crate::coord::CanvasRect;
use crate::gpu::atlas::LayerTexture;

/// One of the five orthogonal transforms. Discriminants match the `xform`
/// codes the shader switches on; keep the two in step.
#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OrthoXform {
    FlipH = 0,
    FlipV = 1,
    Rot180 = 2,
    Rot90Cw = 3,
    Rot90Ccw = 4,
}

impl OrthoXform {
    /// True for the two transforms that swap width and height.
    pub fn swaps_dims(self) -> bool {
        matches!(self, OrthoXform::Rot90Cw | OrthoXform::Rot90Ccw)
    }

    /// The transform that undoes this one (its inverse permutation).
    pub fn inverse(self) -> OrthoXform {
        match self {
            OrthoXform::FlipH => OrthoXform::FlipH,
            OrthoXform::FlipV => OrthoXform::FlipV,
            OrthoXform::Rot180 => OrthoXform::Rot180,
            OrthoXform::Rot90Cw => OrthoXform::Rot90Ccw,
            OrthoXform::Rot90Ccw => OrthoXform::Rot90Cw,
        }
    }

    /// Map a local pixel rect `(i0, j0, w, h)` inside a frame of size `(fw, fh)`
    /// to its transformed local rect. The forward map this and the shader's
    /// `inv_map` invert; the canonical source of the per-variant index algebra.
    /// `i0`/`j0` are `i32` (they can be negative for paste-extent nodes whose
    /// extent runs outside the frame).
    pub fn map_local(
        self,
        i0: i32,
        j0: i32,
        w: u32,
        h: u32,
        fw: u32,
        fh: u32,
    ) -> (i32, i32, u32, u32) {
        let (w, h, fw, fh) = (w as i32, h as i32, fw as i32, fh as i32);
        let (ni0, nj0, nw, nh) = match self {
            OrthoXform::FlipH => (fw - w - i0, j0, w, h),
            OrthoXform::FlipV => (i0, fh - h - j0, w, h),
            OrthoXform::Rot180 => (fw - w - i0, fh - h - j0, w, h),
            OrthoXform::Rot90Cw => (fh - h - j0, i0, h, w),
            OrthoXform::Rot90Ccw => (j0, fw - w - i0, h, w),
        };
        (ni0, nj0, nw as u32, nh as u32)
    }
}

/// Packed shader uniform: `[xform, src_w, src_h, has_mask]`.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    p: [u32; 4],
}

/// Render pipelines + layouts for the ortho-transform shader. One pipeline per
/// (entry point × target format).
pub struct OrthoTransformPass {
    remap_rgba: wgpu::RenderPipeline,
    remap_r8: wgpu::RenderPipeline,
    mirror_rgba: wgpu::RenderPipeline,
    mirror_r8: wgpu::RenderPipeline,
    /// `[src texture, params]` — used by `fs_remap`.
    remap_bgl: wgpu::BindGroupLayout,
    /// `[src texture, params, mask texture]` — used by `fs_mirror_masked`.
    mirror_bgl: wgpu::BindGroupLayout,
}

impl std::fmt::Debug for OrthoTransformPass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrthoTransformPass").finish_non_exhaustive()
    }
}

fn tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            // Sampled with textureLoad — no hardware filtering.
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

impl OrthoTransformPass {
    pub fn new(device: &wgpu::Device) -> Self {
        let remap_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ortho-remap-bgl"),
            entries: &[tex_entry(0), uniform_entry(1)],
        });
        let mirror_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ortho-mirror-bgl"),
            entries: &[tex_entry(0), uniform_entry(1), tex_entry(2)],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ortho-transform-shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../shaders/ortho_transform.wgsl").into(),
            ),
        });

        let make =
            |bgl: &wgpu::BindGroupLayout, entry: &str, format: wgpu::TextureFormat, label: &str| {
                let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some(label),
                    bind_group_layouts: &[Some(bgl)],
                    immediate_size: 0,
                });
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
        OrthoTransformPass {
            remap_rgba: make(&remap_bgl, "fs_remap", rgba, "ortho-remap-rgba"),
            remap_r8: make(&remap_bgl, "fs_remap", r8, "ortho-remap-r8"),
            mirror_rgba: make(&mirror_bgl, "fs_mirror_masked", rgba, "ortho-mirror-rgba"),
            mirror_r8: make(&mirror_bgl, "fs_mirror_masked", r8, "ortho-mirror-r8"),
            remap_bgl,
            mirror_bgl,
        }
    }

    /// Whole-surface transform: permute `src` (at its old extent) into a fresh
    /// `LayerTexture` at `new_extent` (dims already swapped for rotations by the
    /// caller). Exact `textureLoad` gather; `format` selects RGBA vs R8.
    #[allow(clippy::too_many_arguments)]
    pub fn remap_node(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        src: &LayerTexture,
        new_extent: CanvasRect,
        xform: OrthoXform,
        format: wgpu::TextureFormat,
    ) -> LayerTexture {
        let is_r8 = format == wgpu::TextureFormat::R8Unorm;
        let old = src.layer_extent();
        let dest = if is_r8 {
            LayerTexture::new_mask_with_extent(device, queue, new_extent)
        } else {
            LayerTexture::with_bounds(device, new_extent)
        };
        let params = Params {
            p: [xform as u32, old.width, old.height, 0],
        };
        let pipeline = if is_r8 {
            &self.remap_r8
        } else {
            &self.remap_rgba
        };
        let bind_group = self.remap_bind_group(device, queue, src.view(), &params);
        self.run(encoder, pipeline, &bind_group, dest.view());
        dest
    }

    /// Permute an arbitrary `src` view (dims `src_w`×`src_h`) into `out_view`
    /// (sized for `xform`), for surfaces the compositor manages directly (the
    /// selection mask's ping-pong textures). Exact `textureLoad` gather.
    #[allow(clippy::too_many_arguments)]
    pub fn render_remap(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        src_view: &wgpu::TextureView,
        out_view: &wgpu::TextureView,
        src_w: u32,
        src_h: u32,
        xform: OrthoXform,
        format: wgpu::TextureFormat,
    ) {
        let pipeline = if format == wgpu::TextureFormat::R8Unorm {
            &self.remap_r8
        } else {
            &self.remap_rgba
        };
        let params = Params {
            p: [xform as u32, src_w, src_h, 0],
        };
        let bind_group = self.remap_bind_group(device, queue, src_view, &params);
        self.run(encoder, pipeline, &bind_group, out_view);
    }

    /// In-place H/V mirror of a region about its own centre, gated by an R8
    /// mask: where `mask_view` is selected (>0.5) the texel takes the mirror,
    /// elsewhere it passes through unchanged. `src_view` and `out_view` are the
    /// same `w`×`h` region (caller copies the live region into `src_view` and
    /// copies `out_view` back). `xform` must be `FlipH` or `FlipV`.
    #[allow(clippy::too_many_arguments)]
    pub fn render_mirror_masked(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        src_view: &wgpu::TextureView,
        mask_view: &wgpu::TextureView,
        out_view: &wgpu::TextureView,
        w: u32,
        h: u32,
        xform: OrthoXform,
        format: wgpu::TextureFormat,
    ) {
        debug_assert!(matches!(xform, OrthoXform::FlipH | OrthoXform::FlipV));
        let pipeline = if format == wgpu::TextureFormat::R8Unorm {
            &self.mirror_r8
        } else {
            &self.mirror_rgba
        };
        let params = Params {
            p: [xform as u32, w, h, 1],
        };
        let ubuf = self.params_buffer(device, queue, &params);
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ortho-mirror-bg"),
            layout: &self.mirror_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: ubuf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(mask_view),
                },
            ],
        });
        self.run(encoder, pipeline, &bind_group, out_view);
    }

    fn params_buffer(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        params: &Params,
    ) -> wgpu::Buffer {
        let ubuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ortho-params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&ubuf, 0, bytemuck::bytes_of(params));
        ubuf
    }

    fn remap_bind_group(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        src_view: &wgpu::TextureView,
        params: &Params,
    ) -> wgpu::BindGroup {
        let ubuf = self.params_buffer(device, queue, params);
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ortho-remap-bg"),
            layout: &self.remap_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: ubuf.as_entire_binding(),
                },
            ],
        })
    }

    fn run(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::RenderPipeline,
        bind_group: &wgpu::BindGroup,
        output_view: &wgpu::TextureView,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ortho-transform-pass"),
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
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_local_flip_h_offset_node() {
        // Frame 10 wide; node at cols [0,2] (w=3) → [7,9].
        let (ni0, nj0, nw, nh) = OrthoXform::FlipH.map_local(0, 4, 3, 5, 10, 8);
        assert_eq!((ni0, nj0, nw, nh), (7, 4, 3, 5));
    }

    #[test]
    fn map_local_flip_v_offset_node() {
        let (ni0, nj0, nw, nh) = OrthoXform::FlipV.map_local(2, 0, 3, 5, 10, 8);
        assert_eq!((ni0, nj0, nw, nh), (2, 3, 3, 5));
    }

    #[test]
    fn map_local_rot180_is_flip_h_then_v() {
        let (ni0, nj0, nw, nh) = OrthoXform::Rot180.map_local(0, 0, 3, 5, 10, 8);
        assert_eq!((ni0, nj0, nw, nh), (7, 3, 3, 5));
    }

    #[test]
    fn map_local_rot90_swaps_dims() {
        // CW: a node w×h becomes h×w in an fh×fw frame.
        let (ni0, nj0, nw, nh) = OrthoXform::Rot90Cw.map_local(0, 0, 3, 5, 10, 8);
        // ni0 = fh - h - j0 = 8 - 5 - 0 = 3; nj0 = i0 = 0; dims swap to (5, 3).
        assert_eq!((ni0, nj0, nw, nh), (3, 0, 5, 3));

        let (ni0, nj0, nw, nh) = OrthoXform::Rot90Ccw.map_local(0, 0, 3, 5, 10, 8);
        // ni0 = j0 = 0; nj0 = fw - w - i0 = 10 - 3 - 0 = 7; dims swap to (5, 3).
        assert_eq!((ni0, nj0, nw, nh), (0, 7, 5, 3));
    }

    #[test]
    fn rot90_round_trips_to_identity() {
        // CW then CCW restores the original local rect (odd dims too).
        let (i0, j0, w, h, fw, fh) = (2, 1, 3, 5, 9, 7);
        let (ai, aj, aw, ah) = OrthoXform::Rot90Cw.map_local(i0, j0, w, h, fw, fh);
        // After a CW turn the frame is fh×fw.
        let (bi, bj, bw, bh) = OrthoXform::Rot90Ccw.map_local(ai, aj, aw, ah, fh, fw);
        assert_eq!((bi, bj, bw, bh), (i0, j0, w, h));
    }

    #[test]
    fn flips_are_self_inverse() {
        assert_eq!(OrthoXform::FlipH.inverse(), OrthoXform::FlipH);
        assert_eq!(OrthoXform::FlipV.inverse(), OrthoXform::FlipV);
        assert_eq!(OrthoXform::Rot180.inverse(), OrthoXform::Rot180);
        assert_eq!(OrthoXform::Rot90Cw.inverse(), OrthoXform::Rot90Ccw);
        assert_eq!(OrthoXform::Rot90Ccw.inverse(), OrthoXform::Rot90Cw);
    }
}
