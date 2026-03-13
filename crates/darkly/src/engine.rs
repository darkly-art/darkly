use crate::clipboard::{Clipboard, ImageClip};
use crate::document::{Document, MoveTarget, SelectionMode};
use crate::gpu::transform::{FloatingContent, FloatingMode, Affine2D, IDENTITY, source_from_clip};
use crate::layer::{BlendMode, Layer, LayerNode};
use crate::undo::{
    UndoStack, TileAction, LayerAddAction, LayerRemoveAction, LayerMoveAction,
    MaskPropertyAction, PropertyAction, SelectionAction, mark_affected_dirty,
};
use crate::undo::property::Property;
use crate::gpu::compositor::Compositor;
use crate::gpu::context::GpuContext;
use crate::gpu::overlay::{
    OverlayPrimitive, KIND_DASHED_LINE, FLAG_CANVAS_SPACE,
};
use crate::gpu::params::{ParamDef, ParamValue};
use crate::gpu::view::ViewTransform;
use crate::tile::{AlphaMask, TileGrid, TILE_SIZE};

// ---------------------------------------------------------------------------
// Shared return types — serde-serializable for any FFI bridge.
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LayerInfo {
    #[serde(rename_all = "camelCase")]
    Raster {
        id: f64, name: String, visible: bool, opacity: f32, blend_mode: u32,
        has_mask: bool, mask_enabled: bool, show_mask: bool,
    },
    #[serde(rename_all = "camelCase")]
    Group {
        id: f64, name: String, visible: bool, collapsed: bool, passthrough: bool,
        opacity: f32, blend_mode: u32,
        has_mask: bool, mask_enabled: bool, show_mask: bool,
        children: Vec<LayerInfo>,
    },
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeilInfo {
    #[serde(rename = "type")]
    pub type_id: String,
    pub visible: bool,
    pub index: usize,
    pub params: Vec<ParamInfo>,
}

/// Flat serialization-friendly view of a parameter definition + current value.
/// Avoids nesting a tagged enum (ParamDef) which serde_wasm_bindgen can't handle.
#[derive(serde::Serialize)]
pub struct ParamInfo {
    pub kind: &'static str,
    pub name: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    pub default: ParamValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<ParamValue>,
}

impl ParamInfo {
    pub fn from_def(def: &ParamDef, value: Option<&ParamValue>) -> Self {
        match def {
            ParamDef::Float { name, min, max, default } => ParamInfo {
                kind: "float", name,
                min: Some(*min as f64), max: Some(*max as f64),
                default: ParamValue::Float(*default),
                value: value.cloned(),
            },
            ParamDef::Int { name, min, max, default } => ParamInfo {
                kind: "int", name,
                min: Some(*min as f64), max: Some(*max as f64),
                default: ParamValue::Int(*default),
                value: value.cloned(),
            },
            ParamDef::Bool { name, default } => ParamInfo {
                kind: "bool", name,
                min: None, max: None,
                default: ParamValue::Bool(*default),
                value: value.cloned(),
            },
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum StrokeOp {
    PaintCircle { x: f32, y: f32, radius: f32, r: u8, g: u8, b: u8, a: u8 },
    EraseCircle { x: f32, y: f32, radius: f32 },
    FloodFill { x: f32, y: f32, r: u8, g: u8, b: u8, a: u8, tolerance: u8 },
    LinearGradient {
        x0: f32, y0: f32, x1: f32, y1: f32,
        r0: u8, g0: u8, b0: u8, a0: u8,
        r1: u8, g1: u8, b1: u8, a1: u8,
    },
}

/// Data returned to the WASM bridge on copy/cut — always RGBA pixels regardless
/// of the internal clipboard variant.
#[derive(serde::Serialize)]
pub struct ClipboardExport {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub offset_x: i32,
    pub offset_y: i32,
}

// ---------------------------------------------------------------------------
// DarklyEngine — platform-agnostic editor core.
// ---------------------------------------------------------------------------

pub struct DarklyEngine {
    doc: Document,
    compositor: Compositor,
    gpu: GpuContext,
    undo_stack: UndoStack,
    active_stroke_layer: Option<u64>,
    /// Which layer has mask editing active (GIMP's `edit_mask` flag).
    /// When set, strokes are routed to the mask instead of the layer.
    editing_mask_layer: Option<u64>,
    view_transform: ViewTransform,
    /// Persistent marching ants overlay (regenerated when selection changes).
    selection_overlay: Vec<OverlayPrimitive>,
    /// Transient tool overlay (set/cleared by the active tool).
    tool_overlay: Vec<OverlayPrimitive>,
    /// Internal clipboard — holds typed content for copy/paste within Darkly.
    clipboard: Option<Clipboard>,
    /// Active floating content (paste-in-place or interactive transform).
    floating: Option<FloatingContent>,
}

impl DarklyEngine {
    pub fn new(gpu: GpuContext, doc_width: u32, doc_height: u32) -> Self {
        let compositor = Compositor::new(
            &gpu.device, &gpu.queue, gpu.surface_format(),
            doc_width, doc_height,
        );
        let doc = Document::new(doc_width, doc_height);
        let undo_stack = UndoStack::new(50);

        DarklyEngine {
            doc,
            compositor,
            gpu,
            undo_stack,
            active_stroke_layer: None,
            editing_mask_layer: None,
            view_transform: ViewTransform::identity(),
            selection_overlay: Vec::new(),
            tool_overlay: Vec::new(),
            clipboard: None,
            floating: None,
        }
    }

    // --- Layer CRUD ---

    pub fn add_raster_layer(&mut self) -> u64 {
        let id = self.doc.add_raster_layer();
        self.compositor.ensure_raster_layer(&self.gpu.device, &self.gpu.queue, id);
        self.compositor.mark_dirty();

        let parent = self.doc.parent_of(id);
        let pos = self.doc.position_in_parent(id).unwrap_or(0);
        self.undo_stack.push(Box::new(LayerAddAction::new(id, parent, pos)));

        id
    }

    pub fn add_raster_layer_in(&mut self, group_id: u64) -> u64 {
        let id = self.doc.add_raster_layer_in(Some(group_id));
        self.compositor.ensure_raster_layer(&self.gpu.device, &self.gpu.queue, id);
        self.compositor.mark_dirty();

        let parent = self.doc.parent_of(id);
        let pos = self.doc.position_in_parent(id).unwrap_or(0);
        self.undo_stack.push(Box::new(LayerAddAction::new(id, parent, pos)));

        id
    }

    pub fn add_group(&mut self) -> u64 {
        let id = self.doc.add_group();

        let parent = self.doc.parent_of(id);
        let pos = self.doc.position_in_parent(id).unwrap_or(0);
        self.undo_stack.push(Box::new(LayerAddAction::new(id, parent, pos)));

        id
    }

    pub fn remove_layer(&mut self, layer_id: u64) -> Result<(), String> {
        if self.doc.node_count() <= 1 {
            return Err("Cannot delete the last layer".into());
        }

        let parent = self.doc.parent_of(layer_id);
        let pos = self.doc.position_in_parent(layer_id).unwrap_or(0);

        if let Some(node) = self.doc.detach_for_undo(layer_id) {
            self.undo_stack.push(Box::new(LayerRemoveAction::new(node, parent, pos)));
        }

        self.compositor.mark_dirty();
        Ok(())
    }

    pub fn move_layer(&mut self, layer_id: u64, target: MoveTarget) {
        let old_parent = self.doc.parent_of(layer_id);
        let old_pos = match self.doc.position_in_parent(layer_id) {
            Some(p) => p,
            None => return,
        };

        self.doc.move_layer(layer_id, target);

        let new_parent = self.doc.parent_of(layer_id);
        let new_pos = self.doc.position_in_parent(layer_id).unwrap_or(0);

        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(LayerMoveAction::new(
            layer_id, old_parent, old_pos, new_parent, new_pos,
        )));
    }

    // --- Layer properties ---

    pub fn set_opacity(&mut self, layer_id: u64, opacity: f32) {
        let old_opacity = match self.doc.find_node(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => r.opacity,
            Some(LayerNode::Group(g)) => g.opacity,
            _ => return,
        };

        match self.doc.find_node_mut(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => r.opacity = opacity,
            Some(LayerNode::Group(g)) => g.opacity = opacity,
            _ => return,
        }

        if let Some(Layer::Raster(r)) = self.doc.layer(layer_id) {
            self.compositor.update_raster_uniforms(
                &self.gpu.queue, layer_id, r.opacity, r.blend_mode,
            );
        } else if let Some(LayerNode::Group(g)) = self.doc.find_node(layer_id) {
            if !g.passthrough {
                self.compositor.update_group_uniforms(
                    &self.gpu.queue, layer_id, g.opacity, g.blend_mode, g.show_mask,
                );
            }
        }
        self.compositor.mark_dirty();

        self.undo_stack.coalesce_property(PropertyAction::new(
            layer_id,
            Property::Opacity(old_opacity),
            Property::Opacity(opacity),
        ));
    }

    pub fn set_blend_mode(&mut self, layer_id: u64, mode: u32) {
        let blend_mode = BlendMode::from_u32(mode);

        let old_mode = match self.doc.find_node(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => r.blend_mode,
            Some(LayerNode::Group(g)) => g.blend_mode,
            _ => return,
        };

        match self.doc.find_node_mut(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => r.blend_mode = blend_mode,
            Some(LayerNode::Group(g)) => g.blend_mode = blend_mode,
            _ => return,
        }

        if let Some(Layer::Raster(r)) = self.doc.layer(layer_id) {
            self.compositor.update_raster_uniforms(
                &self.gpu.queue, layer_id, r.opacity, r.blend_mode,
            );
        } else if let Some(LayerNode::Group(g)) = self.doc.find_node(layer_id) {
            if !g.passthrough {
                self.compositor.update_group_uniforms(
                    &self.gpu.queue, layer_id, g.opacity, g.blend_mode, g.show_mask,
                );
            }
        }
        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(PropertyAction::new(
            layer_id,
            Property::BlendMode(old_mode),
            Property::BlendMode(blend_mode),
        )));
    }

    pub fn set_layer_visible(&mut self, layer_id: u64, visible: bool) {
        let old_visible = match self.doc.find_node(layer_id) {
            Some(n) => n.visible(),
            None => return,
        };

        match self.doc.find_node_mut(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => r.visible = visible,
            Some(LayerNode::Group(g)) => g.visible = visible,
            _ => return,
        }
        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(PropertyAction::new(
            layer_id,
            Property::Visible(old_visible),
            Property::Visible(visible),
        )));
    }

    pub fn set_layer_name(&mut self, layer_id: u64, name: &str) {
        let old_name = match self.doc.find_node(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => r.name.clone(),
            Some(LayerNode::Group(g)) => g.name.clone(),
            _ => return,
        };

        match self.doc.find_node_mut(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => r.name = name.to_string(),
            Some(LayerNode::Group(g)) => g.name = name.to_string(),
            _ => return,
        }

        self.undo_stack.push(Box::new(PropertyAction::new(
            layer_id,
            Property::Name(old_name),
            Property::Name(name.to_string()),
        )));
    }

    pub fn set_group_collapsed(&mut self, group_id: u64, collapsed: bool) {
        if let Some(LayerNode::Group(g)) = self.doc.find_node_mut(group_id) {
            g.collapsed = collapsed;
        }
    }

    pub fn set_group_passthrough(&mut self, group_id: u64, passthrough: bool) {
        let old = match self.doc.find_node(group_id) {
            Some(LayerNode::Group(g)) => g.passthrough,
            _ => return,
        };
        if let Some(LayerNode::Group(g)) = self.doc.find_node_mut(group_id) {
            g.passthrough = passthrough;
        }
        if !passthrough {
            self.compositor.ensure_group_state(&self.gpu.device, &self.gpu.queue, group_id);
            if let Some(LayerNode::Group(g)) = self.doc.find_node(group_id) {
                self.compositor.update_group_uniforms(
                    &self.gpu.queue, group_id, g.opacity, g.blend_mode, g.show_mask,
                );
            }
        }
        self.compositor.mark_dirty();
        self.undo_stack.push(Box::new(PropertyAction::new(
            group_id,
            Property::Passthrough(old),
            Property::Passthrough(passthrough),
        )));
    }

    // --- Layer Masks ---

    pub fn add_mask(&mut self, layer_id: u64) {

        // Snapshot old state for undo
        let node = match self.doc.find_node(layer_id) {
            Some(n) => n,
            None => return,
        };
        let m = node.as_masked();
        let (old_mask, old_enabled, old_show) = (m.mask().clone(), m.mask_enabled(), m.show_mask());

        self.doc.add_mask(layer_id);
        self.compositor.set_layer_mask(&self.gpu.device, &self.gpu.queue, layer_id, true);
        self.sync_mask_state(layer_id);
        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(MaskPropertyAction::new(
            layer_id, old_mask, old_enabled, old_show,
        )));
    }

    pub fn remove_mask(&mut self, layer_id: u64) {

        let node = match self.doc.find_node(layer_id) {
            Some(n) => n,
            None => return,
        };
        let m = node.as_masked();
        let (old_mask, old_enabled, old_show) = (m.mask().clone(), m.mask_enabled(), m.show_mask());

        self.doc.remove_mask(layer_id);
        self.editing_mask_layer = self.editing_mask_layer.filter(|&id| id != layer_id);
        self.compositor.set_layer_mask(&self.gpu.device, &self.gpu.queue, layer_id, false);
        self.sync_mask_state(layer_id);
        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(MaskPropertyAction::new(
            layer_id, old_mask, old_enabled, old_show,
        )));
    }

    pub fn apply_mask(&mut self, layer_id: u64) {
        // apply_mask is raster-only — groups have no pixel data to bake into
        let (old_mask, old_enabled, old_show) = match self.doc.layer(layer_id) {
            Some(Layer::Raster(r)) => (r.mask.clone(), r.mask_enabled, r.show_mask),
            _ => return,
        };
        if old_mask.is_none() {
            return;
        }

        // Record layer tile state before destructive bake
        self.doc.begin_transaction(layer_id);
        self.doc.apply_mask_destructive(layer_id);
        let tile_memento = self.doc.commit_transaction(layer_id);

        self.editing_mask_layer = self.editing_mask_layer.filter(|&id| id != layer_id);
        self.compositor.set_layer_mask(&self.gpu.device, &self.gpu.queue, layer_id, false);
        self.sync_mask_state(layer_id);
        self.compositor.mark_dirty();

        // Push tile action first (for the alpha bake), then mask property action
        if let Some(memento) = tile_memento {
            self.undo_stack.push(Box::new(TileAction::new(memento)));
        }
        self.undo_stack.push(Box::new(MaskPropertyAction::new(
            layer_id, old_mask, old_enabled, old_show,
        )));
    }

    pub fn set_mask_enabled(&mut self, layer_id: u64, enabled: bool) {

        let old = match self.doc.find_node(layer_id) {
            Some(n) => n.as_masked().mask_enabled(),
            None => return,
        };
        self.doc.set_mask_enabled(layer_id, enabled);
        self.sync_mask_state(layer_id);
        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(MaskPropertyAction::new(
            layer_id, None, old, false,
        )));
    }

    pub fn set_show_mask(&mut self, layer_id: u64, show: bool) {

        let old = match self.doc.find_node(layer_id) {
            Some(n) => n.as_masked().show_mask(),
            None => return,
        };
        self.doc.set_show_mask(layer_id, show);
        self.sync_mask_state(layer_id);
        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(MaskPropertyAction::new(
            layer_id, None, false, old,
        )));
    }

    pub fn set_editing_mask(&mut self, layer_id: u64, editing: bool) {
        if editing {
            self.editing_mask_layer = Some(layer_id);
        } else if self.editing_mask_layer == Some(layer_id) {
            self.editing_mask_layer = None;
        }
    }

    pub fn selection_to_mask(&mut self, layer_id: u64) {

        let node = match self.doc.find_node(layer_id) {
            Some(n) => n,
            None => return,
        };
        let m = node.as_masked();
        let (old_mask, old_enabled, old_show) = (m.mask().clone(), m.mask_enabled(), m.show_mask());

        self.doc.selection_to_mask(layer_id);
        self.compositor.set_layer_mask(&self.gpu.device, &self.gpu.queue, layer_id, true);

        // Mark all mask tiles dirty for upload
        let mask_coords: Vec<(i32, i32)> = self.doc.find_node(layer_id)
            .and_then(|n| n.as_masked().mask().as_ref())
            .map(|m| m.iter().map(|((tx, ty), _)| (tx, ty)).collect())
            .unwrap_or_default();
        let dirty = self.doc.mask_dirty.entry(layer_id).or_default();
        for (tx, ty) in mask_coords {
            dirty.mark(tx, ty);
        }

        self.sync_mask_state(layer_id);
        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(MaskPropertyAction::new(
            layer_id, old_mask, old_enabled, old_show,
        )));
    }

    pub fn mask_to_selection(&mut self, layer_id: u64) {
        let old_sel = self.doc.selection.clone();
        self.doc.mask_to_selection(layer_id);
        self.undo_stack.push(Box::new(SelectionAction::new(old_sel)));
        self.update_selection_overlay();
    }

    /// Sync compositor mask state (bind group + uniforms) for a layer or group.
    fn sync_mask_state(&mut self, layer_id: u64) {

        let node = match self.doc.find_node(layer_id) {
            Some(n) => n,
            None => return,
        };
        let m = node.as_masked();
        let has_mask = m.mask().is_some();
        let mask_enabled = m.mask_enabled();
        let show_mask = m.show_mask();

        self.compositor.set_layer_mask(&self.gpu.device, &self.gpu.queue, layer_id, has_mask);
        self.compositor.update_mask_binding(
            &self.gpu.device, layer_id, mask_enabled, show_mask,
        );

        // Update uniforms for the appropriate cache type
        match node {
            LayerNode::Layer(Layer::Raster(r)) => {
                self.compositor.update_raster_uniforms_full(
                    &self.gpu.queue, layer_id, r.opacity, r.blend_mode, show_mask,
                );
            }
            LayerNode::Group(g) => {
                self.compositor.update_group_uniforms(
                    &self.gpu.queue, layer_id, g.opacity, g.blend_mode, show_mask,
                );
            }
        }
    }

    // --- Painting ---

    pub fn paint(
        &mut self,
        layer_id: u64,
        x: f32, y: f32, radius: f32,
        r: u8, g: u8, b: u8, a: u8,
    ) {
        self.doc.paint_circle(layer_id, x, y, radius, [r, g, b, a]);
    }

    pub fn fill_gradient(&mut self, layer_id: u64) {
        self.doc.fill_gradient(layer_id);
    }

    // --- Stroke lifecycle ---
    // Following GIMP's edit_mask flag: when editing_mask_layer is set,
    // strokes are routed to the mask instead of the layer.

    pub fn begin_stroke(&mut self, layer_id: u64) {
        self.auto_commit_floating();
        self.doc.set_mask_editing(
            if self.editing_mask_layer == Some(layer_id) { Some(layer_id) } else { None }
        );
        self.doc.begin_transaction(layer_id);
        self.active_stroke_layer = Some(layer_id);
    }

    pub fn stroke_to(&mut self, op: StrokeOp) {
        let layer_id = match self.active_stroke_layer {
            Some(id) => id,
            None => return,
        };
        match op {
            StrokeOp::PaintCircle { x, y, radius, r, g, b, a } =>
                self.doc.paint_circle(layer_id, x, y, radius, [r, g, b, a]),
            StrokeOp::EraseCircle { x, y, radius } =>
                self.doc.erase_circle(layer_id, x, y, radius),
            StrokeOp::FloodFill { x, y, r, g, b, a, tolerance } =>
                self.doc.flood_fill(layer_id, x as i32, y as i32, [r, g, b, a], tolerance),
            StrokeOp::LinearGradient { x0, y0, x1, y1, r0, g0, b0, a0, r1, g1, b1, a1 } =>
                self.doc.linear_gradient(layer_id, x0, y0, x1, y1,
                    [r0, g0, b0, a0], [r1, g1, b1, a1]),
        }
    }

    pub fn end_stroke(&mut self) {
        if let Some(layer_id) = self.active_stroke_layer.take() {
            if let Some(memento) = self.doc.commit_transaction(layer_id) {
                self.undo_stack.push(Box::new(TileAction::new(memento)));
            }
            self.doc.set_mask_editing(None);
        }
    }

    // --- View transform ---

    pub fn set_view_transform(
        &mut self,
        pan_x: f32, pan_y: f32,
        zoom: f32, rotation: f32,
        screen_w: f32, screen_h: f32,
    ) {
        let transform = ViewTransform::from_pan_zoom_rotate(
            pan_x, pan_y, zoom, rotation,
            screen_w, screen_h,
            self.doc.width as f32, self.doc.height as f32,
        );
        self.view_transform = transform;
        self.compositor.update_view_transform(&self.gpu.queue, &transform);
        self.compositor.mark_needs_present();
    }

    pub fn screen_to_canvas(&self, screen_x: f32, screen_y: f32) -> (f32, f32) {
        self.view_transform.screen_to_canvas(screen_x, screen_y)
    }

    pub fn pick_color(&self, _x: f32, _y: f32) -> [u8; 4] {
        // TODO: GPU readback from composite cache.
        [0, 0, 0, 255]
    }

    // --- Thumbnails ---

    /// Generate an RGBA thumbnail of a layer's content.
    /// Returns `width * height * 4` bytes (RGBA). Transparent areas show checkerboard.
    pub fn layer_thumbnail(&self, layer_id: u64, width: u32, height: u32) -> Vec<u8> {
        let tiles = match self.doc.layer(layer_id) {
            Some(Layer::Raster(r)) => &r.tiles,
            _ => return vec![0u8; (width * height * 4) as usize],
        };
        generate_rgba_thumbnail(tiles, self.doc.width, self.doc.height, width, height)
    }

    /// Generate an RGBA thumbnail of a layer's mask (grayscale).
    /// Returns empty vec if the layer has no mask.
    pub fn mask_thumbnail(&self, layer_id: u64, width: u32, height: u32) -> Vec<u8> {
        let mask = match self.doc.find_node(layer_id)
            .and_then(|n| n.as_masked().mask().as_ref())
        {
            Some(m) => m,
            None => return Vec::new(),
        };
        generate_mask_thumbnail(mask, self.doc.width, self.doc.height, width, height)
    }

    // --- Rendering ---

    /// Render a frame. Returns true if animations need another frame.
    pub fn render(&mut self, time_secs: f32) -> bool {
        self.compositor.update_animations(&self.gpu.queue, time_secs);
        self.compositor.render(
            &self.gpu.device,
            &self.gpu.queue,
            &self.gpu.surface,
            &self.gpu.surface_config,
            &mut self.doc,
        );
        self.compositor.needs_animation()
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.gpu.resize(width, height);
        self.compositor.veil_chain_mut().resize(&self.gpu.device, &self.gpu.queue, width, height);
        self.compositor.mark_needs_present();
    }

    // --- Undo / Redo ---

    pub fn undo(&mut self) {
        self.auto_commit_floating();
        if let Some(affected) = self.undo_stack.undo(&mut self.doc) {
            mark_affected_dirty(&mut self.doc.dirty, &affected);
            self.sync_compositor_layers();
            self.compositor.mark_dirty();
            self.update_selection_overlay();
        }
    }

    pub fn redo(&mut self) {
        self.auto_commit_floating();
        if let Some(affected) = self.undo_stack.redo(&mut self.doc) {
            mark_affected_dirty(&mut self.doc.dirty, &affected);
            self.sync_compositor_layers();
            self.compositor.mark_dirty();
            self.update_selection_overlay();
        }
    }

    // --- Veils ---

    pub fn add_veil(&mut self, veil_type: &str, params: &[ParamValue]) {
        let chain = self.compositor.veil_chain_mut();
        let format = chain.accum_format();
        let veil = chain.registry_mut().create_veil(
            veil_type, params, &self.gpu.device, format,
        );
        chain.add_veil(&self.gpu.device, &self.gpu.queue, veil);
    }

    pub fn remove_veil(&mut self, index: usize) {
        self.compositor.veil_chain_mut().remove_veil(index);
    }

    pub fn clear_veils(&mut self) {
        self.compositor.veil_chain_mut().clear_veils();
    }

    pub fn set_veil_visible(&mut self, index: usize, visible: bool) {
        self.compositor.veil_chain_mut().set_veil_visible(index, visible);
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
        let new_veil = chain.registry_mut().create_veil(
            type_id, params, &self.gpu.device, format,
        );
        chain.update_veil(&self.gpu.device, &self.gpu.queue, index, new_veil);
    }

    // --- Queries ---

    pub fn layer_tree(&self) -> Vec<LayerInfo> {
        self.doc.root.children.iter().rev().map(node_to_layer_info).collect()
    }

    pub fn veil_list(&self) -> Vec<VeilInfo> {
        let chain = self.compositor.veil_chain();
        let count = chain.count();
        let mut list = Vec::with_capacity(count);
        for i in (0..count).rev() {
            if let Some((type_id, visible)) = chain.info(i) {
                let param_defs = chain.registry().param_defs(type_id);
                let values = chain.param_values(i).unwrap_or_default();
                let params = param_defs.iter().enumerate().map(|(j, def)| {
                    ParamInfo::from_def(def, values.get(j))
                }).collect();
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

    /// Get the parameter definitions for a veil type.
    pub fn veil_param_defs(&self, type_id: &str) -> &'static [ParamDef] {
        self.compositor.veil_chain().registry().param_defs(type_id)
    }

    // --- Tool Overlay ---

    pub fn set_overlay_primitives(&mut self, prims: Vec<OverlayPrimitive>) {
        self.tool_overlay = prims;
        self.push_merged_overlay();
    }

    pub fn clear_overlay(&mut self) {
        self.tool_overlay.clear();
        self.push_merged_overlay();
    }

    pub fn overlay_hit_test(&self, screen_x: f32, screen_y: f32) -> Option<usize> {
        self.compositor.overlay_hit_test(screen_x, screen_y)
    }

    /// Merge selection_overlay + tool_overlay and push to compositor.
    fn push_merged_overlay(&mut self) {
        let mut merged = Vec::with_capacity(self.selection_overlay.len() + self.tool_overlay.len());
        merged.extend_from_slice(&self.selection_overlay);
        merged.extend_from_slice(&self.tool_overlay);
        if merged.is_empty() {
            self.compositor.clear_overlay();
        } else {
            self.compositor.set_overlay_primitives(merged);
        }
    }

    // --- Selection ---

    pub fn select_rect(
        &mut self,
        x: f32, y: f32, w: f32, h: f32,
        mode: SelectionMode,
        antialias: bool,
        feather: f32,
    ) {
        let old_sel = self.doc.selection.clone();
        let mask = crate::tools::rect_select::rasterize(x, y, w, h, antialias, feather);
        self.doc.apply_selection(mask, mode);
        self.undo_stack.push(Box::new(SelectionAction::new(old_sel)));
        self.update_selection_overlay();
    }

    pub fn select_ellipse(
        &mut self,
        x: f32, y: f32, w: f32, h: f32,
        mode: SelectionMode,
        antialias: bool,
        feather: f32,
    ) {
        let old_sel = self.doc.selection.clone();
        let mask = crate::tools::ellipse_select::rasterize(x, y, w, h, antialias, feather);
        self.doc.apply_selection(mask, mode);
        self.undo_stack.push(Box::new(SelectionAction::new(old_sel)));
        self.update_selection_overlay();
    }

    pub fn select_lasso(
        &mut self,
        vertices: &[[f32; 2]],
        mode: SelectionMode,
        antialias: bool,
        feather: f32,
    ) {
        let old_sel = self.doc.selection.clone();
        let mask = crate::tools::lasso_select::rasterize(vertices, antialias, feather);
        self.doc.apply_selection(mask, mode);
        self.undo_stack.push(Box::new(SelectionAction::new(old_sel)));
        self.update_selection_overlay();
    }

    pub fn select_magic_wand(
        &mut self,
        layer_id: u64,
        seed_x: i32,
        seed_y: i32,
        tolerance: u8,
        mode: SelectionMode,
    ) {
        let source = match self.doc.layer(layer_id) {
            Some(Layer::Raster(r)) => &r.tiles,
            _ => return,
        };
        let mask = crate::tools::magic_wand::rasterize(
            source,
            seed_x, seed_y,
            self.doc.width as i32, self.doc.height as i32,
            tolerance,
        );
        let old_sel = self.doc.selection.clone();
        self.doc.apply_selection(mask, mode);
        self.undo_stack.push(Box::new(SelectionAction::new(old_sel)));
        self.update_selection_overlay();
    }

    pub fn clear_selection(&mut self) {
        if self.doc.selection.is_none() {
            return;
        }
        let old_sel = self.doc.selection.clone();
        self.doc.selection = None;
        self.undo_stack.push(Box::new(SelectionAction::new(old_sel)));
        self.update_selection_overlay();
    }

    pub fn select_all(&mut self) {
        let old_sel = self.doc.selection.clone();
        let mask = crate::tools::rect_select::rasterize(
            0.0, 0.0, self.doc.width as f32, self.doc.height as f32, false, 0.0,
        );
        self.doc.selection = Some(mask);
        self.undo_stack.push(Box::new(SelectionAction::new(old_sel)));
        self.update_selection_overlay();
    }

    pub fn invert_selection(&mut self) {
        let old_sel = self.doc.selection.clone();
        if let Some(sel) = &mut self.doc.selection {
            sel.invert(self.doc.width, self.doc.height);
        }
        self.undo_stack.push(Box::new(SelectionAction::new(old_sel)));
        self.update_selection_overlay();
    }

    pub fn clear_selection_contents(&mut self, layer_id: u64) {
        self.auto_commit_floating();
        if self.doc.selection.is_none() {
            return;
        }
        self.doc.begin_transaction(layer_id);
        self.doc.clear_selection_contents(layer_id);
        if let Some(memento) = self.doc.commit_transaction(layer_id) {
            self.undo_stack.push(Box::new(TileAction::new(memento)));
        }
    }

    pub fn has_selection(&self) -> bool {
        self.doc.selection.is_some()
    }

    // --- Copy / Cut / Paste ---

    /// Copy the active layer's content (masked by selection) into the internal
    /// clipboard. Returns a `ClipboardExport` with raw RGBA bytes for the JS
    /// side to push to the system clipboard as PNG.
    pub fn copy(&mut self, layer_id: u64) -> Option<ClipboardExport> {
        let layer = match self.doc.layer(layer_id) {
            Some(Layer::Raster(r)) => r,
            _ => return None,
        };

        // When editing a mask, copy from the mask instead of the layer tiles.
        let clip = if self.editing_mask_layer == Some(layer_id) {
            let mask = layer.mask.as_ref()?;
            ImageClip::from_mask(mask, self.doc.selection.as_ref())?
        } else {
            ImageClip::from_layer(
                layer,
                self.doc.selection.as_ref(),
                self.doc.width,
                self.doc.height,
            )?
        };

        let (rgba, width, height, offset_x, offset_y) = clip.to_rgba();
        self.clipboard = Some(Clipboard::ImageData(clip));

        Some(ClipboardExport { rgba, width, height, offset_x, offset_y })
    }

    /// Cut = copy + clear selection contents. Returns the same export as copy.
    pub fn cut(&mut self, layer_id: u64) -> Option<ClipboardExport> {
        let export = self.copy(layer_id)?;

        // Clear the selected region (or entire layer if no selection).
        if self.doc.selection.is_some() {
            self.doc.begin_transaction(layer_id);
            self.doc.clear_selection_contents(layer_id);
            if let Some(memento) = self.doc.commit_transaction(layer_id) {
                self.undo_stack.push(Box::new(TileAction::new(memento)));
            }
        } else {
            // No selection — clear all layer tiles.
            self.doc.begin_transaction(layer_id);
            if let Some(Layer::Raster(r)) = self.doc.layer_mut(layer_id) {
                // Touch each tile to record it in the transaction, then clear.
                let keys: Vec<(i32, i32)> = r.tiles.iter().map(|(k, _)| k).collect();
                for (tx, ty) in keys {
                    r.tiles.get_or_create(tx, ty).write().0.fill(0);
                }
            }
            if let Some(memento) = self.doc.commit_transaction(layer_id) {
                self.undo_stack.push(Box::new(TileAction::new(memento)));
            }
        }

        Some(export)
    }

    /// Paste raw RGBA bytes as a new layer. Used for both internal and external
    /// clipboard content. Returns the new layer ID.
    pub fn paste_image(
        &mut self,
        width: u32,
        height: u32,
        rgba: &[u8],
        offset_x: i32,
        offset_y: i32,
        active_layer_id: Option<u64>,
    ) -> u64 {
        let clip = ImageClip::from_rgba(width, height, rgba, offset_x, offset_y);

        // Create a new layer and insert above the active layer.
        let id = self.doc.add_raster_layer();
        if let Some(Layer::Raster(r)) = self.doc.layer_mut(id) {
            r.name = "Pasted Layer".to_string();
            clip.write_to_layer(&mut r.tiles, offset_x, offset_y);
        }

        // Mark all written tiles dirty for GPU upload.
        let tile_keys: Vec<(i32, i32)> = self.doc.layer(id)
            .and_then(|l| match l { Layer::Raster(r) => Some(r) })
            .map(|r| r.tiles.iter().map(|(k, _)| k).collect())
            .unwrap_or_default();
        let dirty = self.doc.dirty.entry(id).or_default();
        for (tx, ty) in tile_keys {
            dirty.mark(tx, ty);
        }

        self.compositor.ensure_raster_layer(&self.gpu.device, &self.gpu.queue, id);
        self.compositor.mark_dirty();

        // Position above active layer if specified.
        if let Some(active_id) = active_layer_id {
            self.doc.move_layer(id, MoveTarget::After(active_id));
        }

        let parent = self.doc.parent_of(id);
        let pos = self.doc.position_in_parent(id).unwrap_or(0);
        self.undo_stack.push(Box::new(LayerAddAction::new(id, parent, pos)));

        id
    }

    /// Paste from the internal clipboard at its original position.
    /// Returns the new layer ID, or None if clipboard is empty.
    pub fn paste_in_place(&mut self, active_layer_id: Option<u64>) -> Option<u64> {
        let clip = self.clipboard.as_ref()?.as_image()?;
        let (rgba, width, height, offset_x, offset_y) = clip.to_rgba();
        Some(self.paste_image(width, height, &rgba, offset_x, offset_y, active_layer_id))
    }

    // --- Floating Content (Phase 7) ---

    /// Auto-commit any active floating content before performing other edits.
    /// Call this before operations that would conflict with floating content
    /// (layer switch, paint, undo, etc.).
    pub fn auto_commit_floating(&mut self) {
        if self.floating.is_some() {
            self.commit_floating();
        }
    }

    /// Check if there is active floating content.
    pub fn has_floating(&self) -> bool {
        self.floating.is_some()
    }

    /// Return floating content info for the frontend overlay:
    /// (source_origin_x, source_origin_y, source_width, source_height, matrix[6]).
    /// Returns None if no floating content is active.
    pub fn floating_info(&self) -> Option<(f32, f32, f32, f32, Affine2D)> {
        self.floating.as_ref().map(|fc| (
            fc.source_origin.0 as f32,
            fc.source_origin.1 as f32,
            fc.source_width as f32,
            fc.source_height as f32,
            fc.matrix,
        ))
    }

    /// Paste from the internal clipboard as floating content on the current
    /// layer/mask. Returns true if floating content was created.
    pub fn paste_in_place_floating(&mut self, layer_id: u64) -> bool {
        // Auto-commit any existing floating content first.
        self.auto_commit_floating();

        let clip = match self.clipboard.as_ref().and_then(|c| c.as_image()) {
            Some(c) => c,
            None => return false,
        };

        let target_is_mask = self.editing_mask_layer == Some(layer_id);

        // Clone source tiles and dimensions from clipboard
        let (source_tiles, source_origin, source_width, source_height) = source_from_clip(clip);

        // Upload to GPU for preview
        self.compositor.set_floating_content(
            &self.gpu.device,
            &self.gpu.queue,
            &source_tiles,
            source_origin,
            source_width,
            source_height,
            layer_id,
            target_is_mask,
        );

        self.floating = Some(FloatingContent {
            source_tiles,
            source_origin,
            source_width,
            source_height,
            matrix: IDENTITY,
            target_layer: layer_id,
            target_is_mask,
            mode: FloatingMode::Paste,
        });

        true
    }

    /// Begin an interactive transform on the current layer/mask content.
    /// Returns true if floating content was created.
    pub fn begin_transform(&mut self, layer_id: u64) -> bool {
        self.auto_commit_floating();

        let target_is_mask = self.editing_mask_layer == Some(layer_id);

        let layer = match self.doc.layer(layer_id) {
            Some(Layer::Raster(r)) => r,
            _ => return false,
        };

        // Clone the relevant tiles
        let (source_tiles, source_origin, source_width, source_height) = if target_is_mask {
            let mask = match &layer.mask {
                Some(m) => m,
                None => return false,
            };
            // Convert mask to RGBA for the floating content
            let clip = match ImageClip::from_mask(mask, self.doc.selection.as_ref()) {
                Some(c) => c,
                None => return false,
            };
            source_from_clip(&clip)
        } else {
            let clip = match ImageClip::from_layer(
                layer,
                self.doc.selection.as_ref(),
                self.doc.width,
                self.doc.height,
            ) {
                Some(c) => c,
                None => return false,
            };
            source_from_clip(&clip)
        };

        // Clear the source tiles (within a transaction for undo)
        self.doc.begin_transaction(layer_id);
        if target_is_mask {
            if let Some(Layer::Raster(r)) = self.doc.layer_mut(layer_id) {
                if let Some(mask) = &mut r.mask {
                    clear_mask_in_bounds(mask, source_origin, source_width, source_height);
                }
            }
        } else {
            if let Some(Layer::Raster(r)) = self.doc.layer_mut(layer_id) {
                clear_rgba_in_bounds(&mut r.tiles, source_origin, source_width, source_height);
            }
        }
        let cancel_undo: Option<Box<dyn crate::undo::UndoAction>> =
            self.doc.commit_transaction(layer_id)
                .map(|m| Box::new(TileAction::new(m)) as Box<dyn crate::undo::UndoAction>);

        // Mark cleared tiles dirty
        self.mark_bounds_dirty(layer_id, target_is_mask, source_origin, source_width, source_height);

        // Upload to GPU for preview
        self.compositor.set_floating_content(
            &self.gpu.device,
            &self.gpu.queue,
            &source_tiles,
            source_origin,
            source_width,
            source_height,
            layer_id,
            target_is_mask,
        );

        self.floating = Some(FloatingContent {
            source_tiles,
            source_origin,
            source_width,
            source_height,
            matrix: IDENTITY,
            target_layer: layer_id,
            target_is_mask,
            mode: match cancel_undo {
                Some(action) => FloatingMode::Transform { cancel_undo: action },
                None => FloatingMode::Paste, // No tiles were cleared, treat like paste
            },
        });

        true
    }

    /// Update the floating content's transform matrix.
    pub fn update_floating_matrix(&mut self, matrix: Affine2D) {
        if let Some(fc) = &mut self.floating {
            fc.matrix = matrix;
            self.compositor.update_floating_matrix(
                &self.gpu.queue,
                &matrix,
                fc.source_origin,
                fc.source_width,
                fc.source_height,
            );
        }
    }

    /// Commit floating content: rasterize transformed pixels into the target.
    pub fn commit_floating(&mut self) {
        let fc = match self.floating.take() {
            Some(fc) => fc,
            None => return,
        };

        let layer_id = fc.target_layer;
        let is_mask = fc.target_is_mask;

        // Temporarily route transaction to the correct surface
        let was_editing_mask = self.editing_mask_layer;
        if is_mask {
            self.editing_mask_layer = Some(layer_id);
        } else if self.editing_mask_layer == Some(layer_id) {
            self.editing_mask_layer = None;
        }

        self.doc.begin_transaction(layer_id);

        // Clone selection for use during rasterization (avoids borrow conflict
        // with layer_mut).
        let sel = self.doc.selection.clone();

        if is_mask {
            if let Some(Layer::Raster(r)) = self.doc.layer_mut(layer_id) {
                if let Some(mask) = &mut r.mask {
                    fc.rasterize_to_mask(mask, sel.as_ref());
                }
            }
        } else {
            if let Some(Layer::Raster(r)) = self.doc.layer_mut(layer_id) {
                fc.rasterize_to_tiles(&mut r.tiles, sel.as_ref());
            }
        }

        // Compute dirty bounds before consuming fc.
        let (min_x, min_y, max_x, max_y) = fc.transformed_bounds();

        if let Some(memento) = self.doc.commit_transaction(layer_id) {
            let rasterize_action: Box<dyn crate::undo::UndoAction> =
                Box::new(TileAction::new(memento));
            match fc.mode {
                FloatingMode::Transform { cancel_undo } => {
                    // Compound: [clear, rasterize] — undo reverses both in one step.
                    self.undo_stack.push(Box::new(
                        crate::undo::CompoundAction::new(vec![cancel_undo, rasterize_action]),
                    ));
                }
                FloatingMode::Paste => {
                    self.undo_stack.push(rasterize_action);
                }
            }
        }

        // Restore mask editing state
        self.editing_mask_layer = was_editing_mask;

        // Mark dirty
        let w = (max_x - min_x).max(0) as u32;
        let h = (max_y - min_y).max(0) as u32;
        self.mark_bounds_dirty(layer_id, is_mask, (min_x, min_y), w, h);

        // Clean up GPU state
        self.compositor.clear_floating_content();
    }

    /// Cancel floating content: discard or restore original tiles.
    pub fn cancel_floating(&mut self) {
        let fc = match self.floating.take() {
            Some(fc) => fc,
            None => return,
        };

        match fc.mode {
            FloatingMode::Paste => {
                // No-op — target layer was never modified.
            }
            FloatingMode::Transform { mut cancel_undo } => {
                // Restore the original tiles that were cleared.
                let dirty = cancel_undo.undo(&mut self.doc);
                for (layer_id, coords) in dirty {
                    let entry = self.doc.dirty.entry(layer_id).or_default();
                    for (tx, ty) in coords {
                        entry.mark(tx, ty);
                    }
                }
            }
        }

        self.compositor.clear_floating_content();
    }

    fn mark_bounds_dirty(
        &mut self,
        layer_id: u64,
        is_mask: bool,
        origin: (i32, i32),
        width: u32,
        height: u32,
    ) {
        let (ox, oy) = origin;
        let tx_min = TileGrid::tile_coords_for_pixel(ox, 0).0;
        let ty_min = TileGrid::tile_coords_for_pixel(0, oy).1;
        let tx_max = TileGrid::tile_coords_for_pixel(ox + width as i32 - 1, 0).0;
        let ty_max = TileGrid::tile_coords_for_pixel(0, oy + height as i32 - 1).1;

        let dirty = if is_mask {
            self.doc.mask_dirty.entry(layer_id).or_default()
        } else {
            self.doc.dirty.entry(layer_id).or_default()
        };
        for ty in ty_min..=ty_max {
            for tx in tx_min..=tx_max {
                dirty.mark(tx, ty);
            }
        }
    }

    /// Regenerate marching ants overlay from the current selection.
    fn update_selection_overlay(&mut self) {
        self.selection_overlay.clear();

        if let Some(sel) = &self.doc.selection {
            let segments = sel.contour_segments(0.5);
            for (a, b) in &segments {
                // Black background line (slightly thicker, solid)
                let mut bg = OverlayPrimitive::new(
                    KIND_DASHED_LINE,
                    FLAG_CANVAS_SPACE,
                    *a, *b,
                );
                bg.color = [0.0, 0.0, 0.0, 1.0];
                bg.thickness = 1.5;
                bg.dash_len = 0.0; // solid
                self.selection_overlay.push(bg);
            }
            for (a, b) in &segments {
                // White foreground dashes
                let mut fg = OverlayPrimitive::new(
                    KIND_DASHED_LINE,
                    FLAG_CANVAS_SPACE,
                    *a, *b,
                );
                fg.color = [1.0, 1.0, 1.0, 1.0];
                fg.thickness = 1.0;
                fg.dash_len = 8.0;
                self.selection_overlay.push(fg);
            }
        }

        self.push_merged_overlay();
    }

    // --- Internal helpers ---

    fn sync_compositor_layers(&mut self) {
        // Collect raster layer info first to avoid borrow conflicts with mask_dirty.
        struct RasterInfo {
            id: u64,
            opacity: f32,
            blend_mode: BlendMode,
            show_mask: bool,
            mask_enabled: bool,
            has_mask: bool,
            mask_coords: Vec<(i32, i32)>,
        }
        let infos: Vec<RasterInfo> = self.doc.all_raster_layers().into_iter().map(|r| {
            let mask_coords: Vec<(i32, i32)> = r.mask.as_ref()
                .map(|m| m.iter().map(|((tx, ty), _)| (tx, ty)).collect())
                .unwrap_or_default();
            RasterInfo {
                id: r.id, opacity: r.opacity, blend_mode: r.blend_mode,
                show_mask: r.show_mask, mask_enabled: r.mask_enabled,
                has_mask: r.mask.is_some(), mask_coords,
            }
        }).collect();

        for info in &infos {
            self.compositor.ensure_raster_layer(&self.gpu.device, &self.gpu.queue, info.id);
            self.compositor.update_raster_uniforms_full(
                &self.gpu.queue, info.id, info.opacity, info.blend_mode, info.show_mask,
            );
            self.compositor.set_layer_mask(&self.gpu.device, &self.gpu.queue, info.id, info.has_mask);
            self.compositor.update_mask_binding(
                &self.gpu.device, info.id, info.mask_enabled, info.show_mask,
            );
            // Mark all mask tiles dirty for re-upload after undo/redo
            if !info.mask_coords.is_empty() {
                let dirty = self.doc.mask_dirty.entry(info.id).or_default();
                for &(tx, ty) in &info.mask_coords {
                    dirty.mark(tx, ty);
                }
            }
        }

        // Sync non-passthrough group state
        let groups: Vec<(u64, f32, BlendMode, bool)> = self.doc.all_groups()
            .iter()
            .filter(|g| !g.passthrough)
            .map(|g| (g.id, g.opacity, g.blend_mode, g.show_mask))
            .collect();
        for (id, opacity, blend_mode, show_mask) in groups {
            self.compositor.ensure_group_state(&self.gpu.device, &self.gpu.queue, id);
            self.compositor.update_group_uniforms(&self.gpu.queue, id, opacity, blend_mode, show_mask);
        }
    }
}

fn node_to_layer_info(node: &LayerNode) -> LayerInfo {
    match node {
        LayerNode::Layer(layer) => match layer {
            Layer::Raster(r) => LayerInfo::Raster {
                id: r.id as f64,
                name: r.name.clone(),
                visible: r.visible,
                opacity: r.opacity,
                blend_mode: r.blend_mode as u32,
                has_mask: r.mask.is_some(),
                mask_enabled: r.mask_enabled,
                show_mask: r.show_mask,
            },
        },
        LayerNode::Group(g) => LayerInfo::Group {
            id: g.id as f64,
            name: g.name.clone(),
            visible: g.visible,
            collapsed: g.collapsed,
            passthrough: g.passthrough,
            opacity: g.opacity,
            blend_mode: g.blend_mode as u32,
            has_mask: g.mask.is_some(),
            mask_enabled: g.mask_enabled,
            show_mask: g.show_mask,
            children: g.children.iter().rev().map(node_to_layer_info).collect(),
        },
    }
}

// ---------------------------------------------------------------------------
// Thumbnail generation — CPU-side nearest-neighbor sampling from tile data
// ---------------------------------------------------------------------------

fn generate_rgba_thumbnail(
    tiles: &TileGrid,
    doc_w: u32, doc_h: u32,
    thumb_w: u32, thumb_h: u32,
) -> Vec<u8> {
    let mut buf = vec![0u8; (thumb_w * thumb_h * 4) as usize];
    let ts = TILE_SIZE as u32;

    for oy in 0..thumb_h {
        let cy = (oy * doc_h / thumb_h).min(doc_h - 1);
        let ty = (cy / ts) as i32;
        let ly = (cy % ts) as usize;

        for ox in 0..thumb_w {
            let cx = (ox * doc_w / thumb_w).min(doc_w - 1);
            let tx = (cx / ts) as i32;
            let lx = (cx % ts) as usize;

            let off = ((oy * thumb_w + ox) * 4) as usize;

            let (r, g, b, a) = if let Some(tile) = tiles.get(tx, ty) {
                let p = tile.data().pixel(lx, ly);
                (p[0], p[1], p[2], p[3])
            } else {
                (0, 0, 0, 0)
            };

            // Checkerboard behind transparent areas
            let check = if ((ox / 4) + (oy / 4)) % 2 == 0 { 102u8 } else { 153u8 };
            let af = a as f32 / 255.0;
            buf[off]     = (r as f32 * af + check as f32 * (1.0 - af)) as u8;
            buf[off + 1] = (g as f32 * af + check as f32 * (1.0 - af)) as u8;
            buf[off + 2] = (b as f32 * af + check as f32 * (1.0 - af)) as u8;
            buf[off + 3] = 255;
        }
    }
    buf
}

fn generate_mask_thumbnail(
    mask: &AlphaMask,
    doc_w: u32, doc_h: u32,
    thumb_w: u32, thumb_h: u32,
) -> Vec<u8> {
    let mut buf = vec![0u8; (thumb_w * thumb_h * 4) as usize];
    let ts = TILE_SIZE as u32;

    for oy in 0..thumb_h {
        let cy = (oy * doc_h / thumb_h).min(doc_h - 1);
        let ty = (cy / ts) as i32;
        let ly = (cy % ts) as usize;

        for ox in 0..thumb_w {
            let cx = (ox * doc_w / thumb_w).min(doc_w - 1);
            let tx = (cx / ts) as i32;
            let lx = (cx % ts) as usize;

            let v = if let Some(tile) = mask.get(tx, ty) {
                (tile.data().get(lx, ly) * 255.0) as u8
            } else {
                255 // no tile = white (reveal all) — matches get_or_create_full() default
            };

            let off = ((oy * thumb_w + ox) * 4) as usize;
            buf[off]     = v;
            buf[off + 1] = v;
            buf[off + 2] = v;
            buf[off + 3] = 255;
        }
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::params::{ParamDef, ParamValue};

    #[test]
    fn param_info_serializes_flat() {
        let def = ParamDef::Float { name: "speed", min: 0.0, max: 10.0, default: 1.0 };
        let info = ParamInfo::from_def(&def, Some(&ParamValue::Float(2.5)));
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["kind"], "float");
        assert_eq!(json["name"], "speed");
        assert_eq!(json["min"], 0.0);
        assert_eq!(json["max"], 10.0);
        assert_eq!(json["default"], 1.0);
        assert_eq!(json["value"], 2.5);
    }

    #[test]
    fn param_info_bool_omits_min_max() {
        let def = ParamDef::Bool { name: "soft", default: true };
        let info = ParamInfo::from_def(&def, None);
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["kind"], "bool");
        assert_eq!(json["name"], "soft");
        assert_eq!(json["default"], true);
        assert!(json.get("min").is_none());
        assert!(json.get("max").is_none());
        assert!(json.get("value").is_none());
    }

    #[test]
    fn veil_info_serializes_correctly() {
        let info = VeilInfo {
            type_id: "pixelate".into(),
            visible: true,
            index: 0,
            params: vec![
                ParamInfo::from_def(
                    &ParamDef::Int { name: "scale", min: 1, max: 32, default: 2 },
                    Some(&ParamValue::Int(4)),
                ),
                ParamInfo::from_def(
                    &ParamDef::Bool { name: "soft", default: true },
                    Some(&ParamValue::Bool(false)),
                ),
            ],
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["type"], "pixelate");
        assert_eq!(json["visible"], true);
        assert_eq!(json["index"], 0);

        let params = json["params"].as_array().unwrap();
        assert_eq!(params.len(), 2);
        assert_eq!(params[0]["kind"], "int");
        assert_eq!(params[0]["name"], "scale");
        assert_eq!(params[0]["value"], 4);
        assert_eq!(params[1]["kind"], "bool");
        assert_eq!(params[1]["name"], "soft");
        assert_eq!(params[1]["value"], false);
    }
}

