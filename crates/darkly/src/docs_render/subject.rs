//! The fixed image every documentation asset is rendered against.
//!
//! The editor's own pickers sample whatever is on the user's canvas, which is
//! exactly the right answer there and exactly the wrong one for documentation:
//! two assets are only comparable if they depict the same thing. So the subject
//! is generated rather than shipped — no binary in the repository, no licensing
//! question on a published crate, and no screenshot step that cannot run
//! headlessly.
//!
//! Both fields are described in normalized coordinates and sampled at pixel
//! centres, so each one is a single continuous image evaluated at whatever
//! resolution is asked for. That is what makes a `2 · dim` render a genuine
//! supersample of the `dim` one rather than a different picture.

use crate::gpu::preview::{field_rgba, PREVIEW_MAX_DIM};

/// Edge length of every rendered documentation frame.
///
/// This is [`PREVIEW_MAX_DIM`] rather than a number of its own: the offscreen
/// veil and void renderers are hard-wired to fit their output into that box, so
/// matching it is what makes every asset the same size regardless of which
/// mechanism produced it.
pub const DOCS_SUBJECT_DIM: u32 = PREVIEW_MAX_DIM;

/// A solid shape laid over the smooth field, in normalized coordinates.
enum Shape {
    /// `[x0, y0, x1, y1]`, half-open.
    Rect([f32; 4], [f32; 3]),
    /// Centre and radius.
    Disc([f32; 2], f32, [f32; 3]),
}

impl Shape {
    fn covers(&self, u: f32, v: f32) -> Option<[f32; 3]> {
        match self {
            Shape::Rect([x0, y0, x1, y1], c) => {
                (u >= *x0 && u < *x1 && v >= *y0 && v < *y1).then_some(*c)
            }
            Shape::Disc([cx, cy], r, c) => {
                let (dx, dy) = (u - cx, v - cy);
                (dx * dx + dy * dy < r * r).then_some(*c)
            }
        }
    }
}

/// Hard-edged solids over the smooth field: saturated primaries for the effects
/// that displace or resample colour channels, and a near-black / near-white pair
/// giving the tone controls something to clip against. Their edges are what the
/// blur, pixelate, painting and aberration previews are read by.
const SHAPES: &[Shape] = &[
    Shape::Disc([0.5, 0.30], 0.16, [0.16, 0.47, 0.92]),
    Shape::Rect([0.06, 0.62, 0.30, 0.86], [0.90, 0.12, 0.16]),
    Shape::Rect([0.36, 0.62, 0.60, 0.86], [0.03, 0.03, 0.04]),
    Shape::Rect([0.66, 0.62, 0.94, 0.86], [0.97, 0.97, 0.95]),
];

/// Fully-saturated colour at `hue` degrees, at value `v`.
fn hue_ramp(hue: f32, v: f32) -> [f32; 3] {
    let h = (hue / 60.0).rem_euclid(6.0);
    let f = h - h.floor();
    let (p, q, t) = (0.0, 1.0 - f, f);
    let rgb = match h as u32 {
        0 => [1.0, t, p],
        1 => [q, 1.0, p],
        2 => [p, 1.0, t],
        3 => [p, q, 1.0],
        4 => [t, p, 1.0],
        _ => [1.0, p, q],
    };
    [rgb[0] * v, rgb[1] * v, rgb[2] * v]
}

/// The subject's fields are square and fully opaque; everything else about the
/// rasterization is [`field_rgba`]'s.
fn pack(dim: u32, field: impl Fn(f32, f32) -> [f32; 3]) -> Vec<u8> {
    field_rgba(dim, dim, |u, v| {
        let c = field(u, v);
        [c[0], c[1], c[2], 1.0]
    })
}

/// The documentation subject at `dim × dim`, RGBA8 and fully opaque.
///
/// A horizontal sweep through the hue wheel crossed with a vertical ramp from
/// black to full value — colour and the whole tonal range, which is what the
/// hue, desaturation, curves, levels and brightness previews are read against —
/// overlaid with [`SHAPES`] for the effects whose subject is an edge.
///
/// Opacity is not a variable here: `test_readback_canvas` reads the composite
/// cache, where premultiplied and straight alpha coincide only for opaque
/// content.
pub fn subject_rgba(dim: u32) -> Vec<u8> {
    pack(dim, |u, v| {
        SHAPES
            .iter()
            .find_map(|s| s.covers(u, v))
            .unwrap_or_else(|| hue_ramp(u * 360.0, v))
    })
}

