//! Layer CRUD and property operations.

use super::DarklyEngine;
use crate::document::MoveTarget;
use crate::layer::{Layer, LayerId, LayerNode};
use crate::undo::property::Property;
use crate::undo::{
    CompoundAction, LayerAddAction, LayerMoveAction, LayerRemoveAction, PropertyAction, UndoAction,
};

impl DarklyEngine {
    // --- Layer CRUD ---

    pub fn add_raster_layer(&mut self, anchor: Option<LayerId>) -> LayerId {
        let id = self.doc.add_raster_layer(anchor);
        let bounds = match self.doc.layer(id) {
            Some(Layer::Raster(r)) => r.pixels.bounds,
            _ => crate::coord::CanvasRect::from_xywh(0, 0, self.doc.width, self.doc.height),
        };
        self.compositor
            .ensure_raster_layer(&self.gpu.device, &self.gpu.queue, id, bounds);
        self.compositor.mark_dirty();

        let parent = self.doc.parent_of(id);
        let pos = self.doc.position_in_parent(id).unwrap_or(0);
        self.push_undo(Box::new(LayerAddAction::new(id, parent, pos)));

        id
    }

    pub fn add_group(&mut self, anchor: Option<LayerId>) -> LayerId {
        let id = self.doc.add_group(anchor);

        let parent = self.doc.parent_of(id);
        let pos = self.doc.position_in_parent(id).unwrap_or(0);
        self.push_undo(Box::new(LayerAddAction::new(id, parent, pos)));

        id
    }

    /// Add a new void (procedural) layer. `params` is matched against the
    /// void type's `ParamDef` schema by index — callers that don't have a
    /// hand-rolled slice should use the type's defaults via
    /// `void_param_defs(type).iter().map(ParamDef::default_value)`.
    ///
    /// Returns `None` if `void_type` is not a registered void kind. (We
    /// surface this rather than silently fall back, matching how
    /// `set_blend_mode` rejects unknown blend ids.)
    pub fn add_void_layer(
        &mut self,
        void_type: &str,
        params: Vec<crate::gpu::params::ParamValue>,
        anchor: Option<LayerId>,
    ) -> Option<LayerId> {
        if !self.compositor.void_registry().has(void_type) {
            return None;
        }
        // Default-name the layer after the void's display label so the
        // panel reads "Noise 1" / "Noise 2" rather than a generic "Void N".
        let display_label = self.compositor.void_registry().display_name(void_type);
        let id =
            self.doc
                .add_void_layer(void_type.to_string(), display_label, params.clone(), anchor);
        self.compositor.ensure_void_layer(
            &self.gpu.device,
            &self.gpu.queue,
            id,
            void_type,
            &params,
        );
        self.compositor.mark_dirty();

        let parent = self.doc.parent_of(id);
        let pos = self.doc.position_in_parent(id).unwrap_or(0);
        self.push_undo(Box::new(LayerAddAction::new(id, parent, pos)));

        Some(id)
    }

    /// Replace a void layer's parameter values. Coalesces with prior
    /// `VoidParams` edits on the same layer so a slider drag is one undo
    /// step, mirroring how `set_opacity` already behaves.
    pub fn update_void_params(
        &mut self,
        layer_id: LayerId,
        new_params: Vec<crate::gpu::params::ParamValue>,
    ) {
        if !self.doc.is_node_editable(layer_id) {
            return;
        }
        let (old_params, void_type) = match self.doc.find_node(layer_id) {
            Some(LayerNode::Layer(Layer::Void(v))) => (v.params.clone(), v.void_type.clone()),
            _ => return,
        };
        if let Some(LayerNode::Layer(Layer::Void(v))) = self.doc.find_node_mut(layer_id) {
            v.params = new_params.clone();
        }
        self.compositor.update_void_layer_params(
            &self.gpu.device,
            &self.gpu.queue,
            layer_id,
            &void_type,
            &new_params,
        );
        self.compositor.mark_dirty();

        self.coalesce_property_undo(PropertyAction::new(
            layer_id,
            Property::VoidParams(old_params),
            Property::VoidParams(new_params),
        ));
    }

