//! 2D view transform for canvas navigation.
//! Compositing happens in canvas-pixel space. This transform is applied
//! only in the present shader to map canvas pixels to screen pixels.

/// Fallback workspace color used until the frontend pushes the theme-sourced
/// color via `set_viewport_bg()`.
pub const DEFAULT_WORKSPACE_BG: [f32; 4] = [0.11, 0.11, 0.11, 1.0];
/// Decomposed view parameters: the session-state inputs from which the
/// present [`ViewTransform`] is derived. Retained so the matrix can be rebuilt
/// whenever *either* input changes: pointer-driven pan/zoom/rotation (via
/// `set_view_transform`) **or** a canvas-dimension change (resize / crop /
/// undo). The matrix bakes in `canvas_w/h` (both the present shader's
/// sampling-normalization and the canvas center), so a dims change with stale
/// params would present a stretched/offset image until the next pointer event.
#[derive(Copy, Clone, Debug)]
pub struct ViewParams {
    pub pan_x: f32,
    pub pan_y: f32,
    pub zoom: f32,
    pub rotation: f32,
    pub mirror_h: bool,
    pub screen_w: f32,
    pub screen_h: f32,
}

impl Default for ViewParams {
    fn default() -> Self {
        ViewParams {
            pan_x: 0.0,
            pan_y: 0.0,
            zoom: 1.0,
            rotation: 0.0,
            mirror_h: false,
            screen_w: 1.0,
            screen_h: 1.0,
        }
    }
}

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
    /// Per-present flags consumed by the present shader. Stamped by the
    /// compositor on upload, parallel to `bg`.
    /// `flags[0]` = pixel filter mode: 0 = linear, 1 = nearest, 2 = auto
    /// (nearest when zoom > 1, linear otherwise, decided in the shader
    /// from the inverse-zoom magnitude of `row0.xy`).
    pub flags: [f32; 4],
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
            flags: [0.0; 4],
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
            flags: [0.0; 4],
        }
    }

    /// Transform a screen point to **window-local** canvas coordinates (origin at
    /// the canvas-window top-left, `canvas_origin` not applied) using the stored
    /// inverse matrix. This is the frame the present shader samples in; tools and
    /// the overlay want plane coords instead (see [`Self::screen_to_plane`]).
    pub fn screen_to_window_local(&self, screen_x: f32, screen_y: f32) -> (f32, f32) {
        apply_2x3(&self.matrix, screen_x, screen_y)
    }

    /// Inverse (screen → **plane**) matrix: the stored window-local inverse with
    /// `canvas_origin` added to its translate column. `plane = window_local +
    /// canvas_origin` is a pure output translation, so the linear coefficients are
    /// unchanged. This is the frame every tool and the overlay operate in.
    pub fn screen_to_plane_matrix(&self, origin_x: f32, origin_y: f32) -> [[f32; 4]; 3] {
        let mut m = self.matrix;
        m[2][0] += origin_x;
        m[2][1] += origin_y;
        m
    }

    /// Forward (plane → screen) matrix: the inverse of [`Self::screen_to_plane_matrix`].
    pub fn plane_to_screen_matrix(&self, origin_x: f32, origin_y: f32) -> [[f32; 4]; 3] {
        invert_2x3(&self.screen_to_plane_matrix(origin_x, origin_y))
    }

    /// Transform a screen point to **plane** coordinates (window-local +
    /// `canvas_origin`). Single chokepoint shared by the engine query and the
    /// pure `compute_view_matrices` bridge fn.
    pub fn screen_to_plane(
        &self,
        screen_x: f32,
        screen_y: f32,
        origin_x: f32,
        origin_y: f32,
    ) -> (f32, f32) {
        apply_2x3(
            &self.screen_to_plane_matrix(origin_x, origin_y),
            screen_x,
            screen_y,
        )
    }
}

