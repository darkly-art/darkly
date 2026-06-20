//! Shared selection→bind-group plumbing used by copy/cut and transform.

use super::DarklyEngine;

impl DarklyEngine {
    /// Crop the live GPU selection at `region` (window-local) into a fresh R8
    /// texture and return a paint-pipeline bind group sampling it. The returned
    /// bind group keeps the underlying texture alive for its own lifetime, so
    /// it stays valid after the caller drops every other handle. Returns `None`
    /// if there is no selection state allocated.
    ///
    /// The crop texture is allocated at `region` size; the portion the live
    /// selection actually covers is blitted in, and the remainder reads as
    /// zero (no selection) thanks to wgpu's lazy zero-initialization.
    pub(crate) fn selection_region_bind_group(
        &self,
        region: crate::coord::WindowRect,
        filter: wgpu::FilterMode,
    ) -> Option<wgpu::BindGroup> {
        let sel_tex = self.compositor.selection_state()?.texture();

        let (crop_tex, crop_view) = crate::gpu::create_texture_with_view(
            &self.gpu.device,
            region.width,
            region.height,
            wgpu::TextureFormat::R8Unorm,
            "selection-region-crop",
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );

        // The selection texture is window-sized, so `region`'s window-local
        // origin indexes it directly; clamp the copy extent to its bounds.
        let sx = region.x0().max(0) as u32;
        let sy = region.y0().max(0) as u32;
        let cw = region.width.min(self.doc.width.saturating_sub(sx));
        let ch = region.height.min(self.doc.height.saturating_sub(sy));
        if cw > 0 && ch > 0 {
            self.gpu.encode("selection-region-crop", |encoder| {
                crate::gpu::blit_region(encoder, sel_tex, (sx, sy), &crop_tex, (0, 0), cw, ch);
            });
        }

        let sampler = self.gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("selection-region-sampler"),
            mag_filter: filter,
            min_filter: filter,
            ..Default::default()
        });
        Some(self.paint_pipelines.create_selection_bind_group(
            &self.gpu.device,
            &crop_view,
            &sampler,
        ))
    }
}