    /// Hand a fresh external image frame to a void's input texture.
    /// Wraps [`crate::gpu::compositor::Compositor::upload_void_external_image`];
    /// no-ops if `layer_id` isn't a void or the void doesn't consume external
    /// input. Frames flow through here every animation frame from the JS
    /// render loop for camera / future screenshare voids.
    ///
    /// Also syncs the doc-side [`crate::layer::VoidLayer::frame`] when the
    /// void declares a new persistent frame size. The save flow reads that
    /// field to decide whether to emit a pixel blob for the void, so
    /// keeping it current here is what makes the last frame round-trip
    /// through `.darkly`.
    pub fn upload_void_external_image(
        &mut self,
        layer_id: LayerId,
        source: crate::gpu::void::ExternalImageSource,
    ) {
        self.compositor.upload_void_external_image(
            &self.gpu.device,
            &self.gpu.queue,
            layer_id,
            source,
        );
        self.sync_void_persistent_frame(layer_id);
    }

    /// Pull the void's current `persistent_frame_size` from the compositor
    /// and mirror it onto [`crate::layer::VoidLayer::frame`]. Cheap when
    /// nothing changed (compares before writing). Called after every
    /// external-image upload and once at document open after a successful
    /// `restore_void_pixels` so saves and reloads stay consistent.
    fn sync_void_persistent_frame(&mut self, layer_id: LayerId) {
        let Some((w, h)) = self.compositor.void_persistent_frame_size(layer_id) else {
            return;
        };
        let blob_key = format!("layers/{}.pixels", layer_id.to_ffi());
        let next = crate::format::manifest::ManifestPixelRef {
            format: crate::format::manifest::texture_format_to_str(wgpu::TextureFormat::Rgba8Unorm)
                .to_string(),
            pixels: blob_key,
            bounds: crate::coord::CanvasRect::from_xywh(0, 0, w, h),
        };
        if let Some(crate::layer::LayerNode::Layer(crate::layer::Layer::Void(v))) =
            self.doc.find_node_mut(layer_id)
        {
            if v.frame.as_ref() != Some(&next) {
                v.frame = Some(next);
                self.doc.dirty = true;
            }
        }
    }

    pub fn has_layer(&self, layer_id: LayerId) -> bool {
        // "Has" means linked into the tree — not just sitting orphaned in the
        // document's slotmap waiting on an undo reattach. Detached-for-undo
        // layers must report `false` so callers (and the layer panel) treat
        // them as gone until reattach.
        self.doc.layer(layer_id).is_some() && self.doc.parent_of(layer_id).is_some()
    }

    /// Returns the layer's pixel-space bounds in canvas coordinates.
    /// Used by tests and the WASM bridge to report storage extent.
    pub fn layer_bounds(&self, layer_id: LayerId) -> Option<crate::coord::CanvasRect> {
        match self.doc.layer(layer_id)? {
            Layer::Raster(r) => Some(r.pixels.bounds),
            // Voids store no pixels — their "bounds" concept is the canvas
            // itself, which callers can ask for directly via `canvas_dimensions`.
            Layer::Void(_) => None,
        }
    }

    /// Returns the pixel-space bounds of any pixel-bearing node id (raster
    /// layer or mask modifier). Generalization of [`Self::layer_bounds`] —
    /// when callers hold a node id without knowing its kind, this resolves
    /// against the document's unified `pixels()` accessor. Returns `None`
    /// for groups (no pixel buffer) or unknown ids.
    pub fn node_pixel_bounds(&self, node_id: LayerId) -> Option<crate::coord::CanvasRect> {
        if let Some(rect) = self.layer_bounds(node_id) {
            return Some(rect);
        }
        self.doc
            .find_modifier(node_id)
            .and_then(|m| m.pixels())
            .map(|p| p.bounds)
    }

