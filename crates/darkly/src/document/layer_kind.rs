//! Layer-kind registry — metadata + serializer dispatch for the structural
//! variants of [`crate::layer::LayerNode`] (today: `raster`, `group`).
//!
//! Adding a new layer kind is one new file under
//! [`crate::document::layer_kinds`]: the module's `register()` returns a
//! [`LayerKindRegistration`] that carries everything the save/load
//! pipeline needs (display name, body serializer, body deserializer,
//! id-remap function). Central save/load code dispatches through the
//! registry — it never branches on which variant it got.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::format::error::LoadError;
use crate::format::manifest::ManifestPixelRef;
use crate::layer::{LayerId, LayerNode};

/// Map from manifest-old ids (`u64`) to fresh slotmap ids — populated
/// in pass 1 of the load (allocate-every-entity), consumed in pass 2
/// (rewrite cross-references). Each kind's [`LayerKindRegistration::remap_ids`]
/// receives a reference and uses it to translate children / modifiers /
/// any future kind-specific id field.
pub type IdMap = HashMap<u64, LayerId>;

/// What the save pipeline collects from each entity's `serialize` call.
pub struct SerializedEntity {
    /// The opaque body to embed in the entity's [`crate::format::manifest::ManifestEntry`].
    pub body: serde_json::Value,
    /// Per-entity pixel-blob declarations. Empty for entities with no
    /// pixel storage (e.g. groups, future transform-only modifiers).
    pub pixel_blobs: Vec<PixelBlobSpec>,
}

/// One pixel-blob declaration produced by an entity's serializer. The
/// save pipeline batch-walks these after building the manifest, asking
/// the compositor for the GPU texture backing `source_node_id` and
/// queueing the readback under `blob_key`.
pub struct PixelBlobSpec {
    /// Zip-relative path the blob will be written under (e.g.
    /// `"layers/42.pixels"`). Must match the corresponding
    /// [`ManifestPixelRef::pixels`] embedded inside the entity's body.
    pub blob_key: String,
    /// The entity whose GPU texture is the readback source — always the
    /// entity declaring this blob. `PixelBlobSpec` does not model
    /// cross-entity texture references; a future kind that needs that
    /// pattern should introduce a separate abstraction rather than
    /// overloading this one.
    pub source_node_id: LayerId,
    /// Mirror of the body's [`ManifestPixelRef`]. Save uses this to find
    /// the readback bounds + declared format without re-parsing the body.
    pub pixels: ManifestPixelRef,
}

pub struct LayerKindRegistration {
    pub type_id: &'static str,
    pub display_name: &'static str,

    /// Produce the manifest body + any pixel-blob refs this entity wants
    /// saved. Infallible by construction — the kind's serialize goes
    /// through a typed body struct + derived `Serialize`, which can only
    /// fail on OOM. Texture-format-to-wire-slug failures are caught at
    /// texture-allocation time (the compositor only allocates
    /// representable formats; [`crate::format::manifest::texture_format_to_str`]
    /// panics on anything else).
    pub serialize: fn(&LayerNode) -> SerializedEntity,

    /// Reconstruct the entity from its manifest body. `id` is the
    /// freshly-allocated slotmap key — passed in because we're called
    /// from inside `entities.insert_with_key(|k| ...)`, before the
    /// entity exists in the doc. Cross-references inside `body` still
    /// carry manifest-old ids; the caller's second pass calls
    /// [`Self::remap_ids`] after every entity has been allocated.
    pub deserialize: fn(body: &serde_json::Value, id: LayerId) -> Result<LayerNode, LoadError>,

    /// Rewrite every cross-reference (children, modifiers, any future
    /// kind-specific id field) in `entity` from manifest-old id to
    /// fresh slotmap id. Non-optional — the contract that a future kind
    /// can't silently break the load by storing ids in a private field
    /// is enforced by this signature.
    pub remap_ids: fn(&mut LayerNode, &IdMap),
}

pub struct LayerKindRegistry {
    /// Owned storage — stable addresses while the registry lives (forever).
    entries: Vec<LayerKindRegistration>,
    by_type_id: HashMap<&'static str, usize>,
}

impl Default for LayerKindRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl LayerKindRegistry {
    pub fn new() -> Self {
        let entries: Vec<LayerKindRegistration> = super::layer_kinds::registrations();
        let mut by_type_id = HashMap::with_capacity(entries.len());
        for (i, reg) in entries.iter().enumerate() {
            by_type_id.insert(reg.type_id, i);
        }
        LayerKindRegistry {
            entries,
            by_type_id,
        }
    }

    /// Look up by stable `type_id`. Returns `&'static` because the registry
    /// itself is — callers can hold the reference indefinitely.
    pub fn get(&'static self, type_id: &str) -> Option<&'static LayerKindRegistration> {
        self.by_type_id.get(type_id).map(|&i| &self.entries[i])
    }

    pub fn all(&'static self) -> Vec<&'static LayerKindRegistration> {
        let mut v: Vec<_> = self.entries.iter().collect();
        v.sort_by_key(|reg| reg.type_id);
        v
    }
}

/// Lazily-initialized process-wide layer-kind registry.
pub fn registry() -> &'static LayerKindRegistry {
    static REGISTRY: OnceLock<LayerKindRegistry> = OnceLock::new();
    REGISTRY.get_or_init(LayerKindRegistry::new)
}
