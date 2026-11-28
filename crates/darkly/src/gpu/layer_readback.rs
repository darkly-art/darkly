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
//! and the extent projects the resulting layer-local bytes onto the canvas
//! window: the frame the selection texture and the paint mask are indexed in.
//! Window pixels the texture does not reach hold the node's uncovered value
//! (transparent for a layer, white for a mask), which is the node's content
//! there: the extent is storage, not a boundary the producers can see.
//! **Do not** call `request_readback` with a canvas rect from such a call site;
//! go through this module.
//!
//! Producers layered on the projection:
//!
//! - [`LayerReadbackExtent::flood_fill_to_canvas_mask`] - magic wand and the
//!   paint-bucket fill tool, via the scanline fills in [`crate::gpu::flood_fill`].
//! - [`LayerReadbackExtent::opacity_to_canvas_mask`] - "alpha to selection",
//!   the per-pixel opacity of the node.
//! - [`LayerReadbackExtent::clip_rgba`] - copy and cut: the node's content
//!   over the copied region as a clipboard clip, read through
//!   [`request_layer_region_readback`] so only the region is read back.

use crate::gpu::paint_target::GpuPaintTarget;
use crate::gpu::readback::{self, ReadbackRequest};

/// Snapshot of a paint target's coordinate frame, captured at readback-request
/// time and carried through the async round-trip.
///
/// Owns no GPU resources (pure metadata). Pairs with the readback request
/// returned by [`request_layer_readback`]: the request reads the texture's full
/// extent (`width × height` pixels starting at texture-local (0,0)), and this
/// struct provides the canvas↔texture translation on the other side so callers
/// receive a window-local R8 mask without re-deriving the layer offset.
#[derive(Copy, Clone)]
pub struct LayerReadbackExtent {
    /// Plane-space offset of the texture's (0, 0) pixel.
    pub offset_x: i32,
    pub offset_y: i32,
    /// Texture pixel dimensions: the size of the readback buffer.
    pub width: u32,
    pub height: u32,
    /// Document canvas (window) dimensions: the size of the produced mask.
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

    /// Run the CPU scanline fill over the node's content on the canvas
    /// window (see [`Self::window_content`]) and return the window-local R8
    /// fill mask. The fill's domain is the window, so a click on window area
    /// the texture does not reach fills the uncovered content there.
    ///
    /// `seed_canvas` is the click point in plane coordinates, translated to
    /// window-local coords before the fill runs. Format dispatch matches the
    /// texture's own format: RGBA reads four bytes per pixel, R8 reads one.
    pub fn flood_fill_to_canvas_mask(
        &self,
        pixels: &[u8],
        seed_canvas: crate::coord::CanvasPoint,
        tolerance: u8,
    ) -> Vec<u8> {
        use crate::gpu::flood_fill::{flood_fill_r8, flood_fill_rgba};

        let content = self.window_content(pixels);
        let seed_x = seed_canvas.x - self.canvas_origin_x;
        let seed_y = seed_canvas.y - self.canvas_origin_y;
        let fill = match self.format {
            wgpu::TextureFormat::R8Unorm => flood_fill_r8,
            _ => flood_fill_rgba,
        };
        fill(
            &content,
            self.canvas_width,
            self.canvas_height,
            seed_x,
            seed_y,
            tolerance,
        )
    }

    /// The node's per-pixel opacity as a window-local R8 mask: the coverage
    /// "alpha to selection" loads. An RGBA texture contributes its alpha
    /// channel; an R8 texture (mask / selection filter) *is* coverage already,
    /// so its bytes pass through.
    pub fn opacity_to_canvas_mask(&self, pixels: &[u8]) -> Vec<u8> {
        let content = self.window_content(pixels);
        match self.format {
            wgpu::TextureFormat::R8Unorm => content,
            _ => content.iter().skip(3).step_by(4).copied().collect(),
        }
    }