// ---------------------------------------------------------------------------
// Free helper functions for floating content (avoid borrow conflicts)
// ---------------------------------------------------------------------------

/// Clear RGBA tiles within a bounding region.
fn clear_rgba_in_bounds(
    tiles: &mut TileGrid,
    origin: (i32, i32),
    width: u32,
    height: u32,
) {
    let ts = TILE_SIZE as i32;
    let (ox, oy) = origin;
    let tx_min = TileGrid::tile_coords_for_pixel(ox, 0).0;
    let ty_min = TileGrid::tile_coords_for_pixel(0, oy).1;
    let tx_max = TileGrid::tile_coords_for_pixel(ox + width as i32 - 1, 0).0;
    let ty_max = TileGrid::tile_coords_for_pixel(0, oy + height as i32 - 1).1;

    for ty in ty_min..=ty_max {
        for tx in tx_min..=tx_max {
            if tiles.get(tx, ty).is_some() {
                let data = tiles.get_or_create(tx, ty).write();
                for py in 0..TILE_SIZE {
                    for px in 0..TILE_SIZE {
                        let canvas_x = tx * ts + px as i32;
                        let canvas_y = ty * ts + py as i32;
                        if canvas_x >= ox && canvas_x < ox + width as i32
                            && canvas_y >= oy && canvas_y < oy + height as i32
                        {
                            data.pixel_mut(px, py).copy_from_slice(&[0, 0, 0, 0]);
                        }
                    }
                }
            }
        }
    }
}

