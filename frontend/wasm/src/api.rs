use darkly::document::MoveTarget;
use darkly::engine::{DarklyEngine, StrokeOp};
use darkly::gpu::context::GpuContext;
use darkly::gpu::params::{ParamDef, ParamValue};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsError;

#[wasm_bindgen]
pub struct DarklyHandle(DarklyEngine);

/// Convert a JS params object to a `Vec<ParamValue>` using `ParamDef` metadata.
fn js_to_param_values(js: &JsValue, defs: &[ParamDef]) -> Vec<ParamValue> {
    defs.iter().map(|def| match def {
        ParamDef::Float { name, default, .. } => {
            let v = js_sys::Reflect::get(js, &(*name).into())
                .ok()
                .and_then(|v| v.as_f64())
                .unwrap_or(*default as f64) as f32;
            ParamValue::Float(v)
        }
        ParamDef::Int { name, default, .. } => {
            let v = js_sys::Reflect::get(js, &(*name).into())
                .ok()
                .and_then(|v| v.as_f64())
                .unwrap_or(*default as f64) as i32;
            ParamValue::Int(v)
        }
        ParamDef::Bool { name, default } => {
            let v = js_sys::Reflect::get(js, &(*name).into())
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(*default);
            ParamValue::Bool(v)
        }
    }).collect()
}

#[wasm_bindgen]
impl DarklyHandle {
    /// Create a new Darkly editor instance from an HTML canvas element.
    pub async fn create(canvas: web_sys::HtmlCanvasElement, doc_width: u32, doc_height: u32) -> DarklyHandle {
        let initial_width = canvas.width();
        let initial_height = canvas.height();

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::BROWSER_WEBGPU,
            ..Default::default()
        });
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
            .expect("Failed to create surface");
        let gpu = GpuContext::new(
            instance, surface,
            wgpu::Limits::downlevel_webgl2_defaults(),
            initial_width, initial_height,
        ).await;

        DarklyHandle(DarklyEngine::new(gpu, doc_width, doc_height))
    }

    // --- Layer CRUD (pass-through) ---

    pub fn add_raster_layer(&mut self) -> u64 { self.0.add_raster_layer() }
    pub fn add_raster_layer_in(&mut self, group_id: u64) -> u64 { self.0.add_raster_layer_in(group_id) }
    pub fn add_group(&mut self) -> u64 { self.0.add_group() }

    pub fn add_filter_layer(&mut self, filter_type: &str, params: JsValue) -> u64 {
        let pv = js_to_param_values(&params, self.0.filter_param_defs(filter_type));
        self.0.add_filter_layer(filter_type, &pv)
    }

    pub fn remove_layer(&mut self, layer_id: u64) -> Result<(), JsError> {
        self.0.remove_layer(layer_id).map_err(|e| JsError::new(&e))
    }

    pub fn move_layer(&mut self, layer_id: u64, target_type: &str, target_id: u64) {
        let target = match target_type {
            "before" => MoveTarget::Before(target_id),
            "after" => MoveTarget::After(target_id),
            "into_top" => MoveTarget::IntoGroupTop(target_id),
            "into_bottom" => MoveTarget::IntoGroupBottom(target_id),
            _ => return,
        };
        self.0.move_layer(layer_id, target)
    }

    // --- Layer properties (pass-through) ---

    pub fn set_opacity(&mut self, layer_id: u64, opacity: f32) { self.0.set_opacity(layer_id, opacity) }
    pub fn set_blend_mode(&mut self, layer_id: u64, mode: u32) { self.0.set_blend_mode(layer_id, mode) }
    pub fn set_layer_visible(&mut self, layer_id: u64, visible: bool) { self.0.set_layer_visible(layer_id, visible) }
    pub fn set_layer_name(&mut self, layer_id: u64, name: &str) { self.0.set_layer_name(layer_id, name) }
    pub fn set_group_collapsed(&mut self, group_id: u64, collapsed: bool) { self.0.set_group_collapsed(group_id, collapsed) }

    // --- Painting (pass-through) ---

    pub fn paint(&mut self, layer_id: u64, x: f32, y: f32, radius: f32, r: u8, g: u8, b: u8, a: u8) {
        self.0.paint(layer_id, x, y, radius, r, g, b, a)
    }
    pub fn fill_gradient(&mut self, layer_id: u64) { self.0.fill_gradient(layer_id) }

    // --- Stroke lifecycle ---

    pub fn begin_stroke(&mut self, layer_id: u64) { self.0.begin_stroke(layer_id) }

    pub fn stroke_to(&mut self, op_type: &str, params: JsValue) {
        // Inject the discriminator tag expected by serde's tagged enum.
        js_sys::Reflect::set(&params, &"op".into(), &op_type.into()).ok();
        if let Ok(op) = serde_wasm_bindgen::from_value::<StrokeOp>(params) {
            self.0.stroke_to(op);
        }
    }

    pub fn end_stroke(&mut self) { self.0.end_stroke() }

    // Legacy compat
    pub fn snapshot(&mut self, layer_id: u64) { self.0.begin_stroke(layer_id) }
    pub fn commit(&mut self) { self.0.end_stroke() }

    // --- View transform ---

    pub fn set_view_transform(&mut self, pan_x: f32, pan_y: f32, zoom: f32, rotation: f32, screen_w: f32, screen_h: f32) {
        self.0.set_view_transform(pan_x, pan_y, zoom, rotation, screen_w, screen_h)
    }

    pub fn screen_to_canvas(&self, screen_x: f32, screen_y: f32) -> Vec<f32> {
        let (cx, cy) = self.0.screen_to_canvas(screen_x, screen_y);
        vec![cx, cy]
    }

    pub fn pick_color(&self, x: f32, y: f32) -> Vec<u8> {
        let c = self.0.pick_color(x, y);
        c.to_vec()
    }

    // --- Rendering ---

    pub fn render(&mut self, time_secs: f32) { self.0.render(time_secs) }
    pub fn resize(&mut self, width: u32, height: u32) { self.0.resize(width, height) }

    // --- Undo / Redo ---

    pub fn undo(&mut self) { self.0.undo() }
    pub fn redo(&mut self) { self.0.redo() }

    // --- Veils ---

    pub fn add_veil(&mut self, veil_type: &str, params: JsValue) {
        let pv = js_to_param_values(&params, self.0.veil_param_defs(veil_type));
        self.0.add_veil(veil_type, &pv)
    }

    pub fn remove_veil(&mut self, index: u32) { self.0.remove_veil(index as usize) }
    pub fn clear_veils(&mut self) { self.0.clear_veils() }
    pub fn set_veil_visible(&mut self, index: u32, visible: bool) { self.0.set_veil_visible(index as usize, visible) }
    pub fn move_veil(&mut self, from: u32, to: u32) { self.0.move_veil(from as usize, to as usize) }

    pub fn update_veil(&mut self, index: u32, params: JsValue) {
        let type_id = match self.0.veil_list().iter().find(|v| v.index == index as usize) {
            Some(v) => v.type_id.clone(),
            None => return,
        };
        let pv = js_to_param_values(&params, self.0.veil_param_defs(&type_id));
        self.0.update_veil(index as usize, &pv)
    }

    // --- Queries (serialize to JS) ---

    pub fn layer_tree(&self) -> JsValue {
        serde_wasm_bindgen::to_value(&self.0.layer_tree()).unwrap()
    }

    pub fn veil_list(&self) -> JsValue {
        serde_wasm_bindgen::to_value(&self.0.veil_list()).unwrap()
    }
}
