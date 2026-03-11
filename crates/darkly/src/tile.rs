use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub const TILE_SIZE: usize = 64;
pub const TILE_BYTES: usize = TILE_SIZE * TILE_SIZE * 4; // RGBA u8

// ---------------------------------------------------------------------------
// TileFormat trait — parameterizes the tile storage over pixel format
// ---------------------------------------------------------------------------

/// Marker trait for tile data formats. Each format defines its own data array type.
pub trait TileFormat: 'static + Send + Sync {
    /// The raw data array stored per tile. Must be a plain, bytemuck-safe type.
    type Data: Clone + Default + Send + Sync + 'static;
}

/// RGBA u8 format (4 bytes per pixel, 16 KB per tile).
#[derive(Clone, Copy)]
pub struct Rgba;
impl TileFormat for Rgba {
    type Data = RgbaData;
}

/// Single-channel f32 format (4 bytes per pixel, 16 KB per tile).
#[derive(Clone, Copy)]
pub struct AlphaF32;
impl TileFormat for AlphaF32 {
    type Data = AlphaF32Data;
}

// ---------------------------------------------------------------------------
// Tile data types
// ---------------------------------------------------------------------------

#[derive(Clone)]
#[repr(transparent)]
pub struct RgbaData(pub [u8; TILE_BYTES]);

// SAFETY: RgbaData is a plain [u8; N] wrapper with repr(transparent).
unsafe impl bytemuck::Zeroable for RgbaData {}

impl Default for RgbaData {
    fn default() -> Self {
        RgbaData([0u8; TILE_BYTES])
    }
}

impl RgbaData {
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

pub const MASK_TILE_PIXELS: usize = TILE_SIZE * TILE_SIZE;

#[derive(Clone)]
#[repr(transparent)]
pub struct AlphaF32Data(pub [f32; MASK_TILE_PIXELS]);

// SAFETY: AlphaF32Data is a plain [f32; N] wrapper with repr(transparent).
unsafe impl bytemuck::Zeroable for AlphaF32Data {}

impl Default for AlphaF32Data {
    fn default() -> Self {
        AlphaF32Data([0.0f32; MASK_TILE_PIXELS])
    }
}

impl AlphaF32Data {
    /// Get the value at (x, y) within the tile.
    pub fn get(&self, x: usize, y: usize) -> f32 {
        debug_assert!(x < TILE_SIZE && y < TILE_SIZE);
        self.0[y * TILE_SIZE + x]
    }

    /// Set the value at (x, y) within the tile.
    pub fn set(&mut self, x: usize, y: usize, value: f32) {
        debug_assert!(x < TILE_SIZE && y < TILE_SIZE);
        self.0[y * TILE_SIZE + x] = value;
    }
}

// ---------------------------------------------------------------------------
// Tile<F> — a single tile with COW semantics
// ---------------------------------------------------------------------------

/// A single tile with COW (copy-on-write) semantics via Arc.
#[derive(Clone)]
pub struct Tile<F: TileFormat> {
    data: Arc<F::Data>,
}

impl<F: TileFormat> Tile<F> {
    /// Create a new empty (zero-initialized) tile.
    pub fn new_empty() -> Self {
        Tile {
            data: Arc::new(F::Data::default()),
        }
    }

    /// Get a read-only reference to the tile data.
    pub fn data(&self) -> &F::Data {
        &self.data
    }

    /// Get a mutable reference with COW semantics.
    pub fn write(&mut self) -> &mut F::Data {
        Arc::make_mut(&mut self.data)
    }

    /// Returns true if this tile's data is shared with other tiles.
    pub fn is_shared(&self) -> bool {
        Arc::strong_count(&self.data) > 1
    }

    /// Get a reference to the underlying Arc (for memento capture).
    pub(crate) fn arc(&self) -> &Arc<F::Data> {
        &self.data
    }

    /// Construct from a raw Arc (for memento restore).
    pub(crate) fn from_arc(data: Arc<F::Data>) -> Self {
        Tile { data }
    }
}

// Shared empty tile singleton for Rgba (the common case).
impl Tile<Rgba> {
    /// Create a new empty (transparent) tile. All empty tiles share the same Arc.
    pub fn empty() -> Self {
        use std::sync::LazyLock;
        static EMPTY: LazyLock<Arc<RgbaData>> = LazyLock::new(|| Arc::new(RgbaData::default()));
        Tile {
            data: Arc::clone(&EMPTY),
        }
    }
}

// Shared empty tile singleton for AlphaF32.
impl Tile<AlphaF32> {
    /// Create a new empty (zero) mask tile. All empty tiles share the same Arc.
    pub fn empty() -> Self {
        use std::sync::LazyLock;
        static EMPTY: LazyLock<Arc<AlphaF32Data>> =
            LazyLock::new(|| Arc::new(AlphaF32Data::default()));
        Tile {
            data: Arc::clone(&EMPTY),
        }
    }