    pub fn remove_layer(&mut self, layer_id: LayerId) -> Result<(), String> {
        if !self.doc.is_node_editable(layer_id) {
            return Err("Layer is locked".into());
        }
        if self.doc.node_count() <= 1 {
            return Err("Cannot delete the last layer".into());
        }

        let parent = self.doc.parent_of(layer_id);
        let pos = self.doc.position_in_parent(layer_id).unwrap_or(0);

        if self.doc.detach_for_undo(layer_id).is_some() {
            // Drop per-layer GPU state to avoid leaking textures across
            // delete-then-add cycles. The orphaned node in the slotmap
            // keeps the layer's metadata for undo; pixel data does not
            // survive (see Compositor::dispose_layer).
            self.compositor.dispose_layer(layer_id);
            self.push_undo(Box::new(LayerRemoveAction::new(layer_id, parent, pos)));
        }

        self.compositor.mark_dirty();
        Ok(())
    }

    pub fn move_layer(&mut self, layer_id: LayerId, target: MoveTarget) {
        if !self.doc.is_node_editable(layer_id) {
            return;
        }
        let old_parent = self.doc.parent_of(layer_id);
        let old_pos = match self.doc.position_in_parent(layer_id) {
            Some(p) => p,
            None => return,
        };

        self.doc.move_layer(layer_id, target);

        let new_parent = self.doc.parent_of(layer_id);
        let new_pos = self.doc.position_in_parent(layer_id).unwrap_or(0);

        self.compositor.mark_dirty();

        self.push_undo(Box::new(LayerMoveAction::new(
            layer_id, old_parent, old_pos, new_parent, new_pos,
        )));
    }

    // --- Layer properties ---

    pub fn set_opacity(&mut self, layer_id: LayerId, opacity: f32) {
        if !self.doc.is_node_editable(layer_id) {
            return;
        }
        let old_opacity = match self.doc.find_node(layer_id) {
            Some(n) => n.blend().opacity,
            None => return,
        };
        if let Some(node) = self.doc.find_node_mut(layer_id) {
            node.blend_mut().opacity = opacity;
        } else {
            return;
        }

        self.refresh_blend_uniforms(layer_id);
        self.compositor.mark_dirty();

        self.coalesce_property_undo(PropertyAction::new(
            layer_id,
            Property::Opacity(old_opacity),
            Property::Opacity(opacity),
        ));
    }

    pub fn set_blend_mode(&mut self, layer_id: LayerId, type_id: &str) {
        if !self.doc.is_node_editable(layer_id) {
            return;
        }
        // Unknown blend-mode strings keep the existing mode rather than
        // silently snapping to Normal — the UI should only ever pass a
        // registered id, so an unknown one is a bug worth surfacing.
        let blend_mode = match crate::gpu::blend_mode::registry().get(type_id) {
            Some(reg) => reg,
            None => return,
        };
        let old_mode = match self.doc.find_node(layer_id) {
            Some(n) => n.blend().blend_mode,
            None => return,
        };
        // Picking a blend mode on a passthrough group implicitly switches it
        // to isolated — passthrough ignores the group's blend mode, so the
        // user's choice would have no visible effect otherwise.
        let was_passthrough = matches!(
            self.doc.find_node(layer_id),
            Some(LayerNode::Group(g)) if g.passthrough,
        );
        if let Some(node) = self.doc.find_node_mut(layer_id) {
            node.blend_mut().blend_mode = blend_mode;
            if was_passthrough {
                if let LayerNode::Group(g) = node {
                    g.passthrough = false;
                }
            }
        } else {
            return;
        }

        if was_passthrough {
            self.compositor
                .ensure_group_state(&self.gpu.device, &self.gpu.queue, layer_id);
        }
        self.refresh_blend_uniforms(layer_id);
        self.compositor.mark_dirty();

        let blend_action: Box<dyn UndoAction> = Box::new(PropertyAction::new(
            layer_id,
            Property::BlendMode(old_mode),
            Property::BlendMode(blend_mode),
        ));
        if was_passthrough {
            let passthrough_action: Box<dyn UndoAction> = Box::new(PropertyAction::new(
                layer_id,
                Property::Passthrough(true),
                Property::Passthrough(false),
            ));
            self.push_undo(Box::new(CompoundAction::new(vec![
                blend_action,
                passthrough_action,
            ])));
        } else {
            self.push_undo(blend_action);
        }
    }

