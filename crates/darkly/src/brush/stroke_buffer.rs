//! Stroke buffer — the per-stroke GPU resources that survive between pen
//! events.
//!
//! Three pieces:
//! - **`scratch`**: the writable stroke scratch and its R/W-hazard read
//!   mirror (see [`Scratch`]).  Dabs render into the write side; brush
//!   shaders sample the read mirror.
//! - **`pre_stroke_texture`**: snapshot of the layer before the stroke
//!   started, used to restore both the stroke buffer and the layer on
//!   rewind.
//! - The per-event composite step writes the final result to the layer:
//!   source-over blend of the scratch onto the pre-stroke snapshot.

use crate::brush::pipeline::BrushPipelines;
use crate::brush::scratch::Scratch;
use crate::coord::CanvasRect;

/// A frozen texture for the brush program's sampled source slot
/// (`@group(3)`), captured once at stroke start by
/// [`StrokeBuffer::save_source_snapshot`] when the stroke samples
/// something other than the painted layer — e.g. clone's cross-layer /
/// sample-merged modes. When absent, the pre-stroke snapshot fills the
/// slot instead. Always RGBA8 (R8 mask sources broadcast during capture,
/// sharing `save_pre_stroke_snapshot`'s format handling).
pub struct SourceSnapshot {
    texture: wgpu::Texture,
    /// Plane-space rect the snapshot covers — the *sampled* texture's
    /// frame, which the shader needs to map plane positions to snapshot
    /// UVs. Frozen with the pixels: destination-layer growth mid-stroke
    /// must not move it.
    frame: CanvasRect,
}

/// Manages the stroke-in-progress scratch (write+read pair) and pre-stroke
/// snapshot textures.
///
/// `StrokeBuffer` owns the raw GPU resources; the stroke *semantics* (how
/// the scratch is initialised, how it lands on the layer) belong to the
/// active terminal node's lifecycle hooks (`begin_stroke` / `commit`). This
/// keeps the engine free of terminal-type branching — swapping in a warp or
/// smudge terminal doesn't require editing this file.
pub struct StrokeBuffer {
    /// Writable stroke scratch + per-dab read mirror, managed as one unit.
    /// The R/W hazard workaround is encapsulated inside `Scratch`.
    scratch: Scratch,

    /// Snapshot of the layer before the stroke started.
    pre_stroke_texture: wgpu::Texture,
    pre_stroke_view: wgpu::TextureView,

    /// Bind group for the pre-stroke texture, compatible with the canvas copy BGL
    /// so the existing composite pipeline can read it as the background.
    pre_stroke_bind_group: wgpu::BindGroup,

    /// Frozen texture for the brush's sampled source slot, or `None` when
    /// the pre-stroke snapshot is the source (same-layer clone) or the
    /// brush doesn't sample one at all. See [`SourceSnapshot`].
    source_snapshot: Option<SourceSnapshot>,

    width: u32,
    height: u32,
}

impl StrokeBuffer {
    /// Create a new stroke buffer matching the given canvas dimensions.
    ///
    /// `pipelines` provides the canvas-copy BGL/sampler that the embedded
    /// `Scratch` needs for both its read-mirror and write bind groups.
    pub fn new(device: &wgpu::Device, width: u32, height: u32, pipelines: &BrushPipelines) -> Self {
        let scratch = Scratch::new(
            device,
            width,
            height,
            pipelines.canvas_copy_bind_group_layout(),
            pipelines.canvas_copy_sampler(),
        );

        let pre_stroke_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pre-stroke-snapshot"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            // RENDER_ATTACHMENT is required when the source layer is an R8
            // mask: `GpuPaintTarget::save_pre_stroke_snapshot` runs a
            // broadcast render pass instead of `copy_texture_to_texture`.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let pre_stroke_view =
            pre_stroke_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let pre_stroke_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("pre-stroke-sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let pre_stroke_bind_group = build_pre_stroke_bind_group(
            device,
            pipelines.canvas_copy_bind_group_layout(),
            &pre_stroke_view,
            &pre_stroke_sampler,
        );

        Self {
            scratch,
            pre_stroke_texture,
            pre_stroke_view,
            pre_stroke_bind_group,
            source_snapshot: None,
            width,
            height,
        }
    }

