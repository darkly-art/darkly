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

/// With `evolution = 0.0`, the noise void must be perfectly static: ticking
/// the animation clock must not change a single pixel of its output. Guards
/// against accidentally rendering an animated frame when the user has
/// explicitly disabled morphing.
#[test]
fn noise_void_evolution_off_is_static() {
    // Pin void_divisor to 1 so ticks fire every call — keeps the test
    // independent of the default-divisor schedule.
    darkly::config::set(
        "animation.void_divisor",
        darkly::config::ConfigValue::Int(1),
    );

    let mut engine = test_engine(64, 64);
    let mut params = noise_defaults(&engine);
    // Force evolution = 0.0, warp = 1.5 so the warp path is active but
    // animation is off.
    set_float_param(&mut params, "evolution", 0.0);
    set_float_param(&mut params, "warp", 1.5);
    engine
        .add_void_layer("noise", params, None)
        .expect("noise void should be addable");

    // First tick seeds last_wall_time (dt=0 by design); the second tick has
    // a real dt and would fire the void clock — but `needs_animation()` is
    // false at evolution=0, so the void is skipped and the texture stays put.
    engine.test_tick_animations(0.1);
    let frame_a = engine.test_readback_canvas();
    engine.test_tick_animations(1.1);
    let frame_b = engine.test_readback_canvas();

    darkly::config::reset("animation.void_divisor");

    assert_eq!(
        frame_a, frame_b,
        "evolution=0 must produce byte-identical frames across animation ticks",
    );
}

/// With `evolution > 0`, the noise void must morph *continuously* —
/// neighbouring frames should differ by a small amount per pixel, never
/// teleport to an uncorrelated field. This is the regression test for the
/// 3D-FBM design: a coin-flip-mix or full-realization-swap implementation
/// would produce per-pixel deltas approaching 255 on flipped pixels and
/// fail the mean-delta bound; 3D FBM is C1-continuous in time so the
/// per-pixel delta is bounded by `~Z_SCALE * dt * evolution * fade_deriv`.
#[test]
fn noise_void_evolution_morphs_continuously() {
    darkly::config::set(
        "animation.void_divisor",
        darkly::config::ConfigValue::Int(1),
    );

    let mut engine = test_engine(64, 64);
    let mut params = noise_defaults(&engine);
    set_float_param(&mut params, "evolution", 1.0);
    set_float_param(&mut params, "warp", 1.5);
    engine
        .add_void_layer("noise", params, None)
        .expect("noise void should be addable");

    // First tick seeds last_wall_time. Second tick has dt=0.5s, which with
    // evolution=1 and Z_SCALE=0.15 advances z by ~0.075 — well under one
    // cell-cross, so deltas stay small but visible.
    engine.test_tick_animations(0.1);
    let frame_a = engine.test_readback_canvas();
    engine.test_tick_animations(0.6);
    let frame_b = engine.test_readback_canvas();

    darkly::config::reset("animation.void_divisor");

    assert_eq!(
        frame_a.len(),
        frame_b.len(),
        "readbacks must be the same size",
    );

    // Sum of absolute per-byte differences. Lets us bound both the typical
    // per-pixel change (mean) and confirm *some* change happened.
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

    // Continuity bound: 3D FBM at dt=0.5 produces mean per-byte delta in
    // single digits. Teleport would produce mean ~70 (uncorrelated FBM
    // realizations have std ~0.2 → ~50 in u8, mean abs delta ~70).
    assert!(
        mean_delta < 25.0,
        "mean per-byte delta {mean_delta:.1} exceeds continuity bound; \
         a value near 70 indicates a teleport-style implementation",
    );

    // Animation actually happened: at least 10% of bytes changed.
    let total_bytes = frame_a.len() as u32;
    assert!(
        changed_bytes * 10 >= total_bytes,
        "only {changed_bytes}/{total_bytes} bytes changed — animation isn't reaching the output",
    );

    // Sanity: max delta within plausible 3D-FBM bound. The theoretical
    // ceiling at dt=0.5 evolution=1 Z_SCALE=0.15 is ~0.14 in [0,1] ≈ 36 in
    // u8; allow 2× headroom for warp-path amplification.
    assert!(
        max_delta < 100,
        "max per-byte delta {max_delta} exceeds plausible 3D-FBM bound",
    );
}

fn set_float_param(params: &mut [ParamValue], name: &str, value: f32) {
    // Param order on the noise void: seed (Int), octaves (Int), frequency,
    // warp, color, evolution. We index by name via the engine's schema to
    // keep the test resilient to additions.
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
