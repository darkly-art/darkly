//! Integration tests for the Void layer kind end-to-end through the engine.
//!
//! Voids live in the layer tree like any other leaf, generate content
//! procedurally on the GPU, and participate in normal undo / blend props.
//! These tests exercise add / undo / redo and the param-edit flow.

use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::params::ParamValue;
use darkly::gpu::test_utils::test_device;

fn test_engine(width: u32, height: u32) -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, width, height)
}

fn noise_defaults(engine: &DarklyEngine) -> Vec<ParamValue> {
    engine
        .void_param_defs("noise")
        .iter()
        .map(darkly::gpu::params::ParamDef::default_value)
        .collect()
}

#[test]
fn noise_void_is_registered() {
    let types: Vec<_> = darkly::gpu::void::catalog()
        .entries
        .into_iter()
        .map(|e| e.type_id)
        .collect();
    assert!(
        types.contains(&"noise"),
        "noise void must be auto-registered by build.rs; got {types:?}",
    );
}

#[test]
fn noise_void_default_params_match_schema() {
    let engine = test_engine(64, 64);
    let defs = engine.void_param_defs("noise");
    let defaults = noise_defaults(&engine);
    assert_eq!(
        defs.len(),
        defaults.len(),
        "default vector length must match the void's ParamDef slice",
    );
}

#[test]
fn fresh_void_is_named_after_its_display_type() {
    // Default layer names: the panel should show "Noise 1", "Noise 2", …,
    // not a generic "Void N". Per-type counters give each void kind its own
    // numbering once more types ship.
    let mut engine = test_engine(64, 64);
    let params = noise_defaults(&engine);

    let id1 = engine
        .add_void_layer("noise", params.clone(), None)
        .expect("noise void should be addable");
    let id2 = engine
        .add_void_layer("noise", params, None)
        .expect("second noise void should be addable");

    let name1 = layer_name(&engine, id1);
    let name2 = layer_name(&engine, id2);
    assert_eq!(name1, "Noise 1");
    assert_eq!(name2, "Noise 2");
}

fn layer_name(engine: &DarklyEngine, layer_id: darkly::layer::LayerId) -> String {
    let tree = engine.layer_tree();
    let json = serde_json::to_value(&tree).unwrap();
    fn walk(node: &serde_json::Value, want: u64) -> Option<String> {
        if let Some(arr) = node.as_array() {
            for n in arr {
                if let Some(s) = walk(n, want) {
                    return Some(s);
                }
            }
            return None;
        }
        if let Some(obj) = node.as_object() {
            if obj.get("id").and_then(|i| i.as_f64()) == Some(want as f64) {
                return obj.get("name").and_then(|n| n.as_str()).map(str::to_string);
            }
            if let Some(children) = obj.get("children") {
                return walk(children, want);
            }
        }
        None
    }
    walk(&json, layer_id.to_ffi()).expect("layer should be in the tree")
}

/// Regression: a void layer must not accept brush strokes. The compositor
/// regenerates the void's texture from `(void_type, params)` each frame
/// (or eagerly on param change), so any paint that lands there either
/// vanishes on the next dirty tick or stays only because the artist hasn't
/// touched a slider yet; both are confusing and neither is the artist's
/// intent when they clicked a void in the layer panel and started painting.
/// `is_node_paintable` rejects voids at every stroke entry point.
#[test]
fn voids_are_not_paintable() {
    let mut engine = test_engine(64, 64);
    let raster_id = engine.add_raster_layer(None);
    let params = noise_defaults(&engine);
    let void_id = engine
        .add_void_layer("noise", params, None)
        .expect("noise void should be addable");

    assert!(
        engine.is_node_paintable(raster_id),
        "raster layers must remain paintable",
    );
    assert!(
        !engine.is_node_paintable(void_id),
        "void layers must not accept paint",
    );
}

#[test]
fn add_void_layer_appears_in_tree() {
    let mut engine = test_engine(64, 64);
    let params = noise_defaults(&engine);
    let id = engine
        .add_void_layer("noise", params, None)
        .expect("noise void should be addable");

    // The void layer surfaces in `layer_tree()` with the `void` type tag.
    let tree = engine.layer_tree();
    let json = serde_json::to_string(&tree).unwrap();
    assert!(
        json.contains("\"type\":\"void\""),
        "layer_tree should expose the void with type=void; got {json}",
    );
    assert!(engine.has_layer(id));
}

