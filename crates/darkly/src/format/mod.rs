//! `.darkly` wire-format schema, helpers, and round-trip tests.
//!
//! Phase 2 of the file-format rollout: pure data layer, no save/load yet.
//! See [`crates/darkly/src/format/manifest.rs`](manifest.rs) for the
//! container schema and [`crates/darkly/src/format/registry_io.rs`](registry_io.rs)
//! for the `(type_id, params)` envelope every modular system uses on disk.

pub mod error;
pub mod manifest;
pub mod registry_io;

#[cfg(test)]
mod tests;

pub use error::LoadError;
pub use manifest::{
    texture_format_from_str, texture_format_to_str, Manifest, ManifestCanvas, ManifestGroupNode,
    ManifestMaskModifier, ManifestModifier, ManifestNode, ManifestPixelRef, ManifestRasterNode,
    ManifestRequires, ManifestSelection, ManifestSelectionModifier, ManifestTree, ManifestVeil,
    ManifestWriter, CONTAINER_VERSION, FORMAT_TAG,
};
pub use registry_io::{deserialize_instance, serialize_instance, InstancePayload};
