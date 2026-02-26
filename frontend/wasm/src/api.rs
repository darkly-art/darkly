use darkly::document::{Document, MoveTarget};
use darkly::layer::{BlendMode, Layer, LayerNode};
use darkly::undo::{
    UndoStack, TileAction, LayerAddAction, LayerRemoveAction, LayerMoveAction,
    PropertyAction, mark_affected_dirty,
};
use darkly::undo::property::Property;
use darkly::gpu::compositor::Compositor;
use darkly::gpu::context::GpuContext;
use darkly::gpu::view::ViewTransform;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsError;

#[wasm_bindgen]
pub struct DarklyHandle {
    doc: Document,
    compositor: Compositor,
    gpu: GpuContext,
    undo_stack: UndoStack,
    /// The layer currently being painted (transaction active).
    active_stroke_layer: Option<u64>,
    /// Current view transform (cached for screen_to_canvas).
    view_transform: ViewTransform,
}

/// Convert a LayerNode to a JS object for the UI.
fn node_to_js(node: &LayerNode) -> js_sys::Object {
    let obj = js_sys::Object::new();
    match node {
        LayerNode::Layer(layer) => match layer {
            Layer::Raster(r) => {
                js_sys::Reflect::set(&obj, &"type".into(), &"raster".into()).ok();
                js_sys::Reflect::set(&obj, &"id".into(), &JsValue::from(r.id as f64)).ok();
                js_sys::Reflect::set(&obj, &"name".into(), &JsValue::from_str(&r.name)).ok();
                js_sys::Reflect::set(&obj, &"visible".into(), &JsValue::from(r.visible)).ok();
                js_sys::Reflect::set(&obj, &"opacity".into(), &JsValue::from(r.opacity as f64)).ok();
                js_sys::Reflect::set(&obj, &"blendMode".into(), &JsValue::from(r.blend_mode as u32)).ok();
            }
            Layer::Filter(f) => {
                js_sys::Reflect::set(&obj, &"type".into(), &"filter".into()).ok();
                js_sys::Reflect::set(&obj, &"id".into(), &JsValue::from(f.id as f64)).ok();
                js_sys::Reflect::set(&obj, &"name".into(), &JsValue::from_str(f.filter.type_id())).ok();
                js_sys::Reflect::set(&obj, &"visible".into(), &JsValue::from(f.visible)).ok();
            }
        },
        LayerNode::Group(g) => {
            js_sys::Reflect::set(&obj, &"type".into(), &"group".into()).ok();
            js_sys::Reflect::set(&obj, &"id".into(), &JsValue::from(g.id as f64)).ok();
            js_sys::Reflect::set(&obj, &"name".into(), &JsValue::from_str(&g.name)).ok();
            js_sys::Reflect::set(&obj, &"visible".into(), &JsValue::from(g.visible)).ok();
            js_sys::Reflect::set(&obj, &"collapsed".into(), &JsValue::from(g.collapsed)).ok();
            js_sys::Reflect::set(&obj, &"passthrough".into(), &JsValue::from(g.passthrough)).ok();
            js_sys::Reflect::set(&obj, &"opacity".into(), &JsValue::from(g.opacity as f64)).ok();
            js_sys::Reflect::set(&obj, &"blendMode".into(), &JsValue::from(g.blend_mode as u32)).ok();
            // Recursively build children array (top-to-bottom display order = reversed)
            let children = js_sys::Array::new();
            for child in g.children.iter().rev() {
                children.push(&node_to_js(child));
            }
            js_sys::Reflect::set(&obj, &"children".into(), &children).ok();
        }
    }
    obj
}

#[wasm_bindgen]
impl DarklyHandle {
    /// Create a new Darkly editor instance, initializing GPU and document.
    /// `doc_width`/`doc_height` set the document (canvas) dimensions;
    /// the viewport size comes from the HTML canvas element.
    pub async fn create(canvas: web_sys::HtmlCanvasElement, doc_width: u32, doc_height: u32) -> DarklyHandle {
        let gpu = GpuContext::new(canvas).await;
        let compositor = Compositor::new(&gpu.device, &gpu.queue, gpu.surface_format(), doc_width, doc_height);
        let doc = Document::new(doc_width, doc_height);
        let undo_stack = UndoStack::new(50);

        DarklyHandle {
            doc,
            compositor,
            gpu,
            undo_stack,
            active_stroke_layer: None,
            view_transform: ViewTransform::identity(),
        }
    }

