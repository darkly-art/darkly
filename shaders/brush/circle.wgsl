// Procedural shape mask for the brush "Circle" node.
//
// Renders a closed `r(θ)` silhouette filling the viewport — a grayscale
// alpha mask with soft edges. Three algorithms branch on `u.algorithm`:
//
// - 0: Sine harmonic         r(θ) = 1 + A·sin(n·θ + φ)
// - 1: 1D Perlin / value-noise fBm   (periodic, `octaves` stacked)
// - 2: Gielis Superformula
//
// All three operate on the same SDF rasterization scaffold: distance from
// the centre vs `r(θ)` with a smoothstep softness band. Asymmetric shapes
// (sine n=1, low-m superformula, Perlin) have their geometric centroid
// translated to the texture centre via `(u.centroid_x, u.centroid_y)`,
// which `crates/darkly/src/brush/nodes/circle.rs` numerically integrates
// per-dab. The Rust `r_theta` function in that file mirrors the formulas
// below; the centroid_alignment test in `tests/circle_node.rs` verifies
// the two stay consistent.
//
// Credits:
// - Gielis superformula: Johan Gielis, "A generic geometric transformation
//   that unifies a wide range of natural and abstract shapes",
//   American Journal of Botany 90(3), 2003.
// - 1D value-noise / fBm fundamentals: Ken Perlin (procedural noise), Inigo
//   Quilez (https://iquilezles.org/articles/morenoise/, value-noise basics).
// - Centroid alignment is independent: see header of nodes/circle.rs.
//
// Note: softness here is implemented as a smoothstep band at the SDF edge
// rather than the Gaussian post-process the design doc describes. Cheap and
// visually equivalent for typical amplitudes.

struct CircleUniforms {
    softness: f32,
    algorithm: u32,
    amplitude: f32,
    frequency: f32,
    phase: f32,
    persistence: f32,
    seed: f32,
    octaves: u32,
    n1: f32,
    n2: f32,
    n3: f32,
    base_radius: f32,
    centroid_x: f32,
    centroid_y: f32,
    _pad0: f32,
    _pad1: f32,
}

@group(0) @binding(0) var<uniform> u: CircleUniforms;

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

@vertex fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    let unit = vec2f(f32((idx << 1u) & 2u), f32(idx & 2u));
    var out: VertexOutput;
    out.position = vec4f(unit * 2.0 - 1.0, 0.0, 1.0);
    out.uv = unit;
    return out;
}

const PI: f32 = 3.14159265358979323846;
const TAU: f32 = 6.28318530717958647692;

// Integer bit-mix hash — bit-identical to `hash1d` in nodes/circle.rs. We
// avoid `fract(sin(x*K)*M)` because `sin` precision differs between CPU and
// GPU and the *43758 amplification turns sub-ULP drift into a totally
// different noise array — which the centroid alignment test would flag.
fn hash1d(x: f32, seed: f32) -> f32 {
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

// Periodic 1D value-noise fBm — mirrors `fbm_1d` in nodes/circle.rs.
fn fbm_1d(t: f32) -> f32 {
    var sum: f32 = 0.0;
    var norm: f32 = 0.0;
    var amp: f32 = 1.0;
    let base_freq = max(i32(u.frequency), 1);
    for (var o: u32 = 0u; o < u.octaves; o = o + 1u) {
        let freq = base_freq << o;
        let freq_f = f32(freq);
        let x = t * freq_f;
        let i = floor(x);
        let f = x - i;
        let s = f * f * (3.0 - 2.0 * f);
        // rem_euclid for non-negative t: i in [0, freq) already.
        let i_wrapped = i - floor(i / freq_f) * freq_f;
        let i_next = i_wrapped + 1.0 - select(0.0, freq_f, (i_wrapped + 1.0) >= freq_f);
        let a = hash1d(i_wrapped, u.seed);
        let b = hash1d(i_next, u.seed);
        sum = sum + amp * (a * (1.0 - s) + b * s);
        norm = norm + amp;
        amp = amp * u.persistence;
    }
    if (norm > 0.0) {
        return sum / norm;
    }
    return 0.5;
}

fn r_sine(theta: f32) -> f32 {
    return 1.0 + u.amplitude * sin(u.frequency * theta);
}

fn r_perlin(theta: f32) -> f32 {
    var t = theta / TAU;
    t = t - floor(t);
    // fbm in [0, 1] → remap to [-1, 1] so amplitude scales 1:1 with sine.
    return 1.0 + u.amplitude * (2.0 * fbm_1d(t) - 1.0);
}

fn r_superformula(theta: f32) -> f32 {
    let m_quarter = u.frequency * theta * 0.25;
    let term_a = pow(abs(cos(m_quarter)), u.n2);
    let term_b = pow(abs(sin(m_quarter)), u.n3);
    let s = term_a + term_b;
    if (s <= 0.0) {
        return 0.0;
    }
    return pow(s, -1.0 / u.n1);
}

fn r_theta(theta: f32) -> f32 {
    let phased = theta + u.phase;
    switch u.algorithm {
        case 1u: { return r_perlin(phased); }
        case 2u: { return r_superformula(phased); }
        default: { return r_sine(phased); }
    }
}

@fragment fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    // Pole-relative coordinate in viewport-radius units, with the centroid
    // pinned to UV (0.5, 0.5).
    //
    //   pole_relative = (uv - 0.5) / base_radius + centroid
    //
    // dist == 1.0 corresponds to the unit-radius reference disc (where
    // r(θ) == 1.0 in the algorithm's natural units). Multiplying the SDF
    // distance by base_radius would cancel the division back out — we keep
    // the math in natural units instead and let r(θ) and dist share the
    // same scale.
    let centroid = vec2f(u.centroid_x, u.centroid_y);
    let pole_relative = (in.uv - vec2f(0.5)) / u.base_radius + centroid;
    let dist = length(pole_relative);
    let theta = atan2(pole_relative.y, pole_relative.x);

    let r = r_theta(theta);

    // Match the old shader's tiny outer margin so anti-aliased edges land
    // inside the viewport regardless of pixel size. Expressed as a fraction
    // of the base radius (since we're in natural units): `0.002 / base_radius`
    // would be hard to reason about, so use a fixed natural-unit value.
    let outer = r;
    let softness_band = max(u.softness, 0.004);
    let coverage = 1.0 - smoothstep(outer - softness_band, outer, dist);
    return vec4f(coverage, coverage, coverage, coverage);
}