#[test]
fn add_unknown_void_type_returns_none() {
    let mut engine = test_engine(64, 64);
    let result = engine.add_void_layer("totally-not-a-real-void", Vec::new(), None);
    assert!(
        result.is_none(),
        "unknown void type ids should be rejected, not silently substituted",
    );
}

#[test]
fn add_void_layer_is_undoable() {
    let mut engine = test_engine(64, 64);
    let params = noise_defaults(&engine);
    let id = engine
        .add_void_layer("noise", params, None)
        .expect("noise void should be addable");

    assert!(engine.has_layer(id), "fresh void should be in the tree");
    engine.undo();
    assert!(
        !engine.has_layer(id),
        "undo should detach the void from the tree (id stays valid for redo)",
    );
    engine.redo();
    assert!(engine.has_layer(id), "redo should re-attach the void");
}

#[test]
fn update_void_params_is_undoable_and_coalesces() {
    let mut engine = test_engine(64, 64);
    let defaults = noise_defaults(&engine);
    let id = engine
        .add_void_layer("noise", defaults.clone(), None)
        .expect("noise void should be addable");

    // Defaults match the schema.
    let info = void_params_via_tree(&engine, id);
    assert_eq!(info.len(), defaults.len());

    // Drag-style edits: bump the seed three times in a row, the
    // PropertyAction coalesce path collapses them into one undo step.
    let mut next = defaults.clone();
    for new_seed in [123, 124, 125] {
        for v in next.iter_mut() {
            if let ParamValue::Int(_) = v {
                *v = ParamValue::Int(new_seed);
                break;
            }
        }
        engine.update_void_params(id, next.clone());
    }

    // One undo restores ALL the way back to the original schema defaults:
    // the three edits coalesced.
    engine.undo();
    let restored = void_params_via_tree(&engine, id);
    assert_eq!(
        restored.first().cloned(),
        defaults.first().cloned(),
        "undo should roll back to pre-drag seed, not the intermediate one",
    );
}

/// The `time` slider must actually scrub the 3D noise field: changing it
/// has to land at the GPU uniform and produce a visibly different
/// cross-section. Catches a silent regression where the param is dropped on
/// the way to the shader (forgotten write in `update_params`, layout drift
/// in `NoiseUniforms`, etc.).
#[test]
fn noise_void_time_scrub_changes_output() {
    let mut engine = test_engine(64, 64);
    let mut params = noise_defaults(&engine);
    set_float_param(&mut params, "time", 0.0);
    let id = engine
        .add_void_layer("noise", params.clone(), None)
        .expect("noise void should be addable");

    let frame_a = engine.test_readback_canvas();

    set_float_param(&mut params, "time", 5.0);
    engine.update_void_params(id, params);
    let frame_b = engine.test_readback_canvas();

    assert_ne!(
        frame_a, frame_b,
        "scrubbing `time` must produce a different cross-section of the field",
    );
}

