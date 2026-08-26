//! The fixed image every documentation asset is rendered against.
//!
//! The editor's own pickers sample whatever is on the user's canvas, which is
//! exactly the right answer there and exactly the wrong one for documentation:
//! two assets are only comparable if they depict the same thing.
//!
//! **The subject is a photograph, not a generated field.** It was generated
//! once — a hue sweep crossed with a tonal ramp, under a disc and three
//! rectangles — chosen so that every effect had something to bite on: the whole
//! hue wheel for the colour effects, black through white for the tone controls,
//! hard edges for blur, pixelate, painting and aberration. It covered all of
//! that and still read as a test card rather than as a picture, which is the one
//! thing a preview cannot afford: a reader decides whether an effect is worth
//! trying by looking at 72 px of it in a table.
//!
//! The photograph is a strong silhouette against a bright caustic field, so it
//! keeps what the generated field was built for — near-black to near-white in
//! one frame, hard edges where the body meets the light, fine detail in the
//! water for the resampling effects — and reads as an image at any size. What it
//! gives up is hue coverage: it is violet and magenta almost throughout, so a
//! hue rotation moves the whole frame together instead of shearing a rainbow
//! apart. That is a fair trade for a preview whose job is to be recognised, and
//! the shipped file is where to start if it ever stops being one.
//!
//! Both this and [`blend_source_rgba`] are pure functions of `dim` — no RNG, no
//! clock, no I/O beyond a `include_bytes!` decode — because every asset in the
//! artifact has to be reproducible from the same commit.

use std::sync::OnceLock;

use crate::gpu::preview::{field_rgba, PREVIEW_MAX_DIM};

/// Edge length of every rendered documentation frame.
///
/// This is [`PREVIEW_MAX_DIM`] rather than a number of its own: the offscreen
/// veil and void renderers are hard-wired to fit their output into that box, so
/// matching it is what makes every asset the same size regardless of which
/// mechanism produced it.
pub const DOCS_SUBJECT_DIM: u32 = PREVIEW_MAX_DIM;

/// The subject: project artwork, cropped square and stored at
/// [`SUBJECT_SRC_DIM`].
const SUBJECT_JPEG: &[u8] = include_bytes!("../../resources/docs/subject.jpg");

/// Edge length of the stored file: [`DOCS_SUBJECT_DIM`], the size every asset is
/// rendered at, and no larger.
///
/// The veil and void paths ask for `DOCS_SUBJECT_DIM * SUBJECT_SCALE` — twice
/// this — so their target's own resample lands at exactly 2:1 rather than
/// softening the image through a 1:1 pass. **Storing the doubled size buys
/// nothing.** [`resample`] serves a request above the stored size by exact pixel
/// replication, and a 2:1 area average of a pixel-replicated image is the image
/// again, so the target sees these very pixels either way. The only difference a
/// 512 px file would make is which resampler downsized the original artwork —
/// this module's box filter instead of the one that produced the file — and the
/// file wins that comparison.
const SUBJECT_SRC_DIM: u32 = DOCS_SUBJECT_DIM;

/// The decoded subject, RGBA8 at [`SUBJECT_SRC_DIM`], decoded once per process.
///
/// A JPEG decode is cheap but a documentation run asks for the subject once per
/// entry across five catalogs, and the blend renderer asks again for every
/// frame's document rebuild.
fn source() -> &'static [u8] {
    static SOURCE: OnceLock<Vec<u8>> = OnceLock::new();
    SOURCE.get_or_init(|| {
        let img = image::load_from_memory(SUBJECT_JPEG)
            .expect("the subject ships with the crate and is decodable")
            .to_rgba8();
        assert_eq!(
            (img.width(), img.height()),
            (SUBJECT_SRC_DIM, SUBJECT_SRC_DIM),
            "resources/docs/subject.jpg is not {SUBJECT_SRC_DIM} square",
        );
        img.into_raw()
    })
}

