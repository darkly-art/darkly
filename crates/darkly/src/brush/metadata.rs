//! A brush as the library holds it: identity, describing metadata, and the
//! node graph that paints.
//!
//! The archive a brush travels in is a *pack* —
//! [`crate::brush::pack_file`] — even when it holds exactly one brush. This
//! module owns the record; that one owns the container.

use serde::{Deserialize, Serialize};

use crate::brush::pack::BrushId;
use crate::brush::stabilizer::StabilizerConfig;
use crate::brush::wire::BrushWireType;
use crate::nodegraph::Graph;

/// A brush's serialized form — one entry in a pack archive, and one record in
/// the painter's stored library.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrushMetadata {
    /// Opaque identity. Shipped brushes use their YAML file stem; a painter's
    /// brushes are given a minted id when saved.
    ///
    /// Separate from `name` so a rename touches no pack member list and no
    /// recent-brushes entry — both hold ids.
    pub id: BrushId,
    pub name: String,
    #[serde(default = "default_engine_version")]
    pub engine_version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub graph: Graph<BrushWireType>,
    /// Stabilizer configuration.  Default = no stabilization (pass-through).
    #[serde(default)]
    pub stabilizer: StabilizerConfig,
}

/// A fully-loaded brush — the unit the library stores and a pack groups.
#[derive(Clone, Debug)]
pub struct Brush {
    pub metadata: BrushMetadata,
    /// Optional pre-rendered preview PNG. Produced by the async thumbnail bake
    /// and consumed by the brush picker grid. `None` for freshly-saved brushes
    /// whose bake hasn't completed yet.
    ///
    /// Deliberately not part of a pack archive: a baked preview is a
    /// theme-derived render cache — `BrushLibrary::clear_thumbnails` drops
    /// every one on theme change — so one baked by the sender would be wrong
    /// for the recipient, whose own bake is a frame away.
    pub thumbnail_png: Option<Vec<u8>>,
    /// Whether this brush ships with the app.
    ///
    /// A shipped brush is rebuilt from embedded YAML on every boot, so it
    /// cannot hold a rename or a deletion: storing one would shadow the YAML
    /// it comes back from. The painter's own brushes are theirs to change.
    /// Same reasoning as [`crate::brush::PackMutability`], one level down.
    pub shipped: bool,
}

fn default_engine_version() -> String {
    crate::VERSION.to_string()
}

impl BrushMetadata {
    /// Create metadata from an id, a name and a graph.
    pub fn from_graph(
        id: impl Into<BrushId>,
        name: impl Into<String>,
        graph: Graph<BrushWireType>,
    ) -> Self {
        BrushMetadata {
            id: id.into(),
            name: name.into(),
            engine_version: default_engine_version(),
            author: String::new(),
            description: String::new(),
            tags: Vec::new(),
            graph,
            stabilizer: StabilizerConfig::default(),
        }
    }
}

impl Brush {
    /// Create a brush the painter owns.
    pub fn from_metadata(metadata: BrushMetadata) -> Self {
        Brush {
            metadata,
            thumbnail_png: None,
            shipped: false,
        }
    }

    /// Mark this brush as one that ships with the app. Only
    /// [`crate::brush::builtin_brushes`] has any business calling this: every
    /// other route into the library is the painter creating or importing.
    pub fn into_shipped(mut self) -> Self {
        self.shipped = true;
        self
    }

    /// Whether the painter may rename or delete this brush.
    pub fn can_edit(&self) -> bool {
        !self.shipped
    }

    pub fn id(&self) -> &str {
        &self.metadata.id
    }

    pub fn name(&self) -> &str {
        &self.metadata.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush;

    #[test]
    fn engine_version_default_is_crate_version() {
        // Lives here because `default_engine_version` is private to this
        // module. The brush breadcrumb is the git-derived crate version.
        assert_eq!(default_engine_version(), crate::VERSION);
    }

    #[test]
    fn metadata_round_trips_through_json() {
        // The record shape a pack archive and a stored library record both
        // carry.
        let metadata = BrushMetadata::from_graph("ink_pen", "Ink Pen", brush::default_graph());
        let json = serde_json::to_string(&metadata).unwrap();
        let back: BrushMetadata = serde_json::from_str(&json).unwrap();

        assert_eq!(back.id, "ink_pen");
        assert_eq!(back.name, "Ink Pen");
        assert_eq!(
            serde_json::to_value(&metadata.graph).unwrap(),
            serde_json::to_value(&back.graph).unwrap(),
        );
    }

    #[test]
    fn unknown_fields_are_ignored() {
        // A record written by a newer build must not fail to load on an older
        // one for carrying a field it does not know.
        let metadata = BrushMetadata::from_graph("compat", "Compat", brush::default_graph());
        let mut value = serde_json::to_value(&metadata).unwrap();
        value["unknown_field"] = serde_json::json!("ignored");
        value["nested_unknown"] = serde_json::json!({ "a": 1, "b": [2, 3] });

        let back: BrushMetadata = serde_json::from_value(value).unwrap();
        assert_eq!(back.name, "Compat");
    }
}
