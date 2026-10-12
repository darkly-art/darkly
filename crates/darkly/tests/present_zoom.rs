//! Present-stage sampling across zoom levels: minification through the root
//! composite's mip chain and the `display.pixelFilter` magnification policy.
//!
//! Every readback goes through the production view transform
//! (`set_view_transform` then `test_readback_viewport`), so the pixels are
//! exactly what the surface would show.
//!
//! Run with: `cargo test -p darkly --test present_zoom --features testing -- --test-threads=1`

use darkly::coord::CanvasRect;
use darkly::engine::types::StrokeOp;
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::rescale::levels_for;
use darkly::gpu::test_utils::*;
use darkly::layer::LayerId;

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

/// Red channel of viewport pixel `(x, y)`. Every pattern here is grey, so one
/// channel is the value.
fn value_at(pixels: &[u8], w: u32, x: u32, y: u32) -> u8 {
    pixels[((y * w + x) * 4) as usize]
}

/// Paste an opaque full-canvas layer whose pixel `(x, y)` is white unless
/// `black(x, y)`.
fn plant(engine: &mut DarklyEngine, w: u32, h: u32, black: impl Fn(u32, u32) -> bool) -> LayerId {
    let mut rgba = vec![255u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            if black(x, y) {
                let i = ((y * w + x) * 4) as usize;
                rgba[i] = 0;
                rgba[i + 1] = 0;
                rgba[i + 2] = 0;
            }
        }
    }
    engine.paste_image(w, h, &rgba, 0, 0, None)
}

/// Centred view at `zoom` with optional rotation and horizontal pan, in a
/// `vw x vh` viewport.
fn view(engine: &mut DarklyEngine, zoom: f32, rotation: f32, pan_x: f32, vw: u32, vh: u32) {
    engine.set_view_transform(pan_x, 0.0, zoom, rotation, false, vw as f32, vh as f32, 1.0);
}

/// Linear interpolation of a 1-D row of texel values at texel-space
/// coordinate `t` (texel `i` is centred at `i + 0.5`), clamped to the edge
/// like a `ClampToEdge` sampler.
fn bilinear_1d(row: &[f32], t: f32) -> f32 {
    let s = t - 0.5;
    let i0 = s.floor();
    let frac = s - i0;
    let at = |i: f32| row[(i.max(0.0) as usize).min(row.len() - 1)];
    at(i0) * (1.0 - frac) + at(i0 + 1.0) * frac
}

/// Canvas x of the centre of viewport column `i` for a centred, unrotated,
/// unpanned view.
fn canvas_x(i: u32, vw: u32, canvas_w: u32, zoom: f32) -> f32 {
    canvas_w as f32 / 2.0 + (i as f32 + 0.5 - vw as f32 / 2.0) / zoom
}

/// REGRESSION: a sharp pattern minified past 2:1 must present as its box
/// average, not as whichever texels a lone bilinear tap happens to land on.
/// Canvas 256 wide with every eighth column black, viewed at 1/8: every 8x8
/// block averages to 7/8, so every presented pixel is 223. The single tap of
/// the unfixed present reads canvas `8i + 4` (two white texels) on every
/// pixel, so the whole viewport reads 255: the failure is deterministic, not
/// a phase accident.
fn assert_eighth_zoom_box_average(rotation: f32) {
    let (w, h) = (256u32, 256u32);
    let (vw, vh) = (32u32, 32u32);
    let mut engine = test_engine(w, h);
    plant(&mut engine, w, h, |x, _| x % 8 == 0);

    // The composite itself holds the pattern: a failure below is the present.
    let canvas = engine.test_readback_canvas();
    assert_eq!(value_at(&canvas, w, 8, 5), 0);
    assert_eq!(value_at(&canvas, w, 9, 5), 255);

    view(&mut engine, 0.125, rotation, 0.0, vw, vh);
    let px = engine.test_readback_viewport(vw, vh);
    let mut sum = 0u64;
    for y in 0..vh {
        for x in 0..vw {
            let v = value_at(&px, vw, x, y);
            sum += v as u64;
            assert!(
                (v as i32 - 223).abs() <= 2,
                "pixel ({x},{y}) = {v}, want 223 +- 2 (box average of a 1-in-8 black column pattern)"
            );
        }
    }
    // Each mip level is stored as 8-bit colour, and every halving of this
    // pattern lands on an exact half (127.5, 191.5, 223.5). How a store rounds
    // a tie is implementation-defined, so level 3 reads 223 on some adapters
    // and 224 on others (Mesa lavapipe). Both are within 1 of the true mean.
    let mean = sum as f32 / (vw * vh) as f32;
    let box_mean = 255.0 * 7.0 / 8.0;
    assert!(
        (mean - box_mean).abs() < 1.0,
        "viewport mean {mean}, want {box_mean} +- 1"
    );
}

