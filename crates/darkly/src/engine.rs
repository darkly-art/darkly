use crate::document::{Document, MoveTarget, SelectionMode};
use crate::layer::{BlendMode, Layer, LayerNode};
use crate::undo::{
    UndoStack, TileAction, LayerAddAction, LayerRemoveAction, LayerMoveAction,
    MaskTileAction, MaskPropertyAction, PropertyAction, SelectionAction, mark_affected_dirty,
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
    Filter { id: f64, name: String, visible: bool },
    #[serde(rename_all = "camelCase")]
    Group {
        id: f64, name: String, visible: bool, collapsed: bool, passthrough: bool,
        opacity: f32, blend_mode: u32, children: Vec<LayerInfo>,
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

    pub fn add_filter_layer(&mut self, filter_type: &str, params: &[ParamValue]) -> u64 {
        let format = self.compositor.accum_format();
        let filter = self.compositor.filter_registry_mut().create_filter(
            filter_type, params, &self.gpu.device, format,
        );

        let id = self.doc.add_filter_layer(filter.clone_boxed());

        if let Some(Layer::Filter(f)) = self.doc.layer(id) {
            self.compositor.ensure_filter_layer(
                &self.gpu.device, &self.gpu.queue, id, f.filter.as_ref(),
            );
        }

        self.compositor.mark_dirty();

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
            Some(LayerNode::Layer(l)) => match l {
                Layer::Raster(r) => r.visible = visible,
                Layer::Filter(f) => f.visible = visible,
            },
            Some(LayerNode::Group(g)) => g.visible = visible,
            None => return,
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

    // --- Layer Masks ---

    pub fn add_mask(&mut self, layer_id: u64) {
        // Snapshot old state for undo
        let (old_mask, old_enabled, old_show) = match self.doc.layer(layer_id) {
            Some(Layer::Raster(r)) => (r.mask.clone(), r.mask_enabled, r.show_mask),
            _ => return,
        };

        self.doc.add_mask(layer_id);
        self.compositor.set_layer_mask(&self.gpu.device, &self.gpu.queue, layer_id, true);
        self.sync_mask_state(layer_id);
        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(MaskPropertyAction::new(
            layer_id, old_mask, old_enabled, old_show,
        )));
    }

    pub fn remove_mask(&mut self, layer_id: u64) {
        let (old_mask, old_enabled, old_show) = match self.doc.layer(layer_id) {
            Some(Layer::Raster(r)) => (r.mask.clone(), r.mask_enabled, r.show_mask),
            _ => return,
        };

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
        if let Some(mementos) = tile_memento {
            self.undo_stack.push(Box::new(TileAction::new(mementos)));
        }
        self.undo_stack.push(Box::new(MaskPropertyAction::new(
            layer_id, old_mask, old_enabled, old_show,
        )));
    }

    pub fn set_mask_enabled(&mut self, layer_id: u64, enabled: bool) {
        let old = match self.doc.layer(layer_id) {
            Some(Layer::Raster(r)) => r.mask_enabled,
            _ => return,
        };
        self.doc.set_mask_enabled(layer_id, enabled);
        self.sync_mask_state(layer_id);
        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(MaskPropertyAction::new(
            layer_id, None, old, false,
        )));
    }

    pub fn set_show_mask(&mut self, layer_id: u64, show: bool) {
        let old = match self.doc.layer(layer_id) {
            Some(Layer::Raster(r)) => r.show_mask,
            _ => return,
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
        let (old_mask, old_enabled, old_show) = match self.doc.layer(layer_id) {
            Some(Layer::Raster(r)) => (r.mask.clone(), r.mask_enabled, r.show_mask),
            _ => return,
        };

        self.doc.selection_to_mask(layer_id);
        self.compositor.set_layer_mask(&self.gpu.device, &self.gpu.queue, layer_id, true);

        // Mark all mask tiles dirty for upload
        let mask_coords: Vec<(i32, i32)> = self.doc.layer(layer_id)
            .and_then(|l| match l { Layer::Raster(r) => r.mask.as_ref(), _ => None })
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

    /// Sync compositor mask state (bind group + uniforms) for a layer.
    fn sync_mask_state(&mut self, layer_id: u64) {
        if let Some(Layer::Raster(r)) = self.doc.layer(layer_id) {
            let has_mask = r.mask.is_some();
            let mask_enabled = r.mask_enabled;
            let show_mask = r.show_mask;

            self.compositor.set_layer_mask(&self.gpu.device, &self.gpu.queue, layer_id, has_mask);
            self.compositor.update_mask_binding(
                &self.gpu.device, layer_id, mask_enabled, show_mask,
            );
            self.compositor.update_raster_uniforms_full(
                &self.gpu.queue, layer_id, r.opacity, r.blend_mode, show_mask,
            );
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
        if self.editing_mask_layer == Some(layer_id) {
            self.doc.begin_mask_transaction(layer_id);
        } else {
            self.doc.begin_transaction(layer_id);
        }
        self.active_stroke_layer = Some(layer_id);
    }

    pub fn stroke_to(&mut self, op: StrokeOp) {
        let layer_id = match self.active_stroke_layer {
            Some(id) => id,
            None => return,
        };

        if self.editing_mask_layer == Some(layer_id) {
            // Route paint ops to mask
            match op {
                StrokeOp::PaintCircle { x, y, radius, r: _, g: _, b: _, a } => {
                    // For mask painting: a > 128 paints white (reveal), a <= 128 paints black (hide)
                    let value = if a > 128 { 1.0 } else { 0.0 };
                    self.doc.paint_mask_circle(layer_id, x, y, radius, value);
                }
                StrokeOp::EraseCircle { x, y, radius } => {
                    self.doc.erase_mask_circle(layer_id, x, y, radius);
                }
                _ => {} // Flood fill and gradient not supported on masks
            }
        } else {
            match op {
                StrokeOp::PaintCircle { x, y, radius, r, g, b, a } => {
                    self.doc.paint_circle(layer_id, x, y, radius, [r, g, b, a]);
                }
                StrokeOp::EraseCircle { x, y, radius } => {
                    self.doc.erase_circle(layer_id, x, y, radius);
                }
                StrokeOp::FloodFill { x, y, r, g, b, a, tolerance } => {
                    self.doc.flood_fill(layer_id, x as i32, y as i32, [r, g, b, a], tolerance);
                }
                StrokeOp::LinearGradient { x0, y0, x1, y1, r0, g0, b0, a0, r1, g1, b1, a1 } => {
                    self.doc.linear_gradient(
                        layer_id, x0, y0, x1, y1,
                        [r0, g0, b0, a0], [r1, g1, b1, a1],
                    );
                }
            }
        }
    }

    pub fn end_stroke(&mut self) {
        if let Some(layer_id) = self.active_stroke_layer.take() {
            if self.editing_mask_layer == Some(layer_id) {
                if let Some(memento) = self.doc.commit_mask_transaction(layer_id) {
                    self.undo_stack.push(Box::new(MaskTileAction::new(layer_id, memento)));
                }
            } else {
                if let Some(mementos) = self.doc.commit_transaction(layer_id) {
                    self.undo_stack.push(Box::new(TileAction::new(mementos)));
                }
            }
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
        let mask = match self.doc.layer(layer_id) {
            Some(Layer::Raster(r)) => match &r.mask {
                Some(m) => m,
                None => return Vec::new(),
            },
            _ => return Vec::new(),
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
        if let Some(affected) = self.undo_stack.undo(&mut self.doc) {
            mark_affected_dirty(&mut self.doc.dirty, &affected);
            self.sync_compositor_layers();
            self.compositor.mark_dirty();
            self.update_selection_overlay();
        }
    }

    pub fn redo(&mut self) {
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

    /// Get the parameter definitions for a filter type.
    pub fn filter_param_defs(&self, type_id: &str) -> &'static [ParamDef] {
        self.compositor.filter_registry().param_defs(type_id)
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
        if self.doc.selection.is_none() {
            return;
        }
        self.doc.begin_transaction(layer_id);
        self.doc.clear_selection_contents(layer_id);
        if let Some(mementos) = self.doc.commit_transaction(layer_id) {
            self.undo_stack.push(Box::new(TileAction::new(mementos)));
        }
    }

    pub fn has_selection(&self) -> bool {
        self.doc.selection.is_some()
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
            Layer::Filter(f) => LayerInfo::Filter {
                id: f.id as f64,
                name: f.filter.type_id().to_string(),
                visible: f.visible,
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
