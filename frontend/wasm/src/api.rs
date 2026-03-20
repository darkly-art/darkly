//! WASM bridge for the Darkly engine.
//!
//! ## Re-entrancy protection
//!
//! `#[wasm_bindgen]` structs are internally wrapped in `Rc<RefCell<T>>`.
//! Every `&mut self` method takes an exclusive borrow on that RefCell for the
//! duration of the call.  This is normally safe because JS is single-threaded.
//!
//! **The bug:** Chromium's WebGPU implementation can synchronously pump the
//! browser event queue during `queue.submit()` and `surface.get_current_texture()`.
//! If a `requestAnimationFrame` callback or pointer event is pending, the browser
//! executes it *inside* the GPU call — while the `&mut self` borrow is still held.
//! If that callback calls another `&mut self` method on the same handle, the
//! RefCell panics with "recursive use of an object detected".  Worse, the panic
//! unwinds through wasm-bindgen without dropping the borrow guard, permanently
//! poisoning the RefCell — every subsequent call fails for the rest of the session.
//!
//! **Concrete scenario:**
//! ```text
//! stroke_to(&mut self)        ← wasm-bindgen borrow #1
//!   → brush engine → queue.submit()
//!     → Chromium pumps event queue
//!       → pending rAF fires
//!         → render(&mut self)  ← borrow #2 — PANIC
//!           → borrow guard leaked → handle permanently dead
//! ```
//!
//! **Options considered:**
//!
//! 1. **JS-side boolean guard** — check a flag before each WASM call, skip if busy.
//!    Simple but doesn't fix the root cause; every new call site must remember
//!    to check, and the RefCell is still vulnerable to permanent poisoning on
//!    any missed guard.
//!
//! 2. **Interior mutability + command queue (chosen)** — two-layer fix:
//!
//!    *Layer 1 — interior mutability:* all `#[wasm_bindgen]` methods take `&self`.
//!    wasm-bindgen's outer RefCell only ever does shared borrows (which can't
//!    conflict).  We manage mutation through our own RefCells inside the handle,
//!    using `try_borrow_mut()` so re-entrant calls see "busy" and gracefully skip
//!    instead of panicking.  This eliminates permanent RefCell poisoning.
//!
//!    *Layer 2 — stroke command queue:* the hot-path conflict is between
//!    `stroke_to` (GPU dab generation during painting) and `render()` (present
//!    to screen).  We separate them structurally: `stroke_to`, `begin_stroke`,
//!    and `end_stroke` push commands to a `RefCell<Vec<StrokeCommand>>` without
//!    touching the engine.  `render()` drains the queue first, then renders.
//!    Since the queue and the engine live in separate RefCells, there is no
//!    borrow conflict — the race is eliminated deterministically, not by luck.
//!
//! 3. **Split into multiple WASM handles** — separate `RenderHandle` and
//!    `PaintHandle` with independent RefCells.  Architecturally cleaner but
//!    requires splitting `DarklyEngine` and sharing state (GPU context, document)
//!    across handles — a much larger refactor.
//!
//! We chose option 2 because it eliminates the race deterministically (command
//! queue) while also providing defense-in-depth against any other re-entrancy
//! vectors we haven't found yet (interior mutability).  If the command queue
//! pattern proves too limiting (e.g. we need synchronous stroke results), the
//! next step would be option 3 (split handles).
//!
//! ## Serialization conventions
//!
//! - **Rust → JS** (queries): return `String` via `serde_json::to_string`.
//!   The JS side calls `JSON.parse()`.  This avoids `serde_wasm_bindgen`
//!   edge cases (e.g. `HashMap<NonStringKey, _>` → JS `Map`).
//!
//! - **JS → Rust** (commands): accept `JsValue` and deserialize with
//!   `serde_wasm_bindgen::from_value`.  This direction is reliable because
//!   JS plain objects always map to Rust structs/maps correctly.
//!
//! - **Hot-path primitives** (overlays): manual `js_sys::Reflect` extraction
//!   for zero-allocation, per-frame data.

use std::cell::RefCell;

use darkly::document::MoveTarget;
use darkly::engine::{DarklyEngine, StrokeOp};
use darkly::gpu::context::GpuContext;
use darkly::gpu::overlay::OverlayPrimitive;
use darkly::gpu::params::{ParamDef, ParamValue};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsError;

