use std::collections::HashMap;
use std::sync::Arc;

pub const TILE_SIZE: usize = 64;
pub const TILE_PIXELS: usize = TILE_SIZE * TILE_SIZE;
pub const TILE_BYTES: usize = TILE_PIXELS * 4; // RGBA u8

/// Raw pixel data for a single 64x64 tile, stored as RGBA u8.
#[derive(Clone)]
pub struct TileData {
    pub pixels: Vec<u8>,
}

impl TileData {
    pub fn new_empty() -> Self {
        TileData {
            pixels: vec![0u8; TILE_BYTES],
        }
    }

    /// Get a mutable reference to the pixel at (x, y) within the tile.
    /// Returns a 4-byte slice [R, G, B, A].
    #[inline]
    pub fn pixel_mut(&mut self, x: usize, y: usize) -> &mut [u8] {
        debug_assert!(x < TILE_SIZE && y < TILE_SIZE);
        let offset = (y * TILE_SIZE + x) * 4;
        &mut self.pixels[offset..offset + 4]
    }

    #[inline]
    pub fn pixel(&self, x: usize, y: usize) -> &[u8] {
        debug_assert!(x < TILE_SIZE && y < TILE_SIZE);
        let offset = (y * TILE_SIZE + x) * 4;
        &self.pixels[offset..offset + 4]
    }
}

/// A single tile with COW (copy-on-write) semantics via Arc.
#[derive(Clone)]
pub struct Tile {
    pub data: Arc<TileData>,
}

impl Tile {
    pub fn empty() -> Self {
        Tile {
            data: Arc::new(TileData::new_empty()),
        }
    }

    /// Get a mutable reference to the tile data, cloning if shared (COW).
    pub fn write(&mut self) -> &mut TileData {
        Arc::make_mut(&mut self.data)
    }

    pub fn is_shared(&self) -> bool {
        Arc::strong_count(&self.data) > 1
    }
}

/// Sparse grid of tiles, keyed by tile coordinates.
#[derive(Clone)]
pub struct TileGrid {
    pub tiles: HashMap<(i32, i32), Tile>,
}

impl TileGrid {
    pub fn new() -> Self {
        TileGrid {
            tiles: HashMap::new(),
        }
    }

    pub fn get(&self, tx: i32, ty: i32) -> Option<&Tile> {
        self.tiles.get(&(tx, ty))
    }

    /// Get or create a tile at the given coordinates.
    /// New tiles start empty (transparent).
    pub fn get_or_create(&mut self, tx: i32, ty: i32) -> &mut Tile {
        self.tiles.entry((tx, ty)).or_insert_with(Tile::empty)
    }

    /// Create a snapshot of this grid for undo. Only increments Arc refcounts.
    pub fn snapshot(&self) -> TileGrid {
        self.clone()
    }

    /// Convert pixel coordinates to tile coordinates.
    #[inline]
    pub fn tile_coords(px: i32, py: i32) -> (i32, i32) {
        (px.div_euclid(TILE_SIZE as i32), py.div_euclid(TILE_SIZE as i32))
    }

    /// Convert pixel coordinates to local coordinates within a tile.
    #[inline]
    pub fn local_coords(px: i32, py: i32) -> (usize, usize) {
        (
            px.rem_euclid(TILE_SIZE as i32) as usize,
            py.rem_euclid(TILE_SIZE as i32) as usize,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_cow_behavior() {
        let mut tile_a = Tile::empty();
        let tile_b = tile_a.clone();

        assert!(Arc::ptr_eq(&tile_a.data, &tile_b.data));
        assert!(tile_a.is_shared());

        // Writing to tile_a triggers a clone
        tile_a.write().pixel_mut(0, 0).copy_from_slice(&[255, 0, 0, 255]);

        assert!(!Arc::ptr_eq(&tile_a.data, &tile_b.data));
        assert_eq!(tile_a.data.pixel(0, 0), &[255, 0, 0, 255]);
        assert_eq!(tile_b.data.pixel(0, 0), &[0, 0, 0, 0]);
    }

    #[test]
    fn tile_coords_conversion() {
        assert_eq!(TileGrid::tile_coords(0, 0), (0, 0));
        assert_eq!(TileGrid::tile_coords(63, 63), (0, 0));
        assert_eq!(TileGrid::tile_coords(64, 0), (1, 0));
        assert_eq!(TileGrid::tile_coords(-1, -1), (-1, -1));
        assert_eq!(TileGrid::tile_coords(-64, 0), (-1, 0));
        assert_eq!(TileGrid::tile_coords(-65, 0), (-2, 0));
    }

    #[test]
    fn local_coords_conversion() {
        assert_eq!(TileGrid::local_coords(0, 0), (0, 0));
        assert_eq!(TileGrid::local_coords(63, 63), (63, 63));
        assert_eq!(TileGrid::local_coords(64, 0), (0, 0));
        assert_eq!(TileGrid::local_coords(-1, -1), (63, 63));
    }

    #[test]
    fn grid_snapshot_is_cow() {
        let mut grid = TileGrid::new();
        grid.get_or_create(0, 0).write().pixel_mut(5, 5).copy_from_slice(&[1, 2, 3, 4]);

        let snapshot = grid.snapshot();

        assert!(Arc::ptr_eq(
            &grid.tiles[&(0, 0)].data,
            &snapshot.tiles[&(0, 0)].data,
        ));

        // Modifying original doesn't affect snapshot
        grid.get_or_create(0, 0).write().pixel_mut(5, 5).copy_from_slice(&[10, 20, 30, 40]);

        assert_eq!(grid.tiles[&(0, 0)].data.pixel(5, 5), &[10, 20, 30, 40]);
        assert_eq!(snapshot.tiles[&(0, 0)].data.pixel(5, 5), &[1, 2, 3, 4]);
    }
}
