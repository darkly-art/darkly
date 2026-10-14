//! Global per-stroke colour jitter: what the colour selector's Jitter tab
//! controls.
//!
//! Three amplitudes (hue in degrees, saturation and value as normalized
//! deltas) live in the config store as `colors.jitter*` prefs, so they are
//! session state the frontend mirrors in like every other pref. A brush
//! stroke draws one uniform offset in `[-amp, amp]` per channel from its
//! seed and pushes the foreground colour through the same
//! [`crate::color::hsv_adjust_rgba`] the `hsv` brush node uses on the CPU
//! path. Applied once at stroke start, so the result is a per-stroke uniform
//! like the paint colour itself, and a pure function of the stroke seed: a
//! pinned seed replays the same colour.

use crate::brush::eval::prng_f32;
use crate::color::hsv_adjust_rgba;

pub const HUE_KEY: &str = "colors.jitterHue";
pub const SATURATION_KEY: &str = "colors.jitterSaturation";
pub const VALUE_KEY: &str = "colors.jitterValue";

/// Mixed into the stroke seed so the jitter draw is independent of the
/// per-node PRNG streams, which salt with a hash of the node id
/// (`EvalContext::prng_at`).
const SALT: u32 = 0x4A17_7E52;

/// Jitter amplitudes: each channel's offset is drawn uniformly from
/// `[-amp, amp]`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ColorJitter {
    /// Degrees.
    pub hue: f32,
    /// Normalized delta, `0..1`.
    pub saturation: f32,
    /// Normalized delta, `0..1`.
    pub value: f32,
}

impl ColorJitter {
    /// The amplitudes the artist set in the colour panel.
    pub fn from_config() -> Self {
        Self {
            hue: crate::config::get_f64(HUE_KEY) as f32,
            saturation: crate::config::get_f64(SATURATION_KEY) as f32,
            value: crate::config::get_f64(VALUE_KEY) as f32,
        }
    }

    pub fn is_zero(&self) -> bool {
        self.hue == 0.0 && self.saturation == 0.0 && self.value == 0.0
    }

    /// The stroke's colour: `rgba` (straight, not premultiplied) with one
    /// offset per channel drawn from `stroke_seed`. Zero amplitudes return
    /// the input untouched, bit for bit, without drawing.
    pub fn apply(&self, rgba: [f32; 4], stroke_seed: u32) -> [f32; 4] {
        if self.is_zero() {
            return rgba;
        }
        let seed = stroke_seed.wrapping_add(SALT.wrapping_mul(0x9E37_79B9));
        let draw = |index: u32, amp: f32| amp * (2.0 * prng_f32(seed, index) - 1.0);
        hsv_adjust_rgba(
            rgba,
            draw(0, self.hue),
            draw(1, self.saturation),
            draw(2, self.value),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::rgb_to_hsv;

    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

    #[test]
    fn zero_amplitudes_return_the_input_bit_identically() {
        let c = [0.3, 0.6, 0.9, 0.5];
        let out = ColorJitter::default().apply(c, 0x5eed);
        assert_eq!(out.map(f32::to_bits), c.map(f32::to_bits));
    }

    #[test]
    fn draws_stay_within_the_amplitude() {
        let jitter = ColorJitter {
            hue: 30.0,
            saturation: 0.25,
            value: 0.25,
        };
        for seed in 0..64 {
            let out = jitter.apply(RED, seed);
            let [h, s, v] = rgb_to_hsv([out[0], out[1], out[2]]);
            // Hue lands within 30 degrees either side of red (0 / 360).
            let hue_dist = h.min(360.0 - h);
            assert!(hue_dist <= 30.0 + 1e-3, "seed {seed}: hue {h}");
            // Saturation can only fade from 1 (`apply_sat` caps at 1), by at
            // most the `ds = -0.25` fade; value likewise.
            assert!((0.75 - 1e-3..=1.0).contains(&s), "seed {seed}: sat {s}");
            assert!((0.75 - 1e-3..=1.0).contains(&v), "seed {seed}: val {v}");
            assert_eq!(out[3], 1.0, "alpha untouched");
        }
    }

    #[test]
    fn same_seed_same_colour_different_seed_different_colour() {
        let jitter = ColorJitter {
            hue: 180.0,
            saturation: 0.0,
            value: 0.0,
        };
        assert_eq!(jitter.apply(RED, 7), jitter.apply(RED, 7));
        assert_ne!(jitter.apply(RED, 7), jitter.apply(RED, 8));
    }
}
