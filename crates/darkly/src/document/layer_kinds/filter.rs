//! Filter layer kind — non-destructive procedural-transform node in the layer
//! tree.
//!
//! A filter layer carries no pixel buffer: its entire state is a `pipeline` id
//! (which [`crate::gpu::filter::FilterPipelineRegistry`] transform to run) plus
//! that transform's parameter values, so the whole layer round-trips through
//! the manifest body — there are no pixel blobs to read back, exactly like a
//! procedural [`void`](crate::document::layer_kinds::void).
//!
//! The pipeline id and parameter values are validated against the
//! [`FilterPipelineRegistry`](crate::gpu::filter::FilterPipelineRegistry) at the
//! engine layer when a filter layer is added; an unknown id in a save file
//! surfaces as a [`LoadError::CorruptManifest`] rather than a silent fallback.

use serde::{Deserialize, Serialize};

use crate::document::layer_kind::{IdMap, LayerKindRegistration, SerializedEntity};
use crate::format::error::LoadError;
use crate::gpu::blend_mode;
use crate::gpu::params::ParamValue;
use crate::layer::{BlendProps, FilterLayer, Layer, LayerId, LayerNode, NodeCommon};

pub const TYPE_ID: &str = "filter";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FilterBody {
    name: String,
    visible: bool,
    locked: bool,
    opacity: f32,
    blend_mode: String,
    /// Stable `type_id` from [`crate::gpu::filter::FilterPipelineRegistry`],
    /// e.g. `"invert"`. Anchors the param vector — a load that doesn't
    /// recognize this id is rejected by the engine as `CorruptManifest`.
    pipeline: String,
    /// Parameter values in the order the filter pipeline's schema declares
    /// them. Empty for parameter-free filters like invert.
    params: Vec<ParamValue>,
    #[serde(default)]
    filters: Vec<u64>,
}

pub fn register() -> LayerKindRegistration {
    LayerKindRegistration {
        type_id: TYPE_ID,
        display_name: "Filter Layer",
        description: "A color adjustment applied to everything composited beneath it.",
        can_have_mask: true,
        can_rename: true,
        has_thumbnail: false,
        icon: "fa6-solid:circle-half-stroke",
        serialize,
        deserialize,
        remap_ids,
    }
}

fn serialize(node: &LayerNode) -> SerializedEntity {
    let f = match node {
        LayerNode::Layer(Layer::Filter(f)) => f,
        _ => panic!("filter::serialize received non-filter LayerNode"),
    };
    let body = FilterBody {
        name: f.common.name.clone(),
        visible: f.common.visible,
        locked: f.common.locked,
        opacity: f.blend.opacity,
        blend_mode: f.blend.blend_mode.type_id.to_string(),
        pipeline: f.pipeline.clone(),
        params: f.params.clone(),
        filters: f.filters.iter().map(|m| m.to_ffi()).collect(),
    };
    SerializedEntity {
        body: serde_json::to_value(&body).expect("derived serde for FilterBody is infallible"),
        pixel_blobs: Vec::new(),
    }
}

fn deserialize(body: &serde_json::Value, id: LayerId) -> Result<LayerNode, LoadError> {
    let body: FilterBody =
        serde_json::from_value(body.clone()).map_err(|e| LoadError::CorruptManifest {
            reason: format!("filter body: {e}"),
        })?;
    let blend_reg = blend_mode::registry()
        .get(&body.blend_mode)
        .ok_or_else(|| LoadError::CorruptManifest {
            reason: format!(
                "filter {} references undeclared blend_mode/{}",
                id.to_ffi(),
                body.blend_mode
            ),
        })?;
    Ok(LayerNode::Layer(Layer::Filter(FilterLayer {
        id,
        common: NodeCommon {
            name: body.name,
            visible: body.visible,
            locked: body.locked,
        },
        blend: BlendProps {
            opacity: body.opacity,
            blend_mode: blend_reg,
        },
        pipeline: body.pipeline,
        params: body.params,
        filters: body.filters.into_iter().map(LayerId::from_ffi).collect(),
    })))
}

