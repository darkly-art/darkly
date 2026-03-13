use crate::tile::{AlphaMask, TileGrid};
use crate::tool::ToolRegistration;

pub fn register() -> ToolRegistration {
    ToolRegistration {
        type_id: "magic_wand",
        params: &[],
    }
}

/// Flood-fill from a seed point on `source` tiles, producing an AlphaMask
/// with 1.0 for all contiguous pixels within `tolerance` of the seed color.
/// Delegates to the shared scanline flood fill in `AlphaMask`.
pub fn rasterize(
    source: &TileGrid,
    seed_x: i32,
    seed_y: i32,
    canvas_w: i32,
    canvas_h: i32,
    tolerance: u8,
) -> AlphaMask {
    AlphaMask::flood_fill(source, seed_x, seed_y, canvas_w, canvas_h, tolerance)
}
