# Phase 1, Session 1 — Scaffold + Tile System

## Scope

Steps 1–2 from the Phase 1 plan. Get the project building end-to-end (Rust workspace + WASM + Svelte + Vite), then implement the core tile data structures with COW semantics and unit tests.

## Done When

- `npm run start` builds WASM and serves the page; a blank canvas appears with no console errors
- `cargo test` passes unit tests for tile COW behavior

---

## Context

Darkly is a browser-based art tool that uses "Veils" (filter layers) to obscure and transform artwork, stimulating creative exploration. Phase 1 establishes the foundation: a tiled raster layer system with GPU compositing and filter shaders.

The project is a new standalone Rust+WASM+Svelte codebase at `/mega/ARTEXP/darkly/`. The Graphite editor at `/mega/ARTEXP/darkly/Graphite/` is reference only. Krita also is at `/mega/ARTEXP/darkly/krita/` if needed.

**Engineering principle:** The core engine does not need to be 100% implemented, but every part that is implemented must be implemented properly on the first iteration. No hacks, no hardcoding, no shortcuts in the engine. The frontend (TypeScript/Svelte) can hardcode and cut corners freely — Rust code cannot, including the WASM bridge.

---

## Step 1: Scaffold the project

Create the workspace, crate structure, frontend boilerplate, and build pipeline.

### Project Structure

```
darkly/
├── Cargo.toml                    # Workspace root
├── crates/
│   └── darkly/                   # Single crate: layers, tiles, undo, GPU
│       ├── Cargo.toml
│       ├── build.rs              # Auto-generates mod.rs registries for filters/, tools/, etc.
│       └── src/
│           ├── lib.rs
│           ├── tile.rs           # TileData, Tile (Arc COW), TileGrid
│           └── ...               # (other modules added in later sessions)
│
├── frontend/
│   ├── package.json
│   ├── vite.config.ts
│   ├── tsconfig.json
│   ├── index.html
│   ├── src/
│   │   ├── main.ts               # Entry: mounts Svelte app
│   │   ├── App.svelte            # Canvas element (mouse handlers + rAF added later)
│   │   └── editor.ts             # WASM init + DarklyHandle bridge
│   └── wasm/
│       ├── Cargo.toml            # cdylib crate for wasm-pack
│       ├── .cargo/config.toml    # WASM linker flags (memory, WebGPU)
│       └── src/
│           ├── lib.rs            # WASM entry, panic hook, logging
│           └── api.rs            # DarklyHandle: #[wasm_bindgen] exports (stub)
│
└── shaders/                      # (populated in later sessions)
```

### Workspace `Cargo.toml`

- Members: `crates/darkly`, `frontend/wasm`
- Workspace dependencies: `wgpu`, `bytemuck`, `serde`, `wasm-bindgen`, `js-sys`, `web-sys`, `log`

### `crates/darkly/Cargo.toml`

Depends on `wgpu`, `bytemuck`, `serde`, `log`. WASM-specific deps (`wasm-bindgen`, `js-sys`, `web-sys`) are behind `cfg(target_arch = "wasm32")`.

### `frontend/wasm/Cargo.toml`

`crate-type = ["cdylib"]`. Depends on `darkly`, `wgpu`, `wasm-bindgen`, `serde-wasm-bindgen`, `js-sys`, `web-sys`, `console_error_panic_hook`, `console_log`.

### `frontend/wasm/.cargo/config.toml`

```toml
[target.wasm32-unknown-unknown]
rustflags = ["--cfg=web_sys_unstable_apis"]

[unstable]
build-std = ["std", "panic_abort"]
```

### Frontend

**`frontend/package.json`:** Svelte 5 + Vite + wasm-pack scripts (modeled on Graphite's `frontend/package.json`).

**`frontend/vite.config.ts`:** Svelte plugin, WASM file serving.

**`frontend/index.html`:** Minimal shell that mounts the Svelte app.

**`frontend/src/main.ts`:** Mount `App.svelte`.

**`frontend/src/App.svelte`:** A full-viewport `<canvas>` element. On mount, calls `editor.ts` to init WASM+GPU. (Mouse listeners and rAF loop added in Session 4.)

**`frontend/src/editor.ts`:** Loads WASM via `init()`, creates `DarklyHandle`, returns the handle.

### Verification

`npm run start` builds WASM and serves the page. A blank canvas appears with no errors in console.

---

## Step 2: Tile system

### `tile.rs`

```rust
pub const TILE_SIZE: usize = 64;
pub const TILE_BYTES: usize = TILE_SIZE * TILE_SIZE * 4; // RGBA u8

#[derive(Clone)]
pub struct TileData(pub [u8; TILE_BYTES]);  // derive bytemuck::Pod

pub struct Tile {
    pub data: Arc<TileData>,
}

impl Tile {
    pub fn empty() -> Self; // shares a static default Arc
    pub fn write(&mut self) -> &mut TileData; // Arc::make_mut, COW
}

/// Sparse tile grid. Key = (tile_x, tile_y) in tile coordinates.
/// Sparse tile grid with built-in transaction recording for undo.
/// When a transaction is active, `get_or_create()` transparently captures
/// pre-write tile state into a `Memento`. Paint tools never know about undo.
pub struct TileGrid {
    tiles: HashMap<(i32, i32), Tile>,
    recording: Option<Memento>,  // active transaction
}

impl TileGrid {
    pub fn new() -> Self;
    pub fn get(&self, tx: i32, ty: i32) -> Option<&Tile>;
    pub fn get_or_create(&mut self, tx: i32, ty: i32) -> &mut Tile;
    pub fn begin_transaction(&mut self);               // start recording
    pub fn commit_transaction(&mut self) -> Option<Memento>; // stop, return diff
    pub fn rollback(&mut self, memento: &Memento) -> (Memento, HashSet<(i32,i32)>);
    pub fn rollforward(&mut self, memento: &Memento) -> (Memento, HashSet<(i32,i32)>);
    pub fn tile_coords_for_pixel(x: i32, y: i32) -> (i32, i32);
}

/// Sparse record of pre-write tile states. Only stores the first access
/// per tile per transaction. `None` = tile was newly created (didn't exist before).
pub struct Memento {
    tiles: HashMap<(i32, i32), Option<Arc<TileData>>>,
}
```

### Key behaviors

- **COW via `Arc::make_mut`**: `Tile::write()` calls `Arc::make_mut(&mut self.data)`, which clones only if refcount > 1. This is the foundation of efficient undo — undo snapshots share `Arc` pointers with the live grid until a tile is actually modified.

- **Transaction recording**: `get_or_create()` checks if `recording` is active. If so, and this tile coord hasn't been recorded yet, it captures `Some(Arc::clone(&tile.data))` (existing tile) or `None` (new tile) into the `Memento`. Paint tools never interact with the undo system directly.

- **Rollback removes tiles**: When `rollback()` encounters a `None` in the memento (tile didn't exist before), it removes the tile from the grid entirely. This is critical for correct undo of strokes on empty areas.

### Verification

Unit tests:
- Create tiles, write pixels, verify COW cloning behavior (shared Arc before write, separate after)
- Transaction recording: begin, modify tiles, commit, verify memento captures pre-write state
- Rollback/rollforward symmetry
- `tile_coords_for_pixel` edge cases (negative coords, tile boundaries)

---

## Key Reference Files (Graphite, patterns only)

- WASM entry: `Graphite/frontend/wasm/src/lib.rs`
- WASM API: `Graphite/frontend/wasm/src/editor_api.rs`
- Frontend init: `Graphite/frontend/src/editor.ts`
- Package setup: `Graphite/frontend/package.json`