    /// Create a fully opaque (1.0) mask tile. All full tiles share the same Arc.
    /// Used as the default for layer masks (white = reveal all).
    pub fn full() -> Self {
        use std::sync::LazyLock;
        static FULL: LazyLock<Arc<AlphaF32Data>> =
            LazyLock::new(|| Arc::new(AlphaF32Data([1.0f32; MASK_TILE_PIXELS])));
        Tile {
            data: Arc::clone(&FULL),
        }
    }
}

// ---------------------------------------------------------------------------
// Memento<F> — snapshot of tile states for undo
// ---------------------------------------------------------------------------

/// A sparse record of tile states captured before modification during a transaction.
#[derive(Clone)]
pub struct Memento<F: TileFormat> {
    tiles: HashMap<(i32, i32), Option<Arc<F::Data>>>,
}

impl<F: TileFormat> Memento<F> {
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

// ---------------------------------------------------------------------------
// TileStore<F> — sparse tile grid, generic over pixel format
// ---------------------------------------------------------------------------

/// Sparse tile grid. Key = (tile_x, tile_y) in tile coordinates.
///
/// When a transaction is active (via `begin_transaction`), the grid automatically
/// captures pre-write tile state into a `Memento` whenever a tile is accessed for
/// writing. This is the Krita-style recording hook: paint operations are completely
/// unaware of undo — the grid intercepts writes transparently.
#[derive(Clone)]
pub struct TileStore<F: TileFormat> {
    tiles: HashMap<(i32, i32), Tile<F>>,
    /// Active recording memento. When `Some`, tile writes are recorded.
    recording: Option<Memento<F>>,
}

impl<F: TileFormat> TileStore<F> {
    pub fn new() -> Self {
        TileStore {
            tiles: HashMap::new(),
            recording: None,
        }
    }

    pub fn get(&self, tx: i32, ty: i32) -> Option<&Tile<F>> {
        self.tiles.get(&(tx, ty))
    }

