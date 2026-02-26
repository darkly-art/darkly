use std::collections::{HashMap, HashSet};
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

/// A sparse record of tile states captured before modification during a transaction.
/// Only stores the first pre-write state per tile — subsequent writes to the same
/// tile within the same transaction are ignored (the memento already has the original).
#[derive(Clone)]
pub struct Memento {
    /// Old tile data for tiles that existed before the write.
    /// `None` value means the tile did not exist (was created during this transaction).
    tiles: HashMap<(i32, i32), Option<Arc<TileData>>>,
}

impl Memento {
    pub fn new() -> Self {
        Memento {
            tiles: HashMap::new(),
        }
    }

    /// The set of tile coordinates affected by this memento.
    pub fn affected_tiles(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.tiles.keys().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }
}

/// Sparse tile grid. Key = (tile_x, tile_y) in tile coordinates.
///
/// When a transaction is active (via `begin_transaction`), the grid automatically
/// captures pre-write tile state into a `Memento` whenever a tile is accessed for
/// writing. This is the Krita-style recording hook: paint operations are completely
/// unaware of undo — the grid intercepts writes transparently.
#[derive(Clone)]
pub struct TileGrid {
    tiles: HashMap<(i32, i32), Tile>,
    /// Active recording memento. When `Some`, tile writes are recorded.
    recording: Option<Memento>,
}

impl TileGrid {
    pub fn new() -> Self {
        TileGrid {
            tiles: HashMap::new(),
            recording: None,
        }
    }

    pub fn get(&self, tx: i32, ty: i32) -> Option<&Tile> {
        self.tiles.get(&(tx, ty))
    }

    /// Get a tile for writing, creating an empty one if it doesn't exist.
    /// If a transaction is active, the pre-write state is automatically recorded.
    pub fn get_or_create(&mut self, tx: i32, ty: i32) -> &mut Tile {
        if let Some(ref mut memento) = self.recording {
            let key = (tx, ty);
            // Only record the first access per tile per transaction.
            memento.tiles.entry(key).or_insert_with(|| {
                // Capture the old state: Some(arc) if tile existed, None if new.
                self.tiles.get(&key).map(|t| Arc::clone(&t.data))
            });
        }
        self.tiles.entry((tx, ty)).or_insert_with(Tile::empty)
    }

    /// Start recording tile changes. Panics if a transaction is already active.
    pub fn begin_transaction(&mut self) {
        assert!(
            self.recording.is_none(),
            "begin_transaction called while a transaction is already active"
        );
        self.recording = Some(Memento::new());
    }

    /// Finish recording and return the memento of changed tiles.
    /// Returns `None` if no tiles were written during the transaction.
    pub fn commit_transaction(&mut self) -> Option<Memento> {
        let memento = self.recording.take().expect(
            "commit_transaction called without an active transaction",
        );
        if memento.is_empty() {
            None
        } else {
            Some(memento)
        }
    }

    /// Discard the active transaction without producing a memento.
    pub fn rollback_transaction(&mut self) {
        self.recording = None;
    }

    /// Returns true if a transaction is currently active.
    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// Apply a memento in reverse: restore old tile states and return a forward
    /// memento that can redo the operation. Also returns the set of tile coords
    /// that were affected (for dirty marking).
    pub fn rollback(&mut self, memento: &Memento) -> (Memento, HashSet<(i32, i32)>) {
        let mut forward = Memento::new();
        let mut affected = HashSet::new();

        for (&key, old_data) in &memento.tiles {
            // Capture current state for the forward (redo) memento.
            let current = self.tiles.get(&key).map(|t| Arc::clone(&t.data));
            forward.tiles.insert(key, current);

            // Restore old state.
            match old_data {
                Some(arc) => {
                    // Tile existed before — restore it.
                    self.tiles.insert(key, Tile { data: Arc::clone(arc) });
                }
                None => {
                    // Tile didn't exist before — remove it.
                    self.tiles.remove(&key);
                }
            }
            affected.insert(key);
        }

        (forward, affected)
    }