/// Clear mask tiles within a bounding region.
fn clear_mask_in_bounds(
    mask: &mut AlphaMask,
    origin: (i32, i32),
    width: u32,
    height: u32,
) {
    let ts = TILE_SIZE as i32;
    let (ox, oy) = origin;
    let tx_min = AlphaMask::tile_coords_for_pixel(ox, 0).0;
    let ty_min = AlphaMask::tile_coords_for_pixel(0, oy).1;
    let tx_max = AlphaMask::tile_coords_for_pixel(ox + width as i32 - 1, 0).0;
    let ty_max = AlphaMask::tile_coords_for_pixel(0, oy + height as i32 - 1).1;

    for ty in ty_min..=ty_max {
        for tx in tx_min..=tx_max {
            if mask.get(tx, ty).is_some() {
                let data = mask.get_or_create(tx, ty).write();
                for py in 0..TILE_SIZE {
                    for px in 0..TILE_SIZE {
                        let canvas_x = tx * ts + px as i32;
                        let canvas_y = ty * ts + py as i32;
                        if canvas_x >= ox && canvas_x < ox + width as i32
                            && canvas_y >= oy && canvas_y < oy + height as i32
                        {
                            data.set(px, py, 0.0);
                        }
                    }
                }
            }
        }
    }
}
