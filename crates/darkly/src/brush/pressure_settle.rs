//! Pressure settle: how a stroke's ends take their pressure.
//!
//! A pen's pressure rises from zero as it touches down and falls back to zero
//! as it lifts off. Drawn as read, those ramps thin both ends of a stroke, a
//! stabilizer smoothing pressure along the stroke carries them further in,
//! and a hard tap, which places a single dab where the pen landed, comes out
//! at the lightest reading of the tap. The readings within the settle time
//! `D` of touch-down and of lift-off are the pen landing and leaving, not the
//! artist's intent, so each end holds the strongest reading between it and
//! its boundary:
//!
//! - **Start boundary** `s`: the first sample at least `D` after the first.
//!   Each sample before it takes the strongest reading from itself up to and
//!   including `s`. Until `s` arrives the latest sample stands in for it, so
//!   the start is provisional while the stroke is younger than `D` and final
//!   from then on.
//! - **End boundary** `e`, once the pen is up: the last sample at least `D`
//!   before the stroke's latest time. Each sample after it takes the
//!   strongest reading from `e` up to and including itself.
//!
//! So sample `i` reads the strongest of `min(i, e) ..= max(i, s)`: a ramp-up
//! or a let-up disappears, a rise at the end and a fall at the start are kept,
//! the boundaries keep their own readings and so join the rest of the stroke
//! without a step, and a tap shorter than `D` takes its peak everywhere.
//!
//! Only pressure changes: positions, times and every other sensor are left as
//! drawn. Nothing detects a touch-down or a release; they are always the
//! stroke's first and last `D`. The stroke engine applies the rule to the raw
//! samples it feeds its stabilizer, so a stroke drawn live, redrawn after an
//! edit and replayed all settle the same way, and since positions never
//! depend on pressure, settling moves no vertex.

use super::paint_info::PaintInformation;

/// The pressure settle rule for one stroke: its settle time. See the module
/// docs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PressureSettle {
    secs: f32,
}

impl PressureSettle {
    /// Settle the first and last `secs` of a stroke.
    pub const fn new(secs: f32) -> Self {
        Self { secs }
    }

    /// Rewrite the pressure of `raws`, a stroke's samples in order: the start
    /// always, the end when `pen_up`.
    pub fn apply(&self, raws: &mut [PaintInformation], pen_up: bool) {
        let Some(last) = raws.len().checked_sub(1) else {
            return;
        };
        let s = self.start_boundary(raws).unwrap_or(last);
        for i in (0..s).rev() {
            raws[i].pressure = raws[i].pressure.max(raws[i + 1].pressure);
        }
        if pen_up {
            for i in self.end_boundary(raws) + 1..=last {
                raws[i].pressure = raws[i].pressure.max(raws[i - 1].pressure);
            }
        }
    }

    /// Whether the start of `raws` is final: a sample at least the settle
    /// time after the first has arrived. Scans only the first settle time.
    pub fn start_settled(&self, raws: &[PaintInformation]) -> bool {
        self.start_boundary(raws).is_some()
    }

    /// The first sample of `raws` whose settled pressure a later sample or a
    /// pen-up can still change: 0 while the start is open, then the first
    /// sample after where a pen-up now would put the end boundary. Never
    /// decreases as samples are appended.
    pub fn final_before(&self, raws: &[PaintInformation]) -> usize {
        if raws.is_empty() || !self.start_settled(raws) {
            return 0;
        }
        self.end_boundary(raws) + 1
    }

    /// The first sample at least the settle time after the first, if one has
    /// arrived.
    fn start_boundary(&self, raws: &[PaintInformation]) -> Option<usize> {
        let first = raws.first()?.time;
        raws.iter().position(|r| r.time >= first + self.secs)
    }

