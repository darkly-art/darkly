//! Sampling-frame coordinate emitter shared by the spatial-sampling brush
//! nodes (`noise`, `image`).
//!
//! Both nodes sample a field (`fbm_tile` / `textureSample`) at a `vec2<f32>`
//! coordinate. Historically that coordinate was always `target_pos / scale`
//! — canvas-global pixels — so the pattern stayed pinned to the canvas and
//! the grain "swam" under a rotating stamp. This module folds a `space`
//! selector into the emitted coordinate so a node can instead sample in the
//! dab's own oriented frame, locking the grain to the stamp.
//!
//! The frame is chosen at compile time — the emitter produces only the
//! selected arm, never a runtime `switch`. The emitted WGSL references only
//! skeleton-provided locals (`target_pos`, `local_uv`, `d`) and the caller's
//! own `rotation`/`variation` input expressions, so it composes into both the
//! stroke and cursor-preview shader variants without extra bindings.

/// Coordinate frame a spatial node samples in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFrame {
    /// Canvas-global pixel space — grain pinned to the canvas; overlapping
    /// strokes share one coherent sheet. `rotation`/`variation` are ignored.
    Canvas,
    /// The dab's oriented unit frame — grain rotates and translates rigidly
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
/// `scale_with_brush` is meaningful only in [`SampleFrame::Dab`]: `true`
/// samples the radius-normalized unit-disc frame so the pattern scales with
/// the brush; `false` reconstructs pixel offsets so grain density stays
/// constant in canvas pixels as the brush grows. It is ignored for Canvas.
///
/// `scale_expr` is the caller's `scale` **input expression** — a `{:.6}`
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
    scale_with_brush: bool,
    rotation_expr: &str,
    variation_expr: &str,
    period: f32,
    ident: &str,
) -> (String, String) {
    match space {
        // Grain pinned to the canvas; rotation/variation are meaningless
        // without a brush frame, so they're dropped.
        SampleFrame::Canvas => (String::new(), format!("target_pos / ({scale_expr})")),
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
            let coord = if scale_with_brush {
                // Stamp-relative frequency: the unit-disc offset spans ~[-1,1]
                // across the stamp at any brush size, so the same pattern maps
                // across the whole stamp — the grain scales with the brush.
                format!("{ident}_dab_local / ({scale_expr}) + {offset}")
            } else {
                // Pixel-locked frequency: multiply the unit-disc offset back to
                // oriented dab-pixels so grain density stays constant in canvas
                // px as the brush grows (the stamp is a bigger window onto the
                // same grain).
                preamble.push_str(&format!(
                    "    let {ident}_radius_px = 1.0 / d.inv_radius_target_px;\n"
                ));
                format!("({ident}_dab_local * {ident}_radius_px) / ({scale_expr}) + {offset}")
            };
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
            true,
            "d.n1_rotation",
            "d.n2_variation",
            16.0,
            "noise_3",
        );
        assert!(pre.is_empty(), "Canvas emits no preamble");
        // The scale expression is interpolated parenthesized so a wired
        // expression composes; the literal value is unchanged.
        assert_eq!(coord, "target_pos / (32.000000)");
        // Rotation/variation are dropped in Canvas mode.
        assert!(!coord.contains("dab_local"));
        assert!(!coord.contains("rotation"));
    }

    #[test]
    fn dab_scale_with_brush_normalizes_unit_disc() {
        let (pre, coord) = frame_sample_coord_expr(
            SampleFrame::Dab,
            "8.000000",
            true,
            "1.5",
            "0.0",
            16.0,
            "noise_3",
        );
        // Oriented basis rotates local_uv by the rotation expression.
        assert!(pre.contains("cos(1.5)"));
        assert!(pre.contains("sin(1.5)"));
        assert!(pre.contains("noise_3_dab_local"));
        assert!(pre.contains("local_uv.x * noise_3_ca + local_uv.y * noise_3_sa"));
        // scale_with_brush=true divides the unit-disc offset directly and
        // never reconstructs pixels.
        assert!(coord.contains("noise_3_dab_local / (8.000000)"));
        assert!(!coord.contains("inv_radius_target_px"));
        assert!(!pre.contains("inv_radius_target_px"));
    }

    #[test]
    fn dab_pixel_locked_reconstructs_radius() {
        let (pre, coord) = frame_sample_coord_expr(
            SampleFrame::Dab,
            "8.000000",
            false,
            "0.0",
            "0.0",
            1.0,
            "img_5",
        );
        // scale_with_brush=false multiplies back to oriented dab-pixels.
        assert!(pre.contains("let img_5_radius_px = 1.0 / d.inv_radius_target_px;"));
        assert!(coord.contains("(img_5_dab_local * img_5_radius_px) / (8.000000)"));
    }

    #[test]
    fn dab_variation_offset_is_2d_and_period_bounded() {
        let (pre, coord) = frame_sample_coord_expr(
            SampleFrame::Dab,
            "8.000000",
            true,
            "0.0",
            "d.n2_variation",
            16.0,
            "noise_3",
        );
        // Defects 1+2: the offset is a 2D hash of `variation` bounded to the
        // caller's field period (16) — not the same scalar on both axes, and
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