/// The stored subject, resampled to `dim × dim`.
///
/// Every output pixel is the integral of the source over exactly its own box in
/// normalized coordinates — fractional weights at the edges, not a nearest tap
/// and not a fixed integer ratio. That is what preserves the property the whole
/// offscreen path is built on: because the source is piecewise constant, the
/// integral over one box equals the sum of the integrals over its four
/// quadrants, so a `2 · dim` render is an exact supersample of the `dim` one for
/// *any* `dim`, whether it divides the stored size or not.
///
/// Above the stored size the same rule degenerates to exact pixel replication —
/// an output box that falls inside one source pixel integrates to that pixel —
/// which is what makes [`SUBJECT_SRC_DIM`] the render size rather than twice it.
///
/// Averaging happens in the stored (sRGB) values rather than in light, which is
/// what an image resize conventionally does and what keeps the two renders
/// consistent with each other — the only thing being pinned here.
fn resample(dim: u32) -> Vec<u8> {
    let src = source();
    let src_dim = SUBJECT_SRC_DIM as f64;
    let scale = src_dim / f64::from(dim);
    let mut out = Vec::with_capacity((dim * dim * 4) as usize);

    // Coverage of source column/row `i` by the output box `[lo, hi)`.
    let span = |lo: f64, hi: f64, i: u32| {
        let (a, b) = (f64::from(i), f64::from(i) + 1.0);
        (hi.min(b) - lo.max(a)).max(0.0)
    };

    for y in 0..dim {
        let (y0, y1) = (f64::from(y) * scale, f64::from(y + 1) * scale);
        let rows = (y0.floor() as u32)..(y1.ceil() as u32).min(SUBJECT_SRC_DIM);
        for x in 0..dim {
            let (x0, x1) = (f64::from(x) * scale, f64::from(x + 1) * scale);
            let cols = (x0.floor() as u32)..(x1.ceil() as u32).min(SUBJECT_SRC_DIM);

            let mut acc = [0.0f64; 3];
            let mut weight = 0.0f64;
            for sy in rows.clone() {
                let wy = span(y0, y1, sy);
                for sx in cols.clone() {
                    let w = wy * span(x0, x1, sx);
                    if w == 0.0 {
                        continue;
                    }
                    let px = ((sy * SUBJECT_SRC_DIM + sx) * 4) as usize;
                    for (c, a) in acc.iter_mut().enumerate() {
                        *a += w * f64::from(src[px + c]);
                    }
                    weight += w;
                }
            }

            for a in acc {
                out.push((a / weight).round() as u8);
            }
            // Opacity is not a variable here: `test_readback_canvas` reads the
            // composite cache, where premultiplied and straight alpha coincide
            // only for opaque content.
            out.push(255);
        }
    }
    out
}

/// The documentation subject at `dim × dim`, RGBA8 and fully opaque.
pub fn subject_rgba(dim: u32) -> Vec<u8> {
    resample(dim)
}

