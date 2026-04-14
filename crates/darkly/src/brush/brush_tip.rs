//! Brush tip types — auto-generated (procedural) and predefined (image-based).
//!
//! Matches Krita's `KisBrushModel`: auto tips are computed from parameters
//! (hardness, shape, spikes, etc.), predefined tips are grayscale images.
//! Both are uploaded to the GPU as textures and treated identically after that.

use serde::{Deserialize, Serialize};

/// How the brush tip image is interpreted when compositing with paint color.
///
/// Mirrors Krita's `enumBrushApplication` from `kis_brush.h`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrushTipApplication {
    /// Tip grayscale = opacity mask. Color comes from paint color.
    /// Most common mode — used by the vast majority of brushes.
    #[default]
    AlphaMask,
    /// Tip RGB used directly (for colored/textured brush tips).
    /// Paint color is ignored; the tip provides its own color.
    ImageStamp,
    /// Tip luminance modulates paint color lightness.
    /// Krita's default for color smudge brushes.
    LightnessMap,
    /// Tip luminance indexes a gradient to produce color.
    /// Rarely used but supported by Krita.
    GradientMap,
}

/// Brush tip definition — either procedural (auto) or image-based (predefined).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrushTip {
    /// Procedurally generated tip (SDF circle with parameters).
    /// Generated on the GPU at brush load time, then cached as a texture
    /// and treated identically to predefined tips.
    Auto {
        /// Edge hardness: 0.0 = fully soft (gaussian), 1.0 = fully hard (circle).
        #[serde(default = "default_half")]
        hardness: f32,
        /// Number of spikes (1 = circle, 3+ = star shape).
        #[serde(default = "default_one")]
        spikes: u32,
        /// Aspect ratio: 1.0 = circle, <1.0 = horizontally squashed.
        #[serde(default = "default_one_f")]
        ratio: f32,
        /// Edge fade length as fraction of radius.
        #[serde(default)]
        fade: f32,
    },
    /// Image-based brush tip loaded from a preset resource.
    Predefined {
        /// Resource name (matches `PresetResourceMeta::name` in the preset ZIP).
        resource_name: String,
        /// How the image is applied to dabs.
        #[serde(default)]
        application: BrushTipApplication,
    },
}

impl Default for BrushTip {
    fn default() -> Self {
        BrushTip::Auto {
            hardness: 0.5,
            spikes: 1,
            ratio: 1.0,
            fade: 0.0,
        }
    }
}

fn default_half() -> f32 { 0.5 }
fn default_one() -> u32 { 1 }
fn default_one_f() -> f32 { 1.0 }
