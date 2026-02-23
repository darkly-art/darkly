use crate::dirty::DirtyRegion;
use crate::layer::*;
use crate::tile::{TILE_SIZE, TileGrid};
use std::collections::HashMap;

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
        let layer = RasterLayer::new(id);
        self.layers.push(Layer::Raster(layer));
        self.dirty.insert(id, DirtyRegion::new());
        id
    }

    pub fn add_filter_layer(&mut self, params: Box<dyn FilterParams>) -> LayerId {
        let id = self.alloc_id();
        let layer = FilterLayer {
            id,
            params,
            visible: true,
        };
        self.layers.push(Layer::Filter(layer));
        id
    }

    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id() == id)
    }

    pub fn layer_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.id() == id)
    }

    /// Find the index of a layer by id.
    pub fn layer_index(&self, id: LayerId) -> Option<usize> {
        self.layers.iter().position(|l| l.id() == id)
    }

    /// Get a mutable reference to the raster layer's tiles and the dirty region simultaneously.
    fn raster_tiles_and_dirty<'a>(
        layers: &'a mut Vec<Layer>,
        dirty: &'a mut HashMap<LayerId, DirtyRegion>,
        layer_id: LayerId,
    ) -> Option<(&'a mut TileGrid, &'a mut DirtyRegion)> {
        let layer = layers.iter_mut().find(|l| l.id() == layer_id)?;
        let tiles = match layer {
            Layer::Raster(r) => &mut r.tiles,
            _ => return None,
        };
        let dirty_region = dirty.get_mut(&layer_id)?;
        Some((tiles, dirty_region))
    }

    /// Paint a filled circle on a raster layer.
    pub fn paint_circle(
        &mut self,
        layer_id: LayerId,
        cx: f32,
        cy: f32,
        radius: f32,
        color: [u8; 4],
    ) {
        let (tiles, dirty) =
            match Self::raster_tiles_and_dirty(&mut self.layers, &mut self.dirty, layer_id) {
                Some(v) => v,
                None => return,
            };

        let r2 = radius * radius;
        let x_min = (cx - radius).floor() as i32;
        let x_max = (cx + radius).ceil() as i32;
        let y_min = (cy - radius).floor() as i32;
        let y_max = (cy + radius).ceil() as i32;

        let (tx_min, ty_min) = TileGrid::tile_coords_for_pixel(x_min, y_min);
        let (tx_max, ty_max) = TileGrid::tile_coords_for_pixel(x_max, y_max);

        let tile_size = TILE_SIZE as i32;

        for ty in ty_min..=ty_max {
            for tx in tx_min..=tx_max {
                let tile_px_x = tx * tile_size;
                let tile_px_y = ty * tile_size;

                let mut touched = false;
                let tile = tiles.get_or_create(tx, ty);
                let data = tile.write();

                let lx_start = (x_min - tile_px_x).max(0) as usize;
                let lx_end = ((x_max - tile_px_x).min(tile_size) as usize).min(TILE_SIZE);
                let ly_start = (y_min - tile_px_y).max(0) as usize;
                let ly_end = ((y_max - tile_px_y).min(tile_size) as usize).min(TILE_SIZE);

                for ly in ly_start..ly_end {
                    for lx in lx_start..lx_end {
                        let px = (tile_px_x + lx as i32) as f32 + 0.5;
                        let py = (tile_px_y + ly as i32) as f32 + 0.5;
                        let dx = px - cx;
                        let dy = py - cy;
                        if dx * dx + dy * dy <= r2 {
                            let dst = data.pixel_mut(lx, ly);
                            let src_a = color[3] as f32 / 255.0;
                            let dst_a = dst[3] as f32 / 255.0;
                            let out_a = src_a + dst_a * (1.0 - src_a);
                            if out_a > 0.0 {
                                for c in 0..3 {
                                    let src_c = color[c] as f32 / 255.0;
                                    let dst_c = dst[c] as f32 / 255.0;
                                    let out_c =
                                        (src_c * src_a + dst_c * dst_a * (1.0 - src_a)) / out_a;
                                    dst[c] = (out_c * 255.0).round() as u8;
                                }
                                dst[3] = (out_a * 255.0).round() as u8;
                            }
                            touched = true;
                        }
                    }
                }

                if touched {
                    dirty.mark(tx, ty);
                }
            }
        }
    }

    /// Fill a raster layer with a horizontal gradient (demo helper).
    pub fn fill_gradient(&mut self, layer_id: LayerId) {
        let width = self.width;
        let height = self.height;

        let (tiles, dirty) =
            match Self::raster_tiles_and_dirty(&mut self.layers, &mut self.dirty, layer_id) {
                Some(v) => v,
                None => return,
            };

        let tile_size = TILE_SIZE as i32;
        let tiles_x = (width as i32 + tile_size - 1) / tile_size;
        let tiles_y = (height as i32 + tile_size - 1) / tile_size;

        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let tile = tiles.get_or_create(tx, ty);
                let data = tile.write();
                for ly in 0..TILE_SIZE {
                    for lx in 0..TILE_SIZE {
                        let px = tx * tile_size + lx as i32;
                        let py = ty * tile_size + ly as i32;
                        if px < width as i32 && py < height as i32 {
                            let t = px as f32 / width as f32;
                            let r = (40.0 + t * 80.0) as u8;
                            let g = (20.0 + t * 40.0) as u8;
                            let b = (80.0 + t * 120.0) as u8;
                            data.pixel_mut(lx, ly).copy_from_slice(&[r, g, b, 255]);
                        }
                    }
                }
                dirty.mark(tx, ty);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_layers_and_paint() {
        let mut doc = Document::new(256, 256);
        let id = doc.add_raster_layer();
        doc.paint_circle(id, 32.0, 32.0, 5.0, [255, 0, 0, 255]);

        let dirty = doc.dirty.get(&id).unwrap();
        assert!(!dirty.is_empty());

        if let Some(Layer::Raster(r)) = doc.layer(id) {
            let tile = r.tiles.get(0, 0).unwrap();
            let px = tile.data().pixel(32, 32);
            assert_eq!(px, &[255, 0, 0, 255]);
        } else {
            panic!("Layer not found");
        }
    }

    #[test]
    fn fill_gradient_marks_dirty() {
        let ts = TILE_SIZE as u32;
        let canvas = ts * 2; // use a canvas that's exactly 2 tiles wide/tall
        let mut doc = Document::new(canvas, canvas);
        let id = doc.add_raster_layer();
        doc.fill_gradient(id);

        let dirty = doc.dirty.get(&id).unwrap();
        assert_eq!(dirty.len(), 4); // 2 tiles each axis = 4 tiles

        if let Some(Layer::Raster(r)) = doc.layer(id) {
            let tile = r.tiles.get(0, 0).unwrap();
            let px = tile.data().pixel(0, 0);
            assert_eq!(px[3], 255);
        }
    }
}