    /// The node's content over `region` as a clipboard clip, row-major over
    /// `region`: straight-alpha RGBA whose RGB is the node's value and whose
    /// alpha is coverage. An R8 node's value `v` becomes opaque gray
    /// `[v, v, v, 255]`, so a mask's black copies as black and the white past
    /// its texture travels with the clip; an RGBA node's texels pass through.
    /// `selection`, an R8 buffer over `region`, scales alpha only.
    pub fn clip_rgba(
        &self,
        pixels: &[u8],
        region: crate::coord::CanvasRect,
        selection: Option<&[u8]>,
    ) -> Vec<u8> {
        let content = self.region_content(pixels, region);
        let mut rgba = match self.format {
            wgpu::TextureFormat::R8Unorm => content.iter().flat_map(|&v| [v, v, v, 255]).collect(),
            _ => content,
        };
        if let Some(selection) = selection {
            for (px, &s) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(selection) {
                px[3] = ((px[3] as u16 * s as u16 + 127) / 255) as u8;
            }
        }
        rgba
    }

    /// The node's content over the canvas window: a **window-local** buffer
    /// (the frame the selection texture is indexed in, see `crate::coord`).
    fn window_content(&self, pixels: &[u8]) -> Vec<u8> {
        let window = crate::coord::CanvasRect::from_xywh(
            self.canvas_origin_x,
            self.canvas_origin_y,
            self.canvas_width,
            self.canvas_height,
        );
        self.region_content(pixels, window)
    }

    /// The node's content over the plane rect `region`: a buffer sized to
    /// `region` in the texture's own format, row-major from its top-left.
    /// Pixels the read texels cover take them; the rest take the format's
    /// uncovered value ([`crate::gpu::default_texel`]).
    fn region_content(&self, pixels: &[u8], region: crate::coord::CanvasRect) -> Vec<u8> {
        let bpp = crate::gpu::region_runs::bytes_per_pixel(self.format) as usize;
        let rw = region.width as usize;
        let mut content =
            crate::gpu::default_texel(self.format).repeat(rw * region.height as usize);

        // Plane-space bounds of the read texels clipped to `region`; the output
        // is written at `plane − region origin`.
        let x0 = self.offset_x.max(region.x0());
        let y0 = self.offset_y.max(region.y0());
        let x1 = (self.offset_x + self.width as i32).min(region.x0() + region.width as i32);
        let y1 = (self.offset_y + self.height as i32).min(region.y0() + region.height as i32);
        if x0 >= x1 || y0 >= y1 {
            return content;
        }

        let stride = self.width as usize;
        let row_bytes = (x1 - x0) as usize * bpp;
        let tx = (x0 - self.offset_x) as usize; // layer-local col
        let rx = (x0 - region.x0()) as usize; // region-local col
        for py in y0..y1 {
            let ty = (py - self.offset_y) as usize; // layer-local row
            let ry = (py - region.y0()) as usize; // region-local row
            let src = (ty * stride + tx) * bpp;
            let dst = (ry * rw + rx) * bpp;
            content[dst..dst + row_bytes].copy_from_slice(&pixels[src..src + row_bytes]);
        }

        content
    }
}