#[test]
fn minified_sharp_pattern_presents_as_box_average() {
    assert_eighth_zoom_box_average(0.0);
}

/// The block mean is rotation-invariant, so a rotated view must agree; this
/// catches a wrong axis or sign in the level selection on the real path.
#[test]
fn minified_sharp_pattern_presents_as_box_average_when_rotated() {
    assert_eighth_zoom_box_average(std::f32::consts::FRAC_PI_2);
}

/// REGRESSION (non-zero canvas origin): the same box average after a crop to
/// a window anchored away from the plane origin. The window's accumulators
/// are reallocated by the crop; the period divides the origin, so blocks
/// still average to 223.
#[test]
fn minified_sharp_pattern_presents_as_box_average_after_crop() {
    let (w, h) = (256u32, 256u32);
    let (vw, vh) = (16u32, 16u32);
    let mut engine = test_engine(w, h);
    plant(&mut engine, w, h, |x, _| x % 8 == 0);
    engine.resize_canvas(CanvasRect::from_xywh(16, 16, 128, 128));

    view(&mut engine, 0.125, 0.0, 0.0, vw, vh);
    let px = engine.test_readback_viewport(vw, vh);
    for y in 0..vh {
        for x in 0..vw {
            let v = value_at(&px, vw, x, y);
            assert!(
                (v as i32 - 223).abs() <= 2,
                "pixel ({x},{y}) = {v}, want 223 +- 2 after crop"
            );
        }
    }
}

/// Between 1:2 and 1:1 the present is a level-0 bilinear tap, exactly as
/// before the chain existed: no coarser level bleeds in. A one-texel black
/// line at zoom 0.75 must match the CPU bilinear reference column for column.
#[test]
fn zoom_between_half_and_one_is_a_level0_bilinear_tap() {
    let (w, h) = (64u32, 64u32);
    let (vw, vh) = (48u32, 48u32);
    let zoom = 0.75;
    let mut engine = test_engine(w, h);
    plant(&mut engine, w, h, |x, _| x == 32);
    view(&mut engine, zoom, 0.0, 0.0, vw, vh);
    let px = engine.test_readback_viewport(vw, vh);

    let level0: Vec<f32> = (0..w).map(|x| if x == 32 { 0.0 } else { 255.0 }).collect();
    for i in 0..vw {
        let want = bilinear_1d(&level0, canvas_x(i, vw, w, zoom));
        let got = value_at(&px, vw, i, vh / 2) as f32;
        assert!(
            (got - want).abs() <= 3.0,
            "column {i}: got {got}, want level-0 bilinear {want:.1}"
        );
    }
}

/// REGRESSION: between 1:4 and 1:2 the present reads mip level 1 (the 2x2
/// box of level 0) bilinearly. At zoom 0.3 the centre column maps to canvas
/// 32.0: the unfixed level-0 tap blends texels 31 and 32 to 128, while level
/// 1 holds the line as one half-grey texel and the reference there is 191.
#[test]
fn zoom_between_quarter_and_half_reads_level1() {
    let (w, h) = (64u32, 64u32);
    // 19 / 0.3 = 63.3 canvas texels: the whole row is canvas, no workspace.
    let (vw, vh) = (19u32, 19u32);
    let zoom = 0.3;
    let mut engine = test_engine(w, h);
    plant(&mut engine, w, h, |x, _| x == 32);
    view(&mut engine, zoom, 0.0, 0.0, vw, vh);
    let px = engine.test_readback_viewport(vw, vh);

    let level1: Vec<f32> = (0..w / 2)
        .map(|x| if x == 16 { 127.5 } else { 255.0 })
        .collect();
    for i in 0..vw {
        let want = bilinear_1d(&level1, canvas_x(i, vw, w, zoom) / 2.0);
        let got = value_at(&px, vw, i, vh / 2) as f32;
        assert!(
            (got - want).abs() <= 3.0,
            "column {i}: got {got}, want level-1 bilinear {want:.1}"
        );
    }
}

/// REGRESSION: `auto` samples linearly below 2x. At 1.5x nearest duplicates
/// alternate texels unevenly and a one-texel line presents as one pure-black
/// column; linear spreads it over three columns (212, 43, 128) and preserves
/// its coverage (total darkness 1.5 * 255).
#[test]
fn auto_filter_stays_linear_below_2x() {
    let (w, h) = (64u32, 64u32);
    let (vw, vh) = (96u32, 96u32);
    let mut engine = test_engine(w, h);
    plant(&mut engine, w, h, |x, _| x == 32);
    engine.set_pixel_filter("auto");
    view(&mut engine, 1.5, 0.0, 0.0, vw, vh);
    let px = engine.test_readback_viewport(vw, vh);

    let row: Vec<u8> = (0..vw).map(|x| value_at(&px, vw, x, vh / 2)).collect();
    let min = *row.iter().min().unwrap();
    assert!(
        min > 16,
        "a pure-black column ({min}) means nearest sampling at 1.5x"
    );
    let darkness: f32 = row.iter().map(|&v| 255.0 - v as f32).sum();
    let want = 1.5 * 255.0;
    assert!(
        (darkness - want).abs() / want < 0.05,
        "row darkness {darkness}, want {want} (linear preserves coverage)"
    );
}

