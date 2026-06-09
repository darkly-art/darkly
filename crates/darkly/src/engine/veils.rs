//! Veil (post-processing filter) management and query methods.

use super::types::{
    node_to_layer_info, BlendModeTypeInfo, LayerInfo, LayerKindTypeInfo, ModifierTypeInfo,
    ParamInfo, ToolTypeInfo, VeilInfo, VeilTypeInfo,
};
use super::DarklyEngine;
use super::ReadbackContext;
use super::VeilPreviewJob;
use crate::coord::LayerRect;
use crate::gpu::params::{ParamDef, ParamValue};
use crate::gpu::veil::Veil;
use crate::gpu::veil_preview::ANIMATED_FRAMES;

impl DarklyEngine {
    // --- Veils ---

    pub fn add_veil(&mut self, veil_type: &str, params: &[ParamValue]) {
        let chain = self.compositor.veil_chain_mut();
        let format = chain.accum_format();
        let veil = chain
            .registry_mut()
            .create_veil(veil_type, params, &self.gpu.device, format);
        chain.add_veil(&self.gpu.device, &self.gpu.queue, veil);
    }

    pub fn remove_veil(&mut self, index: usize) {
        self.compositor.veil_chain_mut().remove_veil(index);
    }

    pub fn clear_veils(&mut self) {
        self.compositor.veil_chain_mut().clear_veils();
    }

    pub fn set_veil_visible(&mut self, index: usize, visible: bool) {
        self.compositor
            .veil_chain_mut()
            .set_veil_visible(index, visible);
    }

    pub fn move_veil(&mut self, from: usize, to: usize) {
        self.compositor.veil_chain_mut().move_veil(from, to);
    }