/// Pure constructor of the frontend coordinate matrices: the single source of
/// truth the JS coordinate path consumes (via the WASM bridge) instead of
/// re-deriving the transform. Borrows no engine state, so the bridge wrapper is
/// safe to call during a pointer event (no RefCell aliasing with `render`).
///
/// Returns 12 floats: `[screen→plane (6), plane→screen (6)]`, each packed
/// row-major `[m00, m01, m02, m10, m11, m12]` with
/// `out_x = m00·x + m01·y + m02`, `out_y = m10·x + m11·y + m12`, one matvec
/// convention for both directions on the JS side.
#[allow(clippy::too_many_arguments)]
pub fn compute_view_matrices(
    pan_x: f32,
    pan_y: f32,
    zoom: f32,
    rotation: f32,
    mirror_h: bool,
    screen_w: f32,
    screen_h: f32,
    canvas_origin_x: f32,
    canvas_origin_y: f32,
    canvas_w: f32,
    canvas_h: f32,
) -> [f32; 12] {
    let vt = ViewTransform::from_pan_zoom_rotate(
        pan_x, pan_y, zoom, rotation, mirror_h, screen_w, screen_h, canvas_w, canvas_h,
    );
    // screen→plane is consumed in the column convention (`apply_2x3`): repack to
    // row-major. plane→screen (`invert_2x3` output) is already row-major.
    let s2p = vt.screen_to_plane_matrix(canvas_origin_x, canvas_origin_y);
    let p2s = vt.plane_to_screen_matrix(canvas_origin_x, canvas_origin_y);
    [
        s2p[0][0], s2p[1][0], s2p[2][0], s2p[0][1], s2p[1][1], s2p[2][1], //
        p2s[0][0], p2s[0][1], p2s[2][0], p2s[1][0], p2s[1][1], p2s[2][1],
    ]
}

/// Apply a 2×3 affine (stored as 3 std140 vec4 rows) to a point.
/// `out_x = m[0][0]·x + m[1][0]·y + m[2][0]`, likewise for y with the `[..][1]`
/// column. Matches the convention `present.wgsl` / `overlay.wgsl` apply.
fn apply_2x3(m: &[[f32; 4]; 3], x: f32, y: f32) -> (f32, f32) {
    (
        m[0][0] * x + m[1][0] * y + m[2][0],
        m[0][1] * x + m[1][1] * y + m[2][1],
    )
}