    /// The embedded scratch (write + read mirror).  Borrow mutably — the
    /// per-dab read-mirror sync may need to lazy-grow.
    pub fn scratch_mut(&mut self) -> &mut Scratch {
        &mut self.scratch
    }

    /// Immutable access to the scratch.  For accessors only — anything
    /// that might trigger a grow goes through `scratch_mut`.
    pub fn scratch(&self) -> &Scratch {
        &self.scratch
    }

    /// Split-borrow accessor for `BrushGpuContext` construction: returns
    /// a mutable reference to the scratch alongside immutable references
    /// to the pre-stroke resources and the optional source snapshot, all
    /// from one `&mut self` borrow.  The borrow checker permits this
    /// because the function body proves the borrows are disjoint
    /// (different fields).
    pub fn parts_for_brush_ctx(
        &mut self,
    ) -> (
        &mut Scratch,
        &wgpu::Texture,
        &wgpu::BindGroup,
        Option<&wgpu::Texture>,
    ) {
        (
            &mut self.scratch,
            &self.pre_stroke_texture,
            &self.pre_stroke_bind_group,
            self.source_snapshot.as_ref().map(|cs| &cs.texture),
        )
    }

    /// The pre-stroke snapshot texture.
    pub fn pre_stroke_texture(&self) -> &wgpu::Texture {
        &self.pre_stroke_texture
    }

    /// The pre-stroke snapshot view — the destination of `save_pre_stroke_snapshot`
    /// when the source is an R8 mask (which goes through a render pass instead
    /// of `copy_texture_to_texture`).
    pub fn pre_stroke_view(&self) -> &wgpu::TextureView {
        &self.pre_stroke_view
    }

    /// Bind group over the pre-stroke snapshot using the canvas-copy BGL —
    /// the composite pipeline binds this as the background at commit time.
    pub fn pre_stroke_bind_group(&self) -> &wgpu::BindGroup {
        &self.pre_stroke_bind_group
    }

    /// Save a snapshot of the paint target's pixels into the pre-stroke
    /// snapshot texture. Format-aware via `GpuPaintTarget`'s extension trait:
    /// RGBA8 sources use `copy_texture_to_texture` (hardware-fast); R8 mask
    /// sources go through a broadcast render pass that turns each `r` into
    /// `(r, r, r, 1)` in the RGBA8 snapshot.
    pub fn save_pre_stroke(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        brush_pipelines: &BrushPipelines,
        paint_target: &crate::gpu::paint_target::GpuPaintTarget<'_>,
    ) {
        use crate::brush::paint_target_ext::BrushPaintTargetExt;
        paint_target.save_pre_stroke_snapshot(
            device,
            encoder,
            brush_pipelines,
            &self.pre_stroke_view,
            &self.pre_stroke_texture,
        );
    }

