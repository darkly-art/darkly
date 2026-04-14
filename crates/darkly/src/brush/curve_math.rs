//! Natural cubic spline with precomputed LUT.
//!
//! Used by the curve node to map scalar inputs through an adjustable
//! transfer function.  Natural cubic splines minimize total curvature
//! and have zero second derivative at endpoints — endpoints only set
//! the y-intercept without distorting the interior curve shape.
//!
//! Prior art: Krita's `KisLegacyCubicSpline` in `kis_cubic_curve_spline.h`
//! uses the same natural cubic spline algorithm with a tridiagonal solver.
//! We follow the same math and the same LUT approach: build once when
//! control points change, O(1) lookup per dab.

/// Number of entries in the lookup table.
const LUT_SIZE: usize = 256;

/// A precomputed curve lookup table for O(1) evaluation.
#[derive(Clone, Debug)]
pub struct CurveLut {
    table: [f32; LUT_SIZE],
}

impl CurveLut {
    /// Build a LUT from sorted control points.
    ///
    /// Points must be sorted by x with x values in [0, 1].
    /// Minimum 2 points required.  Panics if fewer than 2.
    pub fn from_points(points: &[[f32; 2]]) -> Self {
        assert!(points.len() >= 2, "curve needs at least 2 control points");

        let n = points.len();
        let mut table = [0.0f32; LUT_SIZE];

        if n == 2 {
            // Simple linear interpolation between two endpoints.
            let [x0, y0] = points[0];
            let [x1, y1] = points[1];
            let dx = (x1 - x0).max(1e-6);
            for i in 0..LUT_SIZE {
                let t = i as f32 / (LUT_SIZE - 1) as f32;
                let frac = ((t - x0) / dx).clamp(0.0, 1.0);
                table[i] = (y0 + frac * (y1 - y0)).clamp(0.0, 1.0);
            }
            return CurveLut { table };
        }

        // Natural cubic spline: compute coefficients per segment.
        let spline = NaturalCubicSpline::from_points(points);

        // Sample the spline into the LUT.
        for i in 0..LUT_SIZE {
            let t = i as f32 / (LUT_SIZE - 1) as f32;
            table[i] = spline.evaluate(t).clamp(0.0, 1.0);
        }

        CurveLut { table }
    }

    /// O(1) lookup with linear interpolation between table entries.
    /// Input is clamped to [0, 1].
    #[inline]
    pub fn evaluate(&self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        let idx = t * (LUT_SIZE - 1) as f32;
        let lo = idx as usize;
        let hi = (lo + 1).min(LUT_SIZE - 1);
        let frac = idx - lo as f32;
        self.table[lo] * (1.0 - frac) + self.table[hi] * frac
    }

    /// Direct access to the table (for serialization or debugging).
    pub fn table(&self) -> &[f32; LUT_SIZE] {
        &self.table
    }
}

/// Natural cubic spline coefficients.
///
/// Each segment i is evaluated as:
///   s(x) = a[i] + b[i]*(x-x[i]) + 0.5*c[i]*(x-x[i])^2 + (1/6)*d[i]*(x-x[i])^3
///
/// Natural boundary conditions: c[0] = 0, c[n] = 0 (zero second derivative
/// at endpoints).  This makes the curve approach endpoints linearly, so
/// moving an endpoint only changes the intercept, not the interior shape.
///
/// Algorithm matches Krita's `KisLegacyCubicSpline` in `kis_cubic_curve_spline.h`.
struct NaturalCubicSpline {
    /// y-values at each control point
    a: Vec<f32>,
    /// First derivative coefficients per segment
    b: Vec<f32>,
    /// Second derivative values at each control point
    c: Vec<f32>,
    /// Third derivative coefficients per segment
    d: Vec<f32>,
    /// Segment widths
    h: Vec<f32>,
    /// x-values at each control point
    x: Vec<f32>,
}

impl NaturalCubicSpline {
    fn from_points(points: &[[f32; 2]]) -> Self {
        let intervals = points.len() - 1;

        let x: Vec<f32> = points.iter().map(|p| p[0]).collect();
        let a: Vec<f32> = points.iter().map(|p| p[1]).collect();

        let mut h = vec![0.0f32; intervals];
        for i in 0..intervals {
            h[i] = (x[i + 1] - x[i]).max(1e-6);
        }

        // Solve tridiagonal system for second derivatives (c).
        // Natural boundary conditions: c[0] = 0, c[n] = 0.
        let c = if intervals > 1 {
            // Build tridiagonal system for interior points.
            let inner = intervals - 1; // number of interior points
            let mut tri_b = vec![0.0f32; inner]; // diagonal
            let mut tri_f = vec![0.0f32; inner]; // right-hand side

            for i in 0..inner {
                tri_b[i] = 2.0 * (h[i] + h[i + 1]);
                tri_f[i] = 6.0
                    * ((a[i + 2] - a[i + 1]) / h[i + 1] - (a[i + 1] - a[i]) / h[i]);
            }

            // Sub/super-diagonal: h[1], h[2], ..., h[n-2]
            let tri_a: Vec<f32> = (1..inner).map(|i| h[i]).collect();

            let inner_c = tridiagonal_solve(&tri_a, &tri_b, &tri_a, &tri_f);

            // Prepend and append zero for natural boundary conditions.
            let mut c = Vec::with_capacity(intervals + 1);
            c.push(0.0);
            c.extend_from_slice(&inner_c);
            c.push(0.0);
            c
        } else {
            vec![0.0; intervals + 1]
        };

        // Compute d and b coefficients from c.
        let mut d = vec![0.0f32; intervals];
        let mut b = vec![0.0f32; intervals];
        for i in 0..intervals {
            d[i] = (c[i + 1] - c[i]) / h[i];
            b[i] = -0.5 * c[i] * h[i] - (1.0 / 6.0) * d[i] * h[i] * h[i]
                + (a[i + 1] - a[i]) / h[i];
        }

        NaturalCubicSpline { a, b, c, d, h, x }
    }