    /// Add a new raster layer and return its ID.
    pub fn add_raster_layer(&mut self) -> u64 {
        let id = self.doc.add_raster_layer();
        self.compositor.ensure_raster_layer(&self.gpu.device, &self.gpu.queue, id);
        self.compositor.mark_dirty();

        let parent = self.doc.parent_of(id);
        let pos = self.doc.position_in_parent(id).unwrap_or(0);
        self.undo_stack.push(Box::new(LayerAddAction::new(id, parent, pos)));

        id
    }

    /// Add a new raster layer inside a group and return its ID.
    pub fn add_raster_layer_in(&mut self, group_id: u64) -> u64 {
        let id = self.doc.add_raster_layer_in(Some(group_id));
        self.compositor.ensure_raster_layer(&self.gpu.device, &self.gpu.queue, id);
        self.compositor.mark_dirty();

        let parent = self.doc.parent_of(id);
        let pos = self.doc.position_in_parent(id).unwrap_or(0);
        self.undo_stack.push(Box::new(LayerAddAction::new(id, parent, pos)));

        id
    }

    /// Add a new empty group and return its ID.
    pub fn add_group(&mut self) -> u64 {
        let id = self.doc.add_group();

        let parent = self.doc.parent_of(id);
        let pos = self.doc.position_in_parent(id).unwrap_or(0);
        self.undo_stack.push(Box::new(LayerAddAction::new(id, parent, pos)));

        id
    }

    /// Add a filter layer. `filter_type` is the filter type string (e.g., "noise").
    /// `params` is a JS object with filter-specific parameters.
    pub fn add_filter_layer(&mut self, filter_type: &str, params: JsValue) -> u64 {
        let format = self.compositor.accum_format();
        let filter = self.compositor.filter_registry_mut().create_filter(
            filter_type,
            params,
            &self.gpu.device,
            format,
        );

        let id = self.doc.add_filter_layer(filter.clone_boxed());

        if let Some(Layer::Filter(f)) = self.doc.layer(id) {
            self.compositor.ensure_filter_layer(
                &self.gpu.device,
                &self.gpu.queue,
                id,
                f.filter.as_ref(),
            );
        }

        self.compositor.mark_dirty();

        let parent = self.doc.parent_of(id);
        let pos = self.doc.position_in_parent(id).unwrap_or(0);
        self.undo_stack.push(Box::new(LayerAddAction::new(id, parent, pos)));

        id
    }

    /// Paint a circle on a raster layer (legacy — used by Phase 1 demo).
    pub fn paint(
        &mut self,
        layer_id: u64,
        x: f32, y: f32, radius: f32,
        r: u8, g: u8, b: u8, a: u8,
    ) {
        self.doc.paint_circle(layer_id, x, y, radius, [r, g, b, a]);
    }

    /// Fill a raster layer with a demo gradient.
    pub fn fill_gradient(&mut self, layer_id: u64) {
        self.doc.fill_gradient(layer_id);
    }

    // --- Stroke lifecycle ---

    /// Begin a stroke on a layer. Starts an undo transaction.
    pub fn begin_stroke(&mut self, layer_id: u64) {
        self.doc.begin_transaction(layer_id);
        self.active_stroke_layer = Some(layer_id);
    }

