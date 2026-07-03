//! Void (procedural-content layer) queries and picker previews.

use darkly_macros::handlers;

use super::types::{ParamInfo, VoidTypeInfo};
use super::DarklyEngine;
use super::PreviewJob;
use super::PreviewKind;
use super::ReadbackContext;
use crate::coord::LayerRect;
use crate::gpu::params::ParamDef;
use crate::gpu::preview::{ANIMATED_FRAMES, PREVIEW_DT};
use crate::gpu::void::Void;

/// Voids render into a straight RGBA8 destination (matching the layer-texture
/// atlas format), which is also the format the per-frame readback expects.
const VOID_PREVIEW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

#[handlers]
impl DarklyEngine {
    // --- Queries ---

    /// Return all registered void types with their parameter definitions, icon,
    /// and whether each supports a rendered picker preview.
    #[handler]
    pub fn void_types(&self) -> Vec<VoidTypeInfo> {
        self.compositor
            .void_registry()
            .types()
            .into_iter()
            .map(|reg| VoidTypeInfo {
                type_id: reg.type_id,
                display_name: reg.display_name,
                params: reg
                    .params
                    .iter()
                    .map(|d| ParamInfo::from_def(d, None))
                    .collect(),
                icon: reg.icon,
                supports_preview: reg.supports_preview,
                capture_kind: reg.capture_kind,
            })
            .collect()
    }

    /// Get the parameter definitions for a void type.
    pub fn void_param_defs(&self, type_id: &str) -> &'static [ParamDef] {
        self.compositor.void_registry().param_defs(type_id)
    }

    /// Coerce a raw JSON params object into typed [`ParamValue`]s against a void
    /// type's `ParamDef` schema. The single param-coercion seam every
    /// void-param handler shares (add / update): it pairs the raw `params` with
    /// the sibling `void_type` that names the schema, which is exactly the
    /// pairing generic request routing can't do for itself.
    ///
    /// [`ParamValue`]: crate::gpu::params::ParamValue
    pub fn coerce_void_params(
        &self,
        type_id: &str,
        params: &serde_json::Value,
    ) -> Vec<crate::gpu::params::ParamValue> {
        crate::gpu::params::param_values_from_json(params, self.void_param_defs(type_id))
    }

    /// Resolve a layer id to its void type, if the layer is a void.
    /// Helper for the WASM bridge so callers don't need to import the layer
    /// enum to query the active void's schema.
    pub fn void_layer_type(&self, layer_id: crate::layer::LayerId) -> Option<String> {
        match self.doc.find_node(layer_id)? {
            crate::layer::LayerNode::Layer(crate::layer::Layer::Void(v)) => {
                Some(v.void_type.clone())
            }
            _ => None,
        }
    }

    // --- Picker previews ---

    /// Begin generating the looping thumbnail preview for void `type_id` — the
    /// void rendered from scratch at the current canvas's aspect-fit preview
    /// size, captured as a sequence of frames. Regenerates on every call (a
    /// no-op only while a generation is already in flight). Frames land
    /// asynchronously; retrieve them with [`poll_preview`](Self::poll_preview).
    ///
    /// Unlike the veil path there's no source composite to refresh — the void
    /// generates its own content — so this just builds the void, encodes its
    /// frames, and reads them back. Fully isolated from the live layer stack: a
    /// fresh void instance renders into the preview renderer's own texture.
    pub fn start_void_preview(&mut self, type_id: &str) {
        let Some(static_id) = self.compositor.void_registry().static_type_id(type_id) else {
            return;
        };

        // Don't queue a duplicate generation while one is in flight.
        if self.readbacks.any(|c| {
            matches!(
                c,
                ReadbackContext::PreviewFrame { kind: PreviewKind::Void, type_id: t, .. }
                    if *t == static_id
            )
        }) {
            return;
        }

        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();
        let format = VOID_PREVIEW_FORMAT;

        let defaults: Vec<_> = self
            .compositor
            .void_registry()
            .param_defs(static_id)
            .iter()
            .map(|d| d.default_value())
            .collect();

        // Build the void + cache over the preview output texture. Borrow-split
        // the disjoint fields (registry on the compositor, the GPU context, the
        // renderer) so they don't alias `self`.
        let (mut void, cache) = {
            let Self {
                compositor,
                gpu,
                void_preview_renderer,
                ..
            } = self;
            let registry = compositor.void_registry_mut();
            void_preview_renderer.build_void(
                &gpu.device,
                &gpu.queue,
                registry,
                static_id,
                &defaults,
                canvas_w,
                canvas_h,
                format,
            )
        };

        let (pw, ph) = self.void_preview_renderer.preview_size();
        let total = if void.needs_animation() {
            ANIMATED_FRAMES
        } else {
            1
        };
        self.previews.insert(
            (PreviewKind::Void, static_id),
            PreviewJob {
                width: pw,
                height: ph,
                frames: vec![None; total as usize],
            },
        );

        let rect = LayerRect::from_xywh(0, 0, pw, ph);
        for frame_idx in 0..total {
            // Advance animated voids between frames so the loop shows motion;
            // static voids (the common case — noise) render a single frame.
            if frame_idx > 0 && void.needs_animation() {
                void.update_time(&self.gpu.queue, &cache, PREVIEW_DT);
            }
            let void_ref: &dyn Void = void.as_ref();
            let Self {
                gpu,
                readbacks,
                void_preview_renderer,
                ..
            } = self;
            let output = void_preview_renderer.output_texture();
            Self::encode_preview_frame(
                gpu,
                readbacks,
                PreviewKind::Void,
                static_id,
                frame_idx,
                total,
                output,
                format,
                rect,
                |encoder| void_preview_renderer.encode_frame(encoder, void_ref, &cache),
            );
        }
    }
}
