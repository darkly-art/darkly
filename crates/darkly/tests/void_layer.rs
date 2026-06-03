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
    let engine = test_engine(64, 64);
    let types: Vec<_> = engine.void_types().into_iter().map(|t| t.type_id).collect();
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
    // Default layer names: the panel should show "Noise 1", "Noise 2", … —
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
/// vanishes on the next dirty tick or stays only because the user hasn't
/// touched a slider yet — both are confusing and neither is the user's
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

    // One undo restores ALL the way back to the original schema defaults —
    // the three edits coalesced.
    engine.undo();
    let restored = void_params_via_tree(&engine, id);
    assert_eq!(
        restored.first().cloned(),
        defaults.first().cloned(),
        "undo should roll back to pre-drag seed, not the intermediate one",
    );
}

/// The `time` slider must actually scrub the 3D noise field — changing it
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

/// The `time` slider must scrub *continuously* through the 3D noise volume —
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
    // ~0.075 along the noise Z-axis — well under one cell-cross, so deltas
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
        "only {changed_bytes}/{total_bytes} bytes changed — `time` may not \
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
/// void's params must not cause an unrelated void to re-encode — the
/// compositor must not flip dirty for every procedural layer on every
/// param edit. Verified by reading back two voids' textures, mutating
/// just one, and confirming the *un-mutated* one's pixels are byte-equal.
#[test]
fn void_dirty_is_per_instance() {
    let mut engine = test_engine(64, 64);
    let mut params_a = noise_defaults(&engine);
    let mut params_b = noise_defaults(&engine);
    // Different seeds so the two voids' textures are visibly distinct —
    // helps catch any accidental aliasing in addition to the dirty-bit
    // check.
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

    // Mutate only A. B must not re-encode — its dirty bit was cleared
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
        "untouched void must not re-encode — its dirty bit is its own state",
    );
}

fn set_int_param(params: &mut [ParamValue], name: &str, value: i32) {
    let engine = test_engine(1, 1);
    let defs = engine.void_param_defs("noise");
    let idx = defs
        .iter()
        .position(|d| match d {
            darkly::gpu::params::ParamDef::Int { name: n, .. } => *n == name,
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
        .position(|d| match d {
            darkly::gpu::params::ParamDef::Float { name: n, .. } => *n == name,
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
