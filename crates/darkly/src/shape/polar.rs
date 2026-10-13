//! Polar-radius curves `r(θ)`: the CPU form of the silhouette family the brush
//! `circle` node draws in `shaders/brush/_shape.wgsl`.
//!
//! The formulas are the WGSL's, written in `f64` (kurbo's type) so the shape
//! families feed them straight into path construction. Parity with the shader
//! is formula-level: a brush tip and a placed shape with the same knobs are the
//! same curve, but nothing compares the two pixel for pixel.
//!
//! Credits:
//!   - Gielis superformula: Johan Gielis, AJB 90(3), 2003.
//!   - 1D value-noise / fBm fundamentals: Ken Perlin; Inigo Quilez
//!     (<https://iquilezles.org/articles/morenoise/>).
//!   - `hash1d` finaliser: MurmurHash3's `fmix32` constants, Austin Appleby
//!     (<https://github.com/aappleby/smhasher>).

use std::f64::consts::TAU;

use kurbo::Point;

/// Integer bit-mix hash of a cell index under a seed, in `[0, 1]`.
pub fn hash1d(x: u32, seed: u32) -> f64 {
    let mut h = x.wrapping_add(seed.wrapping_mul(2_654_435_761));
    h ^= h >> 16;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^= h >> 16;
    h as f64 / u32::MAX as f64
}

/// Periodic 1D value-noise fBm over `t ∈ [0, 1)`, in `[0, 1]`. Octave `o` has
/// `frequency << o` cells, so every octave wraps at `t = 1` and the curve
/// closes. Smoothstep blend between cells, normalised by the amplitude sum.
pub fn fbm_1d(t: f64, frequency: u32, persistence: f64, seed: u32, octaves: u32) -> f64 {
    let base = frequency.max(1);
    let (mut sum, mut norm, mut amp) = (0.0, 0.0, 1.0);
    for o in 0..octaves {
        let freq = base.saturating_mul(1 << o.min(16));
        let x = t * freq as f64;
        let i = x.floor();
        let f = x - i;
        let s = f * f * (3.0 - 2.0 * f);
        let i0 = (i as i64).rem_euclid(freq as i64) as u32;
        let i1 = (i0 + 1) % freq;
        sum += amp * (hash1d(i0, seed) * (1.0 - s) + hash1d(i1, seed) * s);
        norm += amp;
        amp *= persistence;
    }
    if norm > 0.0 {
        sum / norm
    } else {
        0.5
    }
}

/// Sine harmonic: `1 + A·sin(f·θ)`.
pub fn r_sine(theta: f64, amplitude: f64, frequency: f64) -> f64 {
    1.0 + amplitude * (frequency * theta).sin()
}

/// Periodic Perlin fBm: `1 + A·(2·fbm − 1)`, so the amplitude swings the
/// radius by the same `±A` the sine does.
pub fn r_perlin(
    theta: f64,
    amplitude: f64,
    frequency: u32,
    persistence: f64,
    seed: u32,
    octaves: u32,
) -> f64 {
    let t = (theta / TAU).rem_euclid(1.0);
    1.0 + amplitude * (2.0 * fbm_1d(t, frequency, persistence, seed, octaves) - 1.0)
}

/// Gielis superformula with `a = b = 1` and symmetry order `m`.
pub fn r_superformula(theta: f64, m: f64, n1: f64, n2: f64, n3: f64) -> f64 {
    let m_quarter = m * theta * 0.25;
    let sum = m_quarter.cos().abs().powf(n2) + m_quarter.sin().abs().powf(n3);
    if sum <= 0.0 {
        return 0.0;
    }
    sum.powf(-1.0 / n1)
}

/// Sample `r` at `n` evenly spaced angles from `θ = 0`, clamping the radius to
/// a finite non-negative value. `n` is rounded up to a multiple of 4 so the
/// axis extremes of a plain ellipse land on samples exactly.
pub fn sample_polar(n: usize, r: impl Fn(f64) -> f64) -> Vec<Point> {
    let n = n.max(4).div_ceil(4) * 4;
    (0..n)
        .map(|i| {
            let theta = TAU * i as f64 / n as f64;
            let rho = r(theta);
            let rho = if rho.is_finite() { rho.max(0.0) } else { 0.0 };
            Point::new(rho * theta.cos(), rho * theta.sin())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash1d_is_unit_deterministic_and_seed_sensitive() {
        for x in 0..64 {
            let h = hash1d(x, 7);
            assert!((0.0..=1.0).contains(&h));
            assert_eq!(h, hash1d(x, 7));
        }
        assert_ne!(hash1d(3, 0), hash1d(3, 1));
        // Pinned against the WGSL formula: 0 under seed 0 mixes to 0.
        assert_eq!(hash1d(0, 0), 0.0);
    }

    #[test]
    fn sine_at_zero_amplitude_is_the_unit_circle() {
        for i in 0..32 {
            let theta = TAU * i as f64 / 32.0;
            assert_eq!(r_sine(theta, 0.0, 6.0), 1.0);
        }
    }

    #[test]
    fn perlin_stays_within_the_amplitude_band() {
        for i in 0..512 {
            let theta = TAU * i as f64 / 512.0;
            let r = r_perlin(theta, 0.3, 3, 0.5, 11, 4);
            assert!((0.7 - 1e-9..=1.3 + 1e-9).contains(&r), "r = {r}");
        }
    }

    #[test]
    fn perlin_closes_at_a_full_turn() {
        let a = r_perlin(0.0, 0.4, 3, 0.6, 5, 3);
        let b = r_perlin(TAU - 1e-9, 0.4, 3, 0.6, 5, 3);
        assert!((a - b).abs() < 1e-6);
    }

    #[test]
    fn superformula_at_zero_symmetry_is_a_circle() {
        for i in 0..16 {
            let theta = TAU * i as f64 / 16.0;
            assert_eq!(r_superformula(theta, 0.0, 0.3, 4.0, 9.0), 1.0);
        }
    }

    #[test]
    fn sample_count_rounds_up_to_a_multiple_of_four() {
        assert_eq!(sample_polar(257, |_| 1.0).len(), 260);
    }
}
