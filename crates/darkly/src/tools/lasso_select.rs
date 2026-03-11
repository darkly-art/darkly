use crate::sdf;
use crate::tile::AlphaMask;
use crate::tool::ToolRegistration;

pub fn register() -> ToolRegistration {
    ToolRegistration {
        type_id: "lasso_select",
        params: &[],
    }
}

/// Rasterize a freehand polygon selection into an AlphaMask via SDF.
/// `vertices` is a list of [x, y] points forming a closed polygon.
pub fn rasterize(vertices: &[[f32; 2]], antialias: bool, feather: f32) -> AlphaMask {
    let mut mask = AlphaMask::new();
    if vertices.len() < 3 {
        return mask;
    }

    // Compute bounding box of the polygon
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for v in vertices {
        min_x = min_x.min(v[0]);
        min_y = min_y.min(v[1]);
        max_x = max_x.max(v[0]);
        max_y = max_y.max(v[1]);
    }

    let x = min_x.floor() as i32;
    let y = min_y.floor() as i32;
    let w = (max_x - min_x).ceil() as i32;
    let h = (max_y - min_y).ceil() as i32;

    mask.rasterize(
        (x, y, w, h),
        |px, py| sdf::sdf_polygon(px, py, vertices),
        antialias,
        feather,
    );
    mask
}