    /// Set the `visible` flag on any node — layer, group, or modifier.
    /// Works uniformly across kinds because they all carry [`NodeCommon`].
    pub fn set_layer_visible(&mut self, node_id: LayerId, visible: bool) {
        // Try layers/groups first; fall through to modifiers.
        let old_visible = if let Some(node) = self.doc.find_node_mut(node_id) {
            let old = node.common().visible;
            node.common_mut().visible = visible;
            Some(old)
        } else if let Some(modifier) = self.doc.find_modifier_mut(node_id) {
            let old = modifier.common.visible;
            modifier.common.visible = visible;
            Some(old)
        } else {
            None
        };
        if let Some(old) = old_visible {
            self.compositor.mark_dirty();
            self.push_undo(Box::new(crate::undo::NodeVisibleAction::new(node_id, old)));
        }
    }

    /// Set the `locked` flag on any node — layer, group, or modifier.
    pub fn set_node_locked(&mut self, node_id: LayerId, locked: bool) {
        let old_locked = if let Some(node) = self.doc.find_node_mut(node_id) {
            let old = node.common().locked;
            node.common_mut().locked = locked;
            Some(old)
        } else if let Some(modifier) = self.doc.find_modifier_mut(node_id) {
            let old = modifier.common.locked;
            modifier.common.locked = locked;
            Some(old)
        } else {
            None
        };
        if let Some(old) = old_locked {
            self.push_undo(Box::new(crate::undo::NodeLockedAction::new(node_id, old)));
        }
    }

    /// Set the session-level "isolate this node" flag.
    ///
    /// When `Some(id)`, the renderer treats `id`'s subtree as the only
    /// thing on the canvas: the compose walk skips off-path siblings and,
    /// when `id` is a mask modifier, the host's blend pass renders the
    /// mask channel as grayscale.
    ///
    /// Pure session state — no document mutation. The eye-icon column on
    /// every layer is independent: toggling visibility while isolated
    /// modifies that layer's `visible` field, and clearing isolation
    /// preserves whatever the user set.
    pub fn set_isolated_node(&mut self, id: Option<LayerId>) {
        if self.isolated_node == id {
            return;
        }
        self.isolated_node = id;
        // Mirror to the compositor so the render walk can filter off-path
        // subtrees, then resync host uniforms — the `isolated` flag on a
        // host flips depending on whether one of its modifiers is the new
        // target.
        self.compositor.set_isolated_node(id);
        self.sync_compositor_layers();
        self.compositor.mark_dirty();
    }

    /// Read the current isolated-node id, if any.
    pub fn isolated_node(&self) -> Option<LayerId> {
        self.isolated_node
    }

    /// True when the host's `isolated` blend uniform should fire — i.e. the
    /// current isolation target is one of `host_id`'s modifiers (the user
    /// asked to see the mask channel as grayscale on canvas). Isolating the
    /// host itself doesn't trigger this; the host renders normally and the
    /// compose walk hides its siblings instead.
    pub(crate) fn host_renders_isolated(&self, host_id: LayerId) -> bool {
        match self.isolated_node {
            Some(t) => self.doc.modifiers_of(host_id).contains(&t),
            None => false,
        }
    }

    /// User-visible document name. Backs the tab title and the Save As
    /// picker's `suggestedName`. Persisted on disk as `manifest.name`.
    pub fn document_name(&self) -> &str {
        &self.doc.name
    }

    /// Current document canvas dimensions in pixels. Read by the WASM
    /// bridge so the JS coord transforms can mirror the actual per-doc
    /// size (rather than the global `canvas.width` config default, which
    /// only seeds new docs).
    pub fn canvas_dimensions(&self) -> (u32, u32) {
        (self.doc.width, self.doc.height)
    }