/// Invert a 2×3 affine (screen↔canvas), preserving the std140 row layout.
/// Single home for the matrix inversion: consumed by [`ViewTransform::plane_to_screen_matrix`]
/// and the overlay's forward matrix. Degenerate (singular) input falls back to identity.
pub(crate) fn invert_2x3(m: &[[f32; 4]; 3]) -> [[f32; 4]; 3] {
    let m00 = m[0][0];
    let m01 = m[0][1];
    let m10 = m[1][0];
    let m11 = m[1][1];
    let tx = m[2][0];
    let ty = m[2][1];

    let det = m00 * m11 - m10 * m01;
    if det.abs() < 1e-12 {
        return [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 0.0],
        ];
    }
    let inv_det = 1.0 / det;

    let f00 = m11 * inv_det;
    let f01 = -m10 * inv_det;
    let f10 = -m01 * inv_det;
    let f11 = m00 * inv_det;
    let ftx = -(f00 * tx + f01 * ty);
    let fty = -(f10 * tx + f11 * ty);

    [
        [f00, f01, 0.0, 0.0],
        [f10, f11, 0.0, 0.0],
        [ftx, fty, 0.0, 0.0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    /// Apply a forward (plane→screen) matrix the way `shaders/overlay.wgsl`
    /// `canvas_to_screen` does: row convention `out_i = row_i · p`. The forward
    /// matrices from `plane_to_screen_matrix` / `invert_2x3` are stored for this
    /// convention (it is the true functional inverse of the column-convention
    /// `apply_2x3` used for screen→canvas).
    fn fwd_apply(m: &[[f32; 4]; 3], x: f32, y: f32) -> (f32, f32) {
        (
            m[0][0] * x + m[0][1] * y + m[2][0],
            m[1][0] * x + m[1][1] * y + m[2][1],
        )
    }

    #[test]
    fn mirror_h_keeps_center_fixed() {
        // With mirror_h on, the screen-center pixel still resolves to the
        // canvas center; mirror reflects around the canvas's vertical axis.
        let t = ViewTransform::from_pan_zoom_rotate(
            0.0, 0.0, 1.0, 0.0, true, 800.0, 600.0, 400.0, 300.0,
        );
        let (cx, cy) = t.screen_to_window_local(400.0, 300.0);
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
            let (ux, uy) = unmirrored.screen_to_window_local(x, y);
            let (mx, my) = mirrored.screen_to_window_local(x, y);
            assert!(
                approx(ux + mx, cw),
                "x={x} y={y}: ux+mx={} (want {cw})",
                ux + mx
            );
            assert!(approx(uy, my), "x={x} y={y}: uy={uy} my={my}");
        }
    }

    /// The plane forward (fed to the overlay) must map a plane point P to the
    /// SAME screen pixel the paint path implies, i.e. the window-local forward
    /// of `P - canvas_origin`. This is the hover-vs-paint mismatch contract:
    /// with a non-zero origin the two must still agree.
    #[test]
    fn plane_forward_matches_window_local_paint_path() {
        let (ox, oy) = (8.0_f32, 4.0_f32);
        let vt = ViewTransform::from_pan_zoom_rotate(
            17.0, -9.0, 1.6, 0.3, false, 800.0, 600.0, 40.0, 40.0,
        );
        let plane_fwd = vt.plane_to_screen_matrix(ox, oy);
        let window_fwd = vt.plane_to_screen_matrix(0.0, 0.0);
        for &(px, py) in &[(28.0, 24.0), (8.0, 4.0), (47.0, 43.0)] {
            let via_plane = fwd_apply(&plane_fwd, px, py);
            let via_paint = fwd_apply(&window_fwd, px - ox, py - oy);
            assert!(
                approx(via_plane.0, via_paint.0) && approx(via_plane.1, via_paint.1),
                "plane forward {via_plane:?} != paint path {via_paint:?} at ({px},{py})"
            );
        }
    }

    /// screen → plane and plane → screen are exact inverses, and shifting the
    /// origin shifts the plane output by exactly the delta.
    #[test]
    fn screen_plane_round_trip_and_origin_shift() {
        let vt = ViewTransform::from_pan_zoom_rotate(
            5.0, 3.0, 1.3, -0.2, true, 800.0, 600.0, 50.0, 70.0,
        );
        let fwd = vt.plane_to_screen_matrix(12.0, -6.0);
        for &(sx, sy) in &[(123.0, 88.0), (400.0, 300.0), (0.0, 0.0)] {
            let p = vt.screen_to_plane(sx, sy, 12.0, -6.0);
            let back = fwd_apply(&fwd, p.0, p.1);
            assert!(
                approx(back.0, sx) && approx(back.1, sy),
                "round-trip ({sx},{sy}) -> {p:?} -> {back:?}"
            );
        }
        // Origin shift: plane output moves by exactly the origin delta.
        let a = vt.screen_to_plane(200.0, 150.0, 0.0, 0.0);
        let b = vt.screen_to_plane(200.0, 150.0, 40.0, 15.0);
        assert!(approx(b.0 - a.0, 40.0) && approx(b.1 - a.1, 15.0));
    }

    /// DRY contract: the pure `compute_view_matrices` (the JS coordinate path's
    /// source) agrees with the engine-side `ViewTransform` for identical inputs,
    /// and its packed row-major matrices round-trip. Guards against the JS coord
    /// path and the present/engine path ever diverging.
    #[test]
    fn compute_view_matrices_matches_view_transform() {
        let (pan_x, pan_y, zoom, rot, mir) = (11.0, -4.0, 1.4, 0.25, false);
        let (sw, sh, cw, ch) = (800.0, 600.0, 50.0, 70.0);
        let (ox, oy) = (9.0, -3.0);
        let m = compute_view_matrices(pan_x, pan_y, zoom, rot, mir, sw, sh, ox, oy, cw, ch);
        let vt = ViewTransform::from_pan_zoom_rotate(pan_x, pan_y, zoom, rot, mir, sw, sh, cw, ch);
        let row = |m: &[f32; 12], o: usize, x: f32, y: f32| {
            (
                m[o] * x + m[o + 1] * y + m[o + 2],
                m[o + 3] * x + m[o + 4] * y + m[o + 5],
            )
        };
        for &(sx, sy) in &[(123.0, 88.0), (400.0, 300.0)] {
            let want = vt.screen_to_plane(sx, sy, ox, oy);
            let got = row(&m, 0, sx, sy);
            assert!(
                approx(got.0, want.0) && approx(got.1, want.1),
                "screen→plane pack {got:?} != {want:?}"
            );
            let back = row(&m, 6, want.0, want.1);
            assert!(
                approx(back.0, sx) && approx(back.1, sy),
                "plane→screen pack did not round-trip: {back:?} != ({sx},{sy})"
            );
        }
    }
}
