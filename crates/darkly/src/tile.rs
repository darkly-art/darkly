//! Sparse tile storage — retained **only for the selection system**.
//!
//! Selections use CPU-side `AlphaMask` (= `TileStore<AlphaF32>`) because
//! selection operations (boolean add/subtract/intersect, contour extraction
//! for marching ants, feathering, SDF rasterization) are infrequent,
//! irregular-shaped, and need random-access CPU reads that don't justify a
//! GPU round-trip.
//!
//! Every other pixel surface in the engine is GPU-authoritative. If selections
//! are ever migrated to GPU compute, this module can be deleted entirely.

use std::collections::HashMap;
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
// TileStore<F> — sparse tile grid, generic over pixel format
// ---------------------------------------------------------------------------

/// Sparse tile grid. Key = (tile_x, tile_y) in tile coordinates.
#[derive(Clone)]
pub struct TileStore<F: TileFormat> {
    tiles: HashMap<(i32, i32), Tile<F>>,
}

impl<F: TileFormat> TileStore<F> {
    pub fn new() -> Self {
        TileStore {
            tiles: HashMap::new(),
        }
    }

    pub fn get(&self, tx: i32, ty: i32) -> Option<&Tile<F>> {
        self.tiles.get(&(tx, ty))
    }

    /// Get a tile for writing, creating an empty one if it doesn't exist.
    pub fn get_or_create(&mut self, tx: i32, ty: i32) -> &mut Tile<F> {
        self.tiles.entry((tx, ty)).or_insert_with(Tile::new_empty)
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
        self.tiles.entry((tx, ty)).or_insert_with(Tile::full)
    }
}

impl<F: TileFormat> Default for TileStore<F> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Type aliases
// ---------------------------------------------------------------------------

/// Single-channel f32 tile grid — used only for the **selection system**.
///
/// Selections use CPU-side AlphaMask because selection operations (boolean
/// add/subtract/intersect, contour extraction for marching ants, feathering,
/// SDF rasterization) are infrequent, irregular-shaped, and need random-access
/// CPU reads that don't justify a GPU round-trip. Every other pixel surface in
/// the engine is GPU-authoritative. If selections are ever migrated to GPU
/// compute, this type (and `tile.rs`) can be deleted entirely.
pub type AlphaMask = TileStore<AlphaF32>;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cow_clone_on_write() {
        let t1 = Tile::<Rgba>::empty();
        let mut t2 = t1.clone();
        t2.write().pixel_mut(0, 0).copy_from_slice(&[255, 0, 0, 255]);
        assert_eq!(t1.data().pixel(0, 0), &[0, 0, 0, 0]);
        assert_eq!(t2.data().pixel(0, 0), &[255, 0, 0, 255]);
    }

    #[test]
    fn tile_store_get_or_create() {
        let mut store = TileStore::<Rgba>::new();
        assert!(store.get(0, 0).is_none());
        let tile = store.get_or_create(0, 0);
        tile.write().pixel_mut(5, 5).copy_from_slice(&[0, 255, 0, 255]);
        assert!(store.get(0, 0).is_some());
        assert_eq!(store.get(0, 0).unwrap().data().pixel(5, 5), &[0, 255, 0, 255]);
    }

    #[test]
    fn tile_coords_for_pixel() {
        let ts = TILE_SIZE as i32;
        assert_eq!(TileStore::<Rgba>::tile_coords_for_pixel(0, 0), (0, 0));
        assert_eq!(TileStore::<Rgba>::tile_coords_for_pixel(ts - 1, ts - 1), (0, 0));
        assert_eq!(TileStore::<Rgba>::tile_coords_for_pixel(ts, 0), (1, 0));
        assert_eq!(TileStore::<Rgba>::tile_coords_for_pixel(-1, -1), (-1, -1));
        assert_eq!(TileStore::<Rgba>::tile_coords_for_pixel(-ts, 0), (-1, 0));
        assert_eq!(TileStore::<Rgba>::tile_coords_for_pixel(-ts - 1, 0), (-2, 0));
    }

    // --- AlphaMask tests ---

    #[test]
    fn alpha_mask_cow() {
        let t1 = Tile::<AlphaF32>::empty();
        let mut t2 = t1.clone();
        t2.write().set(0, 0, 1.0);
        assert_eq!(t1.data().get(0, 0), 0.0);
        assert_eq!(t2.data().get(0, 0), 1.0);
    }
}