    /// The last sample at least the settle time before the latest time any
    /// sample of `raws` carries, or 0 if none is. Measured from the latest
    /// time rather than the last sample's, so a timestamp that runs backwards
    /// cannot move the boundary back.
    fn end_boundary(&self, raws: &[PaintInformation]) -> usize {
        let latest = raws.iter().fold(f32::MIN, |t, r| t.max(r.time));
        raws.iter()
            .rposition(|r| r.time <= latest - self.secs)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 240.0;
    const SETTLE: PressureSettle = PressureSettle { secs: 0.05 };

    /// A sample at `x` along a line, `t` seconds in, reading `pressure`.
    fn sample(x: f32, pressure: f32, t: f32) -> PaintInformation {
        PaintInformation {
            pos: [x, 0.0],
            pressure,
            time: t,
            x_tilt: 0.3,
            rotation: 0.7,
            ..Default::default()
        }
    }

    /// `n` samples with a wavering pressure, unevenly timed with pauses.
    fn wavering(n: usize) -> Vec<PaintInformation> {
        let mut t = 0.0;
        (0..n)
            .map(|i| {
                t += match i % 9 {
                    0 if i > 0 => 4.0 * DT,
                    4 => 0.5 * DT,
                    _ => DT,
                };
                sample(i as f32 * 2.0, 0.5 + 0.4 * (i as f32 * 0.37).sin(), t)
            })
            .collect()
    }

    /// The rule read literally: each sample the strongest reading of
    /// `min(i, e) ..= max(i, s)`.
    fn brute_force(raws: &[PaintInformation], pen_up: bool) -> Vec<f32> {
        let last = raws.len() - 1;
        let first = raws[0].time;
        let latest = raws.iter().fold(f32::MIN, |t, r| t.max(r.time));
        let s = (0..=last)
            .find(|&i| raws[i].time >= first + SETTLE.secs)
            .unwrap_or(last);
        let e = if pen_up {
            (0..=last)
                .rev()
                .find(|&i| raws[i].time <= latest - SETTLE.secs)
                .unwrap_or(0)
        } else {
            last
        };
        (0..=last)
            .map(|i| {
                raws[i.min(e)..=i.max(s)]
                    .iter()
                    .fold(0.0f32, |q, r| q.max(r.pressure))
            })
            .collect()
    }

    fn settled(raws: &[PaintInformation], pen_up: bool) -> Vec<PaintInformation> {
        let mut out = raws.to_vec();
        SETTLE.apply(&mut out, pen_up);
        out
    }

    #[test]
    fn each_sample_takes_the_strongest_reading_to_its_boundary() {
        for n in [1, 2, 5, 12, 13, 20, 30, 200] {
            let drawn = wavering(n);
            for pen_up in [false, true] {
                let after = settled(&drawn, pen_up);
                let expected = brute_force(&drawn, pen_up);
                for (i, (a, b)) in after.iter().zip(&drawn).enumerate() {
                    assert_eq!(a.pressure, expected[i], "{n} samples, pen up {pen_up}, {i}");
                    assert_eq!(
                        PaintInformation {
                            pressure: b.pressure,
                            ..*a
                        },
                        *b,
                        "only pressure changes"
                    );
                }
            }
        }
    }

    /// A start rising into its boundary ends exactly at the boundary's
    /// reading, which, like the end boundary's, is kept as drawn.
    #[test]
    fn the_boundaries_keep_their_readings() {
        let drawn: Vec<PaintInformation> = (0..100)
            .map(|i| sample(i as f32, (i as f32 / 20.0).min(1.0) * 0.5 + 0.1, i as f32 * DT))
            .collect();
        let s = SETTLE.start_boundary(&drawn).unwrap();
        let e = SETTLE.end_boundary(&drawn);
        let after = settled(&drawn, true);
        assert!(after[..s].iter().all(|r| r.pressure == drawn[s].pressure));
        assert_eq!(after[s].pressure, drawn[s].pressure);
        assert_eq!(after[e].pressure, drawn[e].pressure);
    }

    #[test]
    fn settling_twice_changes_nothing() {
        let drawn = wavering(200);
        for pen_up in [false, true] {
            let once = settled(&drawn, pen_up);
            assert_eq!(settled(&once, pen_up), once);
        }
    }

    #[test]
    fn a_tap_takes_its_peak() {
        let tap: Vec<PaintInformation> = [0.1, 0.3, 0.6, 0.85, 0.9, 0.8, 0.5, 0.25, 0.1]
            .into_iter()
            .enumerate()
            .map(|(i, q)| sample(0.0, q, i as f32 * 0.0045))
            .collect();
        assert!(settled(&tap, true).iter().all(|r| r.pressure == 0.9));
        // Pen down, each sample holds the strongest reading from itself to
        // the latest.
        let down = settled(&tap, false);
        assert_eq!(
            down.iter().map(|r| r.pressure).collect::<Vec<_>>(),
            [0.9, 0.9, 0.9, 0.9, 0.9, 0.8, 0.5, 0.25, 0.1]
        );
    }

    /// Appending samples one at a time with the pen down, the start's
    /// pressure only rises, and once the start is settled nothing changes.
    #[test]
    fn the_start_only_rises_then_settles() {
        let drawn = wavering(60);
        let mut previous: Vec<f32> = Vec::new();
        let mut was_settled = false;
        for len in 1..=drawn.len() {
            let now = settled(&drawn[..len], false);
            for (i, (a, b)) in now.iter().zip(&previous).enumerate() {
                if was_settled {
                    assert_eq!(a.pressure, *b, "{len} samples: sample {i} changed");
                } else {
                    assert!(a.pressure >= *b, "{len} samples: sample {i} fell");
                }
            }
            was_settled = SETTLE.start_settled(&drawn[..len]);
            previous = now.iter().map(|r| r.pressure).collect();
        }
        assert!(was_settled);
    }

    /// Appending samples one at a time, the first sample a later sample or a
    /// pen-up can change never decreases and never passes the first sample
    /// a pen-up then rewrites, even when a timestamp runs backwards.
    #[test]
    fn the_final_prefix_never_shrinks_and_never_passes_a_rewrite() {
        let mut drawn = wavering(300);
        drawn[150].time -= 3.0 * DT;
        let mut previous = 0;
        for len in 1..=drawn.len() {
            let raws = &drawn[..len];
            let from = SETTLE.final_before(raws);
            assert!(from >= previous, "{len} samples: {from} after {previous}");
            previous = from;
            let down = settled(raws, false);
            for later in [settled(raws, true), settled(&drawn[..(len + 5).min(300)], false)] {
                if let Some(first) = later.iter().zip(&down).position(|(a, b)| a != b) {
                    assert!(from <= first, "{len} samples: final before {from}, rewritten {first}");
                }
            }
        }
    }

    #[test]
    fn a_single_sample_is_unchanged() {
        let one = [sample(0.0, 0.2, 0.0)];
        assert_eq!(settled(&one, true), one);
    }
}
