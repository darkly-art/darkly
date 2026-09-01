//! Smart objects: a placed image displayed through a stored transform.
//!
//! The point of the kind is that resizing is non-destructive. These tests pin
//! that end to end: placement geometry, the lossless scale round trip,
//! minification quality, save/load, and the coordinate frame the image is
//! anchored in.

use darkly::coord::CanvasRect;
use darkly::engine::{DarklyEngine, SavePurpose};
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use darkly::layer::LayerId;
use darkly::transform::Transform;

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

/// 64×64, left half opaque red, right half opaque blue. Straight alpha, as an
/// image decode delivers it.
fn red_blue_source() -> (u32, u32, Vec<u8>) {
    let (w, h) = (64u32, 64u32);
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for _y in 0..h {
        for x in 0..w {
            if x < w / 2 {
                px.extend_from_slice(&[255, 0, 0, 255]);
            } else {
                px.extend_from_slice(&[0, 0, 255, 255]);
            }
        }
    }
    (w, h, px)
}

/// 64×64 of alternating one-pixel-wide red and blue columns. Under correct
/// area-averaging a heavy minification of this reads uniform mid-purple; under
/// point or bilinear sampling it reads near-pure red or blue depending on
/// sub-texel phase.
fn stripe_source() -> (u32, u32, Vec<u8>) {
    let (w, h) = (64u32, 64u32);
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for _y in 0..h {
        for x in 0..w {
            if x % 2 == 0 {
                px.extend_from_slice(&[255, 0, 0, 255]);
            } else {
                px.extend_from_slice(&[0, 0, 255, 255]);
            }
        }
    }
    (w, h, px)
}

/// Fully opaque red, edge to edge, no transparent margin, so the rectangle's
/// own border is the visible silhouette.
fn opaque_source(w: u32, h: u32) -> (u32, u32, Vec<u8>) {
    let px = std::iter::repeat_n([255u8, 0, 0, 255], (w * h) as usize)
        .flatten()
        .collect();
    (w, h, px)
}

fn px(buf: &[u8], stride: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * stride + x) * 4) as usize;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

/// Drive an in-flight save to completion and hand back the zip bytes. Mirrors
/// the engine's own save-flow tests: pump readbacks + render until
/// `poll_save_result` yields.
fn save_to_zip(engine: &mut DarklyEngine) -> Vec<u8> {
    engine
        .start_save_document(SavePurpose::File)
        .expect("save kicks off");
    for _ in 0..32 {
        engine.test_flush_readbacks();
        engine.render(0.0);
        if let Some(bundle) = engine.poll_save_result() {
            return darkly::format::zip_io::assemble_zip(&bundle);
        }
    }
    panic!("save did not complete within 32 frames");
}

/// Every void layer's id, read out of the serialized layer tree, the same
/// view the frontend consumes, so the test can find a layer whose id was
/// re-minted by a document load.
fn void_layer_ids(engine: &DarklyEngine) -> Vec<LayerId> {
    let json = serde_json::to_value(engine.layer_tree()).unwrap();
    let mut out = Vec::new();
    fn walk(node: &serde_json::Value, out: &mut Vec<LayerId>) {
        match node {
            serde_json::Value::Array(items) => items.iter().for_each(|n| walk(n, out)),
            serde_json::Value::Object(obj) => {
                if obj.get("type").and_then(|t| t.as_str()) == Some("void") {
                    if let Some(id) = obj.get("id").and_then(|i| i.as_f64()) {
                        out.push(LayerId::from_ffi(id as u64));
                    }
                }
                if let Some(children) = obj.get("children") {
                    walk(children, out);
                }
            }
            _ => {}
        }
    }
    walk(&json, &mut out);
    out
}

fn place(engine: &mut DarklyEngine, src: (u32, u32, Vec<u8>)) -> LayerId {
    let (w, h, px) = src;
    engine
        .place_smart_object(w, h, px, None)
        .expect("a well-formed image should place")
}

#[test]
fn smart_object_is_registered_as_a_void_kind() {
    let types: Vec<_> = darkly::gpu::void::catalog()
        .entries
        .into_iter()
        .map(|e| e.type_id)
        .collect();
    assert!(
        types.contains(&"smart_object"),
        "smart_object must be auto-registered by build.rs; got {types:?}",
    );
}

