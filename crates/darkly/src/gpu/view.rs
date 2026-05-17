//! 2D view transform for canvas navigation.
//! Compositing happens in canvas-pixel space. This transform is applied
//! only in the present shader to map canvas pixels to screen pixels.

/// Fallback workspace color (matches the legacy hardcoded value previously
/// baked into `present.wgsl`). The frontend pushes the theme-sourced color
/// via `set_viewport_bg()` once the UI loads.
pub const DEFAULT_WORKSPACE_BG: [f32; 4] = [0.11, 0.11, 0.11, 1.0];
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ViewTransform {
    /// Inverse view matrix (screen -> canvas), stored as 3 vec4s for std140.
    /// Row 0: [m00, m01, canvas_w, 0]
    /// Row 1: [m10, m11, canvas_h, 0]
    /// Row 2: [tx,  ty,  1,        0]
    pub matrix: [[f32; 4]; 3],
    /// Workspace color shown in the present shader for pixels outside the
    /// canvas. Only consumed by the present pipeline; other uniform users
    /// (overlay forward-matrix, etc.) ignore this field. Owned by the
    /// compositor and stamped onto every transform on upload.
    pub bg: [f32; 4],
}

impl ViewTransform {
    pub fn identity() -> Self {
        ViewTransform {
            matrix: [
                [1.0, 0.0, 1.0, 0.0],
                [0.0, 1.0, 1.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
            bg: DEFAULT_WORKSPACE_BG,
        }
    }

    /// Build the inverse view matrix (screen -> canvas) from pan/zoom/rotation.
    /// The forward transform is: canvas -> screen
    ///   1. Translate by -canvas_center
    ///   2. Scale by (-1, 1) if `mirror_h` (X-flip around canvas center)
    ///   3. Scale by zoom
    ///   4. Rotate by rotation
    ///   5. Translate by screen_center + pan
    ///
    /// The present shader needs the inverse: screen -> canvas.
    pub fn from_pan_zoom_rotate(
        pan_x: f32,
        pan_y: f32,
        zoom: f32,
        rotation: f32, // radians
        mirror_h: bool,
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
        let mut m00 = cos_r * inv_zoom;
        let m01 = sin_r * inv_zoom;
        let mut m10 = -sin_r * inv_zoom;
        let m11 = cos_r * inv_zoom;
        let mut tx = cx - m00 * sx - m10 * sy;
        let ty = cy - m01 * sx - m11 * sy;

        // Horizontal mirror: reflect the screen→canvas X output around `cx`.
        // Equivalent to inserting a scale(-1, 1) step right before the final
        // +(cx, cy) translate in the inverse pipeline.
        if mirror_h {
            m00 = -m00;
            m10 = -m10;
            tx = canvas_w - tx;
        }

        ViewTransform {
            matrix: [
                [m00, m01, canvas_w, 0.0],
                [m10, m11, canvas_h, 0.0],
                [tx, ty, 1.0, 0.0],
            ],
            bg: DEFAULT_WORKSPACE_BG,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn mirror_h_keeps_center_fixed() {
        // With mirror_h on, the screen-center pixel still resolves to the
        // canvas center — mirror reflects around the canvas's vertical axis.
        let t = ViewTransform::from_pan_zoom_rotate(
            0.0, 0.0, 1.0, 0.0, true, 800.0, 600.0, 400.0, 300.0,
        );
        let (cx, cy) = t.screen_to_canvas(400.0, 300.0);
        assert!(approx(cx, 200.0), "mirrored center cx was {cx}");
        assert!(approx(cy, 150.0), "mirrored center cy was {cy}");
    }

    #[test]
    fn mirror_h_reflects_canvas_x_around_canvas_center() {
        // At a given screen point, the mirrored transform resolves to a
        // canvas point whose X is the unmirrored result reflected across
        // `canvas_w / 2`; Y is unchanged. This holds for any pan/zoom/rotation
        // because the mirror is composed *inside* the canvas-space side of
        // the transform, not on the screen side.
        let pan_x = 37.0;
        let pan_y = -12.0;
        let zoom = 1.7;
        let rot = 0.4;
        let sw = 800.0;
        let sh = 600.0;
        let cw = 400.0;
        let ch = 300.0;
        let unmirrored =
            ViewTransform::from_pan_zoom_rotate(pan_x, pan_y, zoom, rot, false, sw, sh, cw, ch);
        let mirrored =
            ViewTransform::from_pan_zoom_rotate(pan_x, pan_y, zoom, rot, true, sw, sh, cw, ch);
        for &(x, y) in &[(123.0, 88.0), (600.0, 450.0), (0.0, 0.0), (sw, sh)] {
            let (ux, uy) = unmirrored.screen_to_canvas(x, y);
            let (mx, my) = mirrored.screen_to_canvas(x, y);
            assert!(
                approx(ux + mx, cw),
                "x={x} y={y}: ux+mx={} (want {cw})",
                ux + mx
            );
            assert!(approx(uy, my), "x={x} y={y}: uy={uy} my={my}");
        }
    }
}
