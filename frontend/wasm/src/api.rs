use darkly::document::MoveTarget;
use darkly::engine::{DarklyEngine, StrokeOp};
use darkly::gpu::context::GpuContext;
use darkly::gpu::overlay::OverlayPrimitive;
use darkly::gpu::params::{ParamDef, ParamValue};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsError;

#[wasm_bindgen]
pub struct DarklyHandle(DarklyEngine);

fn parse_selection_mode(mode: &str) -> darkly::document::SelectionMode {
    match mode {
        "add" => darkly::document::SelectionMode::Add,
        "subtract" => darkly::document::SelectionMode::Subtract,
        "intersect" => darkly::document::SelectionMode::Intersect,
        _ => darkly::document::SelectionMode::Replace,
    }
}

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
    pub async fn create(canvas: web_sys::HtmlCanvasElement, doc_width: u32, doc_height: u32, is_software: bool) -> DarklyHandle {
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
            is_software,
        ).await;

        DarklyHandle(DarklyEngine::new(gpu, doc_width, doc_height))
    }

    // --- Layer CRUD (pass-through) ---
    // IDs use f64 because JS has no u64 — any JS-facing backend needs this.

    pub fn add_raster_layer(&mut self) -> f64 { self.0.add_raster_layer() as f64 }
    pub fn add_raster_layer_in(&mut self, group_id: f64) -> f64 { self.0.add_raster_layer_in(group_id as u64) as f64 }
    pub fn add_group(&mut self) -> f64 { self.0.add_group() as f64 }

    pub fn remove_layer(&mut self, layer_id: f64) -> Result<(), JsError> {
        self.0.remove_layer(layer_id as u64).map_err(|e| JsError::new(&e))
    }

    pub fn move_layer(&mut self, layer_id: f64, target_type: &str, target_id: f64) {
        let target = match target_type {
            "before" => MoveTarget::Before(target_id as u64),
            "after" => MoveTarget::After(target_id as u64),
            "into_top" => MoveTarget::IntoGroupTop(target_id as u64),
            "into_bottom" => MoveTarget::IntoGroupBottom(target_id as u64),
            _ => return,
        };
        self.0.move_layer(layer_id as u64, target)
    }

    // --- Layer properties (pass-through) ---

    pub fn set_opacity(&mut self, layer_id: f64, opacity: f32) { self.0.set_opacity(layer_id as u64, opacity) }
    pub fn set_blend_mode(&mut self, layer_id: f64, mode: u32) { self.0.set_blend_mode(layer_id as u64, mode) }
    pub fn set_layer_visible(&mut self, layer_id: f64, visible: bool) { self.0.set_layer_visible(layer_id as u64, visible) }
    pub fn set_layer_name(&mut self, layer_id: f64, name: &str) { self.0.set_layer_name(layer_id as u64, name) }
    pub fn set_group_collapsed(&mut self, group_id: f64, collapsed: bool) { self.0.set_group_collapsed(group_id as u64, collapsed) }
    pub fn set_group_passthrough(&mut self, group_id: f64, passthrough: bool) { self.0.set_group_passthrough(group_id as u64, passthrough) }

    // --- Layer Masks (pass-through) ---

    pub fn add_mask(&mut self, layer_id: f64) { self.0.add_mask(layer_id as u64) }
    pub fn remove_mask(&mut self, layer_id: f64) { self.0.remove_mask(layer_id as u64) }
    pub fn apply_mask(&mut self, layer_id: f64) { self.0.apply_mask(layer_id as u64) }
    pub fn set_mask_enabled(&mut self, layer_id: f64, enabled: bool) { self.0.set_mask_enabled(layer_id as u64, enabled) }
    pub fn set_show_mask(&mut self, layer_id: f64, show: bool) { self.0.set_show_mask(layer_id as u64, show) }
    pub fn set_editing_mask(&mut self, layer_id: f64, editing: bool) { self.0.set_editing_mask(layer_id as u64, editing) }
    pub fn selection_to_mask(&mut self, layer_id: f64) { self.0.selection_to_mask(layer_id as u64) }
    pub fn mask_to_selection(&mut self, layer_id: f64) { self.0.mask_to_selection(layer_id as u64) }

    // --- Painting (pass-through) ---

    pub fn paint(&mut self, layer_id: f64, x: f32, y: f32, radius: f32, r: u8, g: u8, b: u8, a: u8) {
        self.0.paint(layer_id as u64, x, y, radius, r, g, b, a)
    }
    pub fn fill_gradient(&mut self, layer_id: f64) { self.0.fill_gradient(layer_id as u64) }

    // --- Stroke lifecycle ---

    pub fn begin_stroke(&mut self, layer_id: f64) { self.0.begin_stroke(layer_id as u64) }

    pub fn stroke_to(&mut self, op_type: &str, params: JsValue) {
        // Inject the discriminator tag expected by serde's tagged enum.
        js_sys::Reflect::set(&params, &"op".into(), &op_type.into()).ok();
        if let Ok(op) = serde_wasm_bindgen::from_value::<StrokeOp>(params) {
            self.0.stroke_to(op);
        }
    }

    pub fn end_stroke(&mut self) { self.0.end_stroke() }

    // Legacy compat
    pub fn snapshot(&mut self, layer_id: f64) { self.0.begin_stroke(layer_id as u64) }
    pub fn commit(&mut self) { self.0.end_stroke() }

    // --- View transform ---

    pub fn set_view_transform(&mut self, pan_x: f32, pan_y: f32, zoom: f32, rotation: f32, screen_w: f32, screen_h: f32) {
        self.0.set_view_transform(pan_x, pan_y, zoom, rotation, screen_w, screen_h)
    }

    pub fn screen_to_canvas(&self, screen_x: f32, screen_y: f32) -> Vec<f32> {
        let (cx, cy) = self.0.screen_to_canvas(screen_x, screen_y);
        vec![cx, cy]
    }

    /// Start an async color pick. Returns the last picked color immediately
    /// for responsive UI — the real result arrives on the next frame.
    pub fn pick_color(&mut self, x: f32, y: f32) -> Vec<u8> {
        let c = self.0.pick_color(x, y);
        c.to_vec()
    }

    /// Get the most recently resolved color pick result.
    pub fn last_picked_color(&self) -> Vec<u8> {
        self.0.last_picked_color().to_vec()
    }

    /// True if a color pick readback is still in flight.
    pub fn has_pending_color_pick(&self) -> bool {
        self.0.has_pending_color_pick()
    }

    // --- Rendering ---

    /// Render the current frame. Returns true if animations need another frame.
    pub fn render(&mut self, time_secs: f32) -> bool {
        self.0.render(time_secs)
    }

    pub fn resize(&mut self, width: u32, height: u32) { self.0.resize(width, height) }

    // --- Undo / Redo ---

    pub fn undo(&mut self) { self.0.undo() }
    pub fn redo(&mut self) { self.0.redo() }

    // --- Selection ---

    pub fn select_rect(&mut self, x: f32, y: f32, w: f32, h: f32, mode: &str, antialias: bool, feather: f32) {
        self.0.select_rect(x, y, w, h, parse_selection_mode(mode), antialias, feather)
    }

    pub fn select_ellipse(&mut self, x: f32, y: f32, w: f32, h: f32, mode: &str, antialias: bool, feather: f32) {
        self.0.select_ellipse(x, y, w, h, parse_selection_mode(mode), antialias, feather)
    }

    pub fn select_lasso(&mut self, vertices: JsValue, mode: &str, antialias: bool, feather: f32) {
        let verts: Vec<[f32; 2]> = serde_wasm_bindgen::from_value(vertices).unwrap_or_default();
        self.0.select_lasso(&verts, parse_selection_mode(mode), antialias, feather)
    }

    pub fn select_magic_wand(&mut self, layer_id: u64, seed_x: i32, seed_y: i32, tolerance: u8, mode: &str) {
        self.0.select_magic_wand(layer_id, seed_x, seed_y, tolerance, parse_selection_mode(mode))
    }

    pub fn clear_selection(&mut self) { self.0.clear_selection() }
    pub fn clear_selection_contents(&mut self, layer_id: f64) { self.0.clear_selection_contents(layer_id as u64) }
    pub fn select_all(&mut self) { self.0.select_all() }
    pub fn invert_selection(&mut self) { self.0.invert_selection() }
    pub fn has_selection(&self) -> bool { self.0.has_selection() }

    // --- Copy / Cut / Paste ---

    /// Copy active layer content (masked by selection).
    /// Returns a JS object `{rgba, width, height, offsetX, offsetY}` or null.
    pub fn copy(&mut self, layer_id: f64) -> JsValue {
        match self.0.copy(layer_id as u64) {
            Some(export) => serde_wasm_bindgen::to_value(&export).unwrap_or(JsValue::NULL),
            None => JsValue::NULL,
        }
    }

    /// Cut = copy + clear. Returns the same object as copy, or null.
    pub fn cut(&mut self, layer_id: f64) -> JsValue {
        match self.0.cut(layer_id as u64) {
            Some(export) => serde_wasm_bindgen::to_value(&export).unwrap_or(JsValue::NULL),
            None => JsValue::NULL,
        }
    }

    /// Paste raw RGBA bytes as a new layer. Returns the new layer ID.
    pub fn paste_image(
        &mut self,
        width: u32,
        height: u32,
        rgba: &[u8],
        offset_x: i32,
        offset_y: i32,
        active_layer_id: f64,
    ) -> f64 {
        let active = if active_layer_id >= 0.0 { Some(active_layer_id as u64) } else { None };
        self.0.paste_image(width, height, rgba, offset_x, offset_y, active) as f64
    }

    /// Paste from internal clipboard at original position. Returns layer ID or -1.
    pub fn paste_in_place(&mut self, active_layer_id: f64) -> f64 {
        let active = if active_layer_id >= 0.0 { Some(active_layer_id as u64) } else { None };
        match self.0.paste_in_place(active) {
            Some(id) => id as f64,
            None => -1.0,
        }
    }

    // --- Floating Content (Phase 7) ---

    /// Paste from internal clipboard as floating content on the target layer.
    /// Returns true if floating content was created.
    pub fn paste_in_place_floating(&mut self, layer_id: f64) -> bool {
        self.0.paste_in_place_floating(layer_id as u64)
    }

    /// Begin interactive transform on the target layer's content.
    /// Returns true if floating content was created.
    pub fn begin_transform(&mut self, layer_id: f64) -> bool {
        self.0.begin_transform(layer_id as u64)
    }

    /// Update the floating content's affine transform matrix.
    /// Matrix is [a, b, tx, c, d, ty] (2D affine, 6 floats).
    pub fn update_floating_matrix(&mut self, matrix: &[f32]) {
        if matrix.len() >= 6 {
            let m = [matrix[0], matrix[1], matrix[2], matrix[3], matrix[4], matrix[5]];
            self.0.update_floating_matrix(m);
        }
    }

    /// Commit floating content (rasterize into target layer/mask).
    pub fn commit_floating(&mut self) {
        self.0.commit_floating();
    }

    /// Cancel floating content (discard or restore original tiles).
    pub fn cancel_floating(&mut self) {
        self.0.cancel_floating();
    }

    /// Check if there is active floating content.
    pub fn has_floating(&self) -> bool {
        self.0.has_floating()
    }

    /// Return floating content info as a Float32Array of 10 values:
    /// [origin_x, origin_y, width, height, m0, m1, m2, m3, m4, m5]
    /// Returns null/undefined if no floating content is active.
    pub fn floating_info(&self) -> Option<Box<[f32]>> {
        self.0.floating_info().map(|(ox, oy, w, h, m)| {
            vec![ox, oy, w, h, m[0], m[1], m[2], m[3], m[4], m[5]].into_boxed_slice()
        })
    }

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

    pub fn veil_types(&self) -> JsValue {
        serde_wasm_bindgen::to_value(&self.0.veil_types()).unwrap()
    }

    // --- Thumbnails ---

    pub fn layer_thumbnail(&self, layer_id: f64, width: u32, height: u32) -> Vec<u8> {
        self.0.layer_thumbnail(layer_id as u64, width, height)
    }
    pub fn mask_thumbnail(&self, layer_id: f64, width: u32, height: u32) -> Vec<u8> {
        self.0.mask_thumbnail(layer_id as u64, width, height)
    }

    // --- Tool Overlay ---

    /// Set overlay primitives from a JS array of primitive objects.
    pub fn set_overlay(&mut self, primitives: JsValue) {
        let arr: js_sys::Array = match primitives.dyn_into() {
            Ok(a) => a,
            Err(_) => return,
        };
        let mut prims = Vec::with_capacity(arr.length() as usize);
        for i in 0..arr.length() {
            let obj = arr.get(i);
            if let Some(p) = js_to_overlay_primitive(&obj) {
                prims.push(p);
            }
        }
        self.0.set_overlay_primitives(prims);
    }

    /// Clear all overlay primitives.
    pub fn clear_overlay(&mut self) {
        self.0.clear_overlay();
    }

    /// Hit-test overlay primitives at screen coordinates.
    /// Returns the index of the hit primitive, or -1 if none.
    pub fn overlay_hit_test(&self, screen_x: f32, screen_y: f32) -> i32 {
        match self.0.overlay_hit_test(screen_x, screen_y) {
            Some(i) => i as i32,
            None => -1,
        }
    }
}