    fn evaluate(&self, t: f32) -> f32 {
        let t = t.clamp(self.x[0], *self.x.last().unwrap());

        // Find the segment containing t.
        let intervals = self.h.len();
        let mut i = 0;
        while i < intervals - 1 && t >= self.x[i + 1] {
            i += 1;
        }

        let dx = t - self.x[i];
        self.a[i] + self.b[i] * dx + 0.5 * self.c[i] * dx * dx + (1.0 / 6.0) * self.d[i] * dx * dx * dx
    }
}

/// Solve a tridiagonal system using the Thomas algorithm.
///
/// System: sub[i-1]*x[i-1] + diag[i]*x[i] + sup[i]*x[i+1] = rhs[i]
/// where sub has length n-1, diag/rhs have length n, sup has length n-1.
fn tridiagonal_solve(sub: &[f32], diag: &[f32], sup: &[f32], rhs: &[f32]) -> Vec<f32> {
    let n = diag.len();

    if n == 1 {
        return vec![rhs[0] / diag[0]];
    }

    // Forward sweep.
    let mut alpha = vec![0.0f32; n];
    let mut beta = vec![0.0f32; n];

    alpha[1] = -sup[0] / diag[0];
    beta[1] = rhs[0] / diag[0];

    for i in 1..n - 1 {
        let denom = sub[i - 1] * alpha[i] + diag[i];
        alpha[i + 1] = -sup[i] / denom;
        beta[i + 1] = (rhs[i] - sub[i - 1] * beta[i]) / denom;
    }

    // Back substitution.
    let mut x = vec![0.0f32; n];
    x[n - 1] =
        (rhs[n - 1] - sub[n - 2] * beta[n - 1]) / (diag[n - 1] + sub[n - 2] * alpha[n - 1]);

    for i in (0..n - 1).rev() {
        x[i] = alpha[i + 1] * x[i + 1] + beta[i + 1];
    }

    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_curve() {
        let lut = CurveLut::from_points(&[[0.0, 0.0], [1.0, 1.0]]);
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let v = lut.evaluate(t);
            assert!((v - t).abs() < 0.01, "identity at {t}: got {v}");
        }
    }

    #[test]
    fn endpoints_exact() {
        let points = [[0.0, 0.2], [0.5, 0.8], [1.0, 0.6]];
        let lut = CurveLut::from_points(&points);
        assert!((lut.evaluate(0.0) - 0.2).abs() < 0.01, "start: {}", lut.evaluate(0.0));
        assert!((lut.evaluate(1.0) - 0.6).abs() < 0.01, "end: {}", lut.evaluate(1.0));
    }

    #[test]
    fn s_curve_midpoint() {
        let lut = CurveLut::from_points(&[
            [0.0, 0.0],
            [0.5, 0.2],
            [1.0, 1.0],
        ]);
        let v = lut.evaluate(0.5);
        assert!((v - 0.2).abs() < 0.05, "s-curve at 0.5: got {v}, expected ~0.2");
    }

    #[test]
    fn clamped_output() {
        let lut = CurveLut::from_points(&[[0.0, 0.0], [1.0, 1.0]]);
        assert!(lut.evaluate(-0.5) >= 0.0);
        assert!(lut.evaluate(1.5) <= 1.0);
    }

    #[test]
    fn smooth_bump() {
        // Bump curve: should be smooth and symmetric around the peak.
        let lut = CurveLut::from_points(&[
            [0.0, 0.0],
            [0.5, 1.0],
            [1.0, 0.0],
        ]);
        let v = lut.evaluate(0.5);
        assert!((v - 1.0).abs() < 0.05, "peak at 0.5: got {v}");
        let v_low = lut.evaluate(0.25);
        let v_high = lut.evaluate(0.75);
        assert!((v_low - v_high).abs() < 0.05, "symmetry: {v_low} vs {v_high}");
        // Should be smooth — intermediate values between 0 and 1.
        assert!(v_low > 0.3 && v_low < 0.8, "smooth rise at 0.25: got {v_low}");
    }

    #[test]
    fn many_points() {
        let lut = CurveLut::from_points(&[
            [0.0, 0.0],
            [0.1, 0.05],
            [0.2, 0.15],
            [0.3, 0.3],
            [0.5, 0.5],
            [0.7, 0.7],
            [0.8, 0.85],
            [0.9, 0.95],
            [1.0, 1.0],
        ]);
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let v = lut.evaluate(t);
            assert!((v - t).abs() < 0.15, "near-identity at {t}: got {v}");
        }
    }

    #[test]
    fn endpoint_independence() {
        // The key property of natural cubic splines: moving an endpoint
        // should NOT distort the curve between interior points.
        // Test: two curves with same interior point but different endpoints
        // should have similar values near the interior point.
        let lut_a = CurveLut::from_points(&[
            [0.0, 0.0],
            [0.5, 0.5],
            [1.0, 1.0],
        ]);
        let lut_b = CurveLut::from_points(&[
            [0.0, 0.3], // endpoint moved up
            [0.5, 0.5],
            [1.0, 0.7], // endpoint moved down
        ]);
        // At the interior point, both should be ~0.5.
        let va = lut_a.evaluate(0.5);
        let vb = lut_b.evaluate(0.5);
        assert!((va - 0.5).abs() < 0.01, "lut_a at 0.5: {va}");
        assert!((vb - 0.5).abs() < 0.01, "lut_b at 0.5: {vb}");
    }
}
