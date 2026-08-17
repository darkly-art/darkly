//! Layer-aware readback orchestration: the canvas↔texture↔window translation
//! every op that consumes a layer's pixels on the CPU goes through.
//!
//! A GPU layer texture is **not** in general canvas-aligned: it can sit at a
//! non-zero canvas offset and be larger or smaller than the canvas
//! (paste-extent layers, leftward-grown layers from `ensure_layer_covers_dab`,
//! masks parented to off-canvas raster layers).
//!
//! [`request_layer_readback`] + [`LayerReadbackExtent`] are the single place
//! that owns that translation: the readback samples the texture's full extent,
//! and the extent projects the resulting layer-local bytes into a window-local
//! R8 mask — the frame the selection texture and the paint mask are indexed in.
//! **Do not** call `request_readback` with a canvas rect from such a call site;
//! go through this module.
//!
//! Producers layered on the projection:
//!
//! - [`LayerReadbackExtent::flood_fill_to_canvas_mask`] — magic wand and the
//!   paint-bucket fill tool, via the scanline fills in [`crate::gpu::flood_fill`].
//! - [`LayerReadbackExtent::opacity_to_canvas_mask`] — "alpha to selection",
//!   the per-pixel opacity of the node.

use crate::gpu::paint_target::GpuPaintTarget;
use crate::gpu::readback::{self, ReadbackRequest};

/// Snapshot of a paint target's coordinate frame, captured at readback-request
/// time and carried through the async round-trip.
///
/// Owns no GPU resources — pure metadata. Pairs with the readback request
/// returned by [`request_layer_readback`]: the request reads the texture's full
/// extent (`width × height` pixels starting at texture-local (0,0)), and this
/// struct provides the canvas↔texture translation on the other side so callers
/// receive a window-local R8 mask without re-deriving the layer offset.
#[derive(Copy, Clone)]
pub struct LayerReadbackExtent {
    /// Plane-space offset of the texture's (0, 0) pixel.
    pub offset_x: i32,
    pub offset_y: i32,
    /// Texture pixel dimensions — the size of the readback buffer.
    pub width: u32,
    pub height: u32,
    /// Document canvas (window) dimensions — the size of the produced mask.
    pub canvas_width: u32,
    pub canvas_height: u32,
    /// Plane-space origin of the canvas window. The produced mask is
    /// **window-local** (it uploads into the window-sized selection texture),
    /// so the projection subtracts this. `(0, 0)` for an un-cropped doc.
    pub canvas_origin_x: i32,
    pub canvas_origin_y: i32,
    pub format: wgpu::TextureFormat,
}

impl LayerReadbackExtent {
    pub fn from_target(target: &GpuPaintTarget<'_>) -> Self {
        let canvas_extent = target.canvas_extent();
        let layer_extent = target.layer_extent();
        let (canvas_w, canvas_h) = target.canvas_size();
        let (cox, coy) = target.canvas_origin();
        Self {
            offset_x: canvas_extent.x0(),
            offset_y: canvas_extent.y0(),
            width: layer_extent.width,
            height: layer_extent.height,
            canvas_width: canvas_w,
            canvas_height: canvas_h,
            canvas_origin_x: cox,
            canvas_origin_y: coy,
            format: target.format(),
        }
    }

    /// Run the CPU scanline fill on the texture-extent buffer and project the
    /// result into a window-local R8 mask (see [`Self::project_to_window_mask`]).
    ///
    /// `seed_canvas` is the click point in plane coordinates, translated to
    /// texture-local coords before the fill runs. Format dispatch matches the
    /// texture's own format — RGBA reads four bytes per pixel, R8 reads one.
    pub fn flood_fill_to_canvas_mask(
        &self,
        pixels: &[u8],
        seed_canvas: crate::coord::CanvasPoint,
        tolerance: u8,
    ) -> Vec<u8> {
        use crate::gpu::flood_fill::{flood_fill_r8, flood_fill_rgba};

        let layer_seed_x = seed_canvas.x - self.offset_x;
        let layer_seed_y = seed_canvas.y - self.offset_y;

        let layer_mask = match self.format {
            wgpu::TextureFormat::R8Unorm => flood_fill_r8(
                pixels,
                self.width,
                self.height,
                layer_seed_x,
                layer_seed_y,
                tolerance,
            ),
            _ => flood_fill_rgba(
                pixels,
                self.width,
                self.height,
                layer_seed_x,
                layer_seed_y,
                tolerance,
            ),
        };

        self.project_to_window_mask(&layer_mask)
    }

