use crate::tile::{AlphaMask, TileGrid, TILE_SIZE};
use crate::tool::ToolRegistration;

pub fn register() -> ToolRegistration {
    ToolRegistration {
        type_id: "magic_wand",
        params: &[],
    }
}

/// Flood-fill from a seed point on `source` tiles, writing 1.0 into an AlphaMask
/// for all contiguous pixels whose color is within `tolerance` of the seed color.
pub fn rasterize(
    source: &TileGrid,
    seed_x: i32,
    seed_y: i32,
    canvas_w: i32,
    canvas_h: i32,
    tolerance: u8,
) -> AlphaMask {
    let mut mask = AlphaMask::new();

    if seed_x < 0 || seed_y < 0 || seed_x >= canvas_w || seed_y >= canvas_h {
        return mask;
    }

    let tile_size = TILE_SIZE as i32;

    // Read the seed color
    let (stx, sty) = TileGrid::tile_coords_for_pixel(seed_x, seed_y);
    let slx = (seed_x - stx * tile_size) as usize;
    let sly = (seed_y - sty * tile_size) as usize;

    let seed_color = match source.get(stx, sty) {
        Some(t) => *t.data().pixel(slx, sly),
        None => [0, 0, 0, 0],
    };

    let tol = tolerance as i16;
    let matches = |px: &[u8; 4]| -> bool {
        (px[0] as i16 - seed_color[0] as i16).abs() <= tol
            && (px[1] as i16 - seed_color[1] as i16).abs() <= tol
            && (px[2] as i16 - seed_color[2] as i16).abs() <= tol
            && (px[3] as i16 - seed_color[3] as i16).abs() <= tol
    };

    let mut visited = std::collections::HashSet::new();
    let mut stack = vec![(seed_x, seed_y)];

    while let Some((x, y)) = stack.pop() {
        if x < 0 || y < 0 || x >= canvas_w || y >= canvas_h {
            continue;
        }
        if !visited.insert((x, y)) {
            continue;
        }

        let (tx, ty) = TileGrid::tile_coords_for_pixel(x, y);
        let lx = (x - tx * tile_size) as usize;
        let ly = (y - ty * tile_size) as usize;

        let current = match source.get(tx, ty) {
            Some(t) => *t.data().pixel(lx, ly),
            None => [0, 0, 0, 0],
        };

        if !matches(&current) {
            continue;
        }

        // Write 1.0 into the mask at this pixel
        let mask_tile = mask.get_or_create(tx, ty);
        mask_tile.write().set(lx, ly, 1.0);

        stack.push((x + 1, y));
        stack.push((x - 1, y));
        stack.push((x, y + 1));
        stack.push((x, y - 1));
    }

    mask
}