fn remap_ids(node: &mut LayerNode, id_map: &IdMap) {
    let LayerNode::Layer(Layer::Filter(f)) = node else {
        panic!("filter::remap_ids received non-filter LayerNode");
    };
    for m in f.filters.iter_mut() {
        let old_ffi = m.to_ffi();
        if let Some(new_id) = id_map.get(&old_ffi) {
            *m = *new_id;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;

    /// Round-trip a filter layer through its registered serializer +
    /// deserializer. Like a procedural void, there are NO pixel blobs to fall
    /// back on — `pipeline` + `params` are the whole document state.
    #[test]
    fn filter_body_round_trips_through_registration() {
        let mut doc = Document::new(64, 64);
        let id = doc.add_filter_layer("invert".to_string(), "Invert Colors", Vec::new(), None);

        let reg = register();
        let node = doc.find_node(id).expect("filter exists");

        let serialized = (reg.serialize)(node);
        assert!(
            serialized.pixel_blobs.is_empty(),
            "filter layers must declare no pixel blobs",
        );

        let restored = (reg.deserialize)(&serialized.body, id).expect("deserialize must succeed");
        let f_after = match &restored {
            LayerNode::Layer(Layer::Filter(f)) => f,
            _ => panic!("deserialize must yield a Filter layer"),
        };
        assert_eq!(f_after.pipeline, "invert");
        assert!(f_after.params.is_empty());
    }

    /// A parametric filter (curves) round-trips its full param vector — the
    /// eight per-channel curves are the whole document state, exactly like a
    /// void's params. Regression against dropping/reordering the curves on save.
    #[test]
    fn curves_body_round_trips_with_params() {
        use crate::gpu::params::ParamValue;

        let mut doc = Document::new(64, 64);
        // One entry per Krita channel: RGB, R, G, B, A, Hue, Saturation,
        // Lightness — a mix of identity and non-identity curves.
        let params = vec![
            ParamValue::Curve(vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]]),
            ParamValue::Curve(vec![[0.0, 0.1], [1.0, 0.9]]),
            ParamValue::Curve(vec![[0.0, 0.0], [1.0, 1.0]]),
            ParamValue::Curve(vec![[0.0, 0.0], [1.0, 0.5]]),
            ParamValue::Curve(vec![[0.0, 0.0], [1.0, 1.0]]),
            ParamValue::Curve(vec![[0.0, 0.0], [0.5, 0.4], [1.0, 1.0]]),
            ParamValue::Curve(vec![[0.0, 0.2], [1.0, 0.8]]),
            ParamValue::Curve(vec![[0.0, 0.0], [1.0, 0.9]]),
        ];
        let id = doc.add_filter_layer("curves".to_string(), "Curves", params.clone(), None);

        let reg = register();
        let node = doc.find_node(id).expect("filter exists");
        let serialized = (reg.serialize)(node);
        let restored = (reg.deserialize)(&serialized.body, id).expect("deserialize must succeed");
        let f_after = match &restored {
            LayerNode::Layer(Layer::Filter(f)) => f,
            _ => panic!("deserialize must yield a Filter layer"),
        };
        assert_eq!(f_after.pipeline, "curves");
        assert_eq!(
            f_after.params, params,
            "all eight curve params must survive the round-trip in order"
        );
    }

    /// A chromatic-aberration filter layer round-trips its `List` params (the
    /// new list-of-groups kind), and an emptied list survives the benign
    /// `List([]) → Curve([])` degradation as a passthrough (`count == 0`).
    #[test]
    fn chromatic_aberration_list_params_round_trip() {
        use crate::gpu::filters::chromatic_aberration::{pack_uniform, PARAMS};

        let mut doc = Document::new(64, 64);
        let params: Vec<ParamValue> = PARAMS.iter().map(|d| d.default_value()).collect();
        let id = doc.add_filter_layer(
            "chromatic_aberration".to_string(),
            "CA",
            params.clone(),
            None,
        );

        let reg = register();
        let node = doc.find_node(id).expect("filter exists");
        let serialized = (reg.serialize)(node);
        let restored = (reg.deserialize)(&serialized.body, id).expect("deserialize must succeed");
        let f = match &restored {
            LayerNode::Layer(Layer::Filter(f)) => f,
            _ => panic!("deserialize must yield a Filter layer"),
        };
        assert_eq!(f.pipeline, "chromatic_aberration");
        assert_eq!(
            f.params, params,
            "CA list params must survive the round-trip"
        );

        // Emptied list: serializes as `[]`, degrades to `Curve([])` on reload,
        // but `pack_uniform` treats any non-List as empty → passthrough.
        let empty = doc.add_filter_layer(
            "chromatic_aberration".to_string(),
            "CA2",
            vec![ParamValue::List(vec![])],
            None,
        );
        let node = doc.find_node(empty).expect("filter exists");
        let serialized = (reg.serialize)(node);
        let restored =
            (reg.deserialize)(&serialized.body, empty).expect("deserialize must succeed");
        let f = match &restored {
            LayerNode::Layer(Layer::Filter(f)) => f,
            _ => panic!("deserialize must yield a Filter layer"),
        };
        assert_eq!(
            f.params,
            vec![ParamValue::Curve(vec![])],
            "an emptied list degrades to an empty curve on the def-less document path"
        );
        assert_eq!(
            pack_uniform(&f.params).count,
            0,
            "the degraded param still packs as a passthrough"
        );
    }

    /// A corrupt blend_mode in the saved body must surface as
    /// `CorruptManifest`, not a silent fallback — the same contract every
    /// other layer kind holds.
    #[test]
    fn unknown_blend_mode_in_body_returns_corrupt_manifest() {
        let reg = register();
        let body = serde_json::json!({
            "name": "broken",
            "visible": true,
            "locked": false,
            "opacity": 1.0,
            "blend_mode": "definitely-not-real",
            "pipeline": "invert",
            "params": [],
            "filters": []
        });
        let id = Document::new(8, 8).root_id();
        let err = (reg.deserialize)(&body, id);
        assert!(
            err.is_err(),
            "unknown blend_mode must reject the load, not fall through"
        );
    }
}