/// Placement centres the image and pins it to the plane. A 64×64 source on a
/// 128×128 canvas fits without scaling, so it lands at (32, 32).
#[test]
fn placement_lands_source_pixels_at_expected_canvas_positions() {
    let mut engine = test_engine(128, 128);
    let id = place(&mut engine, red_blue_source());
    assert!(engine.void_transform_info(id).is_some());

    let canvas = engine.test_readback_canvas();
    // Left half of the image → red; right half → blue.
    assert_eq!(px(&canvas, 128, 32 + 16, 64), [255, 0, 0, 255]);
    assert_eq!(px(&canvas, 128, 32 + 48, 64), [0, 0, 255, 255]);
    // Outside the placed rect the canvas is untouched.
    assert_eq!(px(&canvas, 128, 4, 4), [0, 0, 0, 0]);
}

/// An oversized image is scaled down to fit rather than landing mostly
/// off-canvas, and the scale is uniform so the aspect ratio survives.
#[test]
fn oversize_placement_fits_the_canvas() {
    let mut engine = test_engine(100, 100);
    let id = place(&mut engine, opaque_source(400, 200));
    let (ox, oy, w, h, t) = engine.void_transform_info(id).expect("info");

    // Content rect is the source's natural size at the plane origin; the fit
    // lives in the transform.
    assert_eq!((ox, oy, w, h), (0.0, 0.0, 400.0, 200.0));
    let m = t.to_affine();
    assert_eq!(m[0], 0.25, "scaled to fit the 100px width");
    assert_eq!(m[4], 0.25, "same scale on both axes");
    assert_eq!(m[2], 0.0, "400 × 0.25 = 100 exactly fills the width");
    assert_eq!(m[5], 25.0, "200 × 0.25 = 50, centred in 100");
}

/// THE headline requirement. Scaling down and back up must return the original
/// pixels bit for bit, because each frame re-samples the untouched source
/// instead of resampling the previous result.
#[test]
fn rescale_round_trip_is_bit_identical() {
    let mut engine = test_engine(128, 128);
    let id = place(&mut engine, red_blue_source());

    let (.., original) = engine.void_transform_info(id).expect("info");
    let before_canvas = engine.test_readback_canvas();
    let before_source = engine
        .test_readback_void_frame(id)
        .expect("a placed smart object owns a source");

    // Shrink hard, blow up, then return to exactly where we started.
    engine.update_void_transform(
        id,
        Transform::from_affine([0.05, 0.0, 40.0, 0.0, 0.05, 40.0]),
    );
    let _ = engine.test_readback_canvas();
    engine.update_void_transform(
        id,
        Transform::from_affine([8.0, 0.0, -100.0, 0.0, 8.0, -100.0]),
    );
    let _ = engine.test_readback_canvas();
    engine.update_void_transform(id, original);

    let after_canvas = engine.test_readback_canvas();
    assert_eq!(
        after_canvas, before_canvas,
        "returning to the original transform must reproduce the original \
         pixels exactly; any intermediate resample would round-trip lossily",
    );

    let after_source = engine
        .test_readback_void_frame(id)
        .expect("the source outlives the transform edits");
    assert_eq!(
        after_source, before_source,
        "the source image itself must never be rewritten by a transform",
    );
}

/// 64 → 8 px, placed at (40, 40): each output pixel covers 8 source columns of
/// [`stripe_source`], i.e. four red and four blue.
fn stripe_minify() -> Transform {
    Transform::from_affine([0.125, 0.0, 40.0, 0.0, 0.125, 40.0])
}

/// Minifying one-pixel stripes must read as their average, not as one of them.
///
/// This pins the user-visible property (a shrunk image resolves rather than
/// shimmering), but not the mechanism: the headless adapter integrates the
/// whole footprint per sample, so it satisfies this with or without a mip
/// chain. `smart_object_source_carries_a_mip_chain` is what pins the chain, and
/// `gpu::rescale`'s own unit tests pin what each level contains.
#[test]
fn minified_stripes_average_instead_of_aliasing() {
    let mut engine = test_engine(128, 128);
    let id = place(&mut engine, stripe_source());

    engine.update_void_transform(id, stripe_minify());
    assert_stripes_averaged(&engine.test_readback_canvas());
}

/// The stripe fixture, minified 8× by [`stripe_minify`], must read as the
/// average of its columns across the whole footprint. Shared by the tests that
/// assert it directly and by the reload test, so the expected value and the
/// tolerance live in one place.
///
/// Area-averaging is the only thing that satisfies both halves: a point or
/// single-tap bilinear sampler returns near-pure red or blue depending on
/// sub-texel phase, missing the expected value, and adjacent output pixels
/// disagree by ~200 instead of agreeing.
fn assert_stripes_averaged(canvas: &[u8]) {
    // Sample across the interior of the shrunk image, avoiding its border.
    let row = 44u32;
    let samples: Vec<[u8; 4]> = (42..46).map(|x| px(canvas, 128, x, row)).collect();

    for s in &samples {
        assert!(
            (i32::from(s[0]) - 128).abs() <= 24 && (i32::from(s[2]) - 128).abs() <= 24,
            "expected a red/blue average near [128, _, 128], got {s:?}: a \
             single-tap sampler returns near-pure red or blue instead",
        );
    }
    let reds: Vec<i32> = samples.iter().map(|s| i32::from(s[0])).collect();
    let spread = reds.iter().max().unwrap() - reds.iter().min().unwrap();
    assert!(
        spread <= 24,
        "adjacent minified pixels must agree; a phase-dependent sampler makes \
         them disagree by ~200. got spread {spread} across {reds:?}",
    );
}