/// Queued stroke command — stored in a separate RefCell from the engine so
/// that `stroke_to` (push) and `render` (drain + execute) never conflict.
enum StrokeCommand {
    Begin(u64),
    Op(StrokeOp),
    End,
}

#[wasm_bindgen]
pub struct DarklyHandle {
    engine: RefCell<DarklyEngine>,
    /// Stroke commands queued between frames.  Drained by `render()`.
    stroke_queue: RefCell<Vec<StrokeCommand>>,
}

impl DarklyHandle {
    /// Try to borrow the engine mutably.  Returns `None` (and logs) if a
    /// re-entrant call is in progress — see module docs for why this happens.
    fn engine_mut(&self) -> Option<std::cell::RefMut<'_, DarklyEngine>> {
        match self.engine.try_borrow_mut() {
            Ok(r) => Some(r),
            Err(_) => {
                log::debug!("engine_mut: skipping re-entrant call");
                None
            }
        }
    }

    /// Try to borrow the engine immutably.  Returns `None` if an exclusive
    /// borrow is held (i.e. a mutation is in progress).
    fn engine(&self) -> Option<std::cell::Ref<'_, DarklyEngine>> {
        match self.engine.try_borrow() {
            Ok(r) => Some(r),
            Err(_) => {
                log::debug!("engine: skipping re-entrant query");
                None
            }
        }
    }

    /// Drain the stroke command queue and execute all pending commands.
    /// Called at the start of `render()` so stroke GPU work and presentation
    /// happen in one engine borrow with no re-entrancy window between them.
    fn drain_stroke_queue(&self, engine: &mut DarklyEngine) {
        let commands: Vec<StrokeCommand> = self.stroke_queue.borrow_mut().drain(..).collect();
        for cmd in commands {
            match cmd {
                StrokeCommand::Begin(layer_id) => engine.begin_stroke(layer_id),
                StrokeCommand::Op(op) => engine.stroke_to(op),
                StrokeCommand::End => engine.end_stroke(),
            }
        }
    }
}

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

        DarklyHandle {
            engine: RefCell::new(DarklyEngine::new(gpu, doc_width, doc_height)),
            stroke_queue: RefCell::new(Vec::new()),
        }
    }

    // --- Layer CRUD (pass-through) ---
    // IDs use f64 because JS has no u64 — any JS-facing backend needs this.

    pub fn add_raster_layer(&self) -> f64 { self.engine_mut().map_or(0.0, |mut e| e.add_raster_layer() as f64) }
    pub fn add_raster_layer_in(&self, group_id: f64) -> f64 { self.engine_mut().map_or(0.0, |mut e| e.add_raster_layer_in(group_id as u64) as f64) }
    pub fn add_group(&self) -> f64 { self.engine_mut().map_or(0.0, |mut e| e.add_group() as f64) }

    pub fn remove_layer(&self, layer_id: f64) -> Result<(), JsError> {
        match self.engine_mut() {
            Some(mut e) => e.remove_layer(layer_id as u64).map_err(|e| JsError::new(&e)),
            None => Ok(()),
        }
    }

    pub fn move_layer(&self, layer_id: f64, target_type: &str, target_id: f64) {
        let Some(mut e) = self.engine_mut() else { return };
        let target = match target_type {
            "before" => MoveTarget::Before(target_id as u64),
            "after" => MoveTarget::After(target_id as u64),
            "into_top" => MoveTarget::IntoGroupTop(target_id as u64),
            "into_bottom" => MoveTarget::IntoGroupBottom(target_id as u64),
            _ => return,
        };
        e.move_layer(layer_id as u64, target)
    }

    // --- Layer properties (pass-through) ---

    pub fn set_opacity(&self, layer_id: f64, opacity: f32) { if let Some(mut e) = self.engine_mut() { e.set_opacity(layer_id as u64, opacity) } }
    pub fn set_blend_mode(&self, layer_id: f64, mode: u32) { if let Some(mut e) = self.engine_mut() { e.set_blend_mode(layer_id as u64, mode) } }
    pub fn set_layer_visible(&self, layer_id: f64, visible: bool) { if let Some(mut e) = self.engine_mut() { e.set_layer_visible(layer_id as u64, visible) } }
    pub fn set_layer_name(&self, layer_id: f64, name: &str) { if let Some(mut e) = self.engine_mut() { e.set_layer_name(layer_id as u64, name) } }
    pub fn set_group_collapsed(&self, group_id: f64, collapsed: bool) { if let Some(mut e) = self.engine_mut() { e.set_group_collapsed(group_id as u64, collapsed) } }
    pub fn set_group_passthrough(&self, group_id: f64, passthrough: bool) { if let Some(mut e) = self.engine_mut() { e.set_group_passthrough(group_id as u64, passthrough) } }

    // --- Layer Masks (pass-through) ---

    pub fn add_mask(&self, layer_id: f64) { if let Some(mut e) = self.engine_mut() { e.add_mask(layer_id as u64) } }
    pub fn remove_mask(&self, layer_id: f64) { if let Some(mut e) = self.engine_mut() { e.remove_mask(layer_id as u64) } }
    pub fn apply_mask(&self, layer_id: f64) { if let Some(mut e) = self.engine_mut() { e.apply_mask(layer_id as u64) } }
    pub fn set_mask_enabled(&self, layer_id: f64, enabled: bool) { if let Some(mut e) = self.engine_mut() { e.set_mask_enabled(layer_id as u64, enabled) } }
    pub fn set_show_mask(&self, layer_id: f64, show: bool) { if let Some(mut e) = self.engine_mut() { e.set_show_mask(layer_id as u64, show) } }
    pub fn set_editing_mask(&self, layer_id: f64, editing: bool) { if let Some(mut e) = self.engine_mut() { e.set_editing_mask(layer_id as u64, editing) } }
    pub fn selection_to_mask(&self, layer_id: f64) { if let Some(mut e) = self.engine_mut() { e.selection_to_mask(layer_id as u64) } }
    pub fn mask_to_selection(&self, layer_id: f64) { if let Some(mut e) = self.engine_mut() { e.mask_to_selection(layer_id as u64) } }

    // --- Painting (pass-through) ---

    pub fn fill_gradient(&self, layer_id: f64) { if let Some(mut e) = self.engine_mut() { e.fill_gradient(layer_id as u64) } }

    // --- Stroke lifecycle (queued) ---
    // These push to a command queue (separate RefCell) instead of borrowing the
    // engine.  The queue is drained at the start of render(), so stroke GPU work
    // and presentation happen in one engine borrow — no re-entrancy window.

    pub fn begin_stroke(&self, layer_id: f64) {
        self.stroke_queue.borrow_mut().push(StrokeCommand::Begin(layer_id as u64));
    }

    pub fn stroke_to(&self, op_type: &str, params: JsValue) {
        // Deserialize here (no engine borrow needed) and queue the result.
        js_sys::Reflect::set(&params, &"op".into(), &op_type.into()).ok();
        match serde_wasm_bindgen::from_value::<StrokeOp>(params) {
            Ok(op) => self.stroke_queue.borrow_mut().push(StrokeCommand::Op(op)),
            Err(e) => log::error!("stroke_to deserialization failed: {e}"),
        }
    }

    pub fn end_stroke(&self) {
        self.stroke_queue.borrow_mut().push(StrokeCommand::End);
    }

    // Legacy compat
    pub fn snapshot(&self, layer_id: f64) {
        self.stroke_queue.borrow_mut().push(StrokeCommand::Begin(layer_id as u64));
    }
    pub fn commit(&self) {
        self.stroke_queue.borrow_mut().push(StrokeCommand::End);
    }

    // --- View transform ---

    pub fn set_view_transform(&self, pan_x: f32, pan_y: f32, zoom: f32, rotation: f32, screen_w: f32, screen_h: f32) {
        if let Some(mut e) = self.engine_mut() { e.set_view_transform(pan_x, pan_y, zoom, rotation, screen_w, screen_h) }
    }

    pub fn screen_to_canvas(&self, screen_x: f32, screen_y: f32) -> Vec<f32> {
        match self.engine() {
            Some(e) => {
                let (cx, cy) = e.screen_to_canvas(screen_x, screen_y);
                vec![cx, cy]
            }
            None => vec![0.0, 0.0],
        }
    }

    /// Start an async color pick. Returns the last picked color immediately
    /// for responsive UI — the real result arrives on the next frame.
    pub fn pick_color(&self, x: f32, y: f32) -> Vec<u8> {
        match self.engine_mut() {
            Some(mut e) => e.pick_color(x, y).to_vec(),
            None => vec![0, 0, 0, 0],
        }
    }

    /// Get the most recently resolved color pick result.
    pub fn last_picked_color(&self) -> Vec<u8> {
        match self.engine() {
            Some(e) => e.last_picked_color().to_vec(),
            None => vec![0, 0, 0, 0],
        }
    }

    /// True if a color pick readback is still in flight.
    pub fn has_pending_color_pick(&self) -> bool {
        self.engine().map_or(false, |e| e.has_pending_color_pick())
    }

    // --- Rendering ---

    /// Render the current frame. Returns true if animations need another frame.
    ///
    /// Drains the stroke command queue first, then renders.  All GPU work
    /// (dab generation, compositing, presentation) happens in this single
    /// engine borrow — no other method needs the engine during a stroke.
    ///
    /// If the engine is busy (re-entrant call from WebGPU event pumping),
    /// returns false — the outer render call is already in progress and will
    /// handle everything.  Returning true here would cause the JS side to
    /// schedule another rAF, which Chromium's event pump fires immediately,
    /// creating an infinite loop that freezes the UI (especially with
    /// high-frequency pen input that keeps refilling the event queue).
    pub fn render(&self, time_secs: f32) -> bool {
        let Some(mut e) = self.engine_mut() else { return false };
        self.drain_stroke_queue(&mut e);
        e.render(time_secs)
    }

    pub fn resize(&self, width: u32, height: u32) { if let Some(mut e) = self.engine_mut() { e.resize(width, height) } }

    // --- Undo / Redo ---

    pub fn undo(&self) { if let Some(mut e) = self.engine_mut() { e.undo() } }
    pub fn redo(&self) { if let Some(mut e) = self.engine_mut() { e.redo() } }

    // --- Selection ---

    pub fn select_rect(&self, x: f32, y: f32, w: f32, h: f32, mode: &str, antialias: bool, feather: f32) {
        if let Some(mut e) = self.engine_mut() { e.select_rect(x, y, w, h, parse_selection_mode(mode), antialias, feather) }
    }

    pub fn select_ellipse(&self, x: f32, y: f32, w: f32, h: f32, mode: &str, antialias: bool, feather: f32) {
        if let Some(mut e) = self.engine_mut() { e.select_ellipse(x, y, w, h, parse_selection_mode(mode), antialias, feather) }
    }

    pub fn select_lasso(&self, vertices: JsValue, mode: &str, antialias: bool, feather: f32) {
        let Some(mut e) = self.engine_mut() else { return };
        let verts: Vec<[f32; 2]> = serde_wasm_bindgen::from_value(vertices).unwrap_or_default();
        e.select_lasso(&verts, parse_selection_mode(mode), antialias, feather)
    }

    pub fn select_magic_wand(&self, layer_id: u64, seed_x: i32, seed_y: i32, tolerance: u8, mode: &str) {
        if let Some(mut e) = self.engine_mut() { e.select_magic_wand(layer_id, seed_x, seed_y, tolerance, parse_selection_mode(mode)) }
    }

    pub fn clear_selection(&self) { if let Some(mut e) = self.engine_mut() { e.clear_selection() } }
    pub fn clear_selection_contents(&self, layer_id: f64) { if let Some(mut e) = self.engine_mut() { e.clear_selection_contents(layer_id as u64) } }
    pub fn select_all(&self) { if let Some(mut e) = self.engine_mut() { e.select_all() } }
    pub fn invert_selection(&self) { if let Some(mut e) = self.engine_mut() { e.invert_selection() } }
    pub fn has_selection(&self) -> bool { self.engine().map_or(false, |e| e.has_selection()) }

    // --- Copy / Cut / Paste ---

    /// Copy active layer content (masked by selection).
    /// Returns a JS object `{rgba, width, height, offsetX, offsetY}` or null.
    pub fn copy(&self, layer_id: f64) -> JsValue {
        let Some(mut e) = self.engine_mut() else { return JsValue::NULL };
        match e.copy(layer_id as u64) {
            Some(export) => serde_wasm_bindgen::to_value(&export).unwrap_or(JsValue::NULL),
            None => JsValue::NULL,
        }
    }

    /// Cut = copy + clear. Returns the same object as copy, or null.
    pub fn cut(&self, layer_id: f64) -> JsValue {
        let Some(mut e) = self.engine_mut() else { return JsValue::NULL };
        match e.cut(layer_id as u64) {
            Some(export) => serde_wasm_bindgen::to_value(&export).unwrap_or(JsValue::NULL),
            None => JsValue::NULL,
        }
    }

    /// Poll for a completed copy/cut result (async readback).
    /// Returns the ClipboardExport object, or null if not ready yet.
    pub fn poll_copy_result(&self) -> JsValue {
        let Some(mut e) = self.engine_mut() else { return JsValue::NULL };
        match e.poll_copy_result() {
            Some(export) => serde_wasm_bindgen::to_value(&export).unwrap_or(JsValue::NULL),
            None => JsValue::NULL,
        }
    }

    /// Paste raw RGBA bytes as a new layer. Returns the new layer ID.
    pub fn paste_image(
        &self,
        width: u32,
        height: u32,
        rgba: &[u8],
        offset_x: i32,
        offset_y: i32,
        active_layer_id: f64,
    ) -> f64 {
        let Some(mut e) = self.engine_mut() else { return 0.0 };
        let active = if active_layer_id >= 0.0 { Some(active_layer_id as u64) } else { None };
        e.paste_image(width, height, rgba, offset_x, offset_y, active) as f64
    }

    /// Paste from internal clipboard at original position. Returns layer ID or -1.
    pub fn paste_in_place(&self, active_layer_id: f64) -> f64 {
        let Some(mut e) = self.engine_mut() else { return -1.0 };
        let active = if active_layer_id >= 0.0 { Some(active_layer_id as u64) } else { None };
        match e.paste_in_place(active) {
            Some(id) => id as f64,
            None => -1.0,
        }
    }

    // --- Floating Content (Phase 7) ---

    /// Paste from internal clipboard as floating content on the target layer.
    /// Returns true if floating content was created.
    pub fn paste_in_place_floating(&self, layer_id: f64) -> bool {
        self.engine_mut().map_or(false, |mut e| e.paste_in_place_floating(layer_id as u64))
    }

    /// Begin interactive transform on the target layer's content.
    /// Returns true if floating content was created.
    pub fn begin_transform(&self, layer_id: f64) -> bool {
        self.engine_mut().map_or(false, |mut e| e.begin_transform(layer_id as u64))
    }

    /// Update the floating content's affine transform matrix.
    /// Matrix is [a, b, tx, c, d, ty] (2D affine, 6 floats).
    pub fn update_floating_matrix(&self, matrix: &[f32]) {
        if matrix.len() >= 6 {
            if let Some(mut e) = self.engine_mut() {
                let m = [matrix[0], matrix[1], matrix[2], matrix[3], matrix[4], matrix[5]];
                e.update_floating_matrix(m);
            }
        }
    }

    /// Commit floating content (rasterize into target layer/mask).
    pub fn commit_floating(&self) { if let Some(mut e) = self.engine_mut() { e.commit_floating() } }

    /// Cancel floating content (discard or restore original tiles).
    pub fn cancel_floating(&self) { if let Some(mut e) = self.engine_mut() { e.cancel_floating() } }

    /// Check if there is active floating content.
    pub fn has_floating(&self) -> bool { self.engine().map_or(false, |e| e.has_floating()) }

    /// Return floating content info as a Float32Array of 10 values:
    /// [origin_x, origin_y, width, height, m0, m1, m2, m3, m4, m5]
    /// Returns null/undefined if no floating content is active.
    pub fn floating_info(&self) -> Option<Box<[f32]>> {
        let e = self.engine()?;
        e.floating_info().map(|(ox, oy, w, h, m)| {
            vec![ox, oy, w, h, m[0], m[1], m[2], m[3], m[4], m[5]].into_boxed_slice()
        })
    }

    // --- Veils ---

    pub fn add_veil(&self, veil_type: &str, params: JsValue) {
        let Some(mut e) = self.engine_mut() else { return };
        let pv = js_to_param_values(&params, e.veil_param_defs(veil_type));
        e.add_veil(veil_type, &pv)
    }

    pub fn remove_veil(&self, index: u32) { if let Some(mut e) = self.engine_mut() { e.remove_veil(index as usize) } }
    pub fn clear_veils(&self) { if let Some(mut e) = self.engine_mut() { e.clear_veils() } }
    pub fn set_veil_visible(&self, index: u32, visible: bool) { if let Some(mut e) = self.engine_mut() { e.set_veil_visible(index as usize, visible) } }
    pub fn move_veil(&self, from: u32, to: u32) { if let Some(mut e) = self.engine_mut() { e.move_veil(from as usize, to as usize) } }

    pub fn update_veil(&self, index: u32, params: JsValue) {
        let Some(mut e) = self.engine_mut() else { return };
        let type_id = match e.veil_list().iter().find(|v| v.index == index as usize) {
            Some(v) => v.type_id.clone(),
            None => return,
        };
        let pv = js_to_param_values(&params, e.veil_param_defs(&type_id));
        e.update_veil(index as usize, &pv)
    }

    // --- Brush Graph (Phase 5) ---

    /// Return metadata for all registered brush node types as a JSON string.
    pub fn brush_node_types(&self) -> String {
        match self.engine() {
            Some(e) => {
                let val = e.brush_node_types();
                serde_json::to_string(&val).unwrap_or_else(|_| "[]".into())
            }
            None => "[]".into(),
        }
    }

    /// Return the default brush graph as a JSON string.
    pub fn brush_graph_default(&self) -> String {
        match self.engine() {
            Some(e) => {
                let val = e.default_brush_graph();
                serde_json::to_string(&val).unwrap_or_else(|_| "null".into())
            }
            None => "null".into(),
        }
    }

    /// Return the currently active brush graph as a JSON string.
    pub fn brush_graph_active(&self) -> String {
        match self.engine() {
            Some(e) => serde_json::to_string(e.active_brush_graph_ref()).unwrap_or_else(|_| "null".into()),
            None => "null".into(),
        }
    }

    /// Validate a brush graph JSON string.  Returns null on success or an error string.
    pub fn brush_graph_validate(&self, json: &str) -> JsValue {
        let Some(e) = self.engine() else { return JsValue::from_str("engine busy") };
        match e.validate_brush_graph(json) {
            Ok(()) => JsValue::NULL,
            Err(e) => JsValue::from_str(&e),
        }
    }

    /// Compile a brush graph from JSON and set it as the active brush.
    /// Returns null on success or an error string.
    pub fn brush_graph_compile(&self, json: &str) -> JsValue {
        let Some(mut e) = self.engine_mut() else { return JsValue::from_str("engine busy") };
        match e.set_brush_graph(json) {
            Ok(()) => JsValue::NULL,
            Err(e) => JsValue::from_str(&e),
        }
    }

    /// Reset the active brush graph to the built-in default.
    pub fn brush_graph_reset(&self) { if let Some(mut e) = self.engine_mut() { e.reset_brush_graph() } }

    // --- Fine-grained graph commands ---
    // Each returns a JsValue: on success, a JS object { graph: "<json>" }.
    // On failure, a JS object { error: "<message>" }.
    // If the engine is busy, returns { error: "engine busy" }.

    fn graph_result(r: Result<String, String>) -> JsValue {
        match r {
            Ok(json) => {
                let obj = js_sys::Object::new();
                js_sys::Reflect::set(&obj, &"graph".into(), &JsValue::from_str(&json)).unwrap();
                obj.into()
            }
            Err(e) => {
                let obj = js_sys::Object::new();
                js_sys::Reflect::set(&obj, &"error".into(), &JsValue::from_str(&e)).unwrap();
                obj.into()
            }
        }
    }

    pub fn brush_graph_add_node(&self, type_id: &str, x: f32, y: f32) -> JsValue {
        let Some(mut e) = self.engine_mut() else { return Self::graph_result(Err("engine busy".into())) };
        Self::graph_result(e.brush_graph_add_node(type_id, x, y))
    }

    pub fn brush_graph_remove_node(&self, node_id: u32) -> JsValue {
        let Some(mut e) = self.engine_mut() else { return Self::graph_result(Err("engine busy".into())) };
        Self::graph_result(e.brush_graph_remove_node(node_id as u64))
    }

    pub fn brush_graph_connect(
        &self,
        from_node: u32,
        from_port: &str,
        to_node: u32,
        to_port: &str,
    ) -> JsValue {
        let Some(mut e) = self.engine_mut() else { return Self::graph_result(Err("engine busy".into())) };
        Self::graph_result(e.brush_graph_connect(from_node as u64, from_port, to_node as u64, to_port))
    }

    pub fn brush_graph_disconnect(
        &self,
        from_node: u32,
        from_port: &str,
        to_node: u32,
        to_port: &str,
    ) -> JsValue {
        let Some(mut e) = self.engine_mut() else { return Self::graph_result(Err("engine busy".into())) };
        Self::graph_result(e.brush_graph_disconnect(from_node as u64, from_port, to_node as u64, to_port))
    }

    pub fn brush_graph_set_param(&self, node_id: u32, param_index: u32, kind: &str, value: JsValue) -> JsValue {
        let Some(mut e) = self.engine_mut() else { return Self::graph_result(Err("engine busy".into())) };
        let pv = match kind {
            "float" => darkly::gpu::params::ParamValue::Float(value.as_f64().unwrap_or(0.0) as f32),
            "int" => darkly::gpu::params::ParamValue::Int(value.as_f64().unwrap_or(0.0) as i32),
            "bool" => darkly::gpu::params::ParamValue::Bool(value.as_bool().unwrap_or(false)),
            _ => return Self::graph_result(Err(format!("unknown param kind: {kind}"))),
        };
        Self::graph_result(e.brush_graph_set_param(node_id as u64, param_index as usize, pv))
    }

    pub fn brush_graph_move_node(&self, node_id: u32, x: f32, y: f32) {
        if let Some(mut e) = self.engine_mut() {
            e.brush_graph_move_node(node_id as u64, x, y);
        }
    }

    // --- Queries ---
    // Rust→JS serialization uses JSON strings (serde_json).  This avoids
    // serde_wasm_bindgen edge cases (e.g. HashMap with non-string keys
    // becomes a JS Map instead of a plain object) and gives a single,
    // predictable convention: all query methods return `String`, JS calls
    // `JSON.parse()`.

    pub fn layer_tree(&self) -> String {
        match self.engine() {
            Some(e) => serde_json::to_string(&e.layer_tree()).unwrap_or_else(|_| "[]".into()),
            None => "[]".into(),
        }
    }

    pub fn veil_list(&self) -> String {
        match self.engine() {
            Some(e) => serde_json::to_string(&e.veil_list()).unwrap_or_else(|_| "[]".into()),
            None => "[]".into(),
        }
    }

    pub fn veil_types(&self) -> String {
        match self.engine() {
            Some(e) => serde_json::to_string(&e.veil_types()).unwrap_or_else(|_| "[]".into()),
            None => "[]".into(),
        }
    }

    // --- Thumbnails ---

    pub fn layer_thumbnail(&self, layer_id: f64, width: u32, height: u32) -> Vec<u8> {
        self.engine_mut().map_or_else(Vec::new, |mut e| e.layer_thumbnail(layer_id as u64, width, height))
    }
    pub fn mask_thumbnail(&self, layer_id: f64, width: u32, height: u32) -> Vec<u8> {
        self.engine_mut().map_or_else(Vec::new, |mut e| e.mask_thumbnail(layer_id as u64, width, height))
    }

    // --- Tool Overlay ---

    /// Set overlay primitives from a JS array of primitive objects.
    pub fn set_overlay(&self, primitives: JsValue) {
        let Some(mut e) = self.engine_mut() else { return };
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
        e.set_overlay_primitives(prims);
    }

    /// Clear all overlay primitives.
    pub fn clear_overlay(&self) { if let Some(mut e) = self.engine_mut() { e.clear_overlay() } }

    /// Hit-test overlay primitives at screen coordinates.
    /// Returns the index of the hit primitive, or -1 if none.
    pub fn overlay_hit_test(&self, screen_x: f32, screen_y: f32) -> i32 {
        match self.engine() {
            Some(e) => match e.overlay_hit_test(screen_x, screen_y) {
                Some(i) => i as i32,
                None => -1,
            },
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