/// The `time` slider must scrub *continuously* through the 3D noise volume:
/// a small step in `time` produces a small bounded per-pixel delta, never a
/// teleport to an uncorrelated field. Catches a regression where `time`
/// reseeds the field or swaps to a different FBM realization instead of
/// advancing along Z; 3D FBM is C1-continuous in Z so the per-pixel delta
/// stays bounded by `~Z_SCALE * dz * fade_deriv`.
#[test]
fn noise_void_time_scrub_is_continuous() {
    let mut engine = test_engine(64, 64);
    let mut params = noise_defaults(&engine);
    set_float_param(&mut params, "time", 0.0);
    set_float_param(&mut params, "warp", 1.5);
    let id = engine
        .add_void_layer("noise", params.clone(), None)
        .expect("noise void should be addable");

    let frame_a = engine.test_readback_canvas();

    // Small scrub step. At Z_SCALE=0.15 (see noise shader), dz=0.5 advances
    // ~0.075 along the noise Z-axis, well under one cell-cross, so deltas
    // stay small but visible.
    set_float_param(&mut params, "time", 0.5);
    engine.update_void_params(id, params);
    let frame_b = engine.test_readback_canvas();

    assert_eq!(
        frame_a.len(),
        frame_b.len(),
        "readbacks must be the same size",
    );

    let mut total_delta: u64 = 0;
    let mut changed_bytes: u32 = 0;
    let mut max_delta: u8 = 0;
    for (a, b) in frame_a.iter().zip(frame_b.iter()) {
        let d = a.abs_diff(*b);
        total_delta += d as u64;
        if d > 0 {
            changed_bytes += 1;
        }
        if d > max_delta {
            max_delta = d;
        }
    }
    let mean_delta = (total_delta as f32) / (frame_a.len() as f32);

    // Continuity bound: a teleport-style implementation would produce mean
    // ~70 (uncorrelated FBM realizations: std ~0.2 → ~50 in u8 → mean abs
    // delta ~70). A genuine 3D-FBM scrub at dz=0.5 sits in single digits;
    // 25 leaves plenty of headroom while still catching teleports.
    assert!(
        mean_delta < 25.0,
        "mean per-byte delta {mean_delta:.1} exceeds continuity bound; \
         a value near 70 indicates `time` is reseeding instead of scrubbing",
    );

    // `time` is actually reaching the output.
    let total_bytes = frame_a.len() as u32;
    assert!(
        changed_bytes * 10 >= total_bytes,
        "only {changed_bytes}/{total_bytes} bytes changed; `time` may not \
         be reaching the GPU uniform",
    );

    // Theoretical ceiling at dz=0.5 / Z_SCALE=0.15 is ~0.14 in [0,1] ≈ 36
    // in u8; allow 2× headroom for warp-path amplification.
    assert!(
        max_delta < 100,
        "max per-byte delta {max_delta} exceeds plausible 3D-FBM bound",
    );
}

/// Regression: the void owns its own "needs re-render" bit. Mutating one
/// void's params must not cause an unrelated void to re-encode: the
/// compositor must not flip dirty for every procedural layer on every
/// param edit. Verified by reading back two voids' textures, mutating
/// just one, and confirming the *un-mutated* one's pixels are byte-equal.
#[test]
fn void_dirty_is_per_instance() {
    let mut engine = test_engine(64, 64);
    let mut params_a = noise_defaults(&engine);
    let mut params_b = noise_defaults(&engine);
    // Different seeds so the two voids' textures are visibly distinct,
    // which helps catch any accidental aliasing in addition to the
    // dirty-bit check.
    set_int_param(&mut params_a, "seed", 100);
    set_int_param(&mut params_b, "seed", 200);

    let id_a = engine
        .add_void_layer("noise", params_a.clone(), None)
        .expect("noise void should be addable");
    let id_b = engine
        .add_void_layer("noise", params_b, None)
        .expect("second noise void should be addable");

    // Force a render so both voids have produced their first frame and
    // cleared their dirty flags.
    let _ = engine.test_readback_canvas();
    let before_a = engine.test_readback_layer(id_a);
    let before_b = engine.test_readback_layer(id_b);
    assert_ne!(
        before_a, before_b,
        "two different seeds must produce different textures",
    );

    // Mutate only A. B must not re-encode: its dirty bit was cleared
    // above and nothing has touched it since.
    set_float_param(&mut params_a, "time", 5.0);
    engine.update_void_params(id_a, params_a);
    let _ = engine.test_readback_canvas();
    let after_a = engine.test_readback_layer(id_a);
    let after_b = engine.test_readback_layer(id_b);

    assert_ne!(
        before_a, after_a,
        "mutated void should re-encode and change pixels",
    );
    assert_eq!(
        before_b, after_b,
        "untouched void must not re-encode: its dirty bit is its own state",
    );
}

fn set_int_param(params: &mut [ParamValue], name: &str, value: i32) {
    let engine = test_engine(1, 1);
    let defs = engine.void_param_defs("noise");
    let idx = defs
        .iter()
        .position(|d| match d.kind {
            darkly::gpu::params::ParamKind::Int { .. } => d.name == name,
            _ => false,
        })
        .unwrap_or_else(|| panic!("noise void has no int param '{name}'"));
    params[idx] = ParamValue::Int(value);
}