/// The boundary of a rotated image must be partially covered, not a hard
/// in-or-out decision. Fails against a shader that masks out-of-frame samples
/// with a boolean test.
#[test]
fn rotated_boundary_has_partial_coverage() {
    let mut engine = test_engine(128, 128);
    let id = place(&mut engine, opaque_source(40, 40));

    // Rotate 30° about the canvas centre.
    let (s, c) = (30f32.to_radians().sin(), 30f32.to_radians().cos());
    let (cx, cy) = (64.0f32, 64.0f32);
    engine.update_void_transform(
        id,
        Transform::from_affine([
            c,
            -s,
            cx - c * 20.0 + s * 20.0,
            s,
            c,
            cy - s * 20.0 - c * 20.0,
        ]),
    );
    let canvas = engine.test_readback_canvas();

    let partial = canvas
        .as_chunks::<4>()
        .0
        .iter()
        .any(|p| p[3] > 8 && p[3] < 247);
    assert!(
        partial,
        "a rotated edge must produce partially covered pixels; a boolean \
         in/out test emits only alpha 0 or 255",
    );

    // The fix must antialias the edge, not smear the image outward.
    assert_eq!(
        px(&canvas, 128, 64, 64),
        [255, 0, 0, 255],
        "the interior stays fully opaque and unblurred",
    );
    assert_eq!(
        px(&canvas, 128, 2, 2),
        [0, 0, 0, 0],
        "far exterior is clear"
    );
}

/// Minification must not project the image's edge across the rest of the
/// canvas. The silhouette is the transformed source rect at every scale; a
/// sample that lands outside it contributes nothing, however deep into the mip
/// chain the hardware reads.
#[test]
fn minification_does_not_smear_the_edge_across_the_canvas() {
    let mut engine = test_engine(128, 128);
    let id = place(&mut engine, opaque_source(64, 64));

    // 64 → 4 px at (60, 60); everything else on the canvas is untouched.
    engine.update_void_transform(
        id,
        Transform::from_affine([0.0625, 0.0, 60.0, 0.0, 0.0625, 60.0]),
    );
    let canvas = engine.test_readback_canvas();

    assert_eq!(
        px(&canvas, 128, 62, 62),
        [255, 0, 0, 255],
        "precondition: the shrunk image itself is still drawn",
    );
    for (x, y) in [(2u32, 2u32), (30, 62), (100, 62), (62, 30), (62, 100)] {
        assert_eq!(
            px(&canvas, 128, x, y),
            [0, 0, 0, 0],
            "({x}, {y}) is outside the shrunk image and must stay clear",
        );
    }
}

/// Cropping the canvas must not move a placed image relative to the artwork.
/// A smart object is anchored in the document plane, not the canvas window.
#[test]
fn crop_does_not_move_the_smart_object() {
    let mut engine = test_engine(128, 128);
    let id = place(&mut engine, red_blue_source());

    let (ox, oy, w, h, _) = engine.void_transform_info(id).expect("info");
    // Plane point that sits inside the image before the crop.
    let probe_plane = (48u32, 64u32);
    let before = px(
        &engine.test_readback_canvas(),
        128,
        probe_plane.0,
        probe_plane.1,
    );
    assert_eq!(before, [255, 0, 0, 255], "precondition: red half");

    // Crop to a window whose origin is non-zero in plane space.
    engine.resize_canvas(CanvasRect::from_xywh(10, 20, 80, 80));

    let (ox2, oy2, w2, h2, _) = engine.void_transform_info(id).expect("info");
    assert_eq!(
        (ox, oy, w, h),
        (ox2, oy2, w2, h2),
        "the gizmo bbox is a plane-space fact and must survive a crop",
    );

    // The same plane pixel, now at window-local (plane − origin).
    let after = px(
        &engine.test_readback_canvas(),
        80,
        probe_plane.0 - 10,
        probe_plane.1 - 20,
    );
    assert_eq!(
        after, before,
        "the image must stay put in the document when the window moves",
    );
}

