//! Smart object: a placed image that survives being resized.
//!
//! A thin config over the shared [`crate::gpu::textured_void`] machinery. The
//! "source texture → inverse-transform sample → layer texture" pipeline lives
//! there; this file declares only what makes a smart object itself.
//!
//! The point of the kind is that scaling is never destructive. A raster layer
//! scaled down and back up has been resampled twice and lost the detail in
//! between; a smart object stores the scale on the layer's transform and
//! re-samples the untouched source every frame, so the round trip returns
//! exactly the original pixels. That is why the source is held at its native
//! resolution rather than rasterized to the canvas.
//!
//! Two consequences follow from the source being installed once rather than
//! streamed, and both are handled generically by [`VoidSource::Image`]: it
//! carries a mip chain (so shrinking averages the texels a screen pixel covers
//! instead of point-sampling a few of them), and it never asks for browser
//! frames or keeps the animation clock running.
//!
//! [`ContentFit::Natural`] anchors the image in the document plane at its own
//! pixel size, so cropping the canvas doesn't slide it across the artwork,
//! unlike a camera, which is anchored to the window and keeps filling it.

use crate::gpu::textured_void::{self, ContentFit, TexturedVoidConfig};
use crate::gpu::void::{ParamDef, VoidRegistration, VoidSource};

pub const TYPE_ID: &str = "smart_object";

/// No parameters. Position, scale, rotation and shear are the layer's generic
/// [`crate::transform::Transform`], driven by the on-canvas gizmo; the source
/// image is installed once at placement. There is nothing left to expose as a
/// per-instance knob.
const PARAMS: &[ParamDef] = &[];

static CONFIG: TexturedVoidConfig = TexturedVoidConfig {
    type_id: TYPE_ID,
    display_name: "Smart Object",
    description: "A placed image that can be resized freely without losing quality.",
    icon: "tabler:photo-scan",
    params: PARAMS,
    source: VoidSource::Image,
    fit: ContentFit::Natural,
    default_transform: |_, _| crate::transform::Transform::identity(),
};

pub fn register() -> VoidRegistration {
    textured_void::registration(&CONFIG, |params, shared| {
        textured_void::build_void(&CONFIG, params, shared)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_as_an_image_source() {
        let reg = register();
        assert_eq!(reg.type_id, TYPE_ID);
        assert_eq!(reg.source, VoidSource::Image);
        // Not a capture kind: choosing this must open a file picker, never a
        // camera prompt, and the engine must not try to feed it frames.
        assert_eq!(reg.source.capture_kind(), None);
        assert!(!reg.source.is_streaming());
    }

    #[test]
    fn opts_into_the_transform_gizmo() {
        // The whole feature is "resize it freely", so the layer has to accept a
        // live transform: that is what `TransformCapability::Live` resolves to.
        assert!(register().supports_live_transform);
    }

    #[test]
    fn has_no_params() {
        assert!(PARAMS.is_empty());
        assert!(register().params.is_empty());
    }

    #[test]
    fn seeds_an_identity_transform() {
        // Placement computes a fit transform from the source's dimensions and
        // passes it explicitly; the registration's seed must not compete.
        let t = (register().default_transform)(800, 600);
        assert_eq!(t, crate::transform::Transform::identity());
    }
}