// ---------------------------------------------------------------------------
// JS ↔ OverlayPrimitive conversion
// ---------------------------------------------------------------------------

fn js_f32(obj: &JsValue, key: &str) -> Option<f32> {
    js_sys::Reflect::get(obj, &JsValue::from_str(key))
        .ok()
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
}

fn js_u32(obj: &JsValue, key: &str) -> Option<u32> {
    js_sys::Reflect::get(obj, &JsValue::from_str(key))
        .ok()
        .and_then(|v| v.as_f64())
        .map(|v| v as u32)
}

fn js_f32_pair(obj: &JsValue, key: &str) -> Option<[f32; 2]> {
    let arr: js_sys::Array = js_sys::Reflect::get(obj, &JsValue::from_str(key))
        .ok()?
        .dyn_into()
        .ok()?;
    Some([arr.get(0).as_f64()? as f32, arr.get(1).as_f64()? as f32])
}

fn js_f32_quad(obj: &JsValue, key: &str) -> Option<[f32; 4]> {
    let arr: js_sys::Array = js_sys::Reflect::get(obj, &JsValue::from_str(key))
        .ok()?
        .dyn_into()
        .ok()?;
    Some([
        arr.get(0).as_f64()? as f32,
        arr.get(1).as_f64()? as f32,
        arr.get(2).as_f64()? as f32,
        arr.get(3).as_f64()? as f32,
    ])
}

fn js_to_overlay_primitive(obj: &JsValue) -> Option<OverlayPrimitive> {
    let kind = js_u32(obj, "kind")?;
    let flags = js_u32(obj, "flags").unwrap_or(0);
    let p0 = js_f32_pair(obj, "p0")?;
    let p1 = js_f32_pair(obj, "p1")?;
    let color = js_f32_quad(obj, "color").unwrap_or([1.0, 1.0, 1.0, 1.0]);
    let thickness = js_f32(obj, "thickness").unwrap_or(1.0);
    let dash_len = js_f32(obj, "dashLen").unwrap_or(0.0);
    let dash_offset = js_f32(obj, "dashOffset").unwrap_or(0.0);
    let corner_radius = js_f32(obj, "cornerRadius").unwrap_or(0.0);

    Some(OverlayPrimitive {
        color,
        p0,
        p1,
        thickness,
        dash_len,
        dash_offset,
        corner_radius,
        kind,
        flags,
        _pad: [0; 2],
    })
}