    /// Apply a stroke operation. Can be called once (fill, gradient)
    /// or many times (brush, eraser — once per pointer event).
    pub fn stroke_to(&mut self, op_type: &str, params: JsValue) {
        let layer_id = match self.active_stroke_layer {
            Some(id) => id,
            None => return,
        };

        // Dispatch to the appropriate tool operation
        match op_type {
            "paint_circle" => {
                #[derive(serde::Deserialize)]
                struct P { x: f32, y: f32, radius: f32, r: u8, g: u8, b: u8, a: u8 }
                if let Ok(p) = serde_wasm_bindgen::from_value::<P>(params) {
                    self.doc.paint_circle(layer_id, p.x, p.y, p.radius, [p.r, p.g, p.b, p.a]);
                }
            }
            "erase_circle" => {
                #[derive(serde::Deserialize)]
                struct P { x: f32, y: f32, radius: f32 }
                if let Ok(p) = serde_wasm_bindgen::from_value::<P>(params) {
                    self.doc.erase_circle(layer_id, p.x, p.y, p.radius);
                }
            }
            "flood_fill" => {
                #[derive(serde::Deserialize)]
                struct P { x: f32, y: f32, r: u8, g: u8, b: u8, a: u8, tolerance: u8 }
                if let Ok(p) = serde_wasm_bindgen::from_value::<P>(params) {
                    self.doc.flood_fill(layer_id, p.x as i32, p.y as i32, [p.r, p.g, p.b, p.a], p.tolerance);
                }
            }
            "linear_gradient" => {
                #[derive(serde::Deserialize)]
                struct P { x0: f32, y0: f32, x1: f32, y1: f32, r0: u8, g0: u8, b0: u8, a0: u8, r1: u8, g1: u8, b1: u8, a1: u8 }
                if let Ok(p) = serde_wasm_bindgen::from_value::<P>(params) {
                    self.doc.linear_gradient(
                        layer_id,
                        p.x0, p.y0, p.x1, p.y1,
                        [p.r0, p.g0, p.b0, p.a0],
                        [p.r1, p.g1, p.b1, p.a1],
                    );
                }
            }
            _ => {
                log::warn!("Unknown stroke op: {op_type}");
            }
        }
    }

    /// End the current stroke. Commits the undo transaction.
    pub fn end_stroke(&mut self) {
        if let Some(layer_id) = self.active_stroke_layer.take() {
            if let Some(mementos) = self.doc.commit_transaction(layer_id) {
                self.undo_stack.push(Box::new(TileAction::new(mementos)));
            }
        }
    }

    // --- Legacy stroke API (Phase 1 compat) ---

    pub fn snapshot(&mut self, layer_id: u64) {
        self.begin_stroke(layer_id);
    }

    pub fn commit(&mut self) {
        self.end_stroke();
    }

    // --- View transform ---

    /// Update the canvas view transform (pan, zoom, rotation).
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

    /// Transform screen coordinates to canvas coordinates for paint input.
    pub fn screen_to_canvas(&self, screen_x: f32, screen_y: f32) -> Vec<f32> {
        let (cx, cy) = self.view_transform.screen_to_canvas(screen_x, screen_y);
        vec![cx, cy]
    }

    /// Pick a color from the composite cache at canvas coordinates.
    pub fn pick_color(&self, _x: f32, _y: f32) -> Vec<u8> {
        // TODO: GPU readback from composite cache. For now return black.
        vec![0, 0, 0, 255]
    }

    // --- Layer operations ---

    /// Render the current frame. Handles dirty checking internally (P2).
    pub fn render(&mut self) {
        self.compositor.render(
            &self.gpu.device,
            &self.gpu.queue,
            &self.gpu.surface,
            &self.gpu.surface_config,
            &mut self.doc,
        );
    }