/// The upper layer of a blend-mode preview at `dim × dim`, RGBA8 and fully
/// opaque.
///
/// A diagonal ramp between two non-symmetric mid-tones, held away from the 0
/// and 1 boundaries so every mode's formula is exercised on its interior rather
/// than on an edge case — the same reasoning behind the fixed colour pair in
/// `tests/blend_modes.rs`. Its axis runs across the subject's, so no two modes
/// collapse onto the same output.
pub fn blend_source_rgba(dim: u32) -> Vec<u8> {
    const NEAR: [f32; 3] = [0.85, 0.34, 0.14];
    const FAR: [f32; 3] = [0.18, 0.44, 0.80];
    pack(dim, |u, v| {
        let d = (u + v) * 0.5;
        std::array::from_fn(|i| NEAR[i] + (FAR[i] - NEAR[i]) * d)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::preview::pixel_centre;

    /// Both fields are pure functions of position — no RNG, no clock, no I/O.
    /// A seed or a timestamp leaking in would make every asset unreproducible
    /// and every frame-to-frame comparison meaningless.
    #[test]
    fn docs_subject_is_deterministic() {
        assert_eq!(subject_rgba(64), subject_rgba(64));
        assert_eq!(blend_source_rgba(64), blend_source_rgba(64));
    }

    /// Every pixel is opaque and both buffers cover the whole canvas — the
    /// precondition the composite readback depends on.
    #[test]
    fn subject_covers_the_canvas_opaquely() {
        for buf in [subject_rgba(64), blend_source_rgba(64)] {
            assert_eq!(buf.len(), 64 * 64 * 4);
            assert!(
                buf.chunks_exact(4).all(|p| p[3] == 255),
                "a pixel is not opaque"
            );
        }
    }

    /// Each 2 × 2 block of the doubled render averages to the corresponding
    /// pixel of the single render, wherever the field is smooth.
    ///
    /// This pins the normalized-coordinate property the veil path relies on: the
    /// veil preview renderer always resamples its source, and it is fed the
    /// subject at 2× precisely so that resample is an exact box average of the
    /// *same* field. If the generator ever sampled in integer pixel space the
    /// two renders would drift apart by a fraction of a pixel and the veil
    /// assets would silently start depicting a slightly different image.
    ///
    /// A shape boundary is the one place a point sample and an area average are
    /// *meant* to differ — one lands inside the solid, the other is part covered.
    /// Asked of the real [`SHAPES`] rather than a second copy of their geometry.
    fn straddles_a_shape(x: u32, y: u32, dim: u32) -> bool {
        let covered = |x: u32, y: u32, d: u32| {
            let (u, v) = pixel_centre(x, y, d, d);
            SHAPES.iter().position(|s| s.covers(u, v).is_some())
        };
        let here = covered(x, y, dim);
        (0..4).any(|i| covered(x * 2 + (i & 1), y * 2 + (i >> 1), dim * 2) != here)
    }

    #[test]
    fn subject_at_2x_area_averages_to_the_1x_field() {
        let dim = DOCS_SUBJECT_DIM;
        let one = subject_rgba(dim);
        let two = subject_rgba(dim * 2);
        assert_eq!(two.len(), one.len() * 4);

        let at = |buf: &[u8], d: u32, x: u32, y: u32, c: usize| {
            buf[((y * d + x) * 4) as usize + c] as i32
        };

        let mut compared = 0usize;
        for y in 0..dim {
            for x in 0..dim {
                if straddles_a_shape(x, y, dim) {
                    continue;
                }
                for c in 0..3 {
                    let block: i32 = (0..4)
                        .map(|i| at(&two, dim * 2, x * 2 + (i & 1), y * 2 + (i >> 1), c))
                        .sum();
                    let avg = (block as f32 / 4.0).round() as i32;
                    let point = at(&one, dim, x, y, c);
                    assert!(
                        (avg - point).abs() <= 1,
                        "at ({x},{y}) channel {c}: 2× block averages {avg}, 1× samples {point}"
                    );
                    compared += 1;
                }
            }
        }
        // The skip rule covers shape outlines, which are a thin minority.
        assert!(
            compared > (dim * dim * 3) as usize * 9 / 10,
            "only {compared} of {} samples were away from a shape edge",
            dim * dim * 3
        );
    }
}
