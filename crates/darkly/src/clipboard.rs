//! Clipboard system — typed internal clipboard with extensible content types.
//!
//! Phase 6 implements the `ImageData` variant for pixel copy/paste.
//! The `Clipboard` enum is designed so future variants (full layers with
//! masks/blend modes, layer groups, etc.) can be added without refactoring.

use crate::layer::RasterLayer;
use crate::tile::{AlphaMask, Rgba, TileGrid, Tile, TILE_SIZE};

// ---------------------------------------------------------------------------
// Clipboard enum — extensible content container
// ---------------------------------------------------------------------------

/// Typed clipboard content. New variants can be added for future content types
/// (e.g. full layers, layer groups) without refactoring the clipboard system.
pub enum Clipboard {
    /// Flattened RGBA pixel region — used for canvas copy/paste and external interop.
    ImageData(ImageClip),
    // Future variants (not implemented in Phase 6):
    // Layer(LayerClip),        — full layer with mask, blend mode, opacity
    // LayerGroup(GroupClip),   — group with children
}

impl Clipboard {
    /// Extract an `ImageClip` reference, regardless of variant.
    /// Future layer/group variants would flatten themselves to pixels on demand.
    pub fn as_image(&self) -> Option<&ImageClip> {
        match self {
            Clipboard::ImageData(clip) => Some(clip),
        }
    }
}

// ---------------------------------------------------------------------------
// ImageClip — flattened RGBA pixel region
// ---------------------------------------------------------------------------

/// A rectangular region of RGBA pixels stored as sparse tiles.
/// Created by copy operations, consumed by paste operations.
pub struct ImageClip {
    /// RGBA pixel data as sparse tiles.
    pub tiles: TileGrid,
    /// Bounding box in canvas pixel coordinates: (x, y, width, height).
    pub bounds: (i32, i32, u32, u32),
}

impl ImageClip {
    /// Extract pixels from a raster layer, optionally masked by a selection.
    ///
    /// - If `selection` is `Some`, copies only the selected region (pixels
    ///   multiplied by selection coverage). The bounding rect comes from the
    ///   selection's tile extent.
    /// - If `selection` is `None`, copies the entire layer content within the
    ///   document bounds.
    ///
    /// Returns `None` if there are no pixels to copy (empty layer or empty selection).
    pub fn from_layer(
        layer: &RasterLayer,
        selection: Option<&AlphaMask>,
        doc_width: u32,
        doc_height: u32,
    ) -> Option<Self> {
        let ts = TILE_SIZE as i32;

        // Determine the tile-coordinate bounding rect to iterate.
        let (tx_min, ty_min, tx_max, ty_max) = if let Some(sel) = selection {
            sel.bounding_rect()?
        } else {
            // No selection — use union of layer tile extent and doc bounds.
            if layer.tiles.is_empty() {
                return None;
            }
            let mut min_x = i32::MAX;
            let mut min_y = i32::MAX;
            let mut max_x = i32::MIN;
            let mut max_y = i32::MIN;
            for ((tx, ty), _) in layer.tiles.iter() {
                min_x = min_x.min(tx);
                min_y = min_y.min(ty);
                max_x = max_x.max(tx);
                max_y = max_y.max(ty);
            }
            (min_x, min_y, max_x, max_y)
        };

        let mut result = TileGrid::new();
        let mut any_pixel = false;

        for ty in ty_min..=ty_max {
            for tx in tx_min..=tx_max {
                let layer_tile = layer.tiles.get(tx, ty);
                let sel_tile = selection.and_then(|s| s.get(tx, ty));

                // If selection exists but has no tile here, this region is unselected — skip.
                if selection.is_some() && sel_tile.is_none() {
                    continue;
                }

                // If layer has no tile here, nothing to copy.
                let layer_tile = match layer_tile {
                    Some(t) => t,
                    None => continue,
                };

                let src = layer_tile.data();
                let dst_tile = result.get_or_create(tx, ty);
                let dst = dst_tile.write();

                for py in 0..TILE_SIZE {
                    for px in 0..TILE_SIZE {
                        let pixel = src.pixel(px, py);

                        if let Some(st) = sel_tile {
                            // Multiply alpha by selection coverage.
                            let coverage = st.data().get(px, py);
                            if coverage <= 0.0 {
                                continue;
                            }
                            let a = (pixel[3] as f32 * coverage).round() as u8;
                            if a == 0 {
                                continue;
                            }
                            // Premultiply-correct: scale RGB by the coverage ratio.
                            let ratio = if pixel[3] > 0 {
                                a as f32 / pixel[3] as f32
                            } else {
                                0.0
                            };
                            let out = dst.pixel_mut(px, py);
                            out[0] = (pixel[0] as f32 * ratio).round() as u8;
                            out[1] = (pixel[1] as f32 * ratio).round() as u8;
                            out[2] = (pixel[2] as f32 * ratio).round() as u8;
                            out[3] = a;
                            any_pixel = true;
                        } else {
                            // No selection — copy pixel as-is.
                            if pixel[3] > 0 {
                                dst.pixel_mut(px, py).copy_from_slice(pixel);
                                any_pixel = true;
                            }
                        }
                    }
                }
            }
        }

        if !any_pixel {
            return None;
        }

        // Compute pixel-coordinate bounding box.
        let x = tx_min * ts;
        let y = ty_min * ts;
        let w = ((tx_max - tx_min + 1) * ts) as u32;
        let h = ((ty_max - ty_min + 1) * ts) as u32;

        // Clamp to document bounds.
        let x = x.max(0);
        let y = y.max(0);
        let w = w.min(doc_width.saturating_sub(x as u32));
        let h = h.min(doc_height.saturating_sub(y as u32));

        Some(ImageClip {
            tiles: result,
            bounds: (x, y, w, h),
        })
    }