/// Encode a readback of a layer's full texture extent and return the request
/// paired with the extent snapshot the completion handler needs.
///
/// Single source of truth for the readback rect used by magic wand, the
/// paint-bucket flood fill, and alpha-to-selection. The rect is the texture's
/// own dimensions, NOT the canvas; see the module docs for why.
pub fn request_layer_readback(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    target: &GpuPaintTarget<'_>,
) -> (ReadbackRequest, LayerReadbackExtent) {
    let extent = LayerReadbackExtent::from_target(target);
    // Texture-local rect spanning the entire layer: the canvas↔texture
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

/// Encode a readback of the part of a node's texture inside the plane rect
/// `region`, for producers that need only that part (copy). The extent
/// describes the texels read, so the projections place them the same way they
/// place a full-extent read. The request is `None` when the texture does not
/// reach `region`: the producer still runs, on no texels, and sees only the
/// node's uncovered value.
pub fn request_layer_region_readback(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    target: &GpuPaintTarget<'_>,
    region: crate::coord::CanvasRect,
) -> (Option<ReadbackRequest>, LayerReadbackExtent) {
    let mut extent = LayerReadbackExtent::from_target(target);
    let Some(read) = target.canvas_extent().intersect(region) else {
        (extent.width, extent.height) = (0, 0);
        return (None, extent);
    };
    let local = crate::coord::LayerRect::from_xywh(
        (read.x0() - extent.offset_x) as u32,
        (read.y0() - extent.offset_y) as u32,
        read.width,
        read.height,
    );
    (extent.offset_x, extent.offset_y) = (read.x0(), read.y0());
    (extent.width, extent.height) = (read.width, read.height);
    let request =
        readback::request_readback(device, encoder, target.texture(), target.format(), local);
    (Some(request), extent)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2×2 layer sitting at plane (3, 2), inside a 6×6 canvas window whose
    /// origin is plane (2, 1), i.e. the layer projects to window-local
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

    /// REGRESSION: the produced mask is **window-local**; the magic-wand fill
    /// must land where the window-sized selection texture expects it after a
    /// crop, i.e. at `plane − canvas_origin`, not at the raw plane coordinate.
    #[test]
    fn flood_fill_mask_is_window_local_after_crop() {
        // A small 2×2 R8 layer, fully black: distinct from the white a mask
        // holds outside its texture, so the fill stops at the layer.
        let pixels = vec![0u8; 2 * 2];
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

    /// Alpha-to-selection reads the RGBA alpha channel, ignoring RGB. An
    /// erased pixel keeps ghost color under `a = 0` (straight-alpha storage)
    /// and must not be selected.
    #[test]
    fn opacity_mask_reads_rgba_alpha_only() {
        // 2×2 RGBA: opaque red, half-transparent red, transparent red ghost,
        // transparent black, reading row-major.
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

    /// An R8 node (mask filter) is coverage already; its bytes pass straight
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
        assert_eq!(at(0, 0), 255, "outside its texture a mask reveals");
    }

    /// The fill's domain is the window: seeded on window area the texture does
    /// not reach, it fills the uncovered (transparent) content there and
    /// stops at the layer's opaque pixels.
    #[test]
    fn flood_fill_seeded_outside_the_texture_fills_uncovered_content() {
        let pixels = [255u8, 0, 0, 255].repeat(2 * 2);
        let ext = cropped_extent(wgpu::TextureFormat::Rgba8Unorm);

        let mask = ext.flood_fill_to_canvas_mask(&pixels, crate::coord::CanvasPoint::new(2, 1), 0);
        let at = |x: usize, y: usize| mask[y * 6 + x];

        assert_eq!(at(0, 0), 255, "the seed, outside the texture");
        assert_eq!(at(5, 5), 255, "the far corner of the window");
        assert_eq!(at(1, 1), 0, "the opaque layer bounds the fill");
    }

    /// A mask's clip is opaque gray everywhere in the region: black stays
    /// black (not "nothing copied"), and the region past the texture carries
    /// the mask's white.
    #[test]
    fn mask_clip_is_opaque_gray_with_white_past_the_texture() {
        let pixels = vec![0u8, 64, 255, 32];
        let ext = cropped_extent(wgpu::TextureFormat::R8Unorm);
        // Plane (2, 2)..(5, 3): one pixel left of the texture, then its top row.
        let clip = ext.clip_rgba(
            &pixels,
            crate::coord::CanvasRect::from_xywh(2, 2, 3, 1),
            None,
        );
        assert_eq!(
            clip,
            [[255, 255, 255, 255], [0, 0, 0, 255], [64, 64, 64, 255]].concat()
        );
    }

    /// A layer's clip carries its texels, transparent past the texture, and a
    /// selection scales coverage only, never the value.
    #[test]
    fn clip_selection_scales_alpha_only() {
        let pixels = [200u8, 100, 50, 255].repeat(4);
        let ext = cropped_extent(wgpu::TextureFormat::Rgba8Unorm);
        let region = crate::coord::CanvasRect::from_xywh(2, 2, 2, 1);
        let clip = ext.clip_rgba(&pixels, region, Some(&[255, 128]));
        assert_eq!(clip, [[0, 0, 0, 0], [200, 100, 50, 128]].concat());

        let mask = cropped_extent(wgpu::TextureFormat::R8Unorm);
        let clip = mask.clip_rgba(&[0u8, 64, 255, 32], region, Some(&[64, 128]));
        assert_eq!(clip, [[255, 255, 255, 64], [0, 0, 0, 128]].concat());
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
