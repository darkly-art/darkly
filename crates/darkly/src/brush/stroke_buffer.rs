//! Stroke buffer — separate texture for dab rendering during a stroke,
//! enabling mid-stroke rewind and re-rendering for the stabilizer.
//!
//! Three textures:
//! - **stroke texture**: dabs render here instead of directly to the layer.
//! - **pre-stroke texture**: snapshot of the layer before the stroke started,
//!   used to restore both the stroke buffer and the layer on rewind.
//! - **checkpoint texture**: snapshot of the stroke buffer at the last checkpoint,
//!   used for O(1) restore on divergence via same-frame GPU→GPU copy.
//!
//! The composite step writes the final result to the layer each frame:
//! source-over blend of the stroke buffer onto the pre-stroke snapshot.

use super::pipelines::{BrushPipelines, CompositeUniforms};

/// Manages the stroke-in-progress, pre-stroke, and checkpoint textures.
pub struct StrokeBuffer {
    /// Dabs render into this texture (instead of directly to the layer).
    stroke_texture: wgpu::Texture,
    stroke_view: wgpu::TextureView,

    /// Snapshot of the layer before the stroke started.
    pre_stroke_texture: wgpu::Texture,
    #[allow(dead_code)] // Kept alive for bind group references.
    pre_stroke_view: wgpu::TextureView,

    /// Snapshot of the stroke buffer at the last checkpoint.
    /// Written by `save_checkpoint`, read by `restore_checkpoint`.
    /// Never touched by dab rendering — only by explicit copy commands.
    checkpoint_texture: wgpu::Texture,

    /// Bind group for the stroke texture, compatible with the dab texture BGL
    /// so the existing composite pipeline can read it.
    stroke_bind_group: wgpu::BindGroup,

    /// Bind group for the pre-stroke texture, compatible with the canvas copy BGL
    /// so the existing composite pipeline can read it as the background.
    pre_stroke_bind_group: wgpu::BindGroup,

    width: u32,
    height: u32,
}

impl StrokeBuffer {
    /// Create a new stroke buffer matching the given canvas dimensions.
    ///
    /// `dab_bgl` must be the bind group layout from `DabTexturePool` (texture+sampler).
    /// `canvas_copy_bgl` must be the canvas copy bind group layout from `BrushPipelines`.
    pub fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        dab_bgl: &wgpu::BindGroupLayout,
        canvas_copy_bgl: &wgpu::BindGroupLayout,
    ) -> Self {
        let stroke_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("stroke-buffer"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let stroke_view = stroke_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let pre_stroke_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pre-stroke-snapshot"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let pre_stroke_view = pre_stroke_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let checkpoint_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("stroke-checkpoint"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("stroke-buffer-sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let stroke_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("stroke-buffer-bg"),
            layout: dab_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&stroke_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let pre_stroke_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pre-stroke-bg"),
            layout: canvas_copy_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&pre_stroke_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        Self {
            stroke_texture,
            stroke_view,
            pre_stroke_texture,
            pre_stroke_view,
            checkpoint_texture,
            stroke_bind_group,
            pre_stroke_bind_group,
            width,
            height,
        }
    }

    /// The texture view dabs render into.
    pub fn stroke_view(&self) -> &wgpu::TextureView {
        &self.stroke_view
    }

    /// The stroke texture (for use as copy source/dest).
    pub fn stroke_texture(&self) -> &wgpu::Texture {
        &self.stroke_texture
    }

    /// Clear the stroke buffer to transparent.
    pub fn clear(&self, encoder: &mut wgpu::CommandEncoder) {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("clear-stroke-buffer"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.stroke_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.0, g: 0.0, b: 0.0, a: 0.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        // Render pass clears on begin — drop immediately.
    }

    /// Save a snapshot of the layer texture before the stroke starts.
    pub fn save_pre_stroke(&self, encoder: &mut wgpu::CommandEncoder, layer_texture: &wgpu::Texture) {
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: layer_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.pre_stroke_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
    }

    /// Restore the stroke buffer from the pre-stroke snapshot within a region.
    /// This is the "rewind" operation — clears dabs from the stroke buffer.
    pub fn restore_region(&self, encoder: &mut wgpu::CommandEncoder, bbox: [u32; 4]) {
        let [x, y, w, h] = bbox;
        if w == 0 || h == 0 { return; }
        // Clamp to texture bounds.
        let w = w.min(self.width.saturating_sub(x));
        let h = h.min(self.height.saturating_sub(y));
        if w == 0 || h == 0 { return; }

        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.pre_stroke_texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.stroke_texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    }

    /// Composite the stroke buffer onto the layer texture.
    ///
    /// The composite is: source-over blend of stroke_buffer onto pre_stroke,
    /// written to the layer texture.  Uses the existing brush composite pipeline
    /// with the stroke buffer as the "dab" and pre_stroke as the "canvas copy".
    ///
    /// For v1, always composites the full canvas.  Dirty-rect optimization
    /// (Phase D) would restrict this to the union of rewind + new dabs regions.
    pub fn composite_onto_layer(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipelines: &BrushPipelines,
        queue: &wgpu::Queue,
        layer_view: &wgpu::TextureView,
        selection_bind_group: &wgpu::BindGroup,
    ) {
        // Full-canvas composite.  origin=[0,0] so the composite shader's
        // copy_uv = canvas_pos / textureDimensions(pre_stroke) = correct UV
        // into the full-canvas pre_stroke texture.
        pipelines.write_composite_uniforms(queue, &CompositeUniforms {
            origin: [0.0, 0.0],
            size: [self.width as f32, self.height as f32],
            canvas_size: [self.width as f32, self.height as f32],
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
            blend_mode: 0, // source-over
            _pad: 0,
        });

        // Composite: render the stroke buffer onto the layer using the existing
        // brush composite pipeline.  The stroke buffer serves as the "dab texture"
        // (group 1) and the pre_stroke serves as the "canvas copy" (group 3).
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("stroke-buffer-composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: layer_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_viewport(
                0.0, 0.0,
                self.width as f32, self.height as f32,
                0.0, 1.0,
            );
            pass.set_pipeline(pipelines.composite_pipeline());
            pass.set_bind_group(0, &pipelines.composite_uniform_bind_group, &[]);
            pass.set_bind_group(1, &self.stroke_bind_group, &[]);
            pass.set_bind_group(2, selection_bind_group, &[]);
            pass.set_bind_group(3, &self.pre_stroke_bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
    }

    /// GPU-copy the stroke buffer into the checkpoint texture.
    /// Same-frame, same encoder — no async delay.
    pub fn save_checkpoint(&self, encoder: &mut wgpu::CommandEncoder) {
        let size = wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 };
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.stroke_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.checkpoint_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            size,
        );
    }

    /// GPU-copy the checkpoint texture back into the stroke buffer.
    /// Restores the stroke buffer to the checkpoint state.
    pub fn restore_checkpoint(&self, encoder: &mut wgpu::CommandEncoder) {
        let size = wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 };
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.checkpoint_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.stroke_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            size,
        );
    }
}