    /// Create an `ImageClip` from raw RGBA bytes (e.g. from an external paste).
    ///
    /// The bytes are chunked into TILE_SIZE×TILE_SIZE tiles. Fully transparent
    /// tiles are skipped to keep the sparse grid efficient.
    pub fn from_rgba(
        width: u32,
        height: u32,
        rgba: &[u8],
        offset_x: i32,
        offset_y: i32,
    ) -> Self {
        let ts = TILE_SIZE as i32;
        let mut tiles = TileGrid::new();

        // Compute the tile range that covers the image.
        let tx_min = TileGrid::tile_coords_for_pixel(offset_x, 0).0;
        let ty_min = TileGrid::tile_coords_for_pixel(0, offset_y).1;
        let tx_max = TileGrid::tile_coords_for_pixel(offset_x + width as i32 - 1, 0).0;
        let ty_max = TileGrid::tile_coords_for_pixel(0, offset_y + height as i32 - 1).1;

        let w = width as i32;
        let stride = width as usize * 4;

        for ty in ty_min..=ty_max {
            for tx in tx_min..=tx_max {
                let tile_origin_x = tx * ts;
                let tile_origin_y = ty * ts;
                let mut any_pixel = false;
                let mut tile = Tile::<Rgba>::new_empty();
                let data = tile.write();

                for py in 0..TILE_SIZE {
                    let img_y = tile_origin_y + py as i32 - offset_y;
                    if img_y < 0 || img_y >= height as i32 {
                        continue;
                    }
                    for px in 0..TILE_SIZE {
                        let img_x = tile_origin_x + px as i32 - offset_x;
                        if img_x < 0 || img_x >= w {
                            continue;
                        }
                        let src_offset = (img_y as usize * stride) + (img_x as usize * 4);
                        let a = rgba[src_offset + 3];
                        if a == 0 {
                            continue;
                        }
                        let out = data.pixel_mut(px, py);
                        out[0] = rgba[src_offset];
                        out[1] = rgba[src_offset + 1];
                        out[2] = rgba[src_offset + 2];
                        out[3] = a;
                        any_pixel = true;
                    }
                }

                if any_pixel {
                    // Must re-insert into the grid since we built the tile externally.
                    let dst = tiles.get_or_create(tx, ty);
                    *dst = tile;
                }
            }
        }

        ImageClip {
            tiles,
            bounds: (offset_x, offset_y, width, height),
        }
    }

    /// Export the clip to a contiguous RGBA byte buffer for JS-side PNG encoding.
    ///
    /// Returns `(bytes, width, height, offset_x, offset_y)`.
    pub fn to_rgba(&self) -> (Vec<u8>, u32, u32, i32, i32) {
        let (x, y, w, h) = self.bounds;
        let w = w as usize;
        let h = h as usize;
        let ts = TILE_SIZE as i32;

        let mut buf = vec![0u8; w * h * 4];

        for ((tx, ty), tile) in self.tiles.iter() {
            let tile_origin_x = tx * ts;
            let tile_origin_y = ty * ts;
            let data = tile.data();

            for py in 0..TILE_SIZE {
                let img_y = tile_origin_y + py as i32 - y;
                if img_y < 0 || img_y >= h as i32 {
                    continue;
                }
                for px in 0..TILE_SIZE {
                    let img_x = tile_origin_x + px as i32 - x;
                    if img_x < 0 || img_x >= w as i32 {
                        continue;
                    }
                    let pixel = data.pixel(px, py);
                    if pixel[3] == 0 {
                        continue;
                    }
                    let dst_offset = (img_y as usize * w + img_x as usize) * 4;
                    buf[dst_offset..dst_offset + 4].copy_from_slice(pixel);
                }
            }
        }

        (buf, w as u32, h as u32, x, y)
    }

