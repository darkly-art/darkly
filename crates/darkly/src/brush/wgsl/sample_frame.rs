//! Sampling-frame coordinate emitter shared by the spatial-sampling brush
//! nodes (`noise`, `image`).
//!
//! Both nodes sample a field (`fbm_tile` / `textureSample`) at a `vec2<f32>`
//! coordinate. Historically that coordinate was always `target_pos / scale`
//! (canvas-global pixels), so the pattern stayed pinned to the canvas and
//! the grain "swam" under a rotating stamp. This module folds a `space`
//! selector into the emitted coordinate so a node can instead sample in the
//! dab's own oriented frame, locking the grain to the stamp.
//!
//! The frame is chosen at compile time: the emitter produces only the
//! selected arm, never a runtime `switch`. The emitted WGSL references only
//! skeleton-provided locals (`target_pos`, `local_uv`, `d`), the intrinsic
//! uniforms (`u.intrinsic`, bound in both skeletons) and the caller's own
//! `rotation`/`variation` input expressions, so it composes into both the
//! stroke and cursor-preview shader variants without extra bindings.
//!
//! This is the one GPU-side reference-to-canvas boundary. A `scale` is an
//! authored reference-pixel feature size, so both arms divide by
//! `scale * u.intrinsic.dpi_factor`: the same document DPI that grows the
//! dab grows the grain on it, and a brush keeps its character on any
//! document. The factor cannot be folded into the baked `scale` literal,
//! because a compiled brush is cached across documents and rendered at the
//! reference for library previews.
//!
//! One caveat on "the graph works in reference pixels": authored lengths
//! (`UnitType::Pixels` ports, `DAB_REFERENCE_SIZE`) are reference pixels,
//! but sensed inputs (`pen_input.position`, `motion`, `distance`, `speed`,
//! `clone_source.position`) are plane pixels. A brush that wires a sensed
//! length into an authored-length port mixes the two frames; that is the
//! author's choice, not something this module can decide for them.

/// Coordinate frame a spatial node samples in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFrame {
    /// Canvas-global pixel space: grain pinned to the canvas; overlapping
    /// strokes share one coherent sheet. `rotation`/`variation` are ignored.
    Canvas,
    /// The dab's oriented unit frame: grain rotates and translates rigidly
    /// with each stamp.
    Dab,
}

impl SampleFrame {
    /// Map a `space` enum param index to a frame. Index order matches the
    /// `options` list on the node (`["Canvas", "Dab"]`).
    pub fn from_index(i: u32) -> Self {
        match i {
            1 => Self::Dab,
            _ => Self::Canvas,
        }
    }
}