/// Look up a noise-void param slot by name. The schema currently exposes
/// `seed, octaves, size, warp, darkness, time`, but tests index by name
/// rather than position so new params don't silently shift them.
fn set_float_param(params: &mut [ParamValue], name: &str, value: f32) {
    let engine = test_engine(1, 1);
    let defs = engine.void_param_defs("noise");
    let idx = defs
        .iter()
        .position(|d| match d.kind {
            darkly::gpu::params::ParamKind::Float { .. } => d.name == name,
            _ => false,
        })
        .unwrap_or_else(|| panic!("noise void has no float param '{name}'"));
    params[idx] = ParamValue::Float(value);
}

/// Pull the current param vector for a void layer back out of the layer tree
/// (the round-trip JSON path the WASM bridge uses).
fn void_params_via_tree(
    engine: &DarklyEngine,
    layer_id: darkly::layer::LayerId,
) -> Vec<ParamValue> {
    let tree = engine.layer_tree();
    let json = serde_json::to_value(&tree).unwrap();
    fn walk(node: &serde_json::Value, want: u64) -> Option<serde_json::Value> {
        if let Some(arr) = node.as_array() {
            for n in arr {
                if let Some(found) = walk(n, want) {
                    return Some(found);
                }
            }
            return None;
        }
        if let Some(obj) = node.as_object() {
            if obj.get("type").and_then(|t| t.as_str()) == Some("void")
                && obj.get("id").and_then(|i| i.as_f64()) == Some(want as f64)
            {
                return Some(node.clone());
            }
            if let Some(children) = obj.get("children") {
                return walk(children, want);
            }
        }
        None
    }
    let v = walk(&json, layer_id.to_ffi()).expect("void layer should be in the tree");
    let params = v.get("params").unwrap().as_array().unwrap();
    params
        .iter()
        .map(|p| {
            let default = p.get("default").unwrap().clone();
            let value = p.get("value").cloned().unwrap_or(default);
            serde_json::from_value(value).expect("ParamValue round-trips through JSON")
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Generic transform on a void (camera): the void consuming the gizmo helper.
// ---------------------------------------------------------------------------

fn camera_defaults(engine: &DarklyEngine) -> Vec<ParamValue> {
    engine
        .void_param_defs("camera")
        .iter()
        .map(darkly::gpu::params::ParamDef::default_value)
        .collect()
}

/// The camera void opts into a live transform; noise and groups don't. The
/// tool reads this string to choose which binding drives the gizmo.
#[test]
fn transform_capability_reports_live_for_camera() {
    let mut engine = test_engine(64, 64);
    let cam = engine
        .add_void_layer("camera", camera_defaults(&engine), None)
        .expect("camera void should be addable");
    let noise = engine
        .add_void_layer("noise", noise_defaults(&engine), None)
        .expect("noise void should be addable");

    // Both camera and noise expose a live transform (every void has a content
    // bbox the gizmo can manipulate). Groups remain non-transformable.
    assert_eq!(engine.layer_transform_capability(cam), "live");
    assert_eq!(engine.layer_transform_capability(noise), "live");
    let group = engine.add_group(None);
    assert_eq!(engine.layer_transform_capability(group), "none");
}

/// THE coordinate-frame guard (docs/coordinate-systems.md #1): after a crop,
/// the gizmo bbox the void reports must be anchored at `canvas_origin`, NOT
/// (0,0), otherwise the handles draw in the wrong plane location. The shader
/// itself works in window-local (origin cancels), so this reporting boundary
/// is where the origin has to be correct.
#[test]
fn void_transform_info_reports_canvas_origin_after_crop() {
    use darkly::coord::CanvasRect;
    let mut engine = test_engine(128, 128);
    let cam = engine
        .add_void_layer("camera", camera_defaults(&engine), None)
        .expect("camera void should be addable");

    // Crop to a window whose origin is non-zero in plane space.
    engine.resize_canvas(CanvasRect::from_xywh(10, 20, 40, 50));
    assert_eq!(engine.canvas_rect().origin.x, 10);
    assert_eq!(engine.canvas_rect().origin.y, 20);

    // With no webcam frame yet, the camera's content rect falls back to
    // canvas-fill, so the bbox is exactly the (cropped) canvas window.
    let (ox, oy, w, h, _t) = engine
        .void_transform_info(cam)
        .expect("camera reports transform info");
    assert_eq!(
        (ox, oy),
        (10.0, 20.0),
        "gizmo origin must be canvas_origin, not (0,0)"
    );
    assert_eq!((w, h), (40.0, 50.0), "gizmo extent must be the canvas size");
}

/// Live transform round-trips through the doc and renders without panicking
/// (exercises the full set_transform → uniform → camera.wgsl path; the camera
/// has no frame on native so the output is transparent, but the pipeline runs).
#[test]
fn update_void_transform_stores_and_renders() {
    use darkly::transform::Transform;
    let mut engine = test_engine(64, 64);
    let cam = engine
        .add_void_layer("camera", camera_defaults(&engine), None)
        .expect("camera void should be addable");

    let t = Transform::from_affine([2.0, 0.0, 5.0, 0.0, 2.0, 9.0]);
    engine.update_void_transform(cam, t);
    let _ = engine.test_readback_canvas(); // must not panic

    let (.., stored) = engine.void_transform_info(cam).expect("info");
    assert_eq!(stored, t, "the stored transform reflects the update");
}

/// Perspective (homography) transforms reach voids now, not just affine. Set a
/// `Perspective` transform via the same `update_void_transform` path, render
/// (the shared `proj_local` sampler runs in the void shader), and read the mode
/// back as 1. Regression for the void path being pinned to affine.
#[test]
fn update_void_transform_supports_perspective() {
    use darkly::transform::{homography_from_corners, Transform};
    let mut engine = test_engine(64, 64);
    let id = engine
        .add_void_layer("noise", noise_defaults(&engine), None)
        .expect("noise void should be addable");

    // A trapezoid (narrowed top edge) over the 64×64 field, a genuine
    // vanishing-point warp, not an affine parallelogram.
    let corners = [(16.0, 0.0), (48.0, 0.0), (64.0, 64.0), (0.0, 64.0)];
    let m = homography_from_corners(64.0, 64.0, corners).expect("non-degenerate");
    let t = Transform::Perspective(m);

    let frame_identity = engine.test_readback_canvas();
    engine.update_void_transform(id, t);
    let frame_persp = engine.test_readback_canvas();

    let (.., stored) = engine.void_transform_info(id).expect("info");
    assert_eq!(
        stored.mode_tag(),
        1,
        "the void stores a Perspective transform"
    );
    assert_eq!(stored, t, "the stored homography round-trips");
    assert_ne!(
        frame_identity, frame_persp,
        "a perspective warp must change the void's rendered output",
    );
}

/// Undo restores the layer's PRE-EDIT transform, not identity. A non-coalescing
/// boundary (a param edit, different `Property` kind) sits between two transform
/// edits so the second is its own undo step whose `old_value` must be the first
/// transform, proving `update_void_transform` reads the current transform as
/// the old value rather than seeding identity.
#[test]
fn void_transform_undo_restores_pre_edit_value() {
    use darkly::transform::Transform;
    let mut engine = test_engine(64, 64);
    let cam = engine
        .add_void_layer("camera", camera_defaults(&engine), None)
        .expect("camera void should be addable");

    let a = Transform::from_affine([1.0, 0.0, 3.0, 0.0, 1.0, 0.0]);
    engine.update_void_transform(cam, a);

    // Different Property kind → breaks transform coalescing, so the next
    // transform edit starts a fresh undo step.
    engine.update_void_params(cam, camera_defaults(&engine));

    let b = Transform::from_affine([1.0, 0.0, 7.0, 0.0, 1.0, 0.0]);
    engine.update_void_transform(cam, b);

    engine.undo();
    let (.., after_undo) = engine.void_transform_info(cam).expect("info");
    assert_eq!(
        after_undo, a,
        "undo must restore the pre-edit transform, not identity"
    );
}

/// A whole gizmo drag (many `update_void_transform` calls in a row) collapses
/// into a single undo step (coalescing on the `Transform` discriminant).
#[test]
fn void_transform_drag_coalesces_to_one_undo_step() {
    use darkly::transform::Transform;
    let mut engine = test_engine(64, 64);
    let cam = engine
        .add_void_layer("camera", camera_defaults(&engine), None)
        .expect("camera void should be addable");

    for i in 1..=8 {
        engine.update_void_transform(
            cam,
            Transform::from_affine([1.0, 0.0, i as f32, 0.0, 1.0, 0.0]),
        );
    }
    // One undo undoes the entire drag back to the camera's seeded initial
    // transform: a horizontal flip about the canvas center (selfie view), not
    // identity. For a 64-wide canvas that's `x' = 64 - x`.
    engine.undo();
    let (.., after) = engine.void_transform_info(cam).expect("info");
    assert_eq!(
        after,
        Transform::from_affine([-1.0, 0.0, 64.0, 0.0, 1.0, 0.0]),
        "the drag must be a single undo step back to the seeded selfie flip"
    );
}

/// The transform gizmo works on a noise void too (generic, not camera-only):
/// applying a transform must reach the noise shader and pan/scale the field,
/// producing a visibly different render. Regression for the inverse-affine
/// uniform plumbing in `Noise::set_transform` / `noise.wgsl`.
#[test]
fn noise_void_transform_changes_output() {
    use darkly::transform::Transform;
    let mut engine = test_engine(64, 64);
    let mut params = noise_defaults(&engine);
    set_float_param(&mut params, "size", 40.0); // visible structure
    let id = engine
        .add_void_layer("noise", params, None)
        .expect("noise void should be addable");

    let frame_a = engine.test_readback_canvas();

    // Scale the field ×2 about the origin: a clearly different cross-section.
    engine.update_void_transform(id, Transform::from_affine([2.0, 0.0, 0.0, 0.0, 2.0, 0.0]));
    let frame_b = engine.test_readback_canvas();

    assert_eq!(frame_a.len(), frame_b.len(), "readbacks must match size");
    assert_ne!(
        frame_a, frame_b,
        "transforming a noise void must scale/pan the field in the output",
    );
}

/// Regression: straight-alpha storage + linear filtering darkened every alpha
/// edge in video-stream voids (docs/lessons-learned/compositing-lessons-learned.md
/// #2, the dark-halo bug reported on the Blender void). The aux frame texture
/// must hold **premultiplied** texels so the hardware bilinear filter
/// interpolates correctly, and the shader must un-premultiply after sampling so
/// the void still emits the straight alpha the compositor expects.
///
/// A 4×4 frame is planted through the load path (`restore_void_pixels`), whose
/// bytes follow the aux texture's premultiplied convention. Cover-fit stretches
/// it over the 64×64 canvas (16× per axis), so nearly every output pixel is a
/// filtered blend of adjacent texels:
///
/// - Columns 0-1 opaque red `(255,0,0,255)`, columns 2-3 transparent
///   `(0,0,0,0)` (byte-identical in both alpha conventions, so the fringe
///   assertion pins filtering behavior regardless of injection convention).
/// - Texel (1,1) is premultiplied half-alpha red `(128,0,0,128)` (straight
///   `(255,0,0)` at α=128). Output pixels near its center sample only
///   columns 0-1, away from the transparent boundary.
#[test]
fn video_stream_void_filtering_keeps_straight_color_at_alpha_edges() {
    let mut engine = test_engine(64, 64);
    let defaults: Vec<ParamValue> = engine
        .void_param_defs("blender")
        .iter()
        .map(darkly::gpu::params::ParamDef::default_value)
        .collect();
    let id = engine
        .add_void_layer("blender", defaults, None)
        .expect("blender void should be addable");

    let (w, h) = (4u32, 4u32);
    let mut frame = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w / 2 {
            let i = ((y * w + x) * 4) as usize;
            frame[i..i + 4].copy_from_slice(&[255, 0, 0, 255]);
        }
    }
    let semi = ((w + 1) * 4) as usize; // texel (1,1)
    frame[semi..semi + 4].copy_from_slice(&[128, 0, 0, 128]);
    engine.test_plant_void_frame(id, w, h, &frame);

    // Force a render so the void encodes the planted frame, then read the
    // void's canvas-sized straight-alpha output.
    let _ = engine.test_readback_canvas();
    let out = engine.test_readback_layer(id);
    assert_eq!(
        out.len(),
        64 * 64 * 4,
        "layer readback should be canvas-sized"
    );
    let px = |x: usize, y: usize| -> [u8; 4] {
        let i = (y * 64 + x) * 4;
        [out[i], out[i + 1], out[i + 2], out[i + 3]]
    };

    // Sanity: deep inside each half the output is exact.
    assert_eq!(
        px(8, 8),
        [255, 0, 0, 255],
        "opaque interior stays solid red"
    );
    assert_eq!(px(55, 8), [0, 0, 0, 0], "transparent interior stays empty");

    // Assertion A (edge fringe): pixel (31, 55) maps to source texel coords
    // (~1.47, ~2.97): a near-even filter blend of opaque-red column 1 and
    // transparent column 2, in plain rows away from the semi texel. The
    // straight color must stay full-brightness red; unfixed straight-alpha
    // filtering yields r ≈ a (a dark halo).
    let edge = px(31, 55);
    assert!(
        edge[3] > 64 && edge[3] < 192,
        "edge pixel should have mid-range alpha, got {edge:?}",
    );
    assert!(
        edge[0] >= 240,
        "filtered alpha edge must keep straight red at full brightness \
         (premultiplied filtering); got {edge:?}, a dark fringe",
    );

    // Assertion B (storage convention): pixel (23, 23) maps to (~0.97, ~0.97),
    // dominated by the semi texel with the remainder from its opaque-red
    // neighbors (transparent columns contribute nothing). The premultiplied
    // bytes `(128,0,0,128)` must read back as straight `(255,0,0)` at α≈136.
    // This pins the premultiplied-storage half of the fix: with straight
    // storage plus an un-premultiply divide, assertion A alone would still
    // pass while semi-transparent content breaks.
    let semi_px = px(23, 23);
    assert!(
        semi_px[3] > 110 && semi_px[3] < 160,
        "semi-transparent texel should read back α≈136, got {semi_px:?}",
    );
    assert!(
        semi_px[0] >= 240,
        "premultiplied semi-transparent red must un-premultiply to full \
         brightness; got {semi_px:?}",
    );
}

/// Regression: the camera void's persistent frame (its last received webcam
/// frame) must survive save → load. The save flow reads the void's aux
/// texture back with `copy_texture_to_buffer`, which requires the texture to
/// carry `COPY_SRC`; the frame texture was created without it, so the readback
/// was a wgpu validation error and the frame silently never reached the
/// `.darkly`; the layer reopened black. This plants a known frame through
/// `restore_void_pixels` (the load entry point) and reads it back exactly the
/// way `queue_pixel_readback` does at save time.
#[test]
fn camera_void_frame_round_trips_through_save_readback() {
    let mut engine = test_engine(64, 64);
    let cam = engine
        .add_void_layer("camera", camera_defaults(&engine), None)
        .expect("camera void should be addable");

    // A known 4×4 RGBA pattern stands in for a captured webcam frame.
    let (w, h) = (4u32, 4u32);
    let known: Vec<u8> = (0..(w * h))
        .flat_map(|i| {
            let v = (i * 16) as u8;
            [v, 255 - v, 128, 255]
        })
        .collect();

    engine.test_plant_void_frame(cam, w, h, &known);

    let read_back = engine
        .test_readback_void_frame(cam)
        .expect("camera void with a planted frame must surface pixel data for save");
    assert_eq!(
        read_back, known,
        "the camera void's persistent frame must read back byte-for-byte through the save path",
    );
}

// ---------------------------------------------------------------------------
// Persistent-source regressions.
//
// A void that declares a persistent frame owns bulk pixel data that is not
// regenerable from its params. Three engine paths predate that fact and treat
// every void as purely procedural. Each test below plants a known frame on a
// camera void and asserts on the bytes.
// ---------------------------------------------------------------------------

/// A 4×4 RGBA pattern standing in for captured external input.
fn known_frame() -> (u32, u32, Vec<u8>) {
    let (w, h) = (4u32, 4u32);
    let bytes = (0..(w * h))
        .flat_map(|i| {
            let v = (i * 16) as u8;
            [v, 255 - v, 128, 255]
        })
        .collect();
    (w, h, bytes)
}

/// Duplicating a void must carry its persistent source. `duplicate_node` copies
/// `void_type` / `params` / `transform` on the assumption that a void's output
/// is reproducible from its params (false for a void holding externally
/// sourced pixels, which leaves the duplicate blank).
#[test]
fn duplicating_a_void_carries_its_persistent_frame() {
    let mut engine = test_engine(64, 64);
    let cam = engine
        .add_void_layer("camera", camera_defaults(&engine), None)
        .expect("camera void should be addable");

    let (w, h, known) = known_frame();
    engine.test_plant_void_frame(cam, w, h, &known);

    let dup = engine
        .duplicate_node(cam)
        .expect("a void layer should be duplicable");
    assert_ne!(dup, cam, "duplicate must be a distinct node");

    let copied = engine.test_readback_void_frame(dup).expect(
        "the duplicate of a void with a persistent frame must itself declare \
         one, otherwise the copy renders blank",
    );
    assert_eq!(
        copied, known,
        "the duplicate's source must be byte-identical to the original's",
    );
}

/// A removed void's source texture must be reclaimed once its undo entry is
/// evicted. `collect_pixel_node_ids` skips voids on the grounds that their
/// output is regenerable, so the tombstone never names the node and the
/// cache (holding the full-resolution source) outlives the undo entry
/// forever.
#[test]
fn evicting_a_removed_void_disposes_its_source() {
    let mut engine = test_engine(64, 64);
    // A second layer, so deleting the void isn't refused as "the last layer".
    engine.add_group(None);
    let cam = engine
        .add_void_layer("camera", camera_defaults(&engine), None)
        .expect("camera void should be addable");

    let (w, h, known) = known_frame();
    engine.test_plant_void_frame(cam, w, h, &known);
    assert!(
        engine.test_readback_void_frame(cam).is_some(),
        "precondition: the planted frame is readable before removal",
    );

    engine
        .remove_layer(cam)
        .expect("the void should be removable");

    // The engine's undo stack caps at 50 steps; push past it so the removal's
    // action is evicted and `on_evict` runs.
    for _ in 0..51 {
        engine.add_group(None);
    }

    assert!(
        engine.test_readback_void_frame(cam).is_none(),
        "once the removal is no longer undoable the void's source texture must \
         be disposed, not retained for the lifetime of the document",
    );
}

/// A void's sampling uniform must track canvas resizes. `content_extent` (the
/// gizmo bbox) is computed from the *live* canvas rect while the shader's
/// uniform is written once at `create_cache` from the canvas dims of that
/// moment, so after a resize the two disagree: the bbox covers the new canvas
/// while the sampler still maps the source across the old one.
#[test]
fn void_uniform_tracks_canvas_resize() {
    use darkly::coord::CanvasRect;
    let mut engine = test_engine(64, 64);
    let cam = engine
        .add_void_layer("camera", camera_defaults(&engine), None)
        .expect("camera void should be addable");

    // Drop the camera's seeded selfie flip, which mirrors about the canvas
    // width at creation time. That transform is document state and must not
    // silently follow a resize, so leaving it in would confound this test with
    // a second, unrelated effect.
    engine.update_void_transform(cam, darkly::transform::Transform::identity());

    // A fully opaque square source. Cover-fit of a 1:1 source into a 1:1
    // canvas is the whole canvas, so every canvas pixel must be opaque,
    // at whatever size the canvas currently is.
    let (w, h) = (8u32, 8u32);
    let opaque: Vec<u8> = std::iter::repeat_n([255u8, 0, 0, 255], (w * h) as usize)
        .flatten()
        .collect();
    engine.test_plant_void_frame(cam, w, h, &opaque);

    let before = engine.test_readback_canvas();
    let px = |buf: &[u8], x: u32, y: u32, stride: u32| {
        let i = ((y * stride + x) * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    };
    assert_eq!(
        px(&before, 32, 32, 64)[3],
        255,
        "precondition: the source covers the 64×64 canvas",
    );

    engine.resize_canvas(CanvasRect::from_xywh(0, 0, 128, 128));
    let after = engine.test_readback_canvas();

    assert_eq!(
        px(&after, 100, 100, 128)[3],
        255,
        "after growing the canvas the source must still cover it; a stale \
         uniform leaves everything outside the old canvas bounds transparent",
    );
}
