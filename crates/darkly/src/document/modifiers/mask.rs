//! Mask modifier — multiplies a host's alpha by an R8 alpha texture.
//!
//! Per the Modularity Principle in [CLAUDE.md], the entire mask kind lives in
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

pub fn register() -> ModifierRegistration {
    ModifierRegistration { type_id: "mask" }
}