    /// The node's per-pixel opacity as a window-local R8 mask — the coverage
    /// "alpha to selection" loads. An RGBA texture contributes its alpha
    /// channel; an R8 texture (mask / selection filter) *is* coverage already,
    /// so its bytes pass through.
    pub fn opacity_to_canvas_mask(&self, pixels: &[u8]) -> Vec<u8> {
        match self.format {
            wgpu::TextureFormat::R8Unorm => self.project_to_window_mask(pixels),
            _ => {
                let alpha: Vec<u8> = pixels.iter().skip(3).step_by(4).copied().collect();
                self.project_to_window_mask(&alpha)
            }
        }
    }

    /// Project a layer-local R8 buffer (one byte per texture pixel) into a
    /// **window-local** R8 mask sized `canvas_width × canvas_height` — the
    /// frame the selection texture is indexed in (see `crate::coord`).
    ///
    /// Pixels outside the layer's canvas-window footprint stay 0.
    fn project_to_window_mask(&self, layer_mask: &[u8]) -> Vec<u8> {
        let cw = self.canvas_width as usize;
        let ch = self.canvas_height as usize;
        let mut canvas_mask = vec![0u8; cw * ch];

        let (cox, coy) = (self.canvas_origin_x, self.canvas_origin_y);
        // Plane-space bounds of the layer footprint clipped to the canvas
        // WINDOW `[canvas_origin, canvas_origin + canvas_size]`; the output is
        // written at the window-local texel `plane − canvas_origin`.
        let x0 = self.offset_x.max(cox);
        let y0 = self.offset_y.max(coy);
        let x1 = (self.offset_x + self.width as i32).min(cox + self.canvas_width as i32);
        let y1 = (self.offset_y + self.height as i32).min(coy + self.canvas_height as i32);
        if x0 >= x1 || y0 >= y1 {
            return canvas_mask;
        }

        let stride = self.width as usize;
        for py in y0..y1 {
            let ty = (py - self.offset_y) as usize; // layer-local row
            let src_row = ty * stride;
            let dst_row = (py - coy) as usize * cw; // window-local row
            for px in x0..x1 {
                let tx = (px - self.offset_x) as usize; // layer-local col
                let wx = (px - cox) as usize; // window-local col
                canvas_mask[dst_row + wx] = layer_mask[src_row + tx];
            }
        }

        canvas_mask
    }
}