    /// Freeze `source` into this stroke's [`SourceSnapshot`] — the texture
    /// the brush's sampled source slot binds instead of the pre-stroke
    /// snapshot (e.g. clone's pinned layer or the root composite). Wrapped
    /// as a paint target so the capture shares `save_pre_stroke_snapshot`'s
    /// format handling (R8 mask sources broadcast to RGBA8). Records into
    /// `encoder`; the caller submits. Strokes that sample the painted
    /// layer itself never call this — the pre-stroke snapshot is their
    /// source.
    pub fn save_source_snapshot(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        brush_pipelines: &BrushPipelines,
        source: &crate::gpu::paint_target::GpuPaintTarget<'_>,
    ) {
        use crate::brush::paint_target_ext::BrushPaintTargetExt;
        let extent = source.layer_extent();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("stroke-source-snapshot"),
            size: wgpu::Extent3d {
                width: extent.width,
                height: extent.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            // RENDER_ATTACHMENT for the R8-mask broadcast pass, COPY_DST
            // for the same-format hardware copy — mirrors the pre-stroke
            // snapshot's dual capture path.
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        // The view only serves the R8-broadcast capture pass; sampling at
        // flush time creates its own binding view from the texture.
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        source.save_pre_stroke_snapshot(device, encoder, brush_pipelines, &view, &texture);
        self.source_snapshot = Some(SourceSnapshot {
            texture,
            frame: source.canvas_extent(),
        });
    }

    /// Plane-space frame of the frozen source snapshot, if one was
    /// captured for this stroke. `None` means the pre-stroke snapshot is
    /// the source (or the brush samples none) — the caller falls back to
    /// the paint target's live extent.
    pub fn source_snapshot_frame(&self) -> Option<CanvasRect> {
        self.source_snapshot.as_ref().map(|cs| cs.frame)
    }

    /// Current scratch dimensions in pixels (the write side, which is layer-sized).
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Reallocate the writable scratch and pre-stroke snapshot textures to
    /// `(new_w, new_h)`, copying existing contents into the new textures
    /// at `(dst_offset_x, dst_offset_y)`. Used during mid-stroke layer
    /// growth so the scratch keeps its canvas-anchored pre-stroke pixels
    /// even though the layer's local-coord origin has shifted.
    ///
    /// The read mirror inside `scratch` is **not** touched here — its
    /// size is per-dab footprint-driven, not layer-driven, and a layer
    /// growth doesn't change what footprint the next dab will request.
    /// The next `Scratch::sync_read_mirror` call will re-copy in the new
    /// write-side coordinate frame.
    ///
    /// `source_snapshot` is also left alone: it lives in the *sampled*
    /// texture's frozen frame, and destination growth doesn't move it.
    pub fn grow_preserving(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        new_w: u32,
        new_h: u32,
        dst_offset_x: u32,
        dst_offset_y: u32,
        canvas_copy_bgl: &wgpu::BindGroupLayout,
    ) {
        if new_w == self.width && new_h == self.height && dst_offset_x == 0 && dst_offset_y == 0 {
            return;
        }
        let target_w = new_w.max(self.width);
        let target_h = new_h.max(self.height);

        // Grow the writable scratch atomically via Scratch — caller can't
        // forget either side.  Read mirror stays footprint-sized.
        self.scratch.grow_write(
            device,
            encoder,
            target_w,
            target_h,
            dst_offset_x,
            dst_offset_y,
        );

        let new_pre_stroke_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pre-stroke-snapshot"),
            size: wgpu::Extent3d {
                width: target_w,
                height: target_h,
                depth_or_array_layers: 1,
            },
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
        let new_pre_stroke_view =
            new_pre_stroke_tex.create_view(&wgpu::TextureViewDescriptor::default());

        // Copy existing pre-stroke contents into the new texture at the
        // canvas-anchored offset.
        if self.width > 0 && self.height > 0 {
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.pre_stroke_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &new_pre_stroke_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: dst_offset_x,
                        y: dst_offset_y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: self.width,
                    height: self.height,
                    depth_or_array_layers: 1,
                },
            );
        }

        let pre_stroke_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("pre-stroke-sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let new_pre_stroke_bg = build_pre_stroke_bind_group(
            device,
            canvas_copy_bgl,
            &new_pre_stroke_view,
            &pre_stroke_sampler,
        );

        self.pre_stroke_texture = new_pre_stroke_tex;
        self.pre_stroke_view = new_pre_stroke_view;
        self.pre_stroke_bind_group = new_pre_stroke_bg;
        self.width = target_w;
        self.height = target_h;
    }
}

fn build_pre_stroke_bind_group(
    device: &wgpu::Device,
    canvas_copy_bgl: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("pre-stroke-bg"),
        layout: canvas_copy_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}
