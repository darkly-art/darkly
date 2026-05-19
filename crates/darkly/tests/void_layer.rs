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
