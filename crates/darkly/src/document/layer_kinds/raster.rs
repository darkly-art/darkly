//! Raster layer kind — pixel-storing leaf in the layer tree.
//!
//! Per the Modularity Principle in [AGENTS.md], the entire raster kind
//! lives in this file: data lives on [`crate::layer::RasterLayer`], and
//! the wire format (`RasterBody`) plus serializer / deserializer /
//! id-remap functions all live here. Adding a new layer kind copies
//! this file's shape — no edits to save.rs, load.rs, or manifest.rs.

use serde::{Deserialize, Serialize};

use crate::document::layer_kind::{IdMap, LayerKindRegistration, PixelBlobSpec, SerializedEntity};
use crate::format::error::LoadError;
use crate::format::manifest::{texture_format_from_str, texture_format_to_str, ManifestPixelRef};
use crate::gpu::blend_mode;
use crate::layer::{BlendProps, Layer, LayerId, LayerNode, NodeCommon, PixelBuffer, RasterLayer};

/// Stable wire-format identifier. Owned by this module so dispatch sites
/// (notably `LayerNode::kind`) reference the same constant the registration
/// uses — no parallel string literal anywhere.
pub const TYPE_ID: &str = "raster";

/// On-disk shape for a raster layer. The central save/load code never
/// names this struct — it crosses the wire as `serde_json::Value` inside
/// a [`crate::format::manifest::ManifestEntry`] envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct RasterBody {
    name: String,
    visible: bool,
    locked: bool,
    opacity: f32,
    /// Blend-mode `type_id` from [`crate::gpu::blend_mode`].
    blend_mode: String,
    /// Pixel storage descriptor — bounds + format + zip-relative path.
    pixels: ManifestPixelRef,
    /// Modifier ids attached to this layer, in bottom-up order. Each id
    /// resolves to an entry in [`crate::format::manifest::Manifest::modifiers`].
    #[serde(default)]
    modifiers: Vec<u64>,
}

pub fn register() -> LayerKindRegistration {
    LayerKindRegistration {
        type_id: TYPE_ID,
        display_name: "Raster Layer",
        serialize,
        deserialize,
        remap_ids,
    }
}

fn serialize(node: &LayerNode) -> SerializedEntity {
    let r = match node {
        LayerNode::Layer(Layer::Raster(r)) => r,
        _ => panic!("raster::serialize received non-raster LayerNode"),
    };
    let blob_path = format!("layers/{}.pixels", r.id.to_ffi());
    let pixels = ManifestPixelRef {
        format: texture_format_to_str(r.pixels.format).to_string(),
        pixels: blob_path.clone(),
        bounds: r.pixels.bounds,
    };
    let body = RasterBody {
        name: r.common.name.clone(),
        visible: r.common.visible,
        locked: r.common.locked,
        opacity: r.blend.opacity,
        blend_mode: r.blend.blend_mode.type_id.to_string(),
        pixels: pixels.clone(),
        modifiers: r.modifiers.iter().map(|m| m.to_ffi()).collect(),
    };
    SerializedEntity {
        body: serde_json::to_value(&body).expect("derived serde for RasterBody is infallible"),
        pixel_blobs: vec![PixelBlobSpec {
            blob_key: blob_path,
            source_node_id: r.id,
            pixels,
        }],
    }
}

fn deserialize(body: &serde_json::Value, id: LayerId) -> Result<LayerNode, LoadError> {
    let body: RasterBody =
        serde_json::from_value(body.clone()).map_err(|e| LoadError::CorruptManifest {
            reason: format!("raster body: {e}"),
        })?;
    let blend_reg = blend_mode::registry()
        .get(&body.blend_mode)
        .ok_or_else(|| LoadError::CorruptManifest {
            reason: format!(
                "raster {} references undeclared blend_mode/{} \
                 — `requires` block lies",
                id.to_ffi(),
                body.blend_mode
            ),
        })?;
    let format =
        texture_format_from_str(&body.pixels.format).ok_or_else(|| LoadError::CorruptManifest {
            reason: format!(
                "raster {} uses unknown texture format '{}'",
                id.to_ffi(),
                body.pixels.format
            ),
        })?;
    Ok(LayerNode::Layer(Layer::Raster(RasterLayer {
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
        pixels: PixelBuffer::new(body.pixels.bounds, format),
        // Modifier ids on the body still carry manifest-old values; the
        // load's second pass calls `remap_ids` to rewrite them. Insert
        // them as-is so the second pass has something to map.
        modifiers: body.modifiers.into_iter().map(LayerId::from_ffi).collect(),
    })))
}

fn remap_ids(node: &mut LayerNode, id_map: &IdMap) {
    let LayerNode::Layer(Layer::Raster(r)) = node else {
        panic!("raster::remap_ids received non-raster LayerNode");
    };
    for m in r.modifiers.iter_mut() {
        let old_ffi = m.to_ffi();
        if let Some(new_id) = id_map.get(&old_ffi) {
            *m = *new_id;
        }
    }
}
