use std::collections::HashMap;
use std::sync::Arc;

pub const TILE_SIZE: usize = 64;
pub const TILE_BYTES: usize = TILE_SIZE * TILE_SIZE * 4; // RGBA u8

#[derive(Clone)]
#[repr(transparent)]
pub struct TileData(pub [u8; TILE_BYTES]);

// SAFETY: TileData is a plain [u8; N] wrapper with repr(transparent).
// Note: TileData is too large for Copy/Pod, so we only implement Zeroable.
// For bytemuck::bytes_of we access the inner array directly.
unsafe impl bytemuck::Zeroable for TileData {}

impl Default for TileData {
    fn default() -> Self {
        TileData([0u8; TILE_BYTES])
    }
}

impl TileData {
    /// Get a mutable reference to the pixel at (x, y) within the tile.
    /// x, y are in tile-local coordinates (0..TILE_SIZE).
    pub fn pixel_mut(&mut self, x: usize, y: usize) -> &mut [u8; 4] {
        debug_assert!(x < TILE_SIZE && y < TILE_SIZE);
        let offset = (y * TILE_SIZE + x) * 4;
        (&mut self.0[offset..offset + 4]).try_into().unwrap()
    }

    /// Get the pixel at (x, y) within the tile.
    pub fn pixel(&self, x: usize, y: usize) -> &[u8; 4] {
        debug_assert!(x < TILE_SIZE && y < TILE_SIZE);
        let offset = (y * TILE_SIZE + x) * 4;
        self.0[offset..offset + 4].try_into().unwrap()
    }
}

/// A single tile with COW (copy-on-write) semantics via Arc.
#[derive(Clone)]
pub struct Tile {
    data: Arc<TileData>,
}

impl Tile {
    /// Create a new empty (transparent) tile. All empty tiles share the same Arc.
    pub fn empty() -> Self {
        use std::sync::LazyLock;
        static EMPTY: LazyLock<Arc<TileData>> = LazyLock::new(|| Arc::new(TileData::default()));
        Tile {
            data: Arc::clone(&EMPTY),
        }
    }

    /// Get a read-only reference to the tile data.
    pub fn data(&self) -> &TileData {
        &self.data
    }

    /// Get a mutable reference with COW semantics.
    /// If this is the only reference, returns the existing data.
    /// Otherwise, clones the data first.
    pub fn write(&mut self) -> &mut TileData {
        Arc::make_mut(&mut self.data)
    }

    /// Returns true if this tile's data is shared with other tiles (i.e., not unique).
    pub fn is_shared(&self) -> bool {
        Arc::strong_count(&self.data) > 1
    }
}

/// Sparse tile grid. Key = (tile_x, tile_y) in tile coordinates.
#[derive(Clone)]
pub struct TileGrid {
    tiles: HashMap<(i32, i32), Tile>,
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

    /// Get a tile, creating an empty one if it doesn't exist.
    pub fn get_or_create(&mut self, tx: i32, ty: i32) -> &mut Tile {
        self.tiles.entry((tx, ty)).or_insert_with(Tile::empty)
    }

    /// Snapshot the entire grid — cheap because tiles use Arc.
    pub fn snapshot(&self) -> TileGrid {
        self.clone()
    }

    /// Convert pixel coordinates to tile coordinates.
    pub fn tile_coords_for_pixel(x: i32, y: i32) -> (i32, i32) {
        (x.div_euclid(TILE_SIZE as i32), y.div_euclid(TILE_SIZE as i32))
    }

    /// Iterate over all tiles.
    pub fn iter(&self) -> impl Iterator<Item = ((i32, i32), &Tile)> {
        self.tiles.iter().map(|(&k, v)| (k, v))
    }

    /// Number of allocated tiles.
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }
}

impl Default for TileGrid {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_tiles_share_arc() {
        let t1 = Tile::empty();
        let t2 = Tile::empty();
        assert!(Arc::ptr_eq(&t1.data, &t2.data));
    }

    #[test]
    fn cow_clone_on_write() {
        let t1 = Tile::empty();
        let mut t2 = t1.clone();
        assert!(Arc::ptr_eq(&t1.data, &t2.data));
        // Writing to t2 should decouple from t1
        t2.write().pixel_mut(0, 0).copy_from_slice(&[255, 0, 0, 255]);
        assert!(!Arc::ptr_eq(&t1.data, &t2.data));
        assert_eq!(t1.data().pixel(0, 0), &[0, 0, 0, 0]);
        assert_eq!(t2.data().pixel(0, 0), &[255, 0, 0, 255]);
    }

    #[test]
    fn tile_grid_get_or_create() {
        let mut grid = TileGrid::new();
        assert!(grid.get(0, 0).is_none());
        let tile = grid.get_or_create(0, 0);
        tile.write().pixel_mut(5, 5).copy_from_slice(&[0, 255, 0, 255]);
        assert!(grid.get(0, 0).is_some());
        assert_eq!(grid.get(0, 0).unwrap().data().pixel(5, 5), &[0, 255, 0, 255]);
    }

    #[test]
    fn tile_coords_for_pixel() {
        assert_eq!(TileGrid::tile_coords_for_pixel(0, 0), (0, 0));
        assert_eq!(TileGrid::tile_coords_for_pixel(63, 63), (0, 0));
        assert_eq!(TileGrid::tile_coords_for_pixel(64, 0), (1, 0));
        assert_eq!(TileGrid::tile_coords_for_pixel(-1, -1), (-1, -1));
        assert_eq!(TileGrid::tile_coords_for_pixel(-64, 0), (-1, 0));
        assert_eq!(TileGrid::tile_coords_for_pixel(-65, 0), (-2, 0));
    }

    #[test]
    fn snapshot_is_cow() {
        let mut grid = TileGrid::new();
        grid.get_or_create(0, 0).write().pixel_mut(0, 0).copy_from_slice(&[1, 2, 3, 4]);
        let snap = grid.snapshot();
        // Modify original — snapshot should be unaffected
        grid.get_or_create(0, 0).write().pixel_mut(0, 0).copy_from_slice(&[5, 6, 7, 8]);
        assert_eq!(snap.get(0, 0).unwrap().data().pixel(0, 0), &[1, 2, 3, 4]);
        assert_eq!(grid.get(0, 0).unwrap().data().pixel(0, 0), &[5, 6, 7, 8]);
    }
}
