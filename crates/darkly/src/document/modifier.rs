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

use crate::coord::CanvasRect;
use crate::document::modifiers::mask::MaskModifier;
use crate::layer::{LayerId, NodeCommon, PixelBuffer};

/// What each modifier module returns from its `register()` function.
/// Mirrors `VeilRegistration` / `ToolRegistration` / `FilterRegistration` —
/// auto-discovered by `build.rs` via the directory scan.
pub struct ModifierRegistration {
    pub type_id: &'static str,
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
    // Future: Selection (Phase 2), Filter, Transform, Colorize, ...
}

impl Modifier {
    /// Pixel storage for the modifier, if any. Mask has one (R8 alpha texture);
    /// future pure-transform/filter kinds may not.
    pub fn pixels(&self) -> Option<&PixelBuffer> {
        match &self.kind {
            ModifierKind::Mask(m) => Some(&m.pixels),
        }
    }

    pub fn pixels_mut(&mut self) -> Option<&mut PixelBuffer> {
        match &mut self.kind {
            ModifierKind::Mask(m) => Some(&mut m.pixels),
        }
    }

    /// Stable type-id string. Mirrors the `register().type_id` string for the kind.
    pub fn type_id(&self) -> &'static str {
        match &self.kind {
            ModifierKind::Mask(_) => "mask",
        }
    }

    pub fn is_mask(&self) -> bool {
        matches!(&self.kind, ModifierKind::Mask(_))
    }
}

impl ModifierKind {
    /// Construct a fresh mask modifier with the given pixel bounds.
    pub fn mask_with_bounds(bounds: CanvasRect) -> Self {
        ModifierKind::Mask(MaskModifier::new(bounds))
    }
}
