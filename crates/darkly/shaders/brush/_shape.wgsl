// Procedural `r(θ)` shape prelude — shared by compute-path brushes that
// stamp the same family of polar-radius silhouettes the `shape` fragment
// node produces.
//
// Functions here are parameterised on a `ShapeParams` struct so the same
// math can be reused by multiple terminals without a tight coupling to a
// shader-global `u`. The terminal that includes this prelude is
// responsible for building a `ShapeParams` for each dab (typically from
// its own dab record).
//
// **Bit-exact parity with `crates/darkly/src/brush/nodes/shape.rs`** is
// load-bearing — the CPU-side `integrate_centroid` walks the same `r(θ)`
// at a finer resolution, and per-pixel render tests catch drift as a
// mismatch. Keep the `hash1d` / `fbm_1d` / `r_sine` / `r_perlin` /
// `r_superformula` formulas here byte-equivalent to `shape.rs`.
//
// Include via `concat!()` at the consumer site:
//
//   concat!(
//       include_str!("../../../../../shaders/source_over.wgsl"),
//       "\n",
//       include_str!("../../../../../shaders/brush/_shape.wgsl"),
//       "\n",
//       include_str!("../../../../../shaders/brush/<terminal>.wgsl"),
//   )
//
// Scope: shape-radius only. The soft-disc coverage function (`r_solid` +
// linear falloff) is small enough that each terminal that needs it
// inlines its own copy. Coverage *math* is per-terminal; *radius* math is
// shared.
//
// Credits:
//   - Gielis superformula: Johan Gielis, AJB 90(3), 2003.
//   - 1D value-noise / fBm fundamentals: Ken Perlin; Inigo Quilez
//     (https://iquilezles.org/articles/morenoise/).

struct ShapeParams {
    /// 0 = sine harmonic, 1 = periodic 1D Perlin fBm, 2 = Gielis
    /// superformula. Matches `ALGO_*` constants in `nodes/shape.rs`.
    algorithm: u32,
    /// Modulation strength on top of the unit-radius reference disc.
    amplitude: f32,
    /// Sine / Perlin period; superformula `m` divided by 4.
    frequency: f32,
    /// Rotation (radians) applied to the shape around its centre.
    /// Subtracted from `θ` so positive values rotate clockwise in
    /// screen-y-down space, matching `pen.drawing_angle`'s
    /// `atan2(dy, dx)` convention — wiring drawing_angle into the
    /// shape node's rotation port orients the silhouette along the stroke.
    rotation: f32,
    /// Perlin fBm amplitude falloff per octave.
    persistence: f32,
    /// Per-dab Perlin seed (typically a random scalar).
    seed: f32,
    /// Perlin fBm octave count.
    octaves: u32,
    /// Superformula exponents.
    n1: f32,
    n2: f32,
    n3: f32,
    /// Anisotropy: squash the tip into an ellipse. `1.0` = round; `< 1.0`
    /// narrows the silhouette along its local x-axis and (area-preservingly)
    /// lengthens it along y. Applied after rotation so the ellipse co-rotates
    /// with the shape — the basis of the calligraphy nib.
    aspect: f32,
}

const SHAPE_TAU: f32 = 6.28318530717958647692;

/// Integer bit-mix hash — bit-identical to `hash1d` in `shape.rs`.
/// We avoid `fract(sin(x*K)*M)` because `sin` precision
/// differs between CPU and GPU and the `*43758` amplification turns
/// sub-ULP drift into a totally different noise array — which the
/// centroid alignment test would flag.
fn shape_hash1d(x: f32, seed: f32) -> f32 {
    let xi = u32(x);
    let si = u32(seed);
    var h = xi + si * 2654435761u;
    h = h ^ (h >> 16u);
    h = h * 0x85ebca6bu;
    h = h ^ (h >> 13u);
    h = h * 0xc2b2ae35u;
    h = h ^ (h >> 16u);
    return f32(h) / 4294967295.0;
}