/// Columns of a middle row that read pure black.
fn black_columns(px: &[u8], vw: u32, vh: u32) -> Vec<u32> {
    (0..vw)
        .filter(|&x| value_at(px, vw, x, vh / 2) == 0)
        .collect()
}

/// `auto` snaps to texel centres from 2x, where texels tile evenly, and
/// `nearest` snaps at any magnification: the modes still differ where the
/// setting says they do.
#[test]
fn auto_snaps_at_2x_and_nearest_snaps_at_any_magnification() {
    let (w, h) = (64u32, 64u32);
    let (vw, vh) = (96u32, 96u32);
    let mut engine = test_engine(w, h);
    plant(&mut engine, w, h, |x, _| x == 32);

    // Canvas x maps to screen 48 + (x - 32) * 2: texel 32 covers columns 48, 49.
    engine.set_pixel_filter("auto");
    view(&mut engine, 2.0, 0.0, 0.0, vw, vh);
    let px = engine.test_readback_viewport(vw, vh);
    assert_eq!(black_columns(&px, vw, vh), vec![48, 49], "auto at 2x");
    assert_eq!(value_at(&px, vw, 47, vh / 2), 255);
    assert_eq!(value_at(&px, vw, 50, vh / 2), 255);

    // Texel 32 covers screen [48.25, 49.75): the pan keeps both column centres
    // it contains a quarter pixel clear of its edges.
    engine.set_pixel_filter("nearest");
    view(&mut engine, 1.5, 0.0, 0.25, vw, vh);
    let px = engine.test_readback_viewport(vw, vh);
    assert_eq!(black_columns(&px, vw, vh), vec![48, 49], "nearest at 1.5x");
    assert_eq!(value_at(&px, vw, 47, vh / 2), 255);
    assert_eq!(value_at(&px, vw, 50, vh / 2), 255);
}

/// Paint a short horizontal stroke so the composite changes.
fn paint_dab_row(engine: &mut DarklyEngine, layer_id: LayerId) {
    engine.begin_stroke(layer_id).unwrap();
    for i in 0..8 {
        engine.stroke_to(StrokeOp::BrushStroke {
            x: 20.0 + i as f32 * 4.0,
            y: 40.0,
            pressure: 1.0,
            x_tilt: 0.0,
            y_tilt: 0.0,
            rotation: 0.0,
            tangential_pressure: 0.0,
            time_ms: i as f64 * 16.0,
            cr: 1.0,
            cg: 0.0,
            cb: 0.0,
            ca: 1.0,
        });
    }
    engine.end_stroke();
}

/// The chain is built only when a minifying present follows a changed
/// composite: never at 1:1, once per composite change while zoomed out, and
/// not for pan or a repeat present.
#[test]
fn mip_chain_is_regenerated_lazily() {
    let (w, h) = (128u32, 128u32);
    let (vw, vh) = (128u32, 128u32);
    let mut engine = test_engine(w, h);
    let layer = plant(&mut engine, w, h, |x, _| x % 8 == 0);

    view(&mut engine, 1.0, 0.0, 0.0, vw, vh);
    engine.test_readback_viewport(vw, vh);
    assert_eq!(
        engine.test_present_mip_runs(),
        0,
        "1:1 never builds the chain"
    );
    assert_eq!(engine.test_root_mip_levels(), levels_for(w, h));

    view(&mut engine, 0.5, 0.0, 0.0, vw, vh);
    engine.test_readback_viewport(vw, vh);
    assert_eq!(
        engine.test_present_mip_runs(),
        1,
        "first minified present builds it"
    );

    engine.test_readback_viewport(vw, vh);
    assert_eq!(
        engine.test_present_mip_runs(),
        1,
        "repeat present reuses it"
    );

    view(&mut engine, 0.5, 0.0, 7.0, vw, vh);
    engine.test_readback_viewport(vw, vh);
    assert_eq!(engine.test_present_mip_runs(), 1, "pan reuses it");

    paint_dab_row(&mut engine, layer);
    engine.test_readback_viewport(vw, vh);
    assert_eq!(
        engine.test_present_mip_runs(),
        2,
        "a changed composite rebuilds it"
    );

    engine.resize_canvas(CanvasRect::from_xywh(8, 8, 64, 64));
    view(&mut engine, 0.5, 0.0, 0.0, vw, vh);
    engine.test_readback_viewport(vw, vh);
    assert_eq!(
        engine.test_present_mip_runs(),
        3,
        "a reallocated root rebuilds it"
    );
    assert_eq!(engine.test_root_mip_levels(), levels_for(64, 64));
}
