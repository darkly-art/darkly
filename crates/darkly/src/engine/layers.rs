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
            _ => self.doc.canvas_rect(),
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

    /// Create a new group and move every id in `ids` into it, preserving
    /// their relative panel order. The group ends up at the panel-topmost
    /// selected layer's slot — its parent, its position — so the act
    /// "wraps" the selection in place. Cross-parent selections are
    /// supported; sources from other groups get pulled into the new group.
    ///
    /// Locked layers and any id whose ancestor is also in `ids` are
    /// skipped; the new group is created only if at least one editable
    /// source remains. The whole op is one [`CompoundAction`], so a
    /// single undo restores the original tree.
    pub fn group_layers(&mut self, ids: Vec<LayerId>) -> Result<LayerId, String> {
        if ids.is_empty() {
            return Err("Need at least one layer to group".into());
        }

        let mut editable: Vec<LayerId> = Vec::with_capacity(ids.len());
        for &id in &ids {
            if self.doc.find_node(id).is_none() {
                continue;
            }
            if !self.doc.is_node_editable(id) {
                continue;
            }
            // Drop any id whose ancestor is also in the batch — moving
            // the ancestor brings the descendant along; processing both
            // would yank the descendant out of its group.
            if ids
                .iter()
                .any(|&other| other != id && self.doc.is_ancestor_of(other, id))
            {
                continue;
            }
            editable.push(id);
        }
        if editable.is_empty() {
            return Err("No editable layers to group".into());
        }

        // Sort by panel order so the last entry is the panel-topmost
        // editable source — its slot is where the new group will live.
        let order = self.doc.all_node_ids_in_order();
        let order_idx = |id: LayerId| order.iter().position(|&x| x == id).unwrap_or(usize::MAX);
        editable.sort_by_key(|&id| order_idx(id));
        let topmost = *editable.last().expect("non-empty");
        let topmost_parent = self.doc.parent_of(topmost);
        let topmost_pos = self.doc.position_in_parent(topmost).unwrap_or(0);

        // Create the group at the top of root — a stable spot that
        // can't accidentally land it inside one of the sources (which
        // would happen if we anchored on a Group-typed `topmost`, since
        // `add_group(Some(group))` resolves to `IntoGroupTop(group)`).
        // We'll move the group to the topmost's slot at the end.
        let group_id = self.doc.add_group(None);
        let group_initial_parent = self.doc.parent_of(group_id);
        let group_initial_pos = self.doc.position_in_parent(group_id).unwrap_or(0);

        let mut actions: Vec<Box<dyn UndoAction>> = Vec::with_capacity(editable.len() + 2);
        actions.push(Box::new(LayerAddAction::new(
            group_id,
            group_initial_parent,
            group_initial_pos,
        )));

        // Move sources into the group, preserving bottom-first ordering
        // (so panel order top-first reads as the original layout). The
        // first source goes to IntoGroupBottom; subsequent sources chain
        // `After(prev)` so they land contiguously in the same relative
        // order they had before.
        let mut prev: Option<LayerId> = None;
        for id in editable {
            let target = match prev {
                None => MoveTarget::IntoGroupBottom(group_id),
                Some(p) => MoveTarget::After(p),
            };
            if let Some(a) = self.move_layer_inner(id, target) {
                actions.push(a);
                prev = Some(id);
            }
        }

        // Reposition the group at the topmost's original slot. The
        // topmost has already been detached (it was the last source
        // moved into the group), so its parent's children Vec is short
        // one entry — `topmost_pos` now points at whatever was just
        // above topmost in panel order. Inserting the group there
        // lands it exactly where topmost used to be.
        let group_pre_move_parent = self.doc.parent_of(group_id);
        let group_pre_move_pos = self.doc.position_in_parent(group_id).unwrap_or(0);
        self.doc.detach_for_undo(group_id);
        let clamped_pos = topmost_pos.min(match topmost_parent {
            Some(p) => self.doc.children_of(p).len(),
            None => self.doc.children_of(self.doc.root_id()).len(),
        });
        self.doc
            .reinsert_node(group_id, topmost_parent, clamped_pos);
        let group_final_parent = self.doc.parent_of(group_id);
        let group_final_pos = self.doc.position_in_parent(group_id).unwrap_or(0);
        if (group_pre_move_parent, group_pre_move_pos) != (group_final_parent, group_final_pos) {
            actions.push(Box::new(LayerMoveAction::new(
                group_id,
                group_pre_move_parent,
                group_pre_move_pos,
                group_final_parent,
                group_final_pos,
            )));
        }

        self.push_undo(Box::new(CompoundAction::new(actions)));
        self.compositor.mark_dirty();
        Ok(group_id)
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
        // Build the trait object here (engine), then hand it to the
        // compositor — the compositor stops caring about `(type_id,
        // params)` as a pair, owning only the constructed `Box<dyn Void>`.
        let format = self.compositor.canvas_content_format();
        let void = self.compositor.void_registry_mut().create_void(
            void_type,
            &params,
            &self.gpu.device,
            format,
        );
        self.compositor
            .ensure_void_layer(&self.gpu.device, &self.gpu.queue, id, void);
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
        let old_params = match self.doc.find_node(layer_id) {
            Some(LayerNode::Layer(Layer::Void(v))) => v.params.clone(),
            _ => return,
        };
        if let Some(LayerNode::Layer(Layer::Void(v))) = self.doc.find_node_mut(layer_id) {
            v.params = new_params.clone();
        }
        self.compositor
            .update_void_layer_params(&self.gpu.queue, layer_id, &new_params);
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
        // Visibility gate: a hidden layer (or any hidden ancestor) means the
        // composited output ignores this layer entirely, so the canvas blit
        // upstream of us plus this GPU copy plus the void's encode pass plus
        // the compositor recomposite would all be pure waste. The
        // authoritative answer lives in the doc — `effective_visible` walks
        // ancestors. The JS-side `CameraSource.tick()` also short-circuits
        // on visibility, but this guard is the canonical correctness one:
        // any future caller (tests, a different frontend, IPC) gets the
        // same behaviour without needing to remember the JS optimization.
        if !self.doc.effective_visible(layer_id) {
            return;
        }
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

        if let Some(action) = self.detach_layer_for_remove(layer_id) {
            self.push_undo(action);
        }
        self.compositor.mark_dirty();
        Ok(())
    }

    /// Detach a single layer for removal and return the matching undo
    /// action without pushing it. Returns `None` if `layer_id` isn't in
    /// the tree. The caller is responsible for any editability or
    /// "last layer" checks; this is the raw mutation half shared between
    /// [`Self::remove_layer`] and [`Self::remove_layers`].
    fn detach_layer_for_remove(&mut self, layer_id: LayerId) -> Option<Box<dyn UndoAction>> {
        let parent = self.doc.parent_of(layer_id);
        let pos = self.doc.position_in_parent(layer_id).unwrap_or(0);
        // Collect tombstones before detaching — `detach_for_undo` severs
        // the parent links `collect_pixel_node_ids` walks to enumerate
        // the subtree.
        let tombstones = self.collect_pixel_node_ids(layer_id);
        self.doc.detach_for_undo(layer_id)?;
        Some(Box::new(LayerRemoveAction::new(
            layer_id, parent, pos, tombstones,
        )))
    }

    /// Remove every id in `ids` in a single undo step. Locked layers and
    /// any id whose ancestor is also in `ids` are silently skipped; the
    /// return value is the count of locked layers that were ignored so the
    /// UI can surface a "N locked layers skipped" toast. Errors only when
    /// removing the editable set would leave zero layers in the document.
    pub fn remove_layers(&mut self, ids: Vec<LayerId>) -> Result<usize, String> {
        let mut editable = Vec::with_capacity(ids.len());
        let mut skipped_locked = 0usize;
        for &id in &ids {
            if self.doc.find_node(id).is_none() {
                continue;
            }
            if !self.doc.is_node_editable(id) {
                skipped_locked += 1;
                continue;
            }
            // Drop any id that's a descendant of another id already in
            // the batch — removing the ancestor takes the subtree with it.
            if ids
                .iter()
                .any(|&other| other != id && self.doc.is_ancestor_of(other, id))
            {
                continue;
            }
            editable.push(id);
        }
        if editable.is_empty() {
            self.compositor.mark_dirty();
            return Ok(skipped_locked);
        }
        if self.doc.node_count().saturating_sub(editable.len()) == 0 {
            return Err("Cannot delete the last layer".into());
        }

        self.batched_undo(&editable, |engine, id| engine.detach_layer_for_remove(id));
        self.compositor.mark_dirty();
        Ok(skipped_locked)
    }

    pub fn move_layer(&mut self, layer_id: LayerId, target: MoveTarget) {
        if let Some(action) = self.move_layer_inner(layer_id, target) {
            self.push_undo(action);
        }
        self.compositor.mark_dirty();
    }

    /// Move a single layer and return the matching undo action without
    /// pushing it. Returns `None` if `layer_id` is locked or not in the
    /// tree. Shared between [`Self::move_layer`] and
    /// [`Self::move_layers`].
    fn move_layer_inner(
        &mut self,
        layer_id: LayerId,
        target: MoveTarget,
    ) -> Option<Box<dyn UndoAction>> {
        if !self.doc.is_node_editable(layer_id) {
            return None;
        }
        let old_parent = self.doc.parent_of(layer_id);
        let old_pos = self.doc.position_in_parent(layer_id)?;
        self.doc.move_layer(layer_id, target);
        let new_parent = self.doc.parent_of(layer_id);
        let new_pos = self.doc.position_in_parent(layer_id).unwrap_or(0);
        Some(Box::new(LayerMoveAction::new(
            layer_id, old_parent, old_pos, new_parent, new_pos,
        )))
    }

    /// Move every id in `ids` to land contiguously at `target`, preserving
    /// their current relative tree order. Locked layers and any id whose
    /// ancestor is also in `ids` are silently skipped; returns the count
    /// of locked-layer skips so the UI can toast. Errors when `target`
    /// references an id in `ids` or a descendant of one (the drop is
    /// self-referential).
    pub fn move_layers(&mut self, ids: Vec<LayerId>, target: MoveTarget) -> Result<usize, String> {
        let target_id = match target {
            MoveTarget::Before(t)
            | MoveTarget::After(t)
            | MoveTarget::IntoGroupTop(t)
            | MoveTarget::IntoGroupBottom(t) => t,
        };
        for &id in &ids {
            if id == target_id || self.doc.is_ancestor_of(id, target_id) {
                return Err("Cannot move a layer into itself".into());
            }
        }

        let mut editable: Vec<LayerId> = Vec::with_capacity(ids.len());
        let mut skipped_locked = 0usize;
        for &id in &ids {
            if self.doc.find_node(id).is_none() {
                continue;
            }
            if !self.doc.is_node_editable(id) {
                skipped_locked += 1;
                continue;
            }
            if ids
                .iter()
                .any(|&other| other != id && self.doc.is_ancestor_of(other, id))
            {
                continue;
            }
            editable.push(id);
        }
        if editable.is_empty() {
            return Ok(skipped_locked);
        }

        // Sort by document DFS order so subsequent `After(prev)` chaining
        // preserves the user's original top-to-bottom layout at the
        // destination.
        let order = self.doc.all_node_ids_in_order();
        let order_idx = |id: LayerId| order.iter().position(|&x| x == id).unwrap_or(usize::MAX);
        editable.sort_by_key(|&id| order_idx(id));

        let mut actions: Vec<Box<dyn UndoAction>> = Vec::with_capacity(editable.len());
        let mut prev: Option<LayerId> = None;
        for id in editable {
            let step_target = match prev {
                None => target,
                Some(p) => MoveTarget::After(p),
            };
            if let Some(a) = self.move_layer_inner(id, step_target) {
                actions.push(a);
                prev = Some(id);
            }
        }
        if !actions.is_empty() {
            self.push_undo(Box::new(CompoundAction::new(actions)));
        }
        self.compositor.mark_dirty();
        Ok(skipped_locked)
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

    /// Snapshot the engine state the frontend mirrors. Cheap CPU reads; `render`
    /// returns this each frame so synchronous UI consumers read a local mirror
    /// instead of awaiting per-value queries. See [`crate::engine::EngineState`].
    /// Call *after* `render` so `frame_count` is the post-increment value.
    pub fn engine_state(&self) -> crate::engine::EngineState {
        crate::engine::EngineState {
            frame_count: self.frame_count() as f64,
            thumbnail_version: self.thumbnail_version(),
            dirty: self.is_dirty(),
            has_selection: self.has_selection(),
        }
    }

    /// Force the document into the unsaved state. Used after restoring a
    /// crash-recovery snapshot: the restored document is unsaved work
    /// with no backing file handle, so it must read as dirty (otherwise
    /// closing the tab would silently discard the recovered work).
    pub fn mark_dirty(&mut self) {
        self.doc.dirty = true;
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
            Some(LayerNode::Layer(layer)) => {
                let blend = layer.blend();
                let opacity = blend.opacity;
                let blend_mode_gpu = blend.blend_mode.gpu_value;
                let isolated = self.host_renders_isolated(layer_id);
                self.compositor.update_layer_uniforms(
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
