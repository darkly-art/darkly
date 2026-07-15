//! Veil (post-processing filter) management and query methods.

use darkly_macros::handlers;

use super::types::{
    node_to_layer_info, BlendModeTypeInfo, LayerInfo, LayerKindTypeInfo, ModifierTypeInfo,
    ParamInfo, ToolTypeInfo, VeilInfo, VeilTypeInfo,
};
use super::DarklyEngine;
use super::PreviewJob;
use super::PreviewKind;
use super::ReadbackContext;
use crate::coord::LayerRect;
use crate::engine::protocol::{params_from_json, RawParams};
use crate::gpu::params::{ParamDef, ParamValue};
use crate::gpu::preview::ANIMATED_FRAMES;
use crate::gpu::veil::Veil;

#[handlers]
impl DarklyEngine {
    // --- Veils ---

    /// Wire entry for `add_veil` — coerces JSON `params` against the veil
    /// type's schema, then [`Self::add_veil_layer`].
    #[handler]
    pub fn add_veil(&mut self, veil_type: String, params: RawParams) {
        let pv = params_from_json(&params.0, self.veil_param_defs(&veil_type));
        self.add_veil_layer(&veil_type, &pv);
    }

    /// Wire entry for `update_veil` — resolves the slot's veil type, coerces
    /// `params` against its schema, then [`Self::update_veil_layer`]. A stale
    /// index is a silent no-op.
    #[handler]
    pub fn update_veil(&mut self, index: usize, params: RawParams) {
        let Some(type_id) = self.compositor.veil_chain().type_id(index) else {
            return;
        };
        let pv = params_from_json(&params.0, self.veil_param_defs(type_id));
        self.update_veil_layer(index, &pv);
    }

    pub fn add_veil_layer(&mut self, veil_type: &str, params: &[ParamValue]) {
        let chain = self.compositor.veil_chain_mut();
        let format = chain.accum_format();
        let veil = chain
            .registry_mut()
            .create_veil(veil_type, params, &self.gpu.device, format);
        chain.add_veil(&self.gpu.device, &self.gpu.queue, veil);
    }

    #[handler]
    pub fn remove_veil(&mut self, index: usize) {
        self.compositor.veil_chain_mut().remove_veil(index);
    }

    #[handler]
    pub fn clear_veils(&mut self) {
        self.compositor.veil_chain_mut().clear_veils();
    }

    #[handler]
    pub fn set_veil_visible(&mut self, index: usize, visible: bool) {
        self.compositor
            .veil_chain_mut()
            .set_veil_visible(index, visible);
    }

    #[handler]
    pub fn move_veil(&mut self, from: usize, to: usize) {
        self.compositor.veil_chain_mut().move_veil(from, to);
    }

    pub fn update_veil_layer(&mut self, index: usize, params: &[ParamValue]) {
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
        if self.readbacks.any(|c| {
            matches!(
                c,
                ReadbackContext::PreviewFrame { kind: PreviewKind::Veil, type_id: t, .. }
                    if *t == static_id
            )
        }) {
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
        self.previews.insert(
            (PreviewKind::Veil, static_id),
            PreviewJob {
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
            let Self {
                gpu,
                readbacks,
                veil_preview_renderer,
                ..
            } = self;
            let output = veil_preview_renderer.output_texture();
            Self::encode_preview_frame(
                gpu,
                readbacks,
                PreviewKind::Veil,
                static_id,
                frame_idx,
                total,
                output,
                format,
                rect,
                |encoder| veil_preview_renderer.encode_frame(encoder, veil_ref, &cache),
            );
        }
    }

    // --- Queries ---

    #[handler]
    pub fn layer_tree(&self) -> Vec<LayerInfo> {
        self.doc
            .children_of(self.doc.root_id())
            .iter()
            .rev()
            .filter_map(|id| {
                node_to_layer_info(
                    &self.doc,
                    self.compositor.void_registry(),
                    self.compositor.filter_pipeline_registry(),
                    *id,
                )
            })
            .collect()
    }

    #[handler]
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
    #[handler]
    pub fn veil_types(&self) -> Vec<VeilTypeInfo> {
        self.compositor
            .veil_chain()
            .registry()
            .types()
            .into_iter()
            .map(|(type_id, display_name, defs)| VeilTypeInfo {
                type_id,
                display_name,
                // Veils render a live preview in their picker, so no icon.
                icon: "",
                params: defs.iter().map(|d| ParamInfo::from_def(d, None)).collect(),
            })
            .collect()
    }

    /// Get the parameter definitions for a veil type.
    pub fn veil_param_defs(&self, type_id: &str) -> &'static [ParamDef] {
        self.compositor.veil_chain().registry().param_defs(type_id)
    }

    /// Return all registered tool types with display name and parameter definitions.
    /// Backs the WASM bridge so the UI can render tool names without hardcoding them.
    #[handler]
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
    #[handler]
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

    /// Return all registered filter kinds. UI uses this to resolve
    /// `ModifierInfo.kind` to a display label and to populate the
    /// "Add filter" menu.
    #[handler]
    pub fn modifier_types(&self) -> Vec<ModifierTypeInfo> {
        crate::document::filter::registry()
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
    #[handler]
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