/// Periodic 1D value-noise fBm — mirrors `fbm_1d` in `shape.rs`.
fn shape_fbm_1d(t: f32, p: ShapeParams) -> f32 {
    var sum: f32 = 0.0;
    var norm: f32 = 0.0;
    var amp: f32 = 1.0;
    let base_freq = max(i32(p.frequency), 1);
    for (var o: u32 = 0u; o < p.octaves; o = o + 1u) {
        let freq = base_freq << o;
        let freq_f = f32(freq);
        let x = t * freq_f;
        let i = floor(x);
        let f = x - i;
        let s = f * f * (3.0 - 2.0 * f);
        // rem_euclid for non-negative t: i is in [0, freq) already.
        let i_wrapped = i - floor(i / freq_f) * freq_f;
        let i_next = i_wrapped + 1.0 - select(0.0, freq_f, (i_wrapped + 1.0) >= freq_f);
        let a = shape_hash1d(i_wrapped, p.seed);
        let b = shape_hash1d(i_next, p.seed);
        sum = sum + amp * (a * (1.0 - s) + b * s);
        norm = norm + amp;
        amp = amp * p.persistence;
    }
    if (norm > 0.0) {
        return sum / norm;
    }
    return 0.5;
}

fn shape_r_sine(p: ShapeParams, theta: f32) -> f32 {
    return 1.0 + p.amplitude * sin(p.frequency * theta);
}

fn shape_r_perlin(p: ShapeParams, theta: f32) -> f32 {
    var t = theta / SHAPE_TAU;
    t = t - floor(t);
    // fbm in [0, 1] → remap to [-1, 1] so amplitude scales 1:1 with sine.
    return 1.0 + p.amplitude * (2.0 * shape_fbm_1d(t, p) - 1.0);
}

fn shape_r_superformula(p: ShapeParams, theta: f32) -> f32 {
    let m_quarter = p.frequency * theta * 0.25;
    let term_a = pow(abs(cos(m_quarter)), p.n2);
    let term_b = pow(abs(sin(m_quarter)), p.n3);
    let s = term_a + term_b;
    if (s <= 0.0) {
        return 0.0;
    }
    return pow(s, -1.0 / p.n1);
}

/// Regular convex N-gon polar radius. `cos(π/n)` is the apothem of a
/// unit-circumradius N-gon; folding `theta` into one sector `[−sector/2,
/// sector/2)` and dividing by `cos` of the folded angle gives the
/// perpendicular distance to that sector's edge. Peaks at the circumradius
/// (`= 1`, at vertices) and dips to the apothem between them.
fn shape_r_polygon(p: ShapeParams, theta: f32) -> f32 {
    // `frequency` is the shared "Points" knob; for a polygon it is the side
    // count, floored to a valid convex minimum of 3.
    let n = max(p.frequency, 3.0);
    let sector = SHAPE_TAU / n;
    // Fold to [-sector/2, sector/2): distance to the nearest sector centre.
    let a = theta - floor(theta / sector) * sector - 0.5 * sector;
    return cos(0.5 * sector) / max(cos(a), 1e-4);
}

/// Polar radius `r(θ)` in the shape's natural units (unmodulated disc has
/// `r = 1`). Branches on `p.algorithm`. Same dispatch table as
/// `shape.rs::r_theta`.
fn shape_r_theta(p: ShapeParams, theta: f32) -> f32 {
    let phased = theta - p.rotation;
    var r_base: f32;
    switch p.algorithm {
        case 1u: { r_base = shape_r_perlin(p, phased); }
        case 2u: { r_base = shape_r_superformula(p, phased); }
        case 3u: { r_base = shape_r_polygon(p, phased); }
        default: { r_base = shape_r_sine(p, phased); }
    }
    // Area-preserving elliptical squash about the (rotated) local axes.
    // Semi-axes a = aspect (x), b = 1/aspect (y); the polar boundary of that
    // ellipse is `1 / sqrt((cos/a)^2 + (sin/b)^2)`. `aspect = 1` ⇒ factor 1.
    let c = cos(phased);
    let s = sin(phased);
    let inv = sqrt((c / p.aspect) * (c / p.aspect) + (s * p.aspect) * (s * p.aspect));
    return r_base / inv;
}
