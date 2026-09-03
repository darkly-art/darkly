//! The layer DTO must carry each kind's capability flags so the frontend can
//! gate "Add mask" / rename / thumbnail rendering without branching on the
//! `type` string. Regression guard for the bug where two Svelte files computed
//! "can this layer take a mask?" from *different* hardcoded kind sets while the
//! engine actually permitted masks on all four kinds.

use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::params::ParamDef;
use darkly::gpu::test_utils::test_device;
use serde_json::Value;

fn test_engine() -> DarklyEngine {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    DarklyEngine::new(gpu, 64, 64)
}

/// Serialize the layer tree and index each top-level node by its `type` tag.
fn dto_by_type(engine: &DarklyEngine) -> std::collections::HashMap<String, Value> {
    let tree = serde_json::to_value(engine.layer_tree()).expect("layer_tree serializes");
    tree.as_array()
        .expect("layer tree is an array")
        .iter()
        .map(|n| {
            let ty = n.get("type").and_then(Value::as_str).unwrap().to_string();
            (ty, n.clone())
        })
        .collect()
}

#[test]
fn layer_dto_carries_capability_flags_per_kind() {
    let mut engine = test_engine();

    engine.add_raster_layer(None);
    let noise_params: Vec<_> = engine
        .void_param_defs("noise")
        .iter()
        .map(ParamDef::default_value)
        .collect();
    engine
        .add_void_layer("noise", noise_params, None)
        .expect("noise void addable");
    engine
        .add_filter_layer("invert", vec![], None)
        .expect("invert filter addable");
    engine.add_group(None);

    let by_type = dto_by_type(&engine);
    for kind in ["raster", "void", "filter", "group"] {
        let dto = by_type
            .get(kind)
            .unwrap_or_else(|| panic!("{kind} layer present in tree"));

        // All four kinds are maskable and renamable (product decision +
        // matches the engine's actual mask-attachment behavior).
        assert_eq!(dto["canHaveMask"], Value::Bool(true), "{kind} canHaveMask");
        assert_eq!(dto["canRename"], Value::Bool(true), "{kind} canRename");

        // Only raster renders a live pixel thumbnail; the rest fall back to a
        // static icon, which must be non-empty so the panel has something to
        // draw.
        let has_thumb = dto["hasThumbnail"].as_bool().unwrap();
        let icon = dto["icon"].as_str().unwrap();
        assert_eq!(has_thumb, kind == "raster", "{kind} hasThumbnail");
        if !has_thumb {
            assert!(!icon.is_empty(), "{kind} must declare a panel icon");
        }

        // Tooltip label.
        assert!(
            !dto["kindName"].as_str().unwrap().is_empty(),
            "{kind} kindName",
        );
    }

    // The original bug: raster and group disagreed on mask capability across
    // the two frontend files. One source of truth now; they must agree.
    assert_eq!(
        by_type["raster"]["canHaveMask"], by_type["group"]["canHaveMask"],
        "raster and group must agree on mask capability",
    );
}