/// Emit the `vec2<f32>` sample-coordinate expression for a spatial node.
///
/// Returns `(preamble, coord)`: `preamble` is zero or more `let` lines that
/// must be spliced into the fragment body *before* `coord` is used; `coord`
/// is the sample-coordinate expression itself. `ident` uniquifies the emitted
/// `let` names per node (pass `cctx.ident(..)`); `rotation_expr` and
/// `variation_expr` are the node's own input expressions (screen-relative
/// radians and a per-dab decorrelation scalar respectively).
///
/// In [`SampleFrame::Dab`] the field is sampled in oriented dab-pixels, so
/// `scale` is a reference-pixel feature size exactly as in Canvas space, and
/// grain density stays constant in pixels as the brush grows. To make the
/// grain scale *with* the brush instead, drive `scale` from
/// `brush_settings.size` (also reference pixels): the dab radius and the
/// scale then grow together and cancel, on any document, because both
/// cross the boundary through the same `dpi_factor`.
///
/// `scale_expr` is the caller's `scale` **input expression**: a `{:.6}`
/// literal when the scale input is unwired, or an upstream WGSL expression
/// when it's driven per-dab. It is interpolated parenthesized so a wired
/// expression composes correctly inside the divide.
///
/// `period` is the repeat period of the field the caller samples, in the same
/// units as `coord` (Dab space only). The per-dab decorrelation offset is a
/// 2D hash of `variation` scattered over `[0, period)²`, so it lands on a fresh
/// phase of the field per dab without resonating with its repeat. Callers pass
/// their field's period (e.g. the baked-tile span for `noise`, `1.0` for an
/// `fract`-wrapped texture). Ignored for Canvas.
pub fn frame_sample_coord_expr(
    space: SampleFrame,
    scale_expr: &str,
    rotation_expr: &str,
    variation_expr: &str,
    period: f32,
    ident: &str,
) -> (String, String) {
    match space {
        // Grain pinned to the canvas; rotation/variation are meaningless
        // without a brush frame, so they're dropped.
        // `target_pos` is canvas pixels and `scale_expr` is reference
        // pixels, so the factor converts the scale, not the position.
        SampleFrame::Canvas => (
            String::new(),
            format!("target_pos / (({scale_expr}) * u.intrinsic.dpi_factor)"),
        ),
        SampleFrame::Dab => {
            // Rotate the canvas-aligned unit-disc offset into the stamp's own
            // frame. `{rotation_expr}` is screen-relative radians, the same
            // convention as the skeleton's `theta`.
            let mut preamble = format!(
                "    let {ident}_ca = cos({rotation_expr});\n\
                 \x20   let {ident}_sa = sin({rotation_expr});\n\
                 \x20   let {ident}_dab_local = vec2<f32>(\n\
                 \x20       local_uv.x * {ident}_ca + local_uv.y * {ident}_sa,\n\
                 \x20      -local_uv.x * {ident}_sa + local_uv.y * {ident}_ca,\n\
                 \x20   );\n"
            );
            // Per-dab 2D decorrelation: hash `variation` into two independent
            // components (via `fbm_offset2`, from the always-prepended
            // `fbm2d.wgsl`) so overlapping dabs sample uncorrelated regions of
            // the periodic field, bounded to one `period` so it can't resonate
            // with the field's repeat. `max(.., 0.0)` keeps the `u32` cast
            // well-defined for any wired input.
            preamble.push_str(&format!(
                "    let {ident}_off = fbm_offset2(u32(max(({variation_expr}), 0.0) * 4096.0), {period:.6});\n"
            ));
            let offset = format!("{ident}_off");
            // Multiply the unit-disc offset back to oriented dab-pixels.
            // The radius is canvas pixels (it crossed the boundary in
            // `effective_radius`) and `scale` is reference pixels, so the
            // factor converts the scale to match and grain density stays
            // constant as the brush grows. Wiring `brush_settings.size`
            // (also reference px) into `scale` makes the radius and scale
            // cancel, so the grain scales with the brush.
            preamble.push_str(&format!(
                "    let {ident}_radius_px = 1.0 / d.inv_radius_target_px;\n"
            ));
            let coord = format!(
                "({ident}_dab_local * {ident}_radius_px) / (({scale_expr}) * u.intrinsic.dpi_factor) + {offset}"
            );
            (preamble, coord)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_interpolates_scale_expr_parenthesized() {
        let (pre, coord) = frame_sample_coord_expr(
            SampleFrame::Canvas,
            "32.000000",
            "d.n1_rotation",
            "d.n2_variation",
            16.0,
            "noise_3",
        );
        assert!(pre.is_empty(), "Canvas emits no preamble");
        // The scale expression is interpolated parenthesized so a wired
        // expression composes; the literal value is unchanged, and the DPI
        // factor multiplies the *scale*, never `target_pos` (which is
        // already canvas pixels).
        assert_eq!(coord, "target_pos / ((32.000000) * u.intrinsic.dpi_factor)");
        // Rotation/variation are dropped in Canvas mode.
        assert!(!coord.contains("dab_local"));
        assert!(!coord.contains("rotation"));
    }

    #[test]
    fn dab_rotates_local_uv_by_rotation_expr() {
        let (pre, _coord) =
            frame_sample_coord_expr(SampleFrame::Dab, "8.000000", "1.5", "0.0", 16.0, "noise_3");
        // Oriented basis rotates local_uv by the rotation expression.
        assert!(pre.contains("cos(1.5)"));
        assert!(pre.contains("sin(1.5)"));
        assert!(pre.contains("noise_3_dab_local"));
        assert!(pre.contains("local_uv.x * noise_3_ca + local_uv.y * noise_3_sa"));
    }

    #[test]
    fn dab_reconstructs_radius_so_scale_is_reference_pixels() {
        let (pre, coord) =
            frame_sample_coord_expr(SampleFrame::Dab, "8.000000", "0.0", "0.0", 1.0, "img_5");
        // Dab space always multiplies the unit-disc offset back to oriented
        // dab-pixels. The radius is canvas pixels (it crossed the boundary
        // in `effective_radius`), so the reference-pixel `scale` is
        // converted with the same factor to match it.
        assert!(pre.contains("let img_5_radius_px = 1.0 / d.inv_radius_target_px;"));
        assert!(coord.contains(
            "(img_5_dab_local * img_5_radius_px) / ((8.000000) * u.intrinsic.dpi_factor)"
        ));
        // The factor converts the scale, not the radius.
        assert!(!coord.contains("img_5_radius_px * u.intrinsic.dpi_factor"));
    }

    /// Both frames convert at the same place and by the same factor, so a
    /// brush that wires `brush_settings.size` (a reference-pixel length)
    /// into `scale` cancels on any document: the dab radius and the feature
    /// size cross the boundary together.
    #[test]
    fn both_frames_convert_the_scale_expression() {
        for (frame, ident) in [
            (SampleFrame::Canvas, "noise_1"),
            (SampleFrame::Dab, "noise_2"),
        ] {
            let (_, coord) = frame_sample_coord_expr(frame, "d.n7_size", "0.0", "0.0", 16.0, ident);
            assert!(
                coord.contains("(d.n7_size) * u.intrinsic.dpi_factor"),
                "{frame:?} must convert the wired scale expression, got {coord}"
            );
        }
    }

    #[test]
    fn dab_variation_offset_is_2d_and_period_bounded() {
        let (pre, coord) = frame_sample_coord_expr(
            SampleFrame::Dab,
            "8.000000",
            "0.0",
            "d.n2_variation",
            16.0,
            "noise_3",
        );
        // Defects 1+2: the offset is a 2D hash of `variation` bounded to the
        // caller's field period (16), not the same scalar on both axes, and
        // not a `* 64.0` stride that resonates with period 16. `fbm_offset2`
        // (fbm2d.wgsl) draws x and y from two different PCG inputs by
        // construction, so referencing it *is* the 2D guarantee.
        assert!(pre.contains(
            "let noise_3_off = fbm_offset2(u32(max((d.n2_variation), 0.0) * 4096.0), 16.000000)"
        ));
        assert!(coord.contains("+ noise_3_off"));
        assert!(!coord.contains("* 64.0"));
        // Old diagonal form must be gone.
        assert!(!coord.contains("d.n2_variation) * 64.0, (d.n2_variation)"));
    }

    #[test]
    fn from_index_maps_options() {
        assert_eq!(SampleFrame::from_index(0), SampleFrame::Canvas);
        assert_eq!(SampleFrame::from_index(1), SampleFrame::Dab);
        // Out-of-range falls back to Canvas (the safe default).
        assert_eq!(SampleFrame::from_index(7), SampleFrame::Canvas);
    }
}