/// Encode a readback of a layer's full texture extent and return the request
/// paired with the extent snapshot the completion handler needs.
///
/// Single source of truth for the readback rect used by magic wand, the
/// paint-bucket flood fill, and alpha-to-selection. The rect is the texture's
/// own dimensions, NOT the canvas — see the module docs for why.
pub fn request_layer_readback(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    target: &GpuPaintTarget<'_>,
) -> (ReadbackRequest, LayerReadbackExtent) {
    let extent = LayerReadbackExtent::from_target(target);
    // Texture-local rect spanning the entire layer — the canvas↔texture
    // translation happens later, in the extent's projection.
    let request = readback::request_readback(
        device,
        encoder,
        target.texture(),
        target.format(),
        target.layer_extent(),
    );
    (request, extent)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2×2 layer sitting at plane (3, 2), inside a 6×6 canvas window whose
    /// origin is plane (2, 1) — i.e. the layer projects to window-local
    /// [1,3)×[1,3).
    fn cropped_extent(format: wgpu::TextureFormat) -> LayerReadbackExtent {
        LayerReadbackExtent {
            offset_x: 3,
            offset_y: 2,
            width: 2,
            height: 2,
            canvas_width: 6,
            canvas_height: 6,
            canvas_origin_x: 2,
            canvas_origin_y: 1,
            format,
        }
    }

    /// REGRESSION: the produced mask is **window-local** — the magic-wand fill
    /// must land where the window-sized selection texture expects it after a
    /// crop, i.e. at `plane − canvas_origin`, not at the raw plane coordinate.
    #[test]
    fn flood_fill_mask_is_window_local_after_crop() {
        // A small 2×2 R8 layer, fully opaque.
        let pixels = vec![255u8; 2 * 2];
        let ext = cropped_extent(wgpu::TextureFormat::R8Unorm);

        // Seed inside the layer (plane (3, 2)); uniform color floods all of it.
        let mask = ext.flood_fill_to_canvas_mask(&pixels, crate::coord::CanvasPoint::new(3, 2), 0);
        let at = |x: usize, y: usize| mask[y * 6 + x];

        // Layer plane footprint [3,5)×[2,4) → window-local [1,3)×[1,3).
        assert_eq!(at(1, 1), 255, "window-local origin of the fill");
        assert_eq!(at(2, 2), 255, "window-local far corner of the fill");
        // The pre-fix plane-anchored projection would have written here instead.
        assert_eq!(at(3, 2), 0, "must NOT land at the raw plane coordinate");
        assert_eq!(at(0, 0), 0, "outside the fill stays empty");
    }

    /// Alpha-to-selection reads the RGBA alpha channel, ignoring RGB — an
    /// erased pixel keeps ghost color under `a = 0` (straight-alpha storage)
    /// and must not be selected.
    #[test]
    fn opacity_mask_reads_rgba_alpha_only() {
        // 2×2 RGBA: opaque red, half-transparent red, transparent red ghost,
        // transparent black — reading row-major.
        let pixels = vec![
            255, 0, 0, 255, // (0, 0)
            255, 0, 0, 128, // (1, 0)
            255, 0, 0, 0, // (0, 1)
            0, 0, 0, 0, // (1, 1)
        ];
        let ext = cropped_extent(wgpu::TextureFormat::Rgba8Unorm);

        let mask = ext.opacity_to_canvas_mask(&pixels);
        let at = |x: usize, y: usize| mask[y * 6 + x];

        assert_eq!(at(1, 1), 255, "opaque texel is fully selected");
        assert_eq!(at(2, 1), 128, "partial alpha carries through as coverage");
        assert_eq!(at(1, 2), 0, "ghost RGB under alpha = 0 is not selected");
        assert_eq!(at(2, 2), 0);
    }

    /// An R8 node (mask filter) is coverage already — its bytes pass straight
    /// through the projection.
    #[test]
    fn opacity_mask_passes_r8_coverage_through() {
        let pixels = vec![255u8, 64, 0, 32];
        let ext = cropped_extent(wgpu::TextureFormat::R8Unorm);

        let mask = ext.opacity_to_canvas_mask(&pixels);
        let at = |x: usize, y: usize| mask[y * 6 + x];

        assert_eq!(at(1, 1), 255);
        assert_eq!(at(2, 1), 64);
        assert_eq!(at(1, 2), 0);
        assert_eq!(at(2, 2), 32);
        assert_eq!(at(0, 0), 0, "outside the layer footprint stays empty");
    }

    /// A layer entirely outside the canvas window projects to an empty mask
    /// rather than indexing out of bounds.
    #[test]
    fn opacity_mask_of_offscreen_layer_is_empty() {
        let mut ext = cropped_extent(wgpu::TextureFormat::Rgba8Unorm);
        ext.offset_x = 40;
        ext.offset_y = 40;

        let mask = ext.opacity_to_canvas_mask(&[255u8; 2 * 2 * 4]);
        assert!(mask.iter().all(|&m| m == 0));
        assert_eq!(mask.len(), 6 * 6);
    }
}
