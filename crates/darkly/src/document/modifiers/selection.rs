//! Selection modifier — global single-channel mask for which pixels are
//! affected by edits (paint, fill, transform, clipboard).
//!
//! Per the Modularity Principle in [AGENTS.md], the entire selection kind
//! lives in this file: data struct, CPU cache, construction, wire format,
//! and the `register()` discovery hook.
//!
//! The selection is structurally a [`crate::document::Modifier`] but, unlike
//! per-host modifiers (mask, future filter/transform), it's attached at the
//! document root rather than on a host's `modifiers` list. That's the only
//! thing special about it — pixel storage, growth, dirty tracking, async
//! readback, and region-pixel undo all share the [`crate::layer::PixelBuffer`]
//! infrastructure, and the boolean ops sit on the same R8 paint pipeline that
//! mask painting uses.

use serde::{Deserialize, Serialize};

use crate::coord::CanvasRect;
use crate::document::layer_kind::{IdMap, PixelBlobSpec, SerializedEntity};
use crate::document::modifier::{Modifier, ModifierKind, ModifierRegistration};
use crate::format::error::LoadError;
use crate::format::manifest::{texture_format_to_str, ManifestPixelRef};
use crate::layer::{LayerId, NodeCommon, PixelBuffer};

/// CPU mirror of the selection's R8 texture, populated lazily by async
/// readback after each mutating op (combine/invert/upload). Read paths that
/// need pixel-level access (transform source bounds, copy region masking,
/// flood-fill intersection) consult this rather than triggering a synchronous
/// GPU readback (forbidden by AGENTS.md "No Blocking GPU Readbacks").
pub struct SelectionCpuCache {
    pub data: Option<Vec<u8>>,
}

impl SelectionCpuCache {
    pub fn new() -> Self {
        SelectionCpuCache { data: None }
    }

    pub fn set(&mut self, data: Vec<u8>) {
        self.data = Some(data);
    }

    pub fn invalidate(&mut self) {
        self.data = None;
    }
}

impl Default for SelectionCpuCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Pixel-bearing global selection — kind-attached at `Document.selection`.
///
/// The R8 GPU texture itself lives in the compositor's selection sub-system
/// (the boolean ops need ping-pong scratch and dedicated bind groups against
/// the brush+paint selection BGLs). [`PixelBuffer`] here holds the canvas-
/// space metadata that the document model owns: bounds, format, growth policy.
pub struct SelectionModifier {
    pub pixels: PixelBuffer,
    pub cpu_cache: SelectionCpuCache,
    /// Cached tight bounds of non-zero selection pixels in canvas coords.
    /// Set from rasterization params on `Replace`, cleared after boolean ops
    /// or invert (recomputed from the next readback when needed).
    pub pixel_bounds: Option<CanvasRect>,
}

impl SelectionModifier {
    pub fn new(bounds: CanvasRect) -> Self {
        SelectionModifier {
            pixels: PixelBuffer::new(bounds, wgpu::TextureFormat::R8Unorm),
            cpu_cache: SelectionCpuCache::new(),
            pixel_bounds: None,
        }
    }
}

pub const TYPE_ID: &str = "selection";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SelectionBody {
    name: String,
    visible: bool,
    locked: bool,
    pixels: ManifestPixelRef,
}

pub fn register() -> ModifierRegistration {
    ModifierRegistration {
        type_id: TYPE_ID,
        display_name: "Selection",
        serialize,
        deserialize,
        remap_ids,
    }
}

fn serialize(modifier: &Modifier) -> SerializedEntity {
    let sel = match &modifier.kind {
        ModifierKind::Selection(s) => s,
        _ => panic!("selection::serialize received non-selection Modifier"),
    };
    let blob_path = "selection.pixels".to_string();
    let pixels = ManifestPixelRef {
        format: texture_format_to_str(sel.pixels.format).to_string(),
        pixels: blob_path.clone(),
        bounds: sel.pixels.bounds,
    };
    let body = SelectionBody {
        name: modifier.common.name.clone(),
        visible: modifier.common.visible,
        locked: modifier.common.locked,
        pixels: pixels.clone(),
    };
    SerializedEntity {
        body: serde_json::to_value(&body).expect("derived serde for SelectionBody is infallible"),
        pixel_blobs: vec![PixelBlobSpec {
            blob_key: blob_path,
            source_node_id: modifier.id,
            pixels,
        }],
    }
}

fn deserialize(body: &serde_json::Value, id: LayerId) -> Result<Modifier, LoadError> {
    let body: SelectionBody =
        serde_json::from_value(body.clone()).map_err(|e| LoadError::CorruptManifest {
            reason: format!("selection body: {e}"),
        })?;
    Ok(Modifier {
        id,
        common: NodeCommon {
            name: body.name,
            visible: body.visible,
            locked: body.locked,
        },
        kind: ModifierKind::Selection(SelectionModifier {
            pixels: PixelBuffer::new(body.pixels.bounds, wgpu::TextureFormat::R8Unorm),
            cpu_cache: SelectionCpuCache::new(),
            pixel_bounds: None,
        }),
    })
}

fn remap_ids(_modifier: &mut Modifier, _id_map: &IdMap) {
    // Selection has no cross-references inside its body.
}