    pub fn update_veil(&mut self, index: usize, params: &[ParamValue]) {
        let type_id: &'static str = match self.compositor.veil_chain().type_id(index) {
            Some(t) => t,
            None => return,
        };
        let chain = self.compositor.veil_chain_mut();
        let format = chain.accum_format();
        let new_veil = chain
            .registry_mut()
            .create_veil(type_id, params, &self.gpu.device, format);
        chain.update_veil(&self.gpu.device, &self.gpu.queue, index, new_veil);
    }

    // --- Picker previews ---

    /// Begin generating the looping thumbnail preview for `type_id` — the veil
    /// applied to the **current canvas**, captured as a sequence of frames.
    /// Regenerates on every call (a no-op only while a generation is already in
    /// flight) so the picker always reflects the live document. Frames land
    /// asynchronously; retrieve them with
    /// [`poll_veil_preview`](Self::poll_veil_preview).
    ///
    /// Fully isolated from the live veil chain: it downscales a snapshot of the
    /// composite into the preview renderer's own textures and runs a fresh veil
    /// instance over it, so the user's active veils, compositor surface, and
    /// document are never mutated.
    pub fn start_veil_preview(&mut self, type_id: &str) {
        // Resolve to the registry's 'static id — it keys both the preview job
        // and the readback context.
        let Some(static_id) = self
            .compositor
            .veil_chain()
            .registry()
            .static_type_id(type_id)
        else {
            return;
        };

        // Don't queue a duplicate generation while one is in flight. (We do not
        // skip when frames already exist — each open re-renders the live
        // canvas, which may have changed since last time.)
        if self.readbacks.any(
            |c| matches!(c, ReadbackContext::VeilPreviewFrame { type_id: t, .. } if *t == static_id),
        ) {
            return;
        }

        // Refresh the composite so the preview reflects the current document,
        // even with no surface present yet (mirrors `start_export`).
        self.compositor
            .render_offscreen(&self.gpu.device, &self.gpu.queue, &mut self.doc);
        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();
        let format = self.compositor.veil_chain().accum_format();

        // Downscale the live composite into the preview input texture. Holds an
        // immutable borrow of `compositor` (via the texture view) alongside the
        // mutable renderer borrow — disjoint fields, so they don't alias.
        {
            let source = self.compositor.composited_texture();
            let source_view = source.create_view(&wgpu::TextureViewDescriptor::default());
            self.veil_preview_renderer.load_source(
                &self.gpu.device,
                &self.gpu.queue,
                &source_view,
                canvas_w,
                canvas_h,
                format,
            );
        }

        let defaults: Vec<ParamValue> = self
            .compositor
            .veil_chain()
            .registry()
            .param_defs(static_id)
            .iter()
            .map(|d| d.default_value())
            .collect();

        // Build the veil + cache over the loaded composite. Borrow-split the
        // disjoint fields (registry on the compositor, the GPU context, the
        // renderer) so they don't alias `self`.
        let (mut veil, cache) = {
            let Self {
                compositor,
                gpu,
                veil_preview_renderer,
                ..
            } = self;
            let registry = compositor.veil_chain_mut().registry_mut();
            veil_preview_renderer.build_veil(
                &gpu.device,
                &gpu.queue,
                registry,
                static_id,
                &defaults,
                format,
            )
        };

        let (pw, ph) = self.veil_preview_renderer.preview_size();
        let total = if veil.needs_animation() {
            ANIMATED_FRAMES
        } else {
            1
        };
        let dt = self.veil_preview_renderer.frame_dt();
        self.veil_previews.insert(
            static_id,
            VeilPreviewJob {
                width: pw,
                height: ph,
                frames: vec![None; total as usize],
            },
        );

        let rect = LayerRect::from_xywh(0, 0, pw, ph);
        for frame_idx in 0..total {
            // Frame 0 captures the initial state; advance animated veils between
            // frames so the loop shows motion. Each frame renders into the same
            // output texture and copies to its own staging buffer in the same
            // submission, so the readback captures that frame before the next
            // overwrites it.
            if frame_idx > 0 && veil.needs_animation() {
                veil.update_time(&self.gpu.queue, &cache, dt);
            }
            let veil_ref: &dyn Veil = veil.as_ref();
            self.gpu.encode("veil-preview-frame", |encoder| {
                self.veil_preview_renderer
                    .encode_frame(encoder, veil_ref, &cache);
                let request = crate::gpu::readback::request_readback(
                    &self.gpu.device,
                    encoder,
                    self.veil_preview_renderer.output_texture(),
                    format,
                    rect,
                );
                self.readbacks.submit(
                    request,
                    ReadbackContext::VeilPreviewFrame {
                        type_id: static_id,
                        frame_idx,
                        total,
                    },
                );
            });
        }
    }

    /// Return the completed preview for `type_id` as `(width, height, frames)`
    /// once every frame has landed, else `None`. Each frame is
    /// `width × height` tightly-packed RGBA8.
    pub fn poll_veil_preview(&self, type_id: &str) -> Option<(u32, u32, Vec<Vec<u8>>)> {
        let job = self.veil_previews.get(type_id)?;
        if job.frames.is_empty() || job.frames.iter().any(Option::is_none) {
            return None;
        }
        let frames = job.frames.iter().map(|f| f.clone().unwrap()).collect();
        Some((job.width, job.height, frames))
    }

    // --- Queries ---

    pub fn layer_tree(&self) -> Vec<LayerInfo> {
        self.doc
            .children_of(self.doc.root_id())
            .iter()
            .rev()
            .filter_map(|id| node_to_layer_info(&self.doc, self.compositor.void_registry(), *id))
            .collect()
    }

    pub fn veil_list(&self) -> Vec<VeilInfo> {
        let chain = self.compositor.veil_chain();
        let count = chain.count();
        let mut list = Vec::with_capacity(count);
        for i in (0..count).rev() {
            if let Some((type_id, visible)) = chain.info(i) {
                let param_defs = chain.registry().param_defs(type_id);
                let values = chain.param_values(i).unwrap_or_default();
                let params = param_defs
                    .iter()
                    .enumerate()
                    .map(|(j, def)| ParamInfo::from_def(def, values.get(j)))
                    .collect();
                list.push(VeilInfo {
                    type_id: type_id.to_string(),
                    visible,
                    index: i,
                    params,
                });
            }
        }
        list
    }

    /// Return all registered veil types with their parameter definitions.
    pub fn veil_types(&self) -> Vec<VeilTypeInfo> {
        self.compositor
            .veil_chain()
            .registry()
            .types()
            .into_iter()
            .map(|(type_id, display_name, defs)| VeilTypeInfo {
                type_id,
                display_name,
                params: defs.iter().map(|d| ParamInfo::from_def(d, None)).collect(),
            })
            .collect()
    }

    /// Get the parameter definitions for a veil type.
    pub fn veil_param_defs(&self, type_id: &str) -> &'static [ParamDef] {
        self.compositor.veil_chain().registry().param_defs(type_id)
    }

    /// Return all registered void types with their parameter definitions.
    /// Same shape as `veil_types()` — the UI consumes both through the
    /// shared `VeilTypeInfo` struct (renamed `VoidTypeInfo` would just be a
    /// type alias; reusing the existing one keeps the JSON identical and
    /// the frontend's render code generic).
    pub fn void_types(&self) -> Vec<VeilTypeInfo> {
        self.compositor
            .void_registry()
            .types()
            .into_iter()
            .map(|(type_id, display_name, defs)| VeilTypeInfo {
                type_id,
                display_name,
                params: defs.iter().map(|d| ParamInfo::from_def(d, None)).collect(),
            })
            .collect()
    }

    /// Get the parameter definitions for a void type.
    pub fn void_param_defs(&self, type_id: &str) -> &'static [ParamDef] {
        self.compositor.void_registry().param_defs(type_id)
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

    /// Return all registered tool types with display name and parameter definitions.
    /// Backs the WASM bridge so the UI can render tool names without hardcoding them.
    pub fn tool_types(&self) -> Vec<ToolTypeInfo> {
        crate::tool::registry()
            .types()
            .into_iter()
            .map(|(type_id, display_name, defs)| ToolTypeInfo {
                type_id,
                display_name,
                params: defs.iter().map(|d| ParamInfo::from_def(d, None)).collect(),
            })
            .collect()
    }

    /// Return all registered blend modes in GPU-value order, with display name
    /// and category. Backs the WASM bridge so the UI populates the blend-mode
    /// dropdown from the registry instead of a hardcoded table.
    pub fn blend_mode_types(&self) -> Vec<BlendModeTypeInfo> {
        crate::gpu::blend_mode::registry()
            .all()
            .into_iter()
            .map(|reg| BlendModeTypeInfo {
                type_id: reg.type_id,
                display_name: reg.display_name,
                category: reg.category,
            })
            .collect()
    }

    /// Return all registered modifier kinds. UI uses this to resolve
    /// `ModifierInfo.kind` to a display label and to populate the
    /// "Add modifier" menu.
    pub fn modifier_types(&self) -> Vec<ModifierTypeInfo> {
        crate::document::modifier::registry()
            .all()
            .into_iter()
            .map(|reg| ModifierTypeInfo {
                type_id: reg.type_id,
                display_name: reg.display_name,
            })
            .collect()
    }

    /// Return all registered layer kinds. UI uses this to resolve a layer's
    /// `type` discriminator to a display label (e.g. "Raster Layer", "Group").
    pub fn layer_kind_types(&self) -> Vec<LayerKindTypeInfo> {
        crate::document::layer_kind::registry()
            .all()
            .into_iter()
            .map(|reg| LayerKindTypeInfo {
                type_id: reg.type_id,
                display_name: reg.display_name,
            })
            .collect()
    }
}
