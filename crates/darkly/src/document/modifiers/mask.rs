//! Mask modifier — multiplies a host's alpha by an R8 alpha texture.
//!
//! Per the Modularity Principle in [AGENTS.md], the entire mask kind lives in
//! this file: data struct, construction, wire format, and the `register()`
//! discovery hook.

use serde::{Deserialize, Serialize};

use crate::coord::CanvasRect;
use crate::document::layer_kind::{IdMap, PixelBlobSpec, SerializedEntity};
use crate::document::modifier::{Modifier, ModifierKind, ModifierRegistration};
use crate::format::error::LoadError;
use crate::format::manifest::{texture_format_to_str, ManifestPixelRef};
use crate::layer::{LayerId, NodeCommon, PixelBuffer};

pub struct MaskModifier {
    pub pixels: PixelBuffer,
}

impl MaskModifier {
    pub fn new(bounds: CanvasRect) -> Self {
        MaskModifier {
            pixels: PixelBuffer::new(bounds, wgpu::TextureFormat::R8Unorm),
        }
    }
}

/// Stable wire-format identifier. Owned by this module so dispatch sites
/// (notably `Modifier::kind`) reference the same constant the registration
/// uses — no parallel string literal anywhere.
pub const TYPE_ID: &str = "mask";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct MaskBody {
    name: String,
    visible: bool,
    locked: bool,
    pixels: ManifestPixelRef,
}

pub fn register() -> ModifierRegistration {
    ModifierRegistration {
        type_id: TYPE_ID,
        display_name: "Mask",
        serialize,
        deserialize,
        remap_ids,
    }
}

fn serialize(modifier: &Modifier) -> SerializedEntity {
    let mask = match &modifier.kind {
        ModifierKind::Mask(m) => m,
        _ => panic!("mask::serialize received non-mask Modifier"),
    };
    let blob_path = format!("layers/{}.mask.pixels", modifier.id.to_ffi());
    let pixels = ManifestPixelRef {
        format: texture_format_to_str(mask.pixels.format).to_string(),
        pixels: blob_path.clone(),
        bounds: mask.pixels.bounds,
    };
    let body = MaskBody {
        name: modifier.common.name.clone(),
        visible: modifier.common.visible,
        locked: modifier.common.locked,
        pixels: pixels.clone(),
    };
    SerializedEntity {
        body: serde_json::to_value(&body).expect("derived serde for MaskBody is infallible"),
        pixel_blobs: vec![PixelBlobSpec {
            blob_key: blob_path,
            source_node_id: modifier.id,
            pixels,
        }],
    }
}

fn deserialize(body: &serde_json::Value, id: LayerId) -> Result<Modifier, LoadError> {
    let body: MaskBody =
        serde_json::from_value(body.clone()).map_err(|e| LoadError::CorruptManifest {
            reason: format!("mask body: {e}"),
        })?;
    Ok(Modifier {
        id,
        common: NodeCommon {
            name: body.name,
            visible: body.visible,
            locked: body.locked,
        },
        kind: ModifierKind::mask_with_bounds(body.pixels.bounds),
    })
}

fn remap_ids(_modifier: &mut Modifier, _id_map: &IdMap) {
    // Mask modifiers carry no cross-references — the host pointer is the
    // document's `parent` map, populated separately by the loader from
    // the host's `modifiers: Vec<u64>` list.
}