/// The upper layer of a blend-mode preview at `dim × dim`, RGBA8 and fully
/// opaque.
///
/// Still generated, and deliberately not a second photograph. A blend mode is
/// read by seeing what one layer's colour does to another's, so the top layer
/// wants to be the simplest thing that exercises the formula: a diagonal ramp
/// between two non-symmetric mid-tones, held away from the 0 and 1 boundaries so
/// every mode is tested on its interior rather than on an edge case — the same
/// reasoning behind the fixed colour pair in `tests/blend_modes.rs`. Two busy
/// images blended together read as neither.
pub fn blend_source_rgba(dim: u32) -> Vec<u8> {
    const NEAR: [f32; 3] = [0.85, 0.34, 0.14];
    const FAR: [f32; 3] = [0.18, 0.44, 0.80];
    field_rgba(dim, dim, |u, v| {
        let d = (u + v) * 0.5;
        let c: [f32; 3] = std::array::from_fn(|i| NEAR[i] + (FAR[i] - NEAR[i]) * d);
        [c[0], c[1], c[2], 1.0]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both fields are pure functions of `dim` — no RNG, no clock, no ambient
    /// state. Either one leaking in would make every asset unreproducible and
    /// every frame-to-frame comparison meaningless.
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
                buf.as_chunks::<4>().0.iter().all(|p| p[3] == 255),
                "a pixel is not opaque"
            );
        }
    }

    /// The subject at its stored size is the stored pixels, untouched.
    ///
    /// The identity case of [`resample`]: at `dim == SUBJECT_SRC_DIM` every box
    /// is exactly one source pixel, so any weighting bug that still averaged to
    /// something plausible at other sizes shows up here as a changed pixel.
    #[test]
    fn subject_at_source_size_is_the_stored_image() {
        let out = subject_rgba(SUBJECT_SRC_DIM);
        let src_pixels = source();
        for (i, (got, src)) in out
            .as_chunks::<4>()
            .0
            .iter()
            .zip(src_pixels.as_chunks::<4>().0)
            .enumerate()
        {
            assert_eq!(&got[..3], &src[..3], "pixel {i} was resampled at 1:1",);
        }
    }

    /// Asking above the stored size replicates pixels rather than inventing
    /// them, in whole 2 × 2 blocks.
    ///
    /// This is why the file is stored at the render size and not at twice it.
    /// The veil and void paths request `2 ·` [`DOCS_SUBJECT_DIM`] so their
    /// target's resample is a clean 2:1 area average — and averaging a
    /// replicated block returns the pixel it came from, so that target sees the
    /// stored image exactly, unsoftened, without a 512 px file existing. If this
    /// ever stops holding, the doubled request stops being free and the stored
    /// size has to grow with it.
    #[test]
    fn asking_above_the_stored_size_replicates_whole_pixels() {
        let dim = SUBJECT_SRC_DIM;
        let two = subject_rgba(dim * 2);
        let src = source();
        for y in 0..dim {
            for x in 0..dim {
                let want = &src[((y * dim + x) * 4) as usize..][..3];
                for i in 0..4u32 {
                    let (bx, by) = (x * 2 + (i & 1), y * 2 + (i >> 1));
                    let got = &two[((by * dim * 2 + bx) * 4) as usize..][..3];
                    assert_eq!(got, want, "the 2× render blurred source pixel ({x},{y})");
                }
            }
        }
    }

    /// Each 2 × 2 block of the doubled render averages to the corresponding
    /// pixel of the single render.
    ///
    /// This pins the normalized-coordinate property the veil path relies on: the
    /// veil preview renderer always resamples its source, and it is fed the
    /// subject at 2× precisely so that resample is an exact box average of the
    /// *same* image. If the resampler ever worked in integer pixel steps the two
    /// renders would drift apart by a fraction of a pixel and the veil assets
    /// would silently start depicting a slightly different picture.
    ///
    /// Every pixel is compared, with no exception for edges: the generated
    /// subject needed one where a shape boundary made a point sample and an area
    /// average legitimately differ, and a resampler that integrates has no such
    /// place. The tolerance is the rounding of five independent u8 quantizations.
    ///
    /// Asked at [`DOCS_SUBJECT_DIM`], which is the stored size, so today it also
    /// exercises the replication path — and is meant to keep holding if the
    /// stored file ever grows past it.
    #[test]
    fn subject_at_2x_area_averages_to_the_1x_render() {
        let dim = DOCS_SUBJECT_DIM;
        let one = subject_rgba(dim);
        let two = subject_rgba(dim * 2);
        assert_eq!(two.len(), one.len() * 4);

        let at = |buf: &[u8], d: u32, x: u32, y: u32, c: usize| {
            i32::from(buf[((y * d + x) * 4) as usize + c])
        };

        for y in 0..dim {
            for x in 0..dim {
                for c in 0..3 {
                    let block: i32 = (0..4)
                        .map(|i| at(&two, dim * 2, x * 2 + (i & 1), y * 2 + (i >> 1), c))
                        .sum();
                    let avg = (block as f32 / 4.0).round() as i32;
                    let point = at(&one, dim, x, y, c);
                    assert!(
                        (avg - point).abs() <= 1,
                        "at ({x},{y}) channel {c}: 2× block averages {avg}, 1× renders {point}"
                    );
                }
            }
        }
    }

    /// The subject is worth rendering against: it spans the tonal range the
    /// tone controls are read by, and it is not one flat colour.
    ///
    /// A guard on the file rather than on the code. Swapping in an image that is
    /// all midtones would not fail anything else here — every asset would render,
    /// and the levels, curves and brightness previews would quietly stop showing
    /// what they do.
    #[test]
    fn subject_spans_the_tonal_range() {
        let buf = subject_rgba(DOCS_SUBJECT_DIM);
        let luma = |p: &[u8]| {
            (0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2])) as u32
        };
        let (mut lo, mut hi) = (255u32, 0u32);
        for px in buf.as_chunks::<4>().0 {
            let l = luma(px);
            lo = lo.min(l);
            hi = hi.max(l);
        }
        assert!(lo < 32, "the subject has no shadows: darkest luma is {lo}");
        assert!(
            hi > 223,
            "the subject has no highlights: brightest luma is {hi}"
        );
    }
}