    /// Set opacity for a layer or group (undoable).
    pub fn set_opacity(&mut self, layer_id: u64, opacity: f32) {
        // Capture old value before mutation.
        let old_opacity = match self.doc.find_node(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => r.opacity,
            Some(LayerNode::Group(g)) => g.opacity,
            _ => return,
        };

        // Apply.
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

    /// Set blend mode for a layer or group (undoable).
    pub fn set_blend_mode(&mut self, layer_id: u64, mode: u32) {
        let blend_mode = BlendMode::from_u32(mode);

        // Capture old value.
        let old_mode = match self.doc.find_node(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => r.blend_mode,
            Some(LayerNode::Group(g)) => g.blend_mode,
            _ => return,
        };

        // Apply.
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

    /// Set visibility for a layer or group (undoable).
    pub fn set_layer_visible(&mut self, layer_id: u64, visible: bool) {
        // Capture old value.
        let old_visible = match self.doc.find_node(layer_id) {
            Some(n) => n.visible(),
            None => return,
        };

        // Apply.
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

    /// Set layer or group name (undoable).
    pub fn set_layer_name(&mut self, layer_id: u64, name: &str) {
        // Capture old value.
        let old_name = match self.doc.find_node(layer_id) {
            Some(LayerNode::Layer(Layer::Raster(r))) => r.name.clone(),
            Some(LayerNode::Group(g)) => g.name.clone(),
            _ => return,
        };

        // Apply.
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

    /// Set group collapsed state (UI only, not undoable).
    pub fn set_group_collapsed(&mut self, group_id: u64, collapsed: bool) {
        if let Some(LayerNode::Group(g)) = self.doc.find_node_mut(group_id) {
            g.collapsed = collapsed;
        }
    }

    /// Get the layer tree as a JS array for the UI.
    /// Returned in top-to-bottom display order (reversed from internal bottom-to-top).
    pub fn layer_tree(&self) -> JsValue {
        let arr = js_sys::Array::new();
        for node in self.doc.layers.iter().rev() {
            arr.push(&node_to_js(node));
        }
        arr.into()
    }

    /// Move a layer or group to a new position (undoable).
    /// `target_type`: "before", "after", "into_top", "into_bottom"
    pub fn move_layer(&mut self, layer_id: u64, target_type: &str, target_id: u64) {
        let target = match target_type {
            "before" => MoveTarget::Before(target_id),
            "after" => MoveTarget::After(target_id),
            "into_top" => MoveTarget::IntoGroupTop(target_id),
            "into_bottom" => MoveTarget::IntoGroupBottom(target_id),
            _ => return,
        };

        // Capture old position before move.
        let old_parent = self.doc.parent_of(layer_id);
        let old_pos = match self.doc.position_in_parent(layer_id) {
            Some(p) => p,
            None => return,
        };

        self.doc.move_layer(layer_id, target);

        // Capture new position after move.
        let new_parent = self.doc.parent_of(layer_id);
        let new_pos = self.doc.position_in_parent(layer_id).unwrap_or(0);

        self.compositor.mark_dirty();

        self.undo_stack.push(Box::new(LayerMoveAction::new(
            layer_id, old_parent, old_pos, new_parent, new_pos,
        )));
    }

    /// Remove a layer or group and all children (undoable).
    pub fn remove_layer(&mut self, layer_id: u64) -> Result<(), JsError> {
        if self.doc.node_count() <= 1 {
            return Err(JsError::new("Cannot delete the last layer"));
        }

        let parent = self.doc.parent_of(layer_id);
        let pos = self.doc.position_in_parent(layer_id).unwrap_or(0);

        if let Some(node) = self.doc.detach_for_undo(layer_id) {
            self.undo_stack.push(Box::new(LayerRemoveAction::new(node, parent, pos)));
        }

        self.compositor.mark_dirty();
        Ok(())
    }

    /// Undo the last action.
    pub fn undo(&mut self) {
        if let Some(affected) = self.undo_stack.undo(&mut self.doc) {
            mark_affected_dirty(&mut self.doc.dirty, &affected);
            self.sync_compositor_layers();
            self.compositor.mark_dirty();
        }
    }

    /// Redo the last undone action.
    pub fn redo(&mut self) {
        if let Some(affected) = self.undo_stack.redo(&mut self.doc) {
            mark_affected_dirty(&mut self.doc.dirty, &affected);
            self.sync_compositor_layers();
            self.compositor.mark_dirty();
        }
    }

    /// Resize the canvas surface. Call when the viewport dimensions change.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.gpu.resize(width, height);
        self.compositor.mark_needs_present();
    }

    // --- Internal helpers ---

    /// Sync compositor state with document after undo/redo.
    /// Ensures GPU resources exist for all layers and uniforms are up to date.
    fn sync_compositor_layers(&mut self) {
        for raster in self.doc.all_raster_layers() {
            self.compositor.ensure_raster_layer(&self.gpu.device, &self.gpu.queue, raster.id);
            self.compositor.update_raster_uniforms(
                &self.gpu.queue, raster.id, raster.opacity, raster.blend_mode,
            );
        }
    }
}
