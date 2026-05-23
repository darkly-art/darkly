// Procedural `r(θ)` shape prelude — shared by compute-path brushes that
// stamp the same family of polar-radius silhouettes the `circle` fragment
// node produces.
//
// Functions here are parameterised on a `ShapeParams` struct so the same
// math can be reused by multiple terminals without a tight coupling to a
// shader-global `u` (the way `shaders/brush/circle.wgsl` does it). The
// terminal that includes this prelude is responsible for building a
// `ShapeParams` for each dab (typically from its own dab record).
//
// **Bit-exact parity with `crates/darkly/src/brush/nodes/circle.rs`** is
// load-bearing — the CPU-side `integrate_centroid` walks the same `r(θ)`
// at a finer resolution, and the centroid alignment test in
// `tests/circle_node.rs` catches drift as a per-pixel mismatch. Keep the
// `hash1d` / `fbm_1d` / `r_sine` / `r_perlin` / `r_superformula` formulas
// here byte-equivalent to `circle.wgsl` and `circle.rs`.
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
// Credits — same as `circle.wgsl`:
//   - Gielis superformula: Johan Gielis, AJB 90(3), 2003.
//   - 1D value-noise / fBm fundamentals: Ken Perlin; Inigo Quilez
//     (https://iquilezles.org/articles/morenoise/).

struct ShapeParams {
    /// 0 = sine harmonic, 1 = periodic 1D Perlin fBm, 2 = Gielis
    /// superformula. Matches `ALGO_*` constants in `nodes/circle.rs`.
    algorithm: u32,
    /// Modulation strength on top of the unit-radius reference disc.
    amplitude: f32,
    /// Sine / Perlin period; superformula `m` divided by 4.
    frequency: f32,
    /// Phase offset added to `θ` before evaluating r(θ).
    phase: f32,
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
}

const SHAPE_TAU: f32 = 6.28318530717958647692;

/// Integer bit-mix hash — bit-identical to `hash1d` in `circle.rs` and
/// `circle.wgsl`. We avoid `fract(sin(x*K)*M)` because `sin` precision
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

/// Periodic 1D value-noise fBm — mirrors `fbm_1d` in `circle.rs`.
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

/// Polar radius `r(θ)` in the shape's natural units (unmodulated disc has
/// `r = 1`). Branches on `p.algorithm`. Same dispatch table as
/// `circle.rs::r_theta` and `circle.wgsl::r_theta`.
fn shape_r_theta(p: ShapeParams, theta: f32) -> f32 {
    let phased = theta + p.phase;
    switch p.algorithm {
        case 1u: { return shape_r_perlin(p, phased); }
        case 2u: { return shape_r_superformula(p, phased); }
        default: { return shape_r_sine(p, phased); }
    }
}
