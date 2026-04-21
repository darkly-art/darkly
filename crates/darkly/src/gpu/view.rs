/// 2D view transform for canvas navigation.
/// Compositing happens in canvas-pixel space. This transform is applied
/// only in the present shader to map canvas pixels to screen pixels.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ViewTransform {
    /// Inverse view matrix (screen -> canvas), stored as 3 vec4s for std140.
    /// Row 0: [m00, m01, canvas_w, 0]
    /// Row 1: [m10, m11, canvas_h, 0]
    /// Row 2: [tx,  ty,  1,        0]
    pub matrix: [[f32; 4]; 3],
}

impl ViewTransform {
    pub fn identity() -> Self {
        ViewTransform {
            matrix: [
                [1.0, 0.0, 1.0, 0.0],
                [0.0, 1.0, 1.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
        }
    }

    /// Build the inverse view matrix (screen -> canvas) from pan/zoom/rotation.
    /// The forward transform is: canvas -> screen
    ///   1. Translate by -canvas_center
    ///   2. Scale by zoom
    ///   3. Rotate by rotation
    ///   4. Translate by screen_center + pan
    ///
    /// The present shader needs the inverse: screen -> canvas.
    pub fn from_pan_zoom_rotate(
        pan_x: f32,
        pan_y: f32,
        zoom: f32,
        rotation: f32, // radians
        screen_w: f32,
        screen_h: f32,
        canvas_w: f32,
        canvas_h: f32,
    ) -> Self {
        let cos_r = rotation.cos();
        let sin_r = rotation.sin();
        let inv_zoom = 1.0 / zoom;

        let cx = canvas_w / 2.0;
        let cy = canvas_h / 2.0;
        let sx = screen_w / 2.0 + pan_x;
        let sy = screen_h / 2.0 + pan_y;

        // Inverse: undo translate, undo rotate, undo scale, undo center
        let m00 = cos_r * inv_zoom;
        let m01 = sin_r * inv_zoom;
        let m10 = -sin_r * inv_zoom;
        let m11 = cos_r * inv_zoom;
        let tx = cx - m00 * sx - m10 * sy;
        let ty = cy - m01 * sx - m11 * sy;

        ViewTransform {
            matrix: [
                [m00, m01, canvas_w, 0.0],
                [m10, m11, canvas_h, 0.0],
                [tx, ty, 1.0, 0.0],
            ],
        }
    }

    /// Transform a screen point to canvas coordinates using the stored inverse matrix.
    pub fn screen_to_canvas(&self, screen_x: f32, screen_y: f32) -> (f32, f32) {
        let m = &self.matrix;
        let cx = m[0][0] * screen_x + m[1][0] * screen_y + m[2][0];
        let cy = m[0][1] * screen_x + m[1][1] * screen_y + m[2][1];
        (cx, cy)
    }
}
