//! Filter nodes — typed effects attached to a host layer or group.
//!
//! A filter is a node in its own right (its own id, its own `NodeCommon`)
//! that sits on a host's `filters` list, separate from the regular layer
//! tree. The compositor renders a host by computing its base projection
//! (raster pixels or composited group children) and then dispatching through
//! `FilterKind::apply` for each visible filter. The outer compositor never
//! branches on whether a host has a mask.
//!
//! Per the Modularity Principle in [AGENTS.md], each kind lives in a single
//! file under `document/filters/<kind>.rs` and exports a `register()` that
//! returns a [`FilterEntityRegistration`]. `build.rs` auto-discovers the directory
//! and emits `document/filters/mod.rs`.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::coord::CanvasRect;
use crate::document::filters::mask::MaskFilter;
use crate::document::filters::selection::SelectionFilter;
use crate::document::layer_kind::{IdMap, SerializedEntity};
use crate::format::error::LoadError;
use crate::layer::{LayerId, NodeCommon, PixelBuffer};

/// What each filter module returns from its `register()` function.
/// Mirrors `VeilRegistration` / `ToolRegistration` / `FilterRegistration` —
/// auto-discovered by `build.rs` via the directory scan.
pub struct FilterEntityRegistration {
    pub type_id: &'static str,
    pub display_name: &'static str,
    /// Iconify name shown on the filter's row under its host layer.
    pub icon: &'static str,
    /// One-sentence summary of what attaching this filter does.
    pub description: &'static str,
    /// Produce the manifest body + any pixel-blob refs. Infallible by
    /// construction; see the analogous note on
    /// [`crate::document::layer_kind::LayerKindRegistration::serialize`].
    pub serialize: fn(&Filter) -> SerializedEntity,
    /// Reconstruct the filter from its manifest body. `id` is the
    /// freshly-allocated slotmap key.
    pub deserialize: fn(body: &serde_json::Value, id: LayerId) -> Result<Filter, LoadError>,
    /// Rewrite every cross-reference inside this filter from
    /// manifest-old id to fresh slotmap id. Non-optional — same
    /// rationale as
    /// [`crate::document::layer_kind::LayerKindRegistration::remap_ids`].
    pub remap_ids: fn(&mut Filter, &IdMap),
}

/// Id of the catalog this registry projects into. Distinct from the `filters`
/// catalog of `crate::gpu::filter`, which registers colour adjustments rather
/// than the mask and selection modifiers attached to a host layer.
pub const CATALOG_ID: &str = "layerFilters";

impl FilterEntityRegistration {
    pub fn catalog_entry(&self) -> crate::catalog::CatalogEntry {
        crate::catalog::CatalogEntry::new(self.type_id, self.display_name)
            .with_icon(self.icon)
            .with_description(self.description)
    }
}

/// The layer-filter catalog — every registered kind, sorted by `type_id`.
pub fn catalog() -> crate::catalog::Catalog {
    crate::catalog::Catalog::new(
        CATALOG_ID,
        "Layer Filters",
        registry()
            .all()
            .into_iter()
            .map(FilterEntityRegistration::catalog_entry)
            .collect(),
    )
    .with_description("Typed effects attached to a single host layer or group.")
}

/// Auto-discovered filter registry — owns the per-kind registration records
/// and hands out `&'static FilterEntityRegistration` references for the dispatch
/// surface (`Filter::kind`) and the UI.
pub struct FilterRegistry {
    entries: Vec<FilterEntityRegistration>,
    by_type_id: HashMap<&'static str, usize>,
}

impl Default for FilterRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl FilterRegistry {
    pub fn new() -> Self {
        let entries: Vec<FilterEntityRegistration> = super::filters::registrations();
        let mut by_type_id = HashMap::with_capacity(entries.len());
        for (i, reg) in entries.iter().enumerate() {
            by_type_id.insert(reg.type_id, i);
        }
        FilterRegistry {
            entries,
            by_type_id,
        }
    }

