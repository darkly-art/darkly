//! Mask filter — multiplies a host's alpha by an R8 alpha texture.
//!
//! Per the Modularity Principle in [AGENTS.md], the entire mask kind lives in
//! this file: data struct, construction, wire format, and the `register()`
//! discovery hook.

use serde::{Deserialize, Serialize};

use crate::coord::CanvasRect;
use crate::document::filter::{Filter, FilterEntityRegistration, FilterKind};
use crate::document::layer_kind::{IdMap, PixelBlobSpec, SerializedEntity};
use crate::format::error::LoadError;
use crate::format::manifest::{texture_format_to_str, ManifestPixelRef};
use crate::layer::{LayerId, NodeCommon, PixelBuffer};

pub struct MaskFilter {
    pub pixels: PixelBuffer,
    pub linked_to_host: bool,
}

impl MaskFilter {
    pub fn new(bounds: CanvasRect) -> Self {
        MaskFilter {
            pixels: PixelBuffer::new(bounds, wgpu::TextureFormat::R8Unorm),
            linked_to_host: true,
        }
    }
}

/// Stable wire-format identifier. Owned by this module so dispatch sites
/// (notably `Filter::kind`) reference the same constant the registration
/// uses — no parallel string literal anywhere.
pub const TYPE_ID: &str = "mask";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct MaskBody {
    name: String,
    visible: bool,
    locked: bool,
    linked_to_host: bool,
    pixels: ManifestPixelRef,
}

pub fn register() -> FilterEntityRegistration {
    FilterEntityRegistration {
        type_id: TYPE_ID,
        display_name: "Mask",
        serialize,
        deserialize,
        remap_ids,
    }
}

fn serialize(filter: &Filter) -> SerializedEntity {
    let mask = match &filter.kind {
        FilterKind::Mask(m) => m,
        _ => panic!("mask::serialize received non-mask Filter"),
    };
    let blob_path = format!("layers/{}.mask.pixels", filter.id.to_ffi());
    let pixels = ManifestPixelRef {
        format: texture_format_to_str(mask.pixels.format).to_string(),
        pixels: blob_path.clone(),
        bounds: mask.pixels.bounds,
    };
    let body = MaskBody {
        name: filter.common.name.clone(),
        visible: filter.common.visible,
        locked: filter.common.locked,
        linked_to_host: mask.linked_to_host,
        pixels: pixels.clone(),
    };
    SerializedEntity {
        body: serde_json::to_value(&body).expect("derived serde for MaskBody is infallible"),
        pixel_blobs: vec![PixelBlobSpec {
            blob_key: blob_path,
            source_node_id: filter.id,
            pixels,
        }],
    }
}

fn deserialize(body: &serde_json::Value, id: LayerId) -> Result<Filter, LoadError> {
    let body: MaskBody =
        serde_json::from_value(body.clone()).map_err(|e| LoadError::CorruptManifest {
            reason: format!("mask body: {e}"),
        })?;
    Ok(Filter {
        id,
        common: NodeCommon {
            name: body.name,
            visible: body.visible,
            locked: body.locked,
        },
        kind: FilterKind::Mask(MaskFilter {
            pixels: PixelBuffer::new(body.pixels.bounds, wgpu::TextureFormat::R8Unorm),
            linked_to_host: body.linked_to_host,
        }),
    })
}

fn remap_ids(_modifier: &mut Filter, _id_map: &IdMap) {
    // Mask filters carry no cross-references — the host pointer is the
    // document's `parent` map, populated separately by the loader from
    // the host's `filters: Vec<u64>` list.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coord::CanvasPoint;

    fn filter(linked_to_host: bool) -> Filter {
        let mut mask = MaskFilter::new(CanvasRect::new(CanvasPoint::new(3, -2), 8, 6));
        mask.linked_to_host = linked_to_host;
        Filter {
            id: LayerId::from_ffi(7),
            common: NodeCommon {
                name: "Mask".into(),
                visible: true,
                locked: false,
            },
            kind: FilterKind::Mask(mask),
        }
    }

    #[test]
    fn link_state_round_trips_through_mask_body() {
        let serialized = serialize(&filter(false));
        let loaded = deserialize(&serialized.body, LayerId::from_ffi(9)).unwrap();
        let FilterKind::Mask(mask) = loaded.kind else {
            panic!("loaded filter is not a mask");
        };
        assert!(!mask.linked_to_host);
    }

    #[test]
    fn missing_link_state_is_rejected() {
        let mut body = serialize(&filter(true)).body;
        body.as_object_mut().unwrap().remove("linked_to_host");
        assert!(deserialize(&body, LayerId::from_ffi(9)).is_err());
    }
}
