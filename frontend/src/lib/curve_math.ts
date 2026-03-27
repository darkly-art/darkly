/**
 * Natural cubic spline evaluation.
 *
 * This is the JS-side mirror of `crates/darkly/src/brush/curve_math.rs`.
 * Used for real-time preview in the CurveEditor component — avoids
 * a WASM round-trip on every pointermove event.  The Rust version is
 * authoritative; this is purely for rendering.
 *
 * Algorithm matches Krita's `KisLegacyCubicSpline` in `kis_cubic_curve_spline.h`:
 * natural cubic spline with tridiagonal solver.  Natural boundary conditions
 * (zero second derivative at endpoints) make the curve approach endpoints
 * linearly — endpoints only set the y-intercept, not the interior shape.
 */

type Point = [number, number];

/** Evaluate the natural cubic spline at parameter `t` (0–1). */
export function evaluateCurve(points: Point[], t: number): number {
    if (points.length < 2) return t;
    t = Math.max(0, Math.min(1, t));

    if (points.length === 2) {
        const [x0, y0] = points[0];
        const [x1, y1] = points[1];
        const dx = Math.max(x1 - x0, 1e-6);
        const frac = Math.max(0, Math.min(1, (t - x0) / dx));
        return Math.max(0, Math.min(1, y0 + frac * (y1 - y0)));
    }

    const spline = buildNaturalCubicSpline(points);
    const v = evaluateSpline(spline, t);
    return Math.max(0, Math.min(1, v));
}

/** Generate `n` evenly-spaced sample points for rendering the curve path. */
export function sampleCurve(points: Point[], n: number): Point[] {
    if (points.length < 2) {
        const result: Point[] = [];
        for (let i = 0; i < n; i++) {
            const t = i / (n - 1);
            result.push([t, t]);
        }
        return result;
    }

    if (points.length === 2) {
        const result: Point[] = [];
        for (let i = 0; i < n; i++) {
            const t = i / (n - 1);
            result.push([t, evaluateCurve(points, t)]);
        }
        return result;
    }

    // Build spline once, sample many times.
    const spline = buildNaturalCubicSpline(points);
    const result: Point[] = [];
    for (let i = 0; i < n; i++) {
        const t = i / (n - 1);
        const v = Math.max(0, Math.min(1, evaluateSpline(spline, t)));
        result.push([t, v]);
    }
    return result;
}

// --- Internal: natural cubic spline ---

interface SplineCoeffs {
    a: number[];  // y-values at control points
    b: number[];  // first derivative coefficients per segment
    c: number[];  // second derivative values at control points
    d: number[];  // third derivative coefficients per segment
    h: number[];  // segment widths
    x: number[];  // x-values at control points
}

function buildNaturalCubicSpline(points: Point[]): SplineCoeffs {
    const intervals = points.length - 1;

    const x = points.map(p => p[0]);
    const a = points.map(p => p[1]);

    const h: number[] = [];
    for (let i = 0; i < intervals; i++) {
        h.push(Math.max(x[i + 1] - x[i], 1e-6));
    }

    // Solve tridiagonal system for second derivatives (c).
    // Natural boundary conditions: c[0] = 0, c[n] = 0.
    let c: number[];
    if (intervals > 1) {
        const inner = intervals - 1;
        const triB: number[] = [];
        const triF: number[] = [];

        for (let i = 0; i < inner; i++) {
            triB.push(2.0 * (h[i] + h[i + 1]));
            triF.push(6.0 * ((a[i + 2] - a[i + 1]) / h[i + 1] - (a[i + 1] - a[i]) / h[i]));
        }

        // Sub/super-diagonal: h[1], h[2], ..., h[inner-1]
        const triA: number[] = [];
        for (let i = 1; i < inner; i++) {
            triA.push(h[i]);
        }

        const innerC = tridiagonalSolve(triA, triB, triA, triF);

        c = [0, ...innerC, 0];
    } else {
        c = new Array(intervals + 1).fill(0);
    }

    // Compute d and b coefficients.
    const d: number[] = [];
    const b: number[] = [];
    for (let i = 0; i < intervals; i++) {
        d.push((c[i + 1] - c[i]) / h[i]);
        b.push(-0.5 * c[i] * h[i] - (1.0 / 6.0) * d[i] * h[i] * h[i]
            + (a[i + 1] - a[i]) / h[i]);
    }

    return { a, b, c, d, h, x };
}

function evaluateSpline(s: SplineCoeffs, t: number): number {
    t = Math.max(s.x[0], Math.min(s.x[s.x.length - 1], t));

    const intervals = s.h.length;
    let i = 0;
    while (i < intervals - 1 && t >= s.x[i + 1]) {
        i++;
    }

    const dx = t - s.x[i];
    return s.a[i] + s.b[i] * dx + 0.5 * s.c[i] * dx * dx + (1.0 / 6.0) * s.d[i] * dx * dx * dx;
}

/**
 * Solve a tridiagonal system using the Thomas algorithm.
 *
 * System: sub[i-1]*x[i-1] + diag[i]*x[i] + sup[i]*x[i+1] = rhs[i]
 */
function tridiagonalSolve(sub: number[], diag: number[], sup: number[], rhs: number[]): number[] {
    const n = diag.length;

    if (n === 1) {
        return [rhs[0] / diag[0]];
    }

    const alpha: number[] = new Array(n).fill(0);
    const beta: number[] = new Array(n).fill(0);

    alpha[1] = -sup[0] / diag[0];
    beta[1] = rhs[0] / diag[0];

    for (let i = 1; i < n - 1; i++) {
        const denom = sub[i - 1] * alpha[i] + diag[i];
        alpha[i + 1] = -sup[i] / denom;
        beta[i + 1] = (rhs[i] - sub[i - 1] * beta[i]) / denom;
    }

    const x: number[] = new Array(n).fill(0);
    x[n - 1] = (rhs[n - 1] - sub[n - 2] * beta[n - 1]) / (diag[n - 1] + sub[n - 2] * alpha[n - 1]);

    for (let i = n - 2; i >= 0; i--) {
        x[i] = alpha[i + 1] * x[i + 1] + beta[i + 1];
    }

    return x;
}