    pub fn get(&'static self, type_id: &str) -> Option<&'static FilterEntityRegistration> {
        self.by_type_id.get(type_id).map(|&i| &self.entries[i])
    }

    pub fn all(&'static self) -> Vec<&'static FilterEntityRegistration> {
        let mut v: Vec<_> = self.entries.iter().collect();
        v.sort_by_key(|reg| reg.type_id);
        v
    }
}

/// Lazily-initialized process-wide filter registry.
pub fn registry() -> &'static FilterRegistry {
    static REGISTRY: OnceLock<FilterRegistry> = OnceLock::new();
    REGISTRY.get_or_init(FilterRegistry::new)
}

/// A filter instance attached to a host node. Carries its own id (allocated
/// from the document's id counter) and its own [`NodeCommon`] so name, visibility,
/// and lock work uniformly with regular layers.
pub struct Filter {
    pub id: LayerId,
    pub common: NodeCommon,
    pub kind: FilterKind,
}

/// One variant per filter kind. Adding a kind = one new variant + one new
/// file in `document/filters/<kind>.rs`. The match arms here delegate; the
/// per-kind struct holds all the kind-specific state.
pub enum FilterKind {
    Mask(MaskFilter),
    Selection(SelectionFilter),
    // Future: Filter, Transform, Colorize, ...
}

impl Filter {
    /// Pixel storage for the filter, if any. Mask + selection both carry
    /// an R8 alpha buffer; future pure-transform/filter kinds may not.
    pub fn pixels(&self) -> Option<&PixelBuffer> {
        match &self.kind {
            FilterKind::Mask(m) => Some(&m.pixels),
            FilterKind::Selection(s) => Some(&s.pixels),
        }
    }

    pub fn pixels_mut(&mut self) -> Option<&mut PixelBuffer> {
        match &mut self.kind {
            FilterKind::Mask(m) => Some(&mut m.pixels),
            FilterKind::Selection(s) => Some(&mut s.pixels),
        }
    }

    /// Registration record for this filter's kind — owns `type_id` (wire
    /// format) and `display_name` (UI). The match dispatch references each
    /// kind module's own `TYPE_ID` constant, so the identity string is
    /// declared exactly once per kind.
    pub fn kind_reg(&self) -> &'static FilterEntityRegistration {
        self.kind.kind_reg()
    }

    /// Convenience for the wire format / save file — just the stable `type_id`.
    pub fn type_id(&self) -> &'static str {
        self.kind_reg().type_id
    }

    pub fn is_mask(&self) -> bool {
        matches!(&self.kind, FilterKind::Mask(_))
    }

    pub fn as_selection(&self) -> Option<&SelectionFilter> {
        match &self.kind {
            FilterKind::Selection(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_selection_mut(&mut self) -> Option<&mut SelectionFilter> {
        match &mut self.kind {
            FilterKind::Selection(s) => Some(s),
            _ => None,
        }
    }
}

impl FilterKind {
    /// Construct a fresh mask filter with the given pixel bounds.
    pub fn mask_with_bounds(bounds: CanvasRect) -> Self {
        FilterKind::Mask(MaskFilter::new(bounds))
    }

    /// Construct a fresh selection filter covering the whole canvas at
    /// the given bounds. The selection is canvas-sized at offset (0, 0).
    pub fn selection_with_bounds(bounds: CanvasRect) -> Self {
        FilterKind::Selection(SelectionFilter::new(bounds))
    }

    /// Registration record for this kind. Pulled from the filter registry
    /// keyed by each kind module's own `TYPE_ID` constant — no parallel
    /// string literals.
    pub fn kind_reg(&self) -> &'static FilterEntityRegistration {
        use super::filters::{mask, selection};
        match self {
            FilterKind::Mask(_) => registry().get(mask::TYPE_ID).unwrap(),
            FilterKind::Selection(_) => registry().get(selection::TYPE_ID).unwrap(),
        }
    }

    /// Convenience for the wire format. Shorthand for `kind_reg().type_id`.
    pub fn type_id(&self) -> &'static str {
        self.kind_reg().type_id
    }
}
