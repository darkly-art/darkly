//! Brush extensions on `GpuPaintTarget`.
//!
//! The brush stack operates in RGBA8 throughout (composition source); the
//! paint target's storage format may be RGBA8 (raster layer) or R8 (mask).
//! Format-bridging at the paint-surface boundary lives here, **not** in the
//! brush terminals; terminals call uniform methods on the paint target and
//! never branch on R8 vs RGBA8.
//!
//! Three operations bridge:
//!   - **`commit_brush_dab`** (write): RGBA8 scratch + RGBA8 pre-stroke →
//!     paint target. Same WGSL shader; pipeline picked by `self.format`.
//!     The GPU silently writes only `.r` to R8 targets.
//!   - **`save_pre_stroke_snapshot`** (read): paint target → RGBA8 snapshot.
//!     RGBA8 source → `copy_texture_to_texture` (hardware-fast). R8 source →
//!     broadcast render pass (`(r, r, r, 1)` into RGBA8 destination).
//!   - **`commit_scratch_blit`** (liquify-style write): RGBA8 scratch →
//!     paint target. RGBA8 dest → `copy_texture_to_texture`. R8 dest →
//!     passthrough render pass (the GPU writes only `.r`).
//!
//! Round-trip property: an unmodified pixel `v` in an R8 mask source is read
//! as `(v, v, v, 1)` (broadcast), passes through the brush stack unmodified,
//! and is committed back as `mix(pre.r, scratch.r, dab.a)`. With `scratch.r =
//! v` and `pre.r = v`, the result equals `v` exactly. Smudge/warp produce
//! `mix(v_dst, v_src, dab.a)`: identical to a pure-R8 blend.

use crate::brush::composite_pipeline::{CompositePipeline, CompositeUniforms};
use crate::brush::pipeline::BrushPipelines;
use crate::gpu::paint_target::GpuPaintTarget;

pub trait BrushPaintTargetExt {
    /// Commit a stroke's finished accumulations onto the paint target.
    ///
    /// Two foreground slots, each with a fixed law. `wash` is laid down
    /// through the per-pigment deposit ceiling, which refuses anything past
    /// what one pass over the pixel would deposit; `build` is then
    /// composited on top with plain Porter-Duff source-over. `None` is an
    /// absent slot, which the shader never reads. Both foregrounds must be
    /// premultiplied, which is how every terminal accumulates.
    ///
    /// Which accumulation goes in which slot is the terminal's knowledge:
    /// a brush under `Wash` fills the first, one under `Build-up` (and
    /// watercolor) fills the second, and a brush inside the accumulation
    /// dial fills both, its `Max` scratch and its source-over channel.
    ///
    /// `opacity` is the stroke-level cap, applied to every present slot.
    /// `blend_mode` is 0 for paint and 1 for erase, where each slot removes
    /// its own coverage and the ceiling is not consulted: removal must be
    /// able to reach zero.
    ///
    /// Selection is already baked into the scratch by the per-dab
    /// composites, so the commit never samples it.
    fn commit_brush_dab(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        brush_pipelines: &BrushPipelines,
        queue: &wgpu::Queue,
        wash: Option<&wgpu::BindGroup>,
        build: Option<&wgpu::BindGroup>,
        pre_stroke_bg: &wgpu::BindGroup,
        opacity: f32,
        blend_mode: u32,
    );

    /// Populate an RGBA8 pre-stroke snapshot from this paint target.
    ///
    /// `snapshot_view` and `snapshot_texture` reference the snapshot's RGBA8
    /// destination. Source and destination must share `(width, height)`:
    /// the brush's `StrokeBuffer` already sizes its pre-stroke snapshot to
    /// match the paint target's pixel dimensions.
    fn save_pre_stroke_snapshot(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        brush_pipelines: &BrushPipelines,
        snapshot_view: &wgpu::TextureView,
        snapshot_texture: &wgpu::Texture,
    );

    /// Commit a fully-rendered RGBA8 scratch onto the paint target with no
    /// blending. Used by liquify-style terminals that produce the final
    /// pixel state in the scratch and need a straight overwrite. RGBA8 dest
    /// is a hardware copy; R8 dest goes through a fragment shader (the GPU
    /// drops G/B/A on the R8 target).
    fn commit_scratch_blit(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        brush_pipelines: &BrushPipelines,
        scratch_view: &wgpu::TextureView,
        scratch_texture: &wgpu::Texture,
    );
}

