//! Static lookup table for known Krita paintop engine IDs.
//!
//! These are the `paintopid` values written to `<Preset paintopid="...">`.
//! Each comes from the corresponding `KisPaintOpFactory::id()` in
//! `krita/plugins/paintops/<engine>/`. The descriptions are short so the
//! inspector header can show "paintbrush — pixel brush (main workhorse)"
//! at a glance.

/// Look up a one-line description for a known paintop engine ID. Returns
/// `None` for unknown IDs — the inspector still shows the raw ID either way.
pub fn describe(paintop_id: &str) -> Option<&'static str> {
    Some(match paintop_id {
        "paintbrush" => "pixel brush — the main pixel-stamping engine",
        "colorsmudge" => "color smudge — picks up and blends canvas color",
        "duplicate" => "duplicate / clone stamp",
        "deformbrush" => "deform — pixel warp (grow, shrink, swirl, push)",
        "filter" => "filter brush — paints with a filter applied to the dab",
        "hairy" => "hairy / bristle — physics-driven multi-bristle strokes",
        "hatching" => "hatching — parallel-line shading",
        "particle" => "particle — physics trajectories with gravity / drag",
        "spray" => "spray — particle cloud per dab",
        "experiment" => "experimental — connect-the-dots vector painting",
        "tangentnormal" => "tangent normal — paints normal-map data",
        "gridbrush" => "grid — paints a grid of shapes",
        "curvebrush" => "curve — connects points along a stroke with curves",
        "dynabrush" => "dyna — physics-driven brush with mass / drag",
        "sketchbrush" => "sketch — connects nearby strokes with cobweb lines",
        "chalk" => "chalk — legacy chalky brush",
        "mypaintbrush" => "mypaint — the MyPaint brush engine",
        "roundmarker" => "round marker — fast circular stamp",
        "quickbrush" => "quick — minimal pixel brush for speed",
        "bristle" => "bristle — alias of hairy on some builds",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_paintbrush() {
        assert!(describe("paintbrush").unwrap().contains("pixel"));
    }

    #[test]
    fn returns_none_for_unknown() {
        assert!(describe("not-a-real-engine").is_none());
    }
}
