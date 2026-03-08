use crate::document::{Document, MoveTarget};
use crate::layer::{BlendMode, Layer, LayerNode};
use crate::undo::{
    UndoStack, TileAction, LayerAddAction, LayerRemoveAction, LayerMoveAction,
    PropertyAction, mark_affected_dirty,
};
use crate::undo::property::Property;
use crate::gpu::compositor::Compositor;
use crate::gpu::context::GpuContext;
use crate::gpu::params::{ParamDef, ParamValue};
use crate::gpu::view::ViewTransform;

// ---------------------------------------------------------------------------
// Shared return types — serde-serializable for any FFI bridge.
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LayerInfo {
    #[serde(rename_all = "camelCase")]
    Raster { id: f64, name: String, visible: bool, opacity: f32, blend_mode: u32 },
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
    view_transform: ViewTransform,
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
            view_transform: ViewTransform::identity(),
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

    pub fn begin_stroke(&mut self, layer_id: u64) {
        self.doc.begin_transaction(layer_id);
        self.active_stroke_layer = Some(layer_id);
    }

    pub fn stroke_to(&mut self, op: StrokeOp) {
        let layer_id = match self.active_stroke_layer {
            Some(id) => id,
            None => return,
        };

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

    pub fn end_stroke(&mut self) {
        if let Some(layer_id) = self.active_stroke_layer.take() {
            if let Some(mementos) = self.doc.commit_transaction(layer_id) {
                self.undo_stack.push(Box::new(TileAction::new(mementos)));
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

    // --- Rendering ---

    pub fn render(&mut self, time_secs: f32) {
        self.compositor.veil_chain_mut().update_time(&self.gpu.queue, time_secs);
        self.compositor.render(
            &self.gpu.device,
            &self.gpu.queue,
            &self.gpu.surface,
            &self.gpu.surface_config,
            &mut self.doc,
        );
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
        }
    }

    pub fn redo(&mut self) {
        if let Some(affected) = self.undo_stack.redo(&mut self.doc) {
            mark_affected_dirty(&mut self.doc.dirty, &affected);
            self.sync_compositor_layers();
            self.compositor.mark_dirty();
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
        self.doc.layers.iter().rev().map(node_to_layer_info).collect()
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

    // --- Internal helpers ---

    fn sync_compositor_layers(&mut self) {
        for raster in self.doc.all_raster_layers() {
            self.compositor.ensure_raster_layer(&self.gpu.device, &self.gpu.queue, raster.id);
            self.compositor.update_raster_uniforms(
                &self.gpu.queue, raster.id, raster.opacity, raster.blend_mode,
            );
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
