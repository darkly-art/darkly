use crate::sdf;
use crate::tile::AlphaMask;
use crate::tool::ToolRegistration;

pub fn register() -> ToolRegistration {
    ToolRegistration {
        type_id: "rect_select",
        display_name: "Rectangle Select",
        params: &[],
    }
}

/// Rasterize a rectangle selection into an AlphaMask via SDF.
pub fn rasterize(x: f32, y: f32, w: f32, h: f32, antialias: bool, feather: f32) -> AlphaMask {
    let cx = x + w * 0.5;
    let cy = y + h * 0.5;
    let half_w = w * 0.5;
    let half_h = h * 0.5;

    let mut mask = AlphaMask::new();
    mask.rasterize(
        (x as i32, y as i32, w.ceil() as i32, h.ceil() as i32),
        |px, py| sdf::sdf_rect(px, py, cx, cy, half_w, half_h),
        antialias,
        feather,
    );
    mask
}