    /// Apply a forward memento (redo). Same mechanics as rollback but using
    /// the forward memento produced by a previous rollback.
    pub fn rollforward(&mut self, memento: &Memento) -> (Memento, HashSet<(i32, i32)>) {
        // Structurally identical to rollback — the memento format is symmetric.
        self.rollback(memento)
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
        let ts = TILE_SIZE as i32;
        assert_eq!(TileGrid::tile_coords_for_pixel(0, 0), (0, 0));
        assert_eq!(TileGrid::tile_coords_for_pixel(ts - 1, ts - 1), (0, 0));
        assert_eq!(TileGrid::tile_coords_for_pixel(ts, 0), (1, 0));
        assert_eq!(TileGrid::tile_coords_for_pixel(-1, -1), (-1, -1));
        assert_eq!(TileGrid::tile_coords_for_pixel(-ts, 0), (-1, 0));
        assert_eq!(TileGrid::tile_coords_for_pixel(-ts - 1, 0), (-2, 0));
    }

    #[test]
    fn transaction_records_changed_tiles() {
        let mut grid = TileGrid::new();
        // Pre-existing tile.
        grid.get_or_create(0, 0).write().pixel_mut(0, 0).copy_from_slice(&[1, 2, 3, 4]);

        grid.begin_transaction();
        // Modify existing tile.
        grid.get_or_create(0, 0).write().pixel_mut(0, 0).copy_from_slice(&[10, 20, 30, 40]);
        // Create new tile.
        grid.get_or_create(1, 0).write().pixel_mut(0, 0).copy_from_slice(&[50, 60, 70, 80]);

        let memento = grid.commit_transaction().unwrap();
        let affected: HashSet<_> = memento.affected_tiles().collect();
        assert!(affected.contains(&(0, 0)));
        assert!(affected.contains(&(1, 0)));
        assert_eq!(affected.len(), 2);
    }

    #[test]
    fn transaction_only_records_first_write() {
        let mut grid = TileGrid::new();
        grid.get_or_create(0, 0).write().pixel_mut(0, 0).copy_from_slice(&[1, 2, 3, 4]);

        grid.begin_transaction();
        // Write twice to the same tile.
        grid.get_or_create(0, 0).write().pixel_mut(0, 0).copy_from_slice(&[10, 20, 30, 40]);
        grid.get_or_create(0, 0).write().pixel_mut(1, 0).copy_from_slice(&[99, 99, 99, 99]);
        let memento = grid.commit_transaction().unwrap();

        // Rollback should restore to state before the transaction (original pixel).
        let (_, _) = grid.rollback(&memento);
        assert_eq!(grid.get(0, 0).unwrap().data().pixel(0, 0), &[1, 2, 3, 4]);
    }

    #[test]
    fn rollback_restores_old_state() {
        let mut grid = TileGrid::new();
        grid.get_or_create(0, 0).write().pixel_mut(0, 0).copy_from_slice(&[1, 2, 3, 4]);

        grid.begin_transaction();
        grid.get_or_create(0, 0).write().pixel_mut(0, 0).copy_from_slice(&[10, 20, 30, 40]);
        grid.get_or_create(1, 0).write().pixel_mut(0, 0).copy_from_slice(&[50, 60, 70, 80]);
        let memento = grid.commit_transaction().unwrap();

        let (forward, affected) = grid.rollback(&memento);
        // Old tile restored.
        assert_eq!(grid.get(0, 0).unwrap().data().pixel(0, 0), &[1, 2, 3, 4]);
        // New tile removed.
        assert!(grid.get(1, 0).is_none());
        assert_eq!(affected.len(), 2);

        // Rollforward (redo) brings the changes back.
        let (_, _) = grid.rollforward(&forward);
        assert_eq!(grid.get(0, 0).unwrap().data().pixel(0, 0), &[10, 20, 30, 40]);
        assert_eq!(grid.get(1, 0).unwrap().data().pixel(0, 0), &[50, 60, 70, 80]);
    }

    #[test]
    fn empty_transaction_returns_none() {
        let mut grid = TileGrid::new();
        grid.begin_transaction();
        // No writes.
        assert!(grid.commit_transaction().is_none());
    }
}