    /// Get a tile for writing, creating an empty one if it doesn't exist.
    /// If a transaction is active, the pre-write state is automatically recorded.
    pub fn get_or_create(&mut self, tx: i32, ty: i32) -> &mut Tile<F> {
        if let Some(ref mut memento) = self.recording {
            let key = (tx, ty);
            let tiles = &self.tiles;
            memento.tiles.entry(key).or_insert_with(|| {
                tiles.get(&key).map(|t| Arc::clone(t.arc()))
            });
        }
        self.tiles.entry((tx, ty)).or_insert_with(Tile::new_empty)
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
    pub fn commit_transaction(&mut self) -> Option<Memento<F>> {
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
    /// memento that can redo the operation.
    pub fn rollback(&mut self, memento: &Memento<F>) -> (Memento<F>, HashSet<(i32, i32)>) {
        let mut forward = Memento::new();
        let mut affected = HashSet::new();

        for (&key, old_data) in &memento.tiles {
            let current = self.tiles.get(&key).map(|t| Arc::clone(t.arc()));
            forward.tiles.insert(key, current);

            match old_data {
                Some(arc) => {
                    self.tiles.insert(key, Tile::from_arc(Arc::clone(arc)));
                }
                None => {
                    self.tiles.remove(&key);
                }
            }
            affected.insert(key);
        }

        (forward, affected)
    }

    /// Apply a forward memento (redo). Same mechanics as rollback.
    pub fn rollforward(&mut self, memento: &Memento<F>) -> (Memento<F>, HashSet<(i32, i32)>) {
        self.rollback(memento)
    }

    /// Convert pixel coordinates to tile coordinates.
    pub fn tile_coords_for_pixel(x: i32, y: i32) -> (i32, i32) {
        (x.div_euclid(TILE_SIZE as i32), y.div_euclid(TILE_SIZE as i32))
    }

    /// Iterate over all tiles.
    pub fn iter(&self) -> impl Iterator<Item = ((i32, i32), &Tile<F>)> {
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

impl TileStore<AlphaF32> {
    /// Get a tile for writing, creating a **full** (1.0) one if it doesn't exist.
    /// Used for layer mask painting where the default is white (reveal all).
    pub fn get_or_create_full(&mut self, tx: i32, ty: i32) -> &mut Tile<AlphaF32> {
        if let Some(ref mut memento) = self.recording {
            let key = (tx, ty);
            let tiles = &self.tiles;
            memento.tiles.entry(key).or_insert_with(|| {
                tiles.get(&key).map(|t| Arc::clone(t.arc()))
            });
        }
        self.tiles.entry((tx, ty)).or_insert_with(Tile::full)
    }
}

impl<F: TileFormat> Default for TileStore<F> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Type aliases — backward-compatible names
// ---------------------------------------------------------------------------

/// The legacy name for RGBA tile data.
pub type TileData = RgbaData;
/// RGBA tile grid (layers).
pub type TileGrid = TileStore<Rgba>;
/// Single-channel f32 tile grid (masks, selections).
pub type AlphaMask = TileStore<AlphaF32>;
/// RGBA memento.
pub type RgbaMemento = Memento<Rgba>;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_tiles_share_arc() {
        let t1 = Tile::<Rgba>::empty();
        let t2 = Tile::<Rgba>::empty();
        assert!(Arc::ptr_eq(t1.arc(), t2.arc()));
    }

    #[test]
    fn cow_clone_on_write() {
        let t1 = Tile::<Rgba>::empty();
        let mut t2 = t1.clone();
        assert!(Arc::ptr_eq(t1.arc(), t2.arc()));
        t2.write().pixel_mut(0, 0).copy_from_slice(&[255, 0, 0, 255]);
        assert!(!Arc::ptr_eq(t1.arc(), t2.arc()));
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
        grid.get_or_create(0, 0).write().pixel_mut(0, 0).copy_from_slice(&[1, 2, 3, 4]);

        grid.begin_transaction();
        grid.get_or_create(0, 0).write().pixel_mut(0, 0).copy_from_slice(&[10, 20, 30, 40]);
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
        grid.get_or_create(0, 0).write().pixel_mut(0, 0).copy_from_slice(&[10, 20, 30, 40]);
        grid.get_or_create(0, 0).write().pixel_mut(1, 0).copy_from_slice(&[99, 99, 99, 99]);
        let memento = grid.commit_transaction().unwrap();

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
        assert_eq!(grid.get(0, 0).unwrap().data().pixel(0, 0), &[1, 2, 3, 4]);
        assert!(grid.get(1, 0).is_none());
        assert_eq!(affected.len(), 2);

        let (_, _) = grid.rollforward(&forward);
        assert_eq!(grid.get(0, 0).unwrap().data().pixel(0, 0), &[10, 20, 30, 40]);
        assert_eq!(grid.get(1, 0).unwrap().data().pixel(0, 0), &[50, 60, 70, 80]);
    }

    #[test]
    fn empty_transaction_returns_none() {
        let mut grid = TileGrid::new();
        grid.begin_transaction();
        assert!(grid.commit_transaction().is_none());
    }

    // --- AlphaMask tests ---

    #[test]
    fn alpha_mask_empty_tiles_share_arc() {
        let t1 = Tile::<AlphaF32>::empty();
        let t2 = Tile::<AlphaF32>::empty();
        assert!(Arc::ptr_eq(t1.arc(), t2.arc()));
    }

    #[test]
    fn alpha_mask_cow() {
        let t1 = Tile::<AlphaF32>::empty();
        let mut t2 = t1.clone();
        t2.write().set(0, 0, 1.0);
        assert!(!Arc::ptr_eq(t1.arc(), t2.arc()));
        assert_eq!(t1.data().get(0, 0), 0.0);
        assert_eq!(t2.data().get(0, 0), 1.0);
    }

    #[test]
    fn alpha_mask_transaction() {
        let mut mask = AlphaMask::new();
        mask.get_or_create(0, 0).write().set(5, 5, 0.5);

        mask.begin_transaction();
        mask.get_or_create(0, 0).write().set(5, 5, 1.0);
        mask.get_or_create(1, 0).write().set(0, 0, 0.75);
        let memento = mask.commit_transaction().unwrap();

        assert_eq!(mask.get(0, 0).unwrap().data().get(5, 5), 1.0);
        assert_eq!(mask.get(1, 0).unwrap().data().get(0, 0), 0.75);

        let (forward, _) = mask.rollback(&memento);
        assert_eq!(mask.get(0, 0).unwrap().data().get(5, 5), 0.5);
        assert!(mask.get(1, 0).is_none());

        let (_, _) = mask.rollforward(&forward);
        assert_eq!(mask.get(0, 0).unwrap().data().get(5, 5), 1.0);
        assert_eq!(mask.get(1, 0).unwrap().data().get(0, 0), 0.75);
    }
}