    /// True when the document has unsaved changes. Set sticky at the
    /// [`crate::undo::UndoStack::push`] chokepoint; cleared on a
    /// successful save (`poll_save_result`) or load (`open_document`
    /// installs a fresh `dirty = false` doc). UI close-tab and
    /// `beforeunload` flows consult this to decide whether to prompt.
    pub fn is_dirty(&self) -> bool {
        self.doc.dirty
    }

    /// Rename the document. Not undoable — renaming is a metadata change
    /// users expect to be free-standing, matching every other editor's
    /// "title bar rename" affordance. The save flow picks the new name
    /// up from `doc.name` the next time `start_save_document` runs.
    pub fn set_document_name(&mut self, name: String) {
        self.doc.name = name;
    }

    pub fn set_layer_name(&mut self, layer_id: LayerId, name: &str) {
        if !self.doc.is_node_editable(layer_id) {
            return;
        }
        let old_name = match self.doc.find_node(layer_id) {
            Some(n) => n.common().name.clone(),
            None => return,
        };
        if let Some(node) = self.doc.find_node_mut(layer_id) {
            node.common_mut().name = name.to_string();
        } else {
            return;
        }

        self.push_undo(Box::new(PropertyAction::new(
            layer_id,
            Property::Name(old_name),
            Property::Name(name.to_string()),
        )));
    }

    /// Push the current opacity/blend_mode of a layer or group into the
    /// compositor's uniform buffer for that node. Group isolation is driven
    /// by `engine.isolated_node` and reflected uniformly across node kinds.
    pub(crate) fn refresh_blend_uniforms(&mut self, layer_id: LayerId) {
        match self.doc.find_node(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => {
                self.compositor.update_raster_uniforms(
                    &self.gpu.queue,
                    layer_id,
                    r.blend.opacity,
                    r.blend.blend_mode.gpu_value,
                );
            }
            Some(LayerNode::Layer(Layer::Void(v))) => {
                let opacity = v.blend.opacity;
                let blend_mode_gpu = v.blend.blend_mode.gpu_value;
                let isolated = self.host_renders_isolated(layer_id);
                self.compositor.update_void_uniforms_full(
                    &self.gpu.queue,
                    layer_id,
                    opacity,
                    blend_mode_gpu,
                    isolated,
                );
            }
            Some(LayerNode::Group(g)) => {
                let opacity = g.blend.opacity;
                let blend_mode_gpu = g.blend.blend_mode.gpu_value;
                let isolated = self.host_renders_isolated(layer_id);
                self.compositor.update_group_uniforms(
                    &self.gpu.queue,
                    layer_id,
                    opacity,
                    blend_mode_gpu,
                    isolated,
                );
            }
            None => {}
        }
    }

    pub fn set_group_collapsed(&mut self, group_id: LayerId, collapsed: bool) {
        if let Some(LayerNode::Group(g)) = self.doc.find_node_mut(group_id) {
            g.collapsed = collapsed;
        }
    }

    pub fn set_group_passthrough(&mut self, group_id: LayerId, passthrough: bool) {
        if !self.doc.is_node_editable(group_id) {
            return;
        }
        let old = match self.doc.find_node(group_id) {
            Some(LayerNode::Group(g)) => g.passthrough,
            _ => return,
        };
        if let Some(LayerNode::Group(g)) = self.doc.find_node_mut(group_id) {
            g.passthrough = passthrough;
        }
        if !passthrough {
            self.compositor
                .ensure_group_state(&self.gpu.device, &self.gpu.queue, group_id);
            let isolated = self.host_renders_isolated(group_id);
            if let Some(LayerNode::Group(g)) = self.doc.find_node(group_id) {
                self.compositor.update_group_uniforms(
                    &self.gpu.queue,
                    group_id,
                    g.blend.opacity,
                    g.blend.blend_mode.gpu_value,
                    isolated,
                );
            }
        }
        self.compositor.mark_dirty();
        self.push_undo(Box::new(PropertyAction::new(
            group_id,
            Property::Passthrough(old),
            Property::Passthrough(passthrough),
        )));
    }
}
