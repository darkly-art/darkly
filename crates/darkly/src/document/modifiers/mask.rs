//! Mask modifier — multiplies a host's alpha by an R8 alpha texture.
//!
//! Per the Modularity Principle in [AGENTS.md], the entire mask kind lives in
//! this file: data struct, construction, and the `register()` discovery hook.

use crate::coord::CanvasRect;
use crate::document::modifier::ModifierRegistration;
use crate::layer::PixelBuffer;

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

pub fn register() -> ModifierRegistration {
    ModifierRegistration {
        type_id: TYPE_ID,
        display_name: "Mask",
    }
}
