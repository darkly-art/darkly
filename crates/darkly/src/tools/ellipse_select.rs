use crate::sdf;
use crate::tile::AlphaMask;
use crate::tool::ToolRegistration;

pub fn register() -> ToolRegistration {
    ToolRegistration {
        type_id: "ellipse_select",
        params: &[],
    }
}

/// Rasterize an ellipse selection into an AlphaMask via SDF.
/// (x, y, w, h) defines the bounding box; the ellipse is inscribed in it.
pub fn rasterize(x: f32, y: f32, w: f32, h: f32, antialias: bool, feather: f32) -> AlphaMask {
    let cx = x + w * 0.5;
    let cy = y + h * 0.5;
    let rx = w * 0.5;
    let ry = h * 0.5;

    let mut mask = AlphaMask::new();
    mask.rasterize(
        (x as i32, y as i32, w.ceil() as i32, h.ceil() as i32),
        |px, py| sdf::sdf_ellipse(px, py, cx, cy, rx, ry),
        antialias,
        feather,
    );
    mask
}