    /// Write this clip's tiles into a layer's TileGrid at the given offset.
    pub fn write_to_layer(
        &self,
        tiles: &mut TileGrid,
        offset_x: i32,
        offset_y: i32,
    ) {
        let ts = TILE_SIZE as i32;
        let (clip_x, clip_y, ..) = self.bounds;

        for ((tx, ty), src_tile) in self.tiles.iter() {
            let src_origin_x = tx * ts;
            let src_origin_y = ty * ts;
            let src = src_tile.data();

            for py in 0..TILE_SIZE {
                let canvas_y = src_origin_y + py as i32 - clip_y + offset_y;
                for px in 0..TILE_SIZE {
                    let canvas_x = src_origin_x + px as i32 - clip_x + offset_x;

                    let pixel = src.pixel(px, py);
                    if pixel[3] == 0 {
                        continue;
                    }

                    let (dtx, dty) = TileGrid::tile_coords_for_pixel(canvas_x, canvas_y);
                    let lx = canvas_x.rem_euclid(ts) as usize;
                    let ly = canvas_y.rem_euclid(ts) as usize;

                    let dst_tile = tiles.get_or_create(dtx, dty);
                    dst_tile.write().pixel_mut(lx, ly).copy_from_slice(pixel);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::{AlphaF32, AlphaMask};

    #[test]
    fn round_trip_rgba() {
        // Create a 4x4 red image at offset (0, 0).
        let w = 4u32;
        let h = 4u32;
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for i in 0..16 {
            rgba[i * 4] = 255;     // R
            rgba[i * 4 + 3] = 255; // A
        }

        let clip = ImageClip::from_rgba(w, h, &rgba, 0, 0);
        assert_eq!(clip.bounds, (0, 0, 4, 4));

        let (out, ow, oh, ox, oy) = clip.to_rgba();
        assert_eq!((ow, oh), (4, 4));
        assert_eq!((ox, oy), (0, 0));
        assert_eq!(out[0], 255); // R
        assert_eq!(out[1], 0);   // G
        assert_eq!(out[2], 0);   // B
        assert_eq!(out[3], 255); // A
    }

    #[test]
    fn from_layer_no_selection() {
        let mut layer = RasterLayer::new(1);
        // Paint a single pixel at (10, 10).
        let (tx, ty) = TileGrid::tile_coords_for_pixel(10, 10);
        let lx = 10usize % TILE_SIZE;
        let ly = 10usize % TILE_SIZE;
        layer.tiles.get_or_create(tx, ty).write().pixel_mut(lx, ly)
            .copy_from_slice(&[255, 0, 0, 255]);

        let clip = ImageClip::from_layer(&layer, None, 256, 256).unwrap();
        // Should have a tile at (0, 0) containing our pixel.
        assert!(clip.tiles.get(0, 0).is_some());
    }

    #[test]
    fn from_layer_with_selection() {
        let mut layer = RasterLayer::new(1);
        // Paint at (5, 5) — fully opaque.
        layer.tiles.get_or_create(0, 0).write().pixel_mut(5, 5)
            .copy_from_slice(&[100, 200, 50, 255]);

        // Selection with 50% coverage at (5, 5).
        let mut sel = AlphaMask::new();
        sel.get_or_create(0, 0).write().set(5, 5, 0.5);

        let clip = ImageClip::from_layer(&layer, Some(&sel), 256, 256).unwrap();
        let (buf, _, _, _, _) = clip.to_rgba();

        // Pixel at (5, 5) in tile (0,0) — offset in output buffer.
        let idx = (5 * TILE_SIZE + 5) * 4;
        // Alpha should be ~128 (255 * 0.5).
        assert!((buf[idx + 3] as i32 - 128).abs() <= 1);
    }

    #[test]
    fn empty_layer_returns_none() {
        let layer = RasterLayer::new(1);
        assert!(ImageClip::from_layer(&layer, None, 256, 256).is_none());
    }

    #[test]
    fn write_to_layer_at_offset() {
        let rgba = vec![255, 0, 0, 255]; // 1x1 red pixel
        let clip = ImageClip::from_rgba(1, 1, &rgba, 0, 0);

        let mut tiles = TileGrid::new();
        clip.write_to_layer(&mut tiles, 32, 32);

        // Pixel should be at canvas (32, 32).
        let (tx, ty) = TileGrid::tile_coords_for_pixel(32, 32);
        let lx = 32usize % TILE_SIZE;
        let ly = 32usize % TILE_SIZE;
        let t = tiles.get(tx, ty).expect("tile should exist");
        assert_eq!(t.data().pixel(lx, ly), &[255, 0, 0, 255]);
    }
}
