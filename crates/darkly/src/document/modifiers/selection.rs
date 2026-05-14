//! Selection modifier — global single-channel mask for which pixels are
//! affected by edits (paint, fill, transform, clipboard).
//!
//! Per the Modularity Principle in [CLAUDE.md], the entire selection kind
//! lives in this file: data struct, CPU cache, construction, and the
//! `register()` discovery hook.
//!
//! The selection is structurally a [`crate::document::Modifier`] but, unlike
//! per-host modifiers (mask, future filter/transform), it's attached at the
//! document root rather than on a host's `modifiers` list. That's the only
//! thing special about it — pixel storage, growth, dirty tracking, async
//! readback, and region-pixel undo all share the [`crate::layer::PixelBuffer`]
//! infrastructure, and the boolean ops sit on the same R8 paint pipeline that
//! mask painting uses.

use crate::coord::CanvasRect;
use crate::document::modifier::ModifierRegistration;
use crate::layer::PixelBuffer;

/// CPU mirror of the selection's R8 texture, populated lazily by async
/// readback after each mutating op (combine/invert/upload). Read paths that
/// need pixel-level access (transform source bounds, copy region masking,
/// flood-fill intersection) consult this rather than triggering a synchronous
/// GPU readback (forbidden by CLAUDE.md "No Blocking GPU Readbacks").
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

pub fn register() -> ModifierRegistration {
    ModifierRegistration {
        type_id: TYPE_ID,
        display_name: "Selection",
    }
}
