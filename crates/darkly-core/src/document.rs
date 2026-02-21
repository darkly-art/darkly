use std::collections::HashMap;

use crate::dirty::DirtyRegion;
use crate::layer::*;
use crate::tile::{TileGrid, TILE_SIZE};

pub struct Document {
    pub layers: Vec<Layer>,
    pub width: u32,
    pub height: u32,
    pub dirty: HashMap<LayerId, DirtyRegion>,
    next_id: LayerId,
}

impl Document {
    pub fn new(width: u32, height: u32) -> Self {
        Document {
            layers: Vec::new(),
            width,
            height,
            dirty: HashMap::new(),
            next_id: 1,
        }
    }

    fn alloc_id(&mut self) -> LayerId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn add_raster_layer(&mut self) -> LayerId {
        let id = self.alloc_id();
        self.layers.push(Layer::Raster(RasterLayer::new(id)));
        self.dirty.insert(id, DirtyRegion::new());
        id
    }

    pub fn add_filter_layer(&mut self, filter_type: FilterType, params: FilterParams) -> LayerId {
        let id = self.alloc_id();
        self.layers.push(Layer::Filter(FilterLayer::new(id, filter_type, params)));
        id
    }

    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id() == id)
    }

    pub fn layer_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.id() == id)
    }

    pub fn raster_layer(&self, id: LayerId) -> Option<&RasterLayer> {
        match self.layer(id)? {
            Layer::Raster(r) => Some(r),
            _ => None,
        }
    }

    pub fn raster_layer_mut(&mut self, id: LayerId) -> Option<&mut RasterLayer> {
        match self.layer_mut(id)? {
            Layer::Raster(r) => Some(r),
            _ => None,
        }
    }

    /// Paint a filled circle on a raster layer.
    /// Iterates only over tiles touched by the circle's bounding box.
    pub fn paint_circle(&mut self, layer_id: LayerId, cx: f32, cy: f32, radius: f32, color: [u8; 4]) {
        let px_min_x = (cx - radius).floor() as i32;
        let px_min_y = (cy - radius).floor() as i32;
        let px_max_x = (cx + radius).ceil() as i32;
        let px_max_y = (cy + radius).ceil() as i32;

        let (tx0, ty0) = TileGrid::tile_coords(px_min_x, px_min_y);
        let (tx1, ty1) = TileGrid::tile_coords(px_max_x, px_max_y);

        let raster = match self.layer_mut(layer_id) {
            Some(Layer::Raster(r)) => r as *mut RasterLayer,
            _ => return,
        };
        // SAFETY: we hold &mut self and only access this one layer through the pointer.
        // We need the pointer to also access self.dirty below.
        let raster = unsafe { &mut *raster };

        let radius_sq = radius * radius;

        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                let tile = raster.tiles.get_or_create(tx, ty);
                let tile_px_x = tx * TILE_SIZE as i32;
                let tile_px_y = ty * TILE_SIZE as i32;

                let mut wrote = false;
                let data = tile.write();

                for ly in 0..TILE_SIZE {
                    let py = tile_px_y + ly as i32;
                    let dy = py as f32 - cy;
                    for lx in 0..TILE_SIZE {
                        let px = tile_px_x + lx as i32;
                        let dx = px as f32 - cx;
                        if dx * dx + dy * dy <= radius_sq {
                            let pixel = data.pixel_mut(lx, ly);
                            // Simple alpha-over compositing
                            let src_a = color[3] as f32 / 255.0;
                            let dst_a = pixel[3] as f32 / 255.0;
                            let out_a = src_a + dst_a * (1.0 - src_a);
                            if out_a > 0.0 {
                                for c in 0..3 {
                                    let src = color[c] as f32 / 255.0;
                                    let dst = pixel[c] as f32 / 255.0;
                                    let out = (src * src_a + dst * dst_a * (1.0 - src_a)) / out_a;
                                    pixel[c] = (out * 255.0).round() as u8;
                                }
                                pixel[3] = (out_a * 255.0).round() as u8;
                            }
                            wrote = true;
                        }
                    }
                }

                if wrote {
                    if let Some(dirty) = self.dirty.get_mut(&layer_id) {
                        dirty.mark(tx, ty);
                    }
                }
            }
        }
    }

    /// Fill a raster layer with a horizontal gradient (demo helper).
    pub fn fill_gradient(&mut self, layer_id: LayerId) {
        let width = self.width;
        let height = self.height;

        let tiles_x = (width as i32 + TILE_SIZE as i32 - 1) / TILE_SIZE as i32;
        let tiles_y = (height as i32 + TILE_SIZE as i32 - 1) / TILE_SIZE as i32;

        let raster = match self.layer_mut(layer_id) {
            Some(Layer::Raster(r)) => r as *mut RasterLayer,
            _ => return,
        };
        let raster = unsafe { &mut *raster };

        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let tile = raster.tiles.get_or_create(tx, ty);
                let tile_px_x = tx * TILE_SIZE as i32;
                let tile_px_y = ty * TILE_SIZE as i32;

                let data = tile.write();

                for ly in 0..TILE_SIZE {
                    let py = tile_px_y + ly as i32;
                    if py >= height as i32 {
                        break;
                    }
                    for lx in 0..TILE_SIZE {
                        let px = tile_px_x + lx as i32;
                        if px >= width as i32 {
                            break;
                        }
                        let t = px as f32 / width as f32;
                        let pixel = data.pixel_mut(lx, ly);
                        // Dark purple to teal gradient
                        pixel[0] = ((1.0 - t) * 80.0 + t * 20.0) as u8;
                        pixel[1] = ((1.0 - t) * 20.0 + t * 120.0) as u8;
                        pixel[2] = ((1.0 - t) * 120.0 + t * 100.0) as u8;
                        pixel[3] = 255;
                    }
                }

                if let Some(dirty) = self.dirty.get_mut(&layer_id) {
                    dirty.mark(tx, ty);
                }
            }
        }
    }

    /// Clear dirty regions after GPU upload.
    pub fn clear_dirty(&mut self) {
        for region in self.dirty.values_mut() {
            region.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_layers_and_find() {
        let mut doc = Document::new(1920, 1080);
        let r1 = doc.add_raster_layer();
        let f1 = doc.add_filter_layer(FilterType::GaussianBlur, FilterParams::blur(8.0));
        let r2 = doc.add_raster_layer();

        assert_eq!(doc.layers.len(), 3);
        assert!(matches!(doc.layer(r1), Some(Layer::Raster(_))));
        assert!(matches!(doc.layer(f1), Some(Layer::Filter(_))));
        assert!(matches!(doc.layer(r2), Some(Layer::Raster(_))));
        assert!(doc.layer(999).is_none());
    }

    #[test]
    fn paint_circle_marks_dirty() {
        let mut doc = Document::new(1920, 1080);
        let id = doc.add_raster_layer();

        doc.paint_circle(id, 32.0, 32.0, 5.0, [255, 0, 0, 255]);

        // Circle at (32,32) r=5 is entirely within tile (0,0)
        let dirty = &doc.dirty[&id];
        assert!(dirty.tiles.contains(&(0, 0)));

        // Check pixels were actually written
        let raster = doc.raster_layer(id).unwrap();
        let tile = raster.tiles.get(0, 0).unwrap();
        assert_eq!(tile.data.pixel(32, 32), &[255, 0, 0, 255]);
    }

    #[test]
    fn paint_circle_spans_tiles() {
        let mut doc = Document::new(1920, 1080);
        let id = doc.add_raster_layer();

        // Paint at tile boundary — should touch tiles (0,0) and (1,0) at minimum
        doc.paint_circle(id, 64.0, 32.0, 5.0, [0, 255, 0, 255]);

        let dirty = &doc.dirty[&id];
        assert!(dirty.tiles.contains(&(0, 0)));
        assert!(dirty.tiles.contains(&(1, 0)));
    }

    #[test]
    fn fill_gradient_populates_tiles() {
        let mut doc = Document::new(128, 128);
        let id = doc.add_raster_layer();

        doc.fill_gradient(id);

        let raster = doc.raster_layer(id).unwrap();
        // 128x128 = 2x2 tiles
        assert!(raster.tiles.get(0, 0).is_some());
        assert!(raster.tiles.get(1, 0).is_some());
        assert!(raster.tiles.get(0, 1).is_some());
        assert!(raster.tiles.get(1, 1).is_some());

        // First pixel should be dark purple-ish
        let pixel = raster.tiles.get(0, 0).unwrap().data.pixel(0, 0);
        assert_eq!(pixel[3], 255); // fully opaque
    }

    #[test]
    fn clear_dirty() {
        let mut doc = Document::new(1920, 1080);
        let id = doc.add_raster_layer();
        doc.paint_circle(id, 32.0, 32.0, 5.0, [255, 0, 0, 255]);

        assert!(!doc.dirty[&id].is_empty());
        doc.clear_dirty();
        assert!(doc.dirty[&id].is_empty());
    }
}