impl BrushPaintTargetExt for GpuPaintTarget<'_> {
    fn commit_brush_dab(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        brush_pipelines: &BrushPipelines,
        queue: &wgpu::Queue,
        wash: Option<&wgpu::BindGroup>,
        build: Option<&wgpu::BindGroup>,
        pre_stroke_bg: &wgpu::BindGroup,
        opacity: f32,
        blend_mode: u32,
    ) {
        // An absent slot still needs something bound to satisfy the
        // pipeline layout, so it borrows the present one and is switched
        // off by its opacity. With neither there is nothing to commit.
        let (Some(wash_bg), Some(build_bg)) = (wash.or(build), build.or(wash)) else {
            return;
        };

        let canvas_ext = self.canvas_extent();
        let layer_w = canvas_ext.width as f32;
        let layer_h = canvas_ext.height as f32;
        let layer_off_x = canvas_ext.x0() as f32;
        let layer_off_y = canvas_ext.y0() as f32;
        let uniforms = CompositeUniforms {
            origin: [layer_off_x, layer_off_y],
            size: [layer_w, layer_h],
            target_offset: [layer_off_x, layer_off_y],
            target_size: [layer_w, layer_h],
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
            blend_mode,
            wash_opacity: if wash.is_some() { opacity } else { 0.0 },
            build_opacity: if build.is_some() { opacity } else { 0.0 },
        };
        let composite = brush_pipelines.get::<CompositePipeline>("composite");
        let offset = composite.write_uniforms(queue, &uniforms);

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("paint-target-commit-brush-dab"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: self.view(),
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_viewport(0.0, 0.0, layer_w, layer_h, 0.0, 1.0);
        pass.set_pipeline(composite.pipeline(self.format()));
        pass.set_bind_group(0, composite.uniform_bind_group(), &[offset]);
        pass.set_bind_group(1, wash_bg, &[]);
        pass.set_bind_group(2, build_bg, &[]);
        pass.set_bind_group(3, pre_stroke_bg, &[]);
        pass.draw(0..6, 0..1);
    }

    fn save_pre_stroke_snapshot(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        brush_pipelines: &BrushPipelines,
        snapshot_view: &wgpu::TextureView,
        snapshot_texture: &wgpu::Texture,
    ) {
        let extent = self.layer_extent();
        if self.format() != wgpu::TextureFormat::R8Unorm {
            // Same-format hardware copy: fast path for raster layers.
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: self.texture(),
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: snapshot_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: extent.width,
                    height: extent.height,
                    depth_or_array_layers: 1,
                },
            );
            return;
        }

        // R8 source → RGBA8 destination via broadcast render pass.
        let _ = snapshot_texture; // referenced only on the same-format path
        let source_bg = brush_pipelines.create_blit_source_bind_group(device, self.view());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("paint-target-save-pre-stroke-r8"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: snapshot_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_viewport(
            0.0,
            0.0,
            extent.width as f32,
            extent.height as f32,
            0.0,
            1.0,
        );
        pass.set_pipeline(brush_pipelines.mask_blit_pipeline());
        pass.set_bind_group(0, &source_bg, &[]);
        pass.draw(0..3, 0..1);
    }

    fn commit_scratch_blit(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        brush_pipelines: &BrushPipelines,
        scratch_view: &wgpu::TextureView,
        scratch_texture: &wgpu::Texture,
    ) {
        let extent = self.layer_extent();
        if self.format() != wgpu::TextureFormat::R8Unorm {
            // Same-format hardware copy: preserves today's path for layers.
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: scratch_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: self.texture(),
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: extent.width,
                    height: extent.height,
                    depth_or_array_layers: 1,
                },
            );
            return;
        }

        // RGBA8 scratch → R8 dest via passthrough render pass; GPU writes
        // only `.r` to the R8 target.
        let _ = scratch_texture; // used only on the same-format path
        let source_bg = brush_pipelines.create_blit_source_bind_group(device, scratch_view);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("paint-target-commit-scratch-blit-r8"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: self.view(),
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_viewport(
            0.0,
            0.0,
            extent.width as f32,
            extent.height as f32,
            0.0,
            1.0,
        );
        pass.set_pipeline(brush_pipelines.scratch_blit_r8_pipeline());
        pass.set_bind_group(0, &source_bg, &[]);
        pass.draw(0..3, 0..1);
    }
}
