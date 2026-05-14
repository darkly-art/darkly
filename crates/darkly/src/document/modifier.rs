//! Modifier nodes — typed effects attached to a host layer or group.
//!
//! A modifier is a node in its own right (its own id, its own `NodeCommon`)
//! that sits on a host's `modifiers` list, separate from the regular layer
//! tree. The compositor renders a host by computing its base projection
//! (raster pixels or composited group children) and then dispatching through
//! `ModifierKind::apply` for each visible modifier. The outer compositor never
//! branches on whether a host has a mask.
//!
//! Per the Modularity Principle in [CLAUDE.md], each kind lives in a single
//! file under `document/modifiers/<kind>.rs` and exports a `register()` that
//! returns a [`ModifierRegistration`]. `build.rs` auto-discovers the directory
//! and emits `document/modifiers/mod.rs`.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::coord::CanvasRect;
use crate::document::modifiers::mask::MaskModifier;
use crate::document::modifiers::selection::SelectionModifier;
use crate::layer::{LayerId, NodeCommon, PixelBuffer};

/// What each modifier module returns from its `register()` function.
/// Mirrors `VeilRegistration` / `ToolRegistration` / `FilterRegistration` —
/// auto-discovered by `build.rs` via the directory scan.
pub struct ModifierRegistration {
    pub type_id: &'static str,
    pub display_name: &'static str,
}

/// Auto-discovered modifier registry — owns the per-kind registration records
/// and hands out `&'static ModifierRegistration` references for the dispatch
/// surface (`Modifier::kind`) and the UI.
pub struct ModifierRegistry {
    entries: Vec<ModifierRegistration>,
    by_type_id: HashMap<&'static str, usize>,
}

impl Default for ModifierRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ModifierRegistry {
    pub fn new() -> Self {
        let entries: Vec<ModifierRegistration> = super::modifiers::registrations();
        let mut by_type_id = HashMap::with_capacity(entries.len());
        for (i, reg) in entries.iter().enumerate() {
            by_type_id.insert(reg.type_id, i);
        }
        ModifierRegistry {
            entries,
            by_type_id,
        }
    }

    pub fn get(&'static self, type_id: &str) -> Option<&'static ModifierRegistration> {
        self.by_type_id.get(type_id).map(|&i| &self.entries[i])
    }

    pub fn all(&'static self) -> Vec<&'static ModifierRegistration> {
        let mut v: Vec<_> = self.entries.iter().collect();
        v.sort_by_key(|reg| reg.type_id);
        v
    }
}

/// Lazily-initialized process-wide modifier registry.
pub fn registry() -> &'static ModifierRegistry {
    static REGISTRY: OnceLock<ModifierRegistry> = OnceLock::new();
    REGISTRY.get_or_init(ModifierRegistry::new)
}

/// A modifier instance attached to a host node. Carries its own id (allocated
/// from the document's id counter) and its own [`NodeCommon`] so name, visibility,
/// and lock work uniformly with regular layers.
pub struct Modifier {
    pub id: LayerId,
    pub common: NodeCommon,
    pub kind: ModifierKind,
}

/// One variant per modifier kind. Adding a kind = one new variant + one new
/// file in `document/modifiers/<kind>.rs`. The match arms here delegate; the
/// per-kind struct holds all the kind-specific state.
pub enum ModifierKind {
    Mask(MaskModifier),
    Selection(SelectionModifier),
    // Future: Filter, Transform, Colorize, ...
}

impl Modifier {
    /// Pixel storage for the modifier, if any. Mask + selection both carry
    /// an R8 alpha buffer; future pure-transform/filter kinds may not.
    pub fn pixels(&self) -> Option<&PixelBuffer> {
        match &self.kind {
            ModifierKind::Mask(m) => Some(&m.pixels),
            ModifierKind::Selection(s) => Some(&s.pixels),
        }
    }

    pub fn pixels_mut(&mut self) -> Option<&mut PixelBuffer> {
        match &mut self.kind {
            ModifierKind::Mask(m) => Some(&mut m.pixels),
            ModifierKind::Selection(s) => Some(&mut s.pixels),
        }
    }

    /// Registration record for this modifier's kind — owns `type_id` (wire
    /// format) and `display_name` (UI). The match dispatch references each
    /// kind module's own `TYPE_ID` constant, so the identity string is
    /// declared exactly once per kind.
    pub fn kind_reg(&self) -> &'static ModifierRegistration {
        self.kind.kind_reg()
    }

    /// Convenience for the wire format / save file — just the stable `type_id`.
    pub fn type_id(&self) -> &'static str {
        self.kind_reg().type_id
    }

    pub fn is_mask(&self) -> bool {
        matches!(&self.kind, ModifierKind::Mask(_))
    }

    pub fn as_selection(&self) -> Option<&SelectionModifier> {
        match &self.kind {
            ModifierKind::Selection(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_selection_mut(&mut self) -> Option<&mut SelectionModifier> {
        match &mut self.kind {
            ModifierKind::Selection(s) => Some(s),
            _ => None,
        }
    }
}

impl ModifierKind {
    /// Construct a fresh mask modifier with the given pixel bounds.
    pub fn mask_with_bounds(bounds: CanvasRect) -> Self {
        ModifierKind::Mask(MaskModifier::new(bounds))
    }

    /// Construct a fresh selection modifier covering the whole canvas at
    /// the given bounds. The selection is canvas-sized at offset (0, 0).
    pub fn selection_with_bounds(bounds: CanvasRect) -> Self {
        ModifierKind::Selection(SelectionModifier::new(bounds))
    }

    /// Registration record for this kind. Pulled from the modifier registry
    /// keyed by each kind module's own `TYPE_ID` constant — no parallel
    /// string literals.
    pub fn kind_reg(&self) -> &'static ModifierRegistration {
        use super::modifiers::{mask, selection};
        match self {
            ModifierKind::Mask(_) => registry().get(mask::TYPE_ID).unwrap(),
            ModifierKind::Selection(_) => registry().get(selection::TYPE_ID).unwrap(),
        }
    }

    /// Convenience for the wire format. Shorthand for `kind_reg().type_id`.
    pub fn type_id(&self) -> &'static str {
        self.kind_reg().type_id
    }
}