/// A placed image survives save and reload: both the transform and the
/// source pixels, at the source's own dimensions.
#[test]
fn transform_round_trips_through_save_and_load() {
    let mut engine = test_engine(128, 128);
    let id = place(&mut engine, red_blue_source());

    let edited = Transform::from_affine([1.5, 0.0, 7.0, 0.0, 1.5, 11.0]);
    engine.update_void_transform(id, edited);
    let source_before = engine.test_readback_void_frame(id).expect("source");
    let (w, h, straight) = red_blue_source();
    assert_eq!(
        source_before.len(),
        straight.len(),
        "the saved region is the source image at its own size ({w}×{h} RGBA)",
    );

    let zip = save_to_zip(&mut engine);

    let mut reloaded = test_engine(128, 128);
    reloaded.open_document(&zip).expect("the document reopens");

    let void_ids = void_layer_ids(&reloaded);
    assert_eq!(
        void_ids.len(),
        1,
        "one smart object survives the round trip"
    );
    let new_id = void_ids[0];

    let (.., stored) = reloaded.void_transform_info(new_id).expect("info");
    assert_eq!(stored, edited, "the stored transform round-trips exactly");
    assert_eq!(
        reloaded.test_readback_void_frame(new_id).expect("source"),
        source_before,
        "the source pixels round-trip byte for byte",
    );
}

/// The transform and the source bytes surviving a reload does not by itself
/// prove the reloaded document *looks* the same: the compositor rebuilds the
/// content rect, the sampling uniform and the mip chain from scratch on load,
/// and a smart object is normally viewed minified, where the chain is what the
/// hardware actually samples. Assert the rendered canvas instead.
#[test]
fn a_reloaded_smart_object_renders_identically() {
    let mut engine = test_engine(128, 128);
    let id = place(&mut engine, stripe_source());
    engine.update_void_transform(id, stripe_minify());
    let before = engine.test_readback_canvas();

    let zip = save_to_zip(&mut engine);
    let mut reloaded = test_engine(128, 128);
    reloaded.open_document(&zip).expect("the document reopens");
    let after = reloaded.test_readback_canvas();

    assert_eq!(
        after, before,
        "a reloaded smart object must render pixel for pixel what it rendered \
         before the save: same source, same transform, same filtering",
    );
    // Equality alone would also be satisfied by both renders being wrong in the
    // same way, so assert the reloaded canvas is right on its own terms: the
    // mip chain a minified source samples through is rebuilt on load, not
    // saved, and this is the assertion only a real chain satisfies.
    assert_stripes_averaged(&after);
}

/// A placed source is allocated with a full mip chain, and the chain is rebuilt
/// on load rather than saved; the blob holds level 0 only. Asserted
/// structurally because no rendered-pixel assertion can see it here (the
/// headless adapter filters a full footprint per sample either way), and
/// because the chain is the whole minification-quality work item: without this
/// the void could quietly stop asking for one and every test would stay green.
#[test]
fn smart_object_source_carries_a_mip_chain() {
    // 64 = 2⁶, so the chain runs 64, 32, 16, 8, 4, 2, 1.
    const LEVELS: u32 = 7;

    let mut engine = test_engine(128, 128);
    let id = place(&mut engine, stripe_source());
    assert_eq!(
        engine.test_void_source_mip_levels(id),
        Some(LEVELS),
        "a freshly placed 64×64 source is allocated with its full chain",
    );

    let zip = save_to_zip(&mut engine);
    let mut reloaded = test_engine(128, 128);
    reloaded.open_document(&zip).expect("the document reopens");
    let new_id = void_layer_ids(&reloaded)[0];

    assert_eq!(
        reloaded.test_void_source_mip_levels(new_id),
        Some(LEVELS),
        "load must reinstall the source through the same path placement uses, \
         chain included; the saved blob is level 0 only",
    );
}

/// Each smart object owns its own pixel blob. Two placements in one document
/// must not converge on one key, which would reload both layers showing
/// whichever image was written last.
#[test]
fn two_smart_objects_keep_their_own_sources() {
    let mut engine = test_engine(128, 128);
    place(&mut engine, red_blue_source());
    place(&mut engine, opaque_source(32, 32));

    let zip = save_to_zip(&mut engine);
    let mut reloaded = test_engine(128, 128);
    reloaded.open_document(&zip).expect("the document reopens");

    let sources: Vec<Vec<u8>> = void_layer_ids(&reloaded)
        .into_iter()
        .map(|id| {
            reloaded
                .test_readback_void_frame(id)
                .expect("each reloaded smart object owns a source")
        })
        .collect();
    assert_eq!(
        sources.len(),
        2,
        "both smart objects survive the round trip"
    );

    let big = sources
        .iter()
        .find(|s| s.len() == 64 * 64 * 4)
        .expect("the 64×64 source keeps its own dimensions");
    let small = sources
        .iter()
        .find(|s| s.len() == 32 * 32 * 4)
        .expect("the 32×32 source keeps its own dimensions");
    assert_eq!(
        px(big, 64, 48, 32),
        [0, 0, 255, 255],
        "the 64×64 source is still the red/blue split, blue on its right half",
    );
    assert!(
        small
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [255, 0, 0, 255]),
        "the 32×32 source is still uniformly red",
    );
}

/// Painting on a smart object is refused, and refused without side effects.
#[test]
fn painting_a_smart_object_is_refused() {
    let mut engine = test_engine(128, 128);
    engine.add_raster_layer(None);
    let id = place(&mut engine, red_blue_source());

    assert!(
        !engine.is_node_paintable(id),
        "a smart object's pixels come from its source, so paint has nowhere \
         to land that would survive the next frame",
    );

    // The refusal has to be reported, not silent: a stroke that paints nothing
    // and says nothing is indistinguishable from a broken brush.
    let refusal = engine
        .begin_stroke(id)
        .expect_err("a smart object must refuse the stroke");
    assert!(
        refusal.contains("Rasterize"),
        "the refusal must point at the way out; got {refusal:?}",
    );
}

/// The way out of the refusal above: rasterizing replaces the smart object
/// with a raster holding exactly what it rendered, and that raster accepts
/// paint. One undo step brings the smart object back.
#[test]
fn rasterizing_a_smart_object_makes_it_paintable() {
    let mut engine = test_engine(128, 128);
    let id = place(&mut engine, red_blue_source());
    let before = engine.test_readback_canvas();

    assert!(
        engine.can_flatten_node(id),
        "a layer whose pixels are generated must offer the rasterize path",
    );
    let raster = engine.flatten_node(id).expect("rasterize succeeds");

    assert!(
        engine.is_node_paintable(raster),
        "the rasterized layer owns its pixels, so paint lands on it",
    );
    engine
        .begin_stroke(raster)
        .expect("the raster accepts paint");
    engine.end_stroke();

    assert_eq!(
        engine.test_readback_canvas(),
        before,
        "rasterizing must not change a single pixel of what was on screen",
    );

    engine.undo();
    assert!(
        engine.void_transform_info(id).is_some(),
        "undo brings the smart object back, transform and all",
    );
    assert_eq!(
        engine.test_readback_canvas(),
        before,
        "and the canvas is unchanged across the round trip",
    );
}

/// A gizmo drag is many transform updates but one undo step.
#[test]
fn undo_of_a_transform_drag_is_one_step() {
    let mut engine = test_engine(128, 128);
    let id = place(&mut engine, red_blue_source());
    let (.., placed) = engine.void_transform_info(id).expect("info");

    for i in 1..=8 {
        let s = 1.0 + i as f32 * 0.1;
        engine.update_void_transform(id, Transform::from_affine([s, 0.0, 0.0, 0.0, s, 0.0]));
    }
    engine.undo();

    let (.., after) = engine.void_transform_info(id).expect("info");
    assert_eq!(
        after, placed,
        "one undo returns to the placement transform, not to an intermediate \
         drag position",
    );
}

/// Placement is a single undo step: the layer and its pixels arrive together,
/// so undoing it leaves no empty smart object behind.
#[test]
fn undo_of_placement_removes_the_layer_entirely() {
    let mut engine = test_engine(128, 128);
    engine.add_raster_layer(None);
    let before = engine.test_readback_canvas();

    let id = place(&mut engine, red_blue_source());
    assert_ne!(engine.test_readback_canvas(), before);

    engine.undo();
    assert!(
        !engine.has_layer(id),
        "the placed layer is out of the tree after a single undo",
    );
    assert_eq!(
        engine.test_readback_canvas(),
        before,
        "and the canvas is exactly as it was",
    );
}

/// Degenerate input is rejected rather than producing a broken layer.
#[test]
fn malformed_placements_are_rejected() {
    let mut engine = test_engine(64, 64);
    assert!(engine.place_smart_object(0, 8, vec![0; 0], None).is_none());
    assert!(engine.place_smart_object(8, 0, vec![0; 0], None).is_none());
    assert!(
        engine.place_smart_object(8, 8, vec![0; 10], None).is_none(),
        "a buffer that doesn't match the stated dimensions must be refused, \
         not read out of bounds",
    );
}
