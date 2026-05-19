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
//! **Architecture: generalized command queue.**
//!
//! All methods take `&self` (shared borrow on wasm-bindgen's outer RefCell —
//! can never conflict).  Internally, methods fall into three categories:
//!
//! 1. **Queued mutations** (~40 methods): push a [`Command`] variant to a
//!    `RefCell<Vec<Command>>` without touching the engine.  Fast, zero
//!    re-entrancy risk.  Covers all fire-and-forget operations: layer props,
//!    masks, selection, undo/redo, view transform, strokes, overlays, etc.
//!
//! 2. **Direct mutations** (~15 methods): call [`flush_if_needed`] then
//!    `self.engine.borrow_mut()`.  These are click-frequency user-initiated
//!    operations (add layer, brush graph compile, copy/cut, paste) that
//!    return values.  They **panic** on re-entrancy — if that ever fires,
//!    it's a bug to fix structurally, not paper over with silent failure.
//!
//! 3. **Queries** (~18 methods): call [`flush_if_needed`] then
//!    `self.engine.borrow()`.  Always succeeds — no competing `borrow_mut()`
//!    exists outside `render()` and the flush path.
//!
//! [`render`] is the **one** method that uses `try_borrow_mut()` — it is the
//! actual re-entrancy target (rAF fired during GPU event pumping).  Returning
//! `false` when busy is correct: the outer render call is already in progress
//! and will handle everything.
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

use std::sync::Arc;

use darkly::brush::paint_info::PaintInformation;
use darkly::document::{MoveTarget, SelectionMode};
use darkly::engine::{DarklyEngine, StrokeOp};
use darkly::gpu::context::{GpuContext, GpuDevice};
use darkly::gpu::overlay::OverlayPrimitive;
use darkly::gpu::params::{ParamDef, ParamValue};
use darkly::layer::LayerId;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsError;

// ---------------------------------------------------------------------------
// Command queue
// ---------------------------------------------------------------------------

/// All fire-and-forget mutations.  Pushed by `#[wasm_bindgen]` methods,
/// drained by [`DarklyHandle::render`] (or [`DarklyHandle::flush_if_needed`]).
enum Command {
    // Stroke lifecycle
    BeginStroke(u64),
    StrokeOp(StrokeOp),
    EndStroke,

    // Layer properties
    SetOpacity(u64, f32),
    SetBlendMode(u64, String),
    /// Toggle visibility on any node — layer, group, or modifier (mask).
    SetLayerVisible(u64, bool),
    SetLayerName(u64, String),
    SetGroupCollapsed(u64, bool),
    SetGroupPassthrough(u64, bool),
    /// Lock toggle on any node — paint/transform/property change refused while locked.
    SetNodeLocked(u64, bool),
    /// Session "isolate this node" flag. `0` = clear; otherwise the node id.
    /// Replaces the previous `SetShowMask` per-layer flag.
    SetIsolatedNode(u64),

    // Modifier (mask) operations
    AddMask(u64),
    RemoveMask(u64),
    ApplyMask(u64),
    SelectionToMask(u64),
    MaskToSelection(u64),

    // Painting
    FillBackground(u64),
    FillBackgroundColor(u64, [u8; 4]),

    // Selection
    SelectRect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        mode: SelectionMode,
        antialias: bool,
        feather: f32,
    },
    SelectEllipse {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        mode: SelectionMode,
        antialias: bool,
        feather: f32,
    },
    SelectLasso {
        verts: Vec<[f32; 2]>,
        mode: SelectionMode,
        antialias: bool,
        feather: f32,
    },
    SelectMagicWand {
        layer_id: u64,
        seed_x: i32,
        seed_y: i32,
        tolerance: u8,
        mode: SelectionMode,
    },
    ClearSelection,
    ClearSelectionContents(u64),
    SelectAll,
    InvertSelection,

    // Undo / Redo
    Undo,
    Redo,

    // View transform
    SetViewTransform {
        pan_x: f32,
        pan_y: f32,
        zoom: f32,
        rotation: f32,
        mirror_h: bool,
        screen_w: f32,
        screen_h: f32,
    },
    Resize(u32, u32),

    // Floating content
    UpdateFloatingMatrix([f32; 6]),
    CommitFloating,
    CancelFloating,

    // Veils
    RemoveVeil(usize),
    ClearVeils,
    SetVeilVisible(usize, bool),
    MoveVeil(usize, usize),

    // Overlay
    SetOverlay(Vec<OverlayPrimitive>),
    ClearOverlay,
    SetOverlayMask(u32, u32, Vec<u8>),
    ClearOverlayMask,

    // Brush config
    SetBrushBlendMode(u32),
    ResetBrushGraph,

    // Color pick (pointer-frequency, starts async readback)
    PickColor(f32, f32),

    // Document name (queued — rename is fire-and-forget)
    SetDocumentName(String),
}

fn drain_commands(commands: &RefCell<Vec<Command>>, engine: &mut DarklyEngine) {
    let cmds: Vec<Command> = commands.borrow_mut().drain(..).collect();
    // Largest backlog of `BrushStroke` ops in a single drain. High values
    // mean the engine is falling behind input — each backed-up event will
    // still be processed in this drain. Fed to the stroke perf summary.
    let brush_backlog = cmds
        .iter()
        .filter(|c| matches!(c, Command::StrokeOp(StrokeOp::BrushStroke { .. })))
        .count() as u32;
    if brush_backlog > 0 {
        engine.record_input_backlog(brush_backlog);
    }
    for cmd in cmds {
        match cmd {
            Command::BeginStroke(id) => engine.begin_stroke(LayerId::from_ffi(id)),
            Command::StrokeOp(op) => engine.stroke_to(op),
            Command::EndStroke => engine.end_stroke(),

            Command::SetOpacity(id, v) => engine.set_opacity(LayerId::from_ffi(id), v),
            Command::SetBlendMode(id, ref v) => engine.set_blend_mode(LayerId::from_ffi(id), v),
            Command::SetLayerVisible(id, v) => engine.set_layer_visible(LayerId::from_ffi(id), v),
            Command::SetLayerName(id, ref name) => {
                engine.set_layer_name(LayerId::from_ffi(id), name)
            }
            Command::SetGroupCollapsed(id, v) => {
                engine.set_group_collapsed(LayerId::from_ffi(id), v)
            }
            Command::SetGroupPassthrough(id, v) => {
                engine.set_group_passthrough(LayerId::from_ffi(id), v)
            }

            Command::AddMask(id) => engine.add_mask(LayerId::from_ffi(id)),
            Command::RemoveMask(id) => engine.remove_mask(LayerId::from_ffi(id)),
            Command::ApplyMask(id) => engine.apply_mask(LayerId::from_ffi(id)),
            Command::SelectionToMask(id) => engine.selection_to_mask(LayerId::from_ffi(id)),
            Command::MaskToSelection(id) => engine.mask_to_selection(LayerId::from_ffi(id)),
            Command::SetNodeLocked(id, v) => engine.set_node_locked(LayerId::from_ffi(id), v),
            Command::SetIsolatedNode(id) => {
                // `id == 0` is the JS-side sentinel for "clear isolation".
                let target = if id == 0 {
                    None
                } else {
                    Some(LayerId::from_ffi(id))
                };
                engine.set_isolated_node(target);
            }

            Command::FillBackground(id) => engine.fill_background(LayerId::from_ffi(id)),
            Command::FillBackgroundColor(id, c) => {
                engine.fill_background_color(LayerId::from_ffi(id), c)
            }

            Command::SelectRect {
                x,
                y,
                w,
                h,
                mode,
                antialias,
                feather,
            } => {
                engine.select_rect(x, y, w, h, mode, antialias, feather);
            }
            Command::SelectEllipse {
                x,
                y,
                w,
                h,
                mode,
                antialias,
                feather,
            } => {
                engine.select_ellipse(x, y, w, h, mode, antialias, feather);
            }
            Command::SelectLasso {
                ref verts,
                mode,
                antialias,
                feather,
            } => {
                engine.select_lasso(verts, mode, antialias, feather);
            }
            Command::SelectMagicWand {
                layer_id,
                seed_x,
                seed_y,
                tolerance,
                mode,
            } => {
                engine.select_magic_wand(
                    LayerId::from_ffi(layer_id),
                    darkly::coord::CanvasPoint::new(seed_x, seed_y),
                    tolerance,
                    mode,
                );
            }
            Command::ClearSelection => engine.clear_selection(),
            Command::ClearSelectionContents(id) => {
                engine.clear_selection_contents(LayerId::from_ffi(id))
            }
            Command::SelectAll => engine.select_all(),
            Command::InvertSelection => engine.invert_selection(),

            Command::Undo => engine.undo(),
            Command::Redo => engine.redo(),

            Command::SetViewTransform {
                pan_x,
                pan_y,
                zoom,
                rotation,
                mirror_h,
                screen_w,
                screen_h,
            } => {
                engine
                    .set_view_transform(pan_x, pan_y, zoom, rotation, mirror_h, screen_w, screen_h);
            }
            Command::Resize(w, h) => engine.resize(w, h),

            Command::UpdateFloatingMatrix(m) => engine.update_floating_matrix(m),
            Command::CommitFloating => engine.commit_floating(),
            Command::CancelFloating => engine.cancel_floating(),

            Command::RemoveVeil(i) => engine.remove_veil(i),
            Command::ClearVeils => engine.clear_veils(),
            Command::SetVeilVisible(i, v) => engine.set_veil_visible(i, v),
            Command::MoveVeil(from, to) => engine.move_veil(from, to),

            Command::SetOverlay(prims) => engine.set_overlay_primitives(prims),
            Command::ClearOverlay => engine.clear_overlay(),
            Command::SetOverlayMask(w, h, data) => engine.set_overlay_mask(w, h, &data),
            Command::ClearOverlayMask => engine.clear_overlay_mask(),

            Command::SetBrushBlendMode(m) => engine.set_brush_blend_mode(m),
            Command::ResetBrushGraph => engine.reset_brush_graph(),

            Command::PickColor(x, y) => {
                engine.pick_color(x, y);
            }

            Command::SetDocumentName(name) => engine.set_document_name(name),
        }
    }
}

// ---------------------------------------------------------------------------
// DarklySession — shared GPU device for multiple DarklyHandles
// ---------------------------------------------------------------------------

/// A process-level GPU session that owns one `wgpu::Instance` and one
/// `Arc<GpuDevice>`. Hand out `DarklyHandle`s via `createHandle(...)` to
/// attach additional canvases to the same WebGPU device — the multi-tab
/// editor uses one session and N handles, one per open document.
///
/// The device is allocated lazily on the first `createHandle` call, since
/// `request_adapter` needs a surface to pick an adapter.
#[wasm_bindgen]
pub struct DarklySession {
    instance: wgpu::Instance,
    /// `None` until the first canvas is attached; `Some` thereafter.
    /// Single-threaded interior mutability is safe — JS calls are serial.
    gpu: RefCell<Option<Arc<GpuDevice>>>,
    /// Shared tool session — generic bag of per-tool state (currently
    /// just `BrushState`, but the container has no module-specific
    /// knowledge). Every `DarklyHandle` minted from this session is
    /// constructed with a clone of the handle, so all engines see the
    /// same tool state. JS-driven mutations write through here once and
    /// every engine sees the change with no per-engine push step.
    tool_session: darkly::tool::SharedToolSession,
}

#[wasm_bindgen]
impl DarklySession {
    /// Create a new session. Cheap — only allocates a `wgpu::Instance`
    /// and an empty shared tool session seeded with a default
    /// `BrushState`. The actual GPU device is acquired on the first
    /// `createHandle` call.
    #[wasm_bindgen(constructor)]
    pub fn new() -> DarklySession {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::BROWSER_WEBGPU,
            ..Default::default()
        });
        let tool_session = darkly::tool::SharedToolSession::new();
        tool_session
            .write()
            .insert(darkly::brush::state::BrushState::new());
        DarklySession {
            instance,
            gpu: RefCell::new(None),
            tool_session,
        }
    }

    /// Build a new `DarklyHandle` bound to `canvas`, sharing this session's
    /// GPU device with every other handle from this session. The first call
    /// allocates the device; subsequent calls reuse it.
    #[wasm_bindgen(js_name = createHandle)]
    pub async fn create_handle(
        &self,
        canvas: web_sys::HtmlCanvasElement,
        doc_width: u32,
        doc_height: u32,
    ) -> DarklyHandle {
        let initial_width = canvas.width();
        let initial_height = canvas.height();

        let surface = self
            .instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
            .expect("Failed to create surface");

        // Lazy device init: first handle bootstraps the adapter+device using
        // its surface; subsequent handles reuse it.
        let existing = self.gpu.borrow().clone();
        let gpu = match existing {
            Some(shared) => {
                GpuContext::new_with_shared_device(
                    shared,
                    &self.instance,
                    surface,
                    initial_width,
                    initial_height,
                )
                .await
            }
            None => {
                let ctx = GpuContext::new(
                    // wgpu::Instance is `Clone`-able cheaply (it's a handle).
                    self.instance.clone(),
                    surface,
                    wgpu::Limits::downlevel_webgl2_defaults(),
                    initial_width,
                    initial_height,
                )
                .await;
                *self.gpu.borrow_mut() = Some(ctx.shared_device());
                ctx
            }
        };

        DarklyHandle {
            engine: RefCell::new(DarklyEngine::new_with_tool_session(
                gpu,
                self.tool_session.clone(),
                doc_width,
                doc_height,
            )),
            commands: RefCell::new(Vec::new()),
        }
    }
}

impl Default for DarklySession {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// DarklyHandle
// ---------------------------------------------------------------------------

#[wasm_bindgen]
pub struct DarklyHandle {
    engine: RefCell<DarklyEngine>,
    /// Unified command queue — all fire-and-forget mutations are queued here.
    /// Drained by [`render`] or [`flush_if_needed`].
    commands: RefCell<Vec<Command>>,
}

impl DarklyHandle {
    /// Drain the command queue if non-empty, ensuring queries and direct
    /// mutations see up-to-date state.
    fn flush_if_needed(&self) {
        if !self.commands.borrow().is_empty() {
            let mut engine = self.engine.borrow_mut();
            drain_commands(&self.commands, &mut engine);
        }
    }

    /// Push a command to the queue.
    fn push(&self, cmd: Command) {
        self.commands.borrow_mut().push(cmd);
    }
}

fn parse_selection_mode(mode: &str) -> SelectionMode {
    match mode {
        "add" => SelectionMode::Add,
        "subtract" => SelectionMode::Subtract,
        "intersect" => SelectionMode::Intersect,
        _ => SelectionMode::Replace,
    }
}

/// Convert a JS params object to a `Vec<ParamValue>` using `ParamDef` metadata.
fn js_to_param_values(js: &JsValue, defs: &[ParamDef]) -> Vec<ParamValue> {
    defs.iter()
        .map(|def| match def {
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
            ParamDef::String { name, default } => {
                let v = js_sys::Reflect::get(js, &(*name).into())
                    .ok()
                    .and_then(|v| v.as_string())
                    .unwrap_or_else(|| default.to_string());
                ParamValue::String(v)
            }
            ParamDef::Curve { name, default } => {
                let v = js_sys::Reflect::get(js, &(*name).into())
                    .ok()
                    .and_then(|v| v.as_string())
                    .and_then(|s| serde_json::from_str::<Vec<[f32; 2]>>(&s).ok())
                    .unwrap_or_else(|| default.to_vec());
                ParamValue::Curve(v)
            }
            ParamDef::Enum { name, default, .. } => {
                let v = js_sys::Reflect::get(js, &(*name).into())
                    .ok()
                    .and_then(|v| v.as_f64())
                    .unwrap_or(*default as f64) as i32;
                ParamValue::Int(v)
            }
            ParamDef::Icon { name, default, .. } => {
                let v = js_sys::Reflect::get(js, &(*name).into())
                    .ok()
                    .and_then(|v| v.as_string())
                    .unwrap_or_else(|| default.to_string());
                ParamValue::String(v)
            }
            ParamDef::FloatInput { name, default, .. } => {
                let v = js_sys::Reflect::get(js, &(*name).into())
                    .ok()
                    .and_then(|v| v.as_f64())
                    .unwrap_or(*default as f64) as f32;
                ParamValue::Float(v)
            }
        })
        .collect()
}

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

// ---------------------------------------------------------------------------
// #[wasm_bindgen] methods
// ---------------------------------------------------------------------------

#[wasm_bindgen]
impl DarklyHandle {
    /// Create a stand-alone Darkly editor instance from an HTML canvas
    /// element. Allocates a fresh `wgpu::Instance` + device — use
    /// `DarklySession.createHandle(canvas, w, h)` instead when you want
    /// multiple handles to share one GPU device (the multi-tab case).
    pub async fn create(
        canvas: web_sys::HtmlCanvasElement,
        doc_width: u32,
        doc_height: u32,
    ) -> DarklyHandle {
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
            instance,
            surface,
            wgpu::Limits::downlevel_webgl2_defaults(),
            initial_width,
            initial_height,
        )
        .await;

        DarklyHandle {
            engine: RefCell::new(DarklyEngine::new(gpu, doc_width, doc_height)),
            commands: RefCell::new(Vec::new()),
        }
    }

    // =======================================================================
    // Queued mutations — fire-and-forget, zero re-entrancy risk
    // =======================================================================

    // --- Layer properties ---

    pub fn set_opacity(&self, layer_id: f64, opacity: f32) {
        self.push(Command::SetOpacity(layer_id as u64, opacity));
    }
    pub fn set_blend_mode(&self, layer_id: f64, type_id: &str) {
        self.push(Command::SetBlendMode(layer_id as u64, type_id.into()));
    }
    pub fn set_layer_visible(&self, layer_id: f64, visible: bool) {
        self.push(Command::SetLayerVisible(layer_id as u64, visible));
    }
    pub fn set_layer_name(&self, layer_id: f64, name: &str) {
        self.push(Command::SetLayerName(layer_id as u64, name.into()));
    }
    pub fn set_group_collapsed(&self, group_id: f64, collapsed: bool) {
        self.push(Command::SetGroupCollapsed(group_id as u64, collapsed));
    }
    pub fn set_group_passthrough(&self, group_id: f64, passthrough: bool) {
        self.push(Command::SetGroupPassthrough(group_id as u64, passthrough));
    }

    // --- Modifier (mask) operations ---

    pub fn add_mask(&self, host_id: f64) {
        self.push(Command::AddMask(host_id as u64));
    }
    pub fn remove_mask(&self, host_id: f64) {
        self.push(Command::RemoveMask(host_id as u64));
    }
    pub fn apply_mask(&self, host_id: f64) {
        self.push(Command::ApplyMask(host_id as u64));
    }
    pub fn selection_to_mask(&self, host_id: f64) {
        self.push(Command::SelectionToMask(host_id as u64));
    }
    pub fn mask_to_selection(&self, modifier_id: f64) {
        self.push(Command::MaskToSelection(modifier_id as u64));
    }
    pub fn set_node_locked(&self, node_id: f64, locked: bool) {
        self.push(Command::SetNodeLocked(node_id as u64, locked));
    }
    /// Set the session-isolated node. Pass `0` to clear.
    pub fn set_isolated_node(&self, node_id: f64) {
        self.push(Command::SetIsolatedNode(node_id as u64));
    }

    // --- Painting ---

    pub fn fill_background(&self, layer_id: f64) {
        self.push(Command::FillBackground(layer_id as u64));
    }

    /// Fill `layer_id` with a solid RGBA color. Used by the "New Document"
    /// flow to seed a fresh raster layer with the user's chosen color.
    pub fn fill_background_color(&self, layer_id: f64, color: &[u8]) {
        if color.len() < 4 {
            log::error!("fill_background_color: color must be 4 bytes (RGBA)");
            return;
        }
        let c = [color[0], color[1], color[2], color[3]];
        self.push(Command::FillBackgroundColor(layer_id as u64, c));
    }

    // --- Stroke lifecycle ---

    pub fn begin_stroke(&self, layer_id: f64) {
        self.push(Command::BeginStroke(layer_id as u64));
    }

    pub fn stroke_to(&self, op_type: &str, params: JsValue) {
        js_sys::Reflect::set(&params, &"op".into(), &op_type.into()).ok();
        match serde_wasm_bindgen::from_value::<StrokeOp>(params) {
            Ok(op) => self.push(Command::StrokeOp(op)),
            Err(e) => log::error!("stroke_to deserialization failed: {e}"),
        }
    }

    pub fn end_stroke(&self) {
        self.push(Command::EndStroke);
    }

    // --- Selection ---

    pub fn select_rect(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        mode: &str,
        antialias: bool,
        feather: f32,
    ) {
        self.push(Command::SelectRect {
            x,
            y,
            w,
            h,
            mode: parse_selection_mode(mode),
            antialias,
            feather,
        });
    }

    pub fn select_ellipse(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        mode: &str,
        antialias: bool,
        feather: f32,
    ) {
        self.push(Command::SelectEllipse {
            x,
            y,
            w,
            h,
            mode: parse_selection_mode(mode),
            antialias,
            feather,
        });
    }

    pub fn select_lasso(&self, vertices: JsValue, mode: &str, antialias: bool, feather: f32) {
        let verts: Vec<[f32; 2]> = serde_wasm_bindgen::from_value(vertices).unwrap_or_default();
        self.push(Command::SelectLasso {
            verts,
            mode: parse_selection_mode(mode),
            antialias,
            feather,
        });
    }

    pub fn select_magic_wand(
        &self,
        layer_id: u64,
        seed_x: i32,
        seed_y: i32,
        tolerance: u8,
        mode: &str,
    ) {
        self.push(Command::SelectMagicWand {
            layer_id,
            seed_x,
            seed_y,
            tolerance,
            mode: parse_selection_mode(mode),
        });
    }

    pub fn clear_selection(&self) {
        self.push(Command::ClearSelection);
    }
    pub fn clear_selection_contents(&self, layer_id: f64) {
        self.push(Command::ClearSelectionContents(layer_id as u64));
    }
    pub fn select_all(&self) {
        self.push(Command::SelectAll);
    }
    pub fn invert_selection(&self) {
        self.push(Command::InvertSelection);
    }

    // --- Undo / Redo ---

    pub fn undo(&self) {
        self.push(Command::Undo);
    }
    pub fn redo(&self) {
        self.push(Command::Redo);
    }

    // --- View transform ---

    pub fn set_view_transform(
        &self,
        pan_x: f32,
        pan_y: f32,
        zoom: f32,
        rotation: f32,
        mirror_h: bool,
        screen_w: f32,
        screen_h: f32,
    ) {
        self.push(Command::SetViewTransform {
            pan_x,
            pan_y,
            zoom,
            rotation,
            mirror_h,
            screen_w,
            screen_h,
        });
    }

    pub fn resize(&self, width: u32, height: u32) {
        self.push(Command::Resize(width, height));
    }

    // --- Floating content ---

    pub fn update_floating_matrix(&self, matrix: &[f32]) {
        if matrix.len() >= 6 {
            self.push(Command::UpdateFloatingMatrix([
                matrix[0], matrix[1], matrix[2], matrix[3], matrix[4], matrix[5],
            ]));
        }
    }

    pub fn commit_floating(&self) {
        self.push(Command::CommitFloating);
    }
    pub fn cancel_floating(&self) {
        self.push(Command::CancelFloating);
    }

    // --- Veils ---

    pub fn remove_veil(&self, index: u32) {
        self.push(Command::RemoveVeil(index as usize));
    }
    pub fn clear_veils(&self) {
        self.push(Command::ClearVeils);
    }
    pub fn set_veil_visible(&self, index: u32, visible: bool) {
        self.push(Command::SetVeilVisible(index as usize, visible));
    }
    pub fn move_veil(&self, from: u32, to: u32) {
        self.push(Command::MoveVeil(from as usize, to as usize));
    }

    // --- Overlay ---

    pub fn set_overlay(&self, primitives: JsValue) {
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
        self.push(Command::SetOverlay(prims));
    }

    pub fn clear_overlay(&self) {
        self.push(Command::ClearOverlay);
    }

    /// Upload an RGBA8 mask texture sampled by KIND_MASKED_STAMP overlay
    /// primitives. The red channel is used as grayscale coverage.
    pub fn set_overlay_mask(&self, width: u32, height: u32, data: &[u8]) {
        self.push(Command::SetOverlayMask(width, height, data.to_vec()));
    }

    pub fn clear_overlay_mask(&self) {
        self.push(Command::ClearOverlayMask);
    }

    /// Canvas-space positioning for the active brush's hover preview, or
    /// null when the current graph has no preview wire. Returned as
    /// `{ halfExtent: [f32, f32], rotation: f32 }`. The overlay mask itself
    /// is already bound internally — the tool only needs this to place the
    /// primitive.
    pub fn get_brush_preview_info(&self) -> JsValue {
        // Drain any pending commands so param changes have already triggered
        // a preview regen before we read the cache. Otherwise the tool can
        // read a stale size the first hover after a slider drag.
        self.flush_if_needed();
        brush_preview_info_as_js(&self.engine.borrow())
    }

    /// Re-render the brush preview with live pen data, then return the
    /// updated positioning info. Called from the brush tool's hover path
    /// so the preview reflects the pen's current tilt / rotation /
    /// pressure — matches what the stamp will actually look like if the
    /// pen presses at this pose.
    ///
    /// Values come straight from the PointerEvent; hardware that doesn't
    /// report a sensor passes 0 (which is also the neutral state, so no
    /// conditional-default logic is needed). Pressure = 0 is remapped to
    /// 0.5 inside the engine because the hover event reports 0 pressure
    /// (no contact) but the preview should reflect what happens "if the
    /// pen presses now."
    pub fn refresh_brush_preview(
        &self,
        x: f32,
        y: f32,
        pressure: f32,
        tilt_x: f32,
        tilt_y: f32,
        rotation: f32,
        tangential_pressure: f32,
    ) -> JsValue {
        self.flush_if_needed();

        let mut pen = PaintInformation::preview_dummy();
        pen.pos = [x, y];
        // Hover reports pressure=0 (no contact) — keep the dummy's 0.5 as
        // a "what-if-pressed-now" fallback. If the pen is actively pressed
        // during hover (some hardware does this near the surface), use it.
        if pressure > 0.0 {
            pen.pressure = pressure;
        }
        pen.x_tilt = tilt_x;
        pen.y_tilt = tilt_y;
        pen.rotation = rotation;
        pen.tangential_pressure = tangential_pressure;

        self.engine
            .borrow_mut()
            .regenerate_brush_preview_with_pen(pen);
        brush_preview_info_as_js(&self.engine.borrow())
    }

    /// Drop any remembered hover pose so the next `refresh_brush_preview`
    /// starts fresh with no derived direction/motion/distance/speed.
    /// Call on pointer-leave and at stroke start.
    pub fn clear_brush_preview_pose(&self) {
        self.engine.borrow_mut().clear_brush_preview_pose();
    }

    /// Full-stroke brush editor preview — renders an S-curve sample stroke
    /// at the canonical `BRUSH_THUMBNAIL_SIZE` and returns PNG bytes. First
    /// call returns an empty Vec while the async readback is in flight;
    /// subsequent calls return the cached PNG, refreshed on a later frame
    /// once the readback completes. Frontend scales via CSS.
    ///
    /// Uses the theme colors stored via `set_preview_theme` — the editor
    /// preview visually matches the brush picker's thumbnails so users can
    /// scan across both without chromatic surprises. Same shape as
    /// `brush_active_dab_preview`: no promises, no async JS boundary plumbing.
    pub fn brush_editor_preview(&self) -> Vec<u8> {
        self.flush_if_needed();
        self.engine.borrow_mut().brush_editor_preview()
    }

    /// Render a single-dab preview of the active brush — the small
    /// tip-shape thumbnail used by the brush options bar trigger and the
    /// picker's active-brush strip. Returns the most recent cached PNG bytes
    /// synchronously; the async readback updates them on a later frame.
    /// Output is byte-identical to `brush_dab_thumbnail(active_name)`
    /// when the active brush matches a preset, so the frontend can
    /// scale the PNG to any display size via CSS.
    pub fn brush_active_dab_preview(&self) -> Vec<u8> {
        self.flush_if_needed();
        self.engine.borrow_mut().brush_active_dab_preview()
    }

    /// Render a thumbnail of a single GPU node's `texture` output and return
    /// the most recent cached PNG bytes synchronously. Same shape as
    /// `brush_active_dab_preview`, but per-node — used by the brush
    /// builder's in-node preview thumbnail. Returns empty bytes if the
    /// node doesn't exist or has no Texture output port; the frontend
    /// treats empty as "preserve the last good thumbnail / show nothing".
    pub fn brush_node_preview(&self, node_id: u32) -> Vec<u8> {
        self.flush_if_needed();
        self.engine.borrow_mut().brush_node_preview(node_id as u64)
    }

    /// Return the cached PNG thumbnail bytes for a library brush, kicking
    /// off a bake on first call. Returns an empty `Uint8Array` while the
    /// bake is in flight (or for unknown brush names); callers poll on
    /// rAF until non-empty bytes arrive.
    pub fn brush_thumbnail(&self, name: &str) -> Vec<u8> {
        self.flush_if_needed();
        self.engine.borrow_mut().brush_thumbnail(name)
    }

    /// Same shape as `brush_thumbnail`, but bakes a single full-pressure
    /// dab instead of an S-curve. Used by the picker tiles to display
    /// the tip silhouette next to the stroke preview.
    pub fn brush_dab_thumbnail(&self, name: &str) -> Vec<u8> {
        self.flush_if_needed();
        self.engine.borrow_mut().brush_dab_thumbnail(name)
    }

    /// Push the current UI theme colors into the engine. Used by both the
    /// live editor preview and brush thumbnail baking — call on theme
    /// change so the live preview re-renders with the new palette.
    pub fn set_preview_theme(&self, fg: &[f32], bg: &[f32]) {
        self.flush_if_needed();
        let fg = if fg.len() >= 4 {
            [fg[0], fg[1], fg[2], fg[3]]
        } else {
            [1.0, 1.0, 1.0, 1.0]
        };
        let bg = if bg.len() >= 4 {
            [bg[0], bg[1], bg[2], bg[3]]
        } else {
            [0.08, 0.08, 0.08, 1.0]
        };
        self.engine.borrow_mut().set_preview_theme(fg, bg);
    }

    /// Push the workspace background color (the area shown around the
    /// canvas in the viewport — the `--canvas-bg` CSS token). Call on
    /// theme change so the present shader uses the new color.
    pub fn set_viewport_bg(&self, bg: &[f32]) {
        self.flush_if_needed();
        let bg = if bg.len() >= 4 {
            [bg[0], bg[1], bg[2], bg[3]]
        } else {
            [0.11, 0.11, 0.11, 1.0]
        };
        self.engine.borrow_mut().set_viewport_bg(bg);
    }

    // --- Brush config ---

    pub fn set_brush_blend_mode(&self, mode: u32) {
        self.push(Command::SetBrushBlendMode(mode));
    }
    pub fn brush_graph_reset(&self) {
        self.push(Command::ResetBrushGraph);
    }

    // --- Color pick ---

    /// Start an async color pick. Returns the last picked color immediately
    /// for responsive UI — the real result arrives on the next frame.
    pub fn pick_color(&self, x: f32, y: f32) -> Vec<u8> {
        self.push(Command::PickColor(x, y));
        // Return cached value without flushing — pick_color is pointer-frequency
        // and the cached value provides immediate feedback.
        self.engine.borrow().last_picked_color().to_vec()
    }

    // =======================================================================
    // Direct mutations — return values, panic on re-entrancy
    // =======================================================================

    // --- Layer CRUD ---
    // IDs use f64 because JS has no u64 — any JS-facing backend needs this.

    pub fn add_raster_layer(&self, anchor_id: f64) -> f64 {
        self.flush_if_needed();
        let anchor = (anchor_id >= 0.0).then(|| LayerId::from_ffi(anchor_id as u64));
        self.engine.borrow_mut().add_raster_layer(anchor).to_ffi() as f64
    }

    pub fn add_group(&self, anchor_id: f64) -> f64 {
        self.flush_if_needed();
        let anchor = (anchor_id >= 0.0).then(|| LayerId::from_ffi(anchor_id as u64));
        self.engine.borrow_mut().add_group(anchor).to_ffi() as f64
    }

    /// Add a void layer. Returns the new layer id, or -1 if `void_type` is
    /// not a registered void kind. `params` is a JS object of
    /// `{ name: value, ... }` matching the void type's `ParamDef` schema —
    /// same marshalling pattern as `add_veil`.
    pub fn add_void_layer(&self, void_type: &str, params: JsValue, anchor_id: f64) -> f64 {
        self.flush_if_needed();
        let anchor = (anchor_id >= 0.0).then(|| LayerId::from_ffi(anchor_id as u64));
        let mut e = self.engine.borrow_mut();
        let defs = e.void_param_defs(void_type);
        let pv = js_to_param_values(&params, defs);
        match e.add_void_layer(void_type, pv, anchor) {
            Some(id) => id.to_ffi() as f64,
            None => -1.0,
        }
    }

    /// Replace the parameter values on a void layer. `params` is a JS object
    /// of `{ name: value, ... }` matching the layer's current `voidType`'s
    /// schema. Coalesces with prior `VoidParams` undo entries so a slider
    /// drag is one step.
    pub fn update_void_params(&self, layer_id: f64, params: JsValue) {
        self.flush_if_needed();
        let id = LayerId::from_ffi(layer_id as u64);
        let mut e = self.engine.borrow_mut();
        let type_id = match e.void_layer_type(id) {
            Some(t) => t,
            None => return,
        };
        let defs = e.void_param_defs(&type_id);
        let pv = js_to_param_values(&params, defs);
        e.update_void_params(id, pv);
    }

    pub fn remove_layer(&self, layer_id: f64) -> Result<(), JsError> {
        self.flush_if_needed();
        self.engine
            .borrow_mut()
            .remove_layer(LayerId::from_ffi(layer_id as u64))
            .map_err(|e| JsError::new(&e))
    }

    pub fn move_layer(&self, layer_id: f64, target_type: &str, target_id: f64) {
        self.flush_if_needed();
        let target_id = LayerId::from_ffi(target_id as u64);
        let target = match target_type {
            "before" => MoveTarget::Before(target_id),
            "after" => MoveTarget::After(target_id),
            "into_top" => MoveTarget::IntoGroupTop(target_id),
            "into_bottom" => MoveTarget::IntoGroupBottom(target_id),
            _ => return,
        };
        self.engine
            .borrow_mut()
            .move_layer(LayerId::from_ffi(layer_id as u64), target)
    }

    /// Deep-copy a layer or group, placing the duplicate directly above the
    /// source. Returns the new node's id, or `0` (a null `LayerId`) if the
    /// source id is unknown.
    pub fn duplicate_node(&self, source_id: f64) -> f64 {
        self.flush_if_needed();
        let id = LayerId::from_ffi(source_id as u64);
        self.engine
            .borrow_mut()
            .duplicate_node(id)
            .map(|n| n.to_ffi() as f64)
            .unwrap_or(0.0)
    }

    /// Merge the active layer / group into the sibling directly below it,
    /// producing a single raster at the lower sibling's position. Returns
    /// the merged result's id.
    pub fn merge_down(&self, source_id: f64) -> Result<f64, JsError> {
        self.flush_if_needed();
        let id = LayerId::from_ffi(source_id as u64);
        self.engine
            .borrow_mut()
            .merge_down(id)
            .map(|n| n.to_ffi() as f64)
            .map_err(|e| JsError::new(&e))
    }

    /// Composite every visible top-level node into a single "Background"
    /// raster at root; everything else is discarded. Returns the result id.
    pub fn flatten_image(&self) -> Result<f64, JsError> {
        self.flush_if_needed();
        self.engine
            .borrow_mut()
            .flatten_image()
            .map(|n| n.to_ffi() as f64)
            .map_err(|e| JsError::new(&e))
    }

    /// Flatten a single node — for a layer, applies its mask; for a group,
    /// bakes the group's children + mask into a single raster that takes
    /// the group's slot and inherits its blend props. Returns the resulting
    /// raster's id (same as the input for layer-with-mask; a fresh id for
    /// groups).
    pub fn flatten_node(&self, node_id: f64) -> Result<f64, JsError> {
        self.flush_if_needed();
        let id = LayerId::from_ffi(node_id as u64);
        self.engine
            .borrow_mut()
            .flatten_node(id)
            .map(|n| n.to_ffi() as f64)
            .map_err(|e| JsError::new(&e))
    }

    /// True when `node_id` has something to flatten — a layer with a mask,
    /// or any group. Used by the frontend to enable/disable the entry.
    pub fn can_flatten_node(&self, node_id: f64) -> bool {
        self.flush_if_needed();
        let id = LayerId::from_ffi(node_id as u64);
        self.engine.borrow().can_flatten_node(id)
    }

    /// True when `source_id` has a same-parent sibling below it — used by
    /// the frontend to enable/disable Merge Down.
    pub fn can_merge_down(&self, source_id: f64) -> bool {
        self.flush_if_needed();
        let id = LayerId::from_ffi(source_id as u64);
        self.engine.borrow().can_merge_down(id)
    }

    /// True when the document has at least one layer — used by the
    /// frontend to enable/disable Flatten Image.
    pub fn can_flatten(&self) -> bool {
        self.flush_if_needed();
        self.engine.borrow().can_flatten()
    }

    // --- Copy / Cut / Paste ---

    pub fn copy(&self, layer_id: f64) -> JsValue {
        self.flush_if_needed();
        match self
            .engine
            .borrow_mut()
            .copy(LayerId::from_ffi(layer_id as u64))
        {
            Some(export) => serde_wasm_bindgen::to_value(&export).unwrap_or(JsValue::NULL),
            None => JsValue::NULL,
        }
    }

    pub fn cut(&self, layer_id: f64) -> JsValue {
        self.flush_if_needed();
        match self
            .engine
            .borrow_mut()
            .cut(LayerId::from_ffi(layer_id as u64))
        {
            Some(export) => serde_wasm_bindgen::to_value(&export).unwrap_or(JsValue::NULL),
            None => JsValue::NULL,
        }
    }

    pub fn poll_copy_result(&self) -> JsValue {
        self.flush_if_needed();
        match self.engine.borrow_mut().poll_copy_result() {
            Some(export) => serde_wasm_bindgen::to_value(&export).unwrap_or(JsValue::NULL),
            None => JsValue::NULL,
        }
    }

    // --- Image export (PNG/JPEG/WebP) ---

    /// Kick off an async readback of the composited canvas. The result lands
    /// on `pending_export_result` and is drained by `poll_export_result()`
    /// on a subsequent frame. JS handles the encoding (PNG/JPEG/WebP) via
    /// `OffscreenCanvas` so the browser's native encoder runs off the WASM
    /// main thread.
    pub fn start_export(&self) {
        self.flush_if_needed();
        self.engine.borrow_mut().start_export();
    }

    /// Drain the most recent export result. Returns
    /// `{ width, height, rgba: Uint8Array }` on completion or `null` while
    /// the readback is still in flight.
    pub fn poll_export_result(&self) -> JsValue {
        self.flush_if_needed();
        let Some(result) = self.engine.borrow_mut().poll_export_result() else {
            return JsValue::NULL;
        };
        let obj = js_sys::Object::new();
        js_sys::Reflect::set(
            &obj,
            &"width".into(),
            &JsValue::from_f64(result.width as f64),
        )
        .ok();
        js_sys::Reflect::set(
            &obj,
            &"height".into(),
            &JsValue::from_f64(result.height as f64),
        )
        .ok();
        let rgba = js_sys::Uint8Array::new_with_length(result.rgba.len() as u32);
        rgba.copy_from(&result.rgba);
        js_sys::Reflect::set(&obj, &"rgba".into(), &rgba.into()).ok();
        obj.into()
    }

    // --- Document name ---

    /// User-visible document name. Backs the tab title, shown in the
    /// Save As picker as `suggestedName`, and serialized as
    /// `manifest.name` inside the `.darkly` container.
    pub fn document_name(&self) -> String {
        self.flush_if_needed();
        self.engine.borrow().document_name().to_string()
    }

    /// Current document canvas dimensions as `[width, height]` in pixels.
    /// The JS side caches this on `DarklyInstance` so canvas↔screen
    /// transforms recenter around the actual doc — calling this every
    /// frame would alias the RefCell borrow that `render()` holds
    /// (see `coordinates.ts` for the re-entrancy rationale).
    pub fn canvas_dimensions(&self) -> Box<[u32]> {
        self.flush_if_needed();
        let (w, h) = self.engine.borrow().canvas_dimensions();
        vec![w, h].into_boxed_slice()
    }

    /// Rename the document. Not undoable — matches every other editor's
    /// title-bar rename affordance. Subsequent saves write the new name
    /// into `manifest.name`.
    pub fn set_document_name(&self, name: &str) {
        self.push(Command::SetDocumentName(name.to_string()));
    }

    /// True when the document has unsaved changes — set sticky by any
    /// undoable mutation, cleared only by a successful save or load.
    /// Consulted by the close-tab modal and the `beforeunload` handler.
    pub fn is_dirty(&self) -> bool {
        self.flush_if_needed();
        self.engine.borrow().is_dirty()
    }

    // --- Native save / open (.darkly container) ---

    /// Kick off a `.darkly` save. Builds the manifest synchronously,
    /// pins every source texture, queues all readbacks. Returns
    /// immediately; the result lands on `poll_save_result()` once every
    /// pixel readback completes.
    ///
    /// Returns a string error when a save is already in flight on this
    /// engine — the UI disables the Save action for the tab while a
    /// save is active so this is an exceptional path.
    pub fn start_save_document(&self) -> Result<(), JsError> {
        self.flush_if_needed();
        self.engine
            .borrow_mut()
            .start_save_document()
            .map_err(|e| JsError::new(&e.to_string()))
    }

    /// Drain the most recent save result. Returns a
    /// `{ manifestJson: Uint8Array, compositeWidth, compositeHeight,
    ///    compositeRgba: Uint8Array, blobs: [{ path, bytes: Uint8Array }] }`
    /// object on completion or `null` while readbacks are in flight.
    ///
    /// JS-side `saveDocument.ts` (Phase 5) PNG-encodes the composite,
    /// generates the thumbnail, assembles the zip via `fflate`, and
    /// writes through the active tab's file handle.
    pub fn poll_save_result(&self) -> JsValue {
        self.flush_if_needed();
        let Some(bundle) = self.engine.borrow_mut().poll_save_result() else {
            return JsValue::NULL;
        };
        let obj = js_sys::Object::new();
        let manifest = js_sys::Uint8Array::new_with_length(bundle.manifest_json.len() as u32);
        manifest.copy_from(&bundle.manifest_json);
        js_sys::Reflect::set(&obj, &"manifestJson".into(), &manifest.into()).ok();
        js_sys::Reflect::set(
            &obj,
            &"compositeWidth".into(),
            &JsValue::from_f64(bundle.composite_width as f64),
        )
        .ok();
        js_sys::Reflect::set(
            &obj,
            &"compositeHeight".into(),
            &JsValue::from_f64(bundle.composite_height as f64),
        )
        .ok();
        let composite = js_sys::Uint8Array::new_with_length(bundle.composite_rgba.len() as u32);
        composite.copy_from(&bundle.composite_rgba);
        js_sys::Reflect::set(&obj, &"compositeRgba".into(), &composite.into()).ok();
        let blobs = js_sys::Array::new();
        for blob in &bundle.blobs {
            let entry = js_sys::Object::new();
            js_sys::Reflect::set(&entry, &"path".into(), &JsValue::from_str(&blob.path)).ok();
            let bytes = js_sys::Uint8Array::new_with_length(blob.bytes.len() as u32);
            bytes.copy_from(&blob.bytes);
            js_sys::Reflect::set(&entry, &"bytes".into(), &bytes.into()).ok();
            blobs.push(&entry);
        }
        js_sys::Reflect::set(&obj, &"blobs".into(), &blobs.into()).ok();
        obj.into()
    }

    /// Load a `.darkly` zip into this engine. All-or-nothing: any
    /// refusal path leaves the engine byte-for-byte untouched. On
    /// failure the rejected error carries a structured JSON payload
    /// (`{ kind, ... }`) that the UI's `LoadErrorToast` consumes
    /// directly. JS-side use:
    ///
    /// ```js
    /// try { handle.open_document(bytes); }
    /// catch (e) {
    ///     const err = JSON.parse(e.message);
    ///     // err = { kind: "unsupportedFeatures", missing: [...], message }
    /// }
    /// ```
    pub fn open_document(&self, bytes: &[u8]) -> Result<(), JsError> {
        self.flush_if_needed();
        self.engine.borrow_mut().open_document(bytes).map_err(|e| {
            // The structured payload is what the UI toast switches on;
            // the throwable message is its JSON serialization so the JS
            // catch handler can `JSON.parse` it.
            JsError::new(&e.to_json().to_string())
        })
    }

    /// Like `copy`, but also captures CPU-side metadata (blend mode,
    /// opacity, name, mask presence). Pixel bytes still flow through the
    /// normal async readback; the JSON envelope is delivered via
    /// `poll_copy_rich_result`. Used by the multi-tab editor to populate
    /// the system clipboard's `web application/x-darkly-layer` MIME
    /// alongside the standard `image/png`.
    pub fn copy_layer_rich(&self, layer_id: f64) {
        self.flush_if_needed();
        self.engine
            .borrow_mut()
            .copy_layer_rich(LayerId::from_ffi(layer_id as u64));
    }

    /// Drain the most recent rich-copy result. Returns the JSON-serialised
    /// `LayerClipboard`, or `null` when no rich copy is pending. Mirrors
    /// `poll_copy_result`'s polling contract.
    pub fn poll_copy_rich_result(&self) -> Option<String> {
        self.flush_if_needed();
        self.engine.borrow_mut().poll_copy_rich_result()
    }

    /// Paste a rich layer payload (JSON envelope) as a new layer. Restores
    /// blend mode, opacity, name, visibility, and pixel data. Returns the
    /// new layer's id, or `-1` on parse / decode failure.
    pub fn paste_layer_rich(&self, json: &str, active_layer_id: f64) -> f64 {
        self.flush_if_needed();
        let active = if active_layer_id >= 0.0 {
            Some(LayerId::from_ffi(active_layer_id as u64))
        } else {
            None
        };
        match self.engine.borrow_mut().paste_layer_rich(json, active) {
            Some(id) => id.to_ffi() as f64,
            None => -1.0,
        }
    }

    pub fn paste_image(
        &self,
        width: u32,
        height: u32,
        rgba: &[u8],
        offset_x: i32,
        offset_y: i32,
        active_layer_id: f64,
    ) -> f64 {
        self.flush_if_needed();
        let active = if active_layer_id >= 0.0 {
            Some(LayerId::from_ffi(active_layer_id as u64))
        } else {
            None
        };
        self.engine
            .borrow_mut()
            .paste_image(width, height, rgba, offset_x, offset_y, active)
            .to_ffi() as f64
    }

    pub fn paste_image_floating(
        &self,
        width: u32,
        height: u32,
        rgba: &[u8],
        offset_x: i32,
        offset_y: i32,
        active_layer_id: f64,
    ) -> f64 {
        self.flush_if_needed();
        let active = if active_layer_id >= 0.0 {
            Some(LayerId::from_ffi(active_layer_id as u64))
        } else {
            None
        };
        self.engine
            .borrow_mut()
            .paste_image_floating(width, height, rgba, offset_x, offset_y, active)
            .to_ffi() as f64
    }

    pub fn paste_in_place(&self, active_layer_id: f64) -> f64 {
        self.flush_if_needed();
        let active = if active_layer_id >= 0.0 {
            Some(LayerId::from_ffi(active_layer_id as u64))
        } else {
            None
        };
        match self.engine.borrow_mut().paste_in_place(active) {
            Some(id) => id.to_ffi() as f64,
            None => -1.0,
        }
    }

    // --- Floating content (direct) ---

    pub fn paste_in_place_floating(&self, layer_id: f64) -> bool {
        self.flush_if_needed();
        self.engine
            .borrow_mut()
            .paste_in_place_floating(LayerId::from_ffi(layer_id as u64))
    }

    pub fn begin_transform(&self, layer_id: f64) -> bool {
        self.flush_if_needed();
        self.engine
            .borrow_mut()
            .begin_transform(LayerId::from_ffi(layer_id as u64))
    }

    // --- Veils (direct — need engine access for param defs) ---

    pub fn add_veil(&self, veil_type: &str, params: JsValue) {
        self.flush_if_needed();
        let mut e = self.engine.borrow_mut();
        let pv = js_to_param_values(&params, e.veil_param_defs(veil_type));
        e.add_veil(veil_type, &pv)
    }

    pub fn update_veil(&self, index: u32, params: JsValue) {
        self.flush_if_needed();
        let mut e = self.engine.borrow_mut();
        let type_id = match e.veil_list().iter().find(|v| v.index == index as usize) {
            Some(v) => v.type_id.clone(),
            None => return,
        };
        let pv = js_to_param_values(&params, e.veil_param_defs(&type_id));
        e.update_veil(index as usize, &pv)
    }

    // --- Brush graph (direct — return results) ---

    pub fn brush_graph_compile(&self, json: &str) -> JsValue {
        self.flush_if_needed();
        match self.engine.borrow_mut().set_brush_graph(json) {
            Ok(()) => JsValue::NULL,
            Err(e) => JsValue::from_str(&e),
        }
    }

    pub fn brush_graph_add_node(&self, type_id: &str) -> JsValue {
        self.flush_if_needed();
        graph_result(self.engine.borrow_mut().brush_graph_add_node(type_id))
    }

    pub fn brush_graph_remove_node(&self, node_id: u32) -> JsValue {
        self.flush_if_needed();
        graph_result(
            self.engine
                .borrow_mut()
                .brush_graph_remove_node(node_id as u64),
        )
    }

    pub fn brush_graph_connect(
        &self,
        from_node: u32,
        from_port: &str,
        to_node: u32,
        to_port: &str,
    ) -> JsValue {
        self.flush_if_needed();
        graph_result(self.engine.borrow_mut().brush_graph_connect(
            from_node as u64,
            from_port,
            to_node as u64,
            to_port,
        ))
    }

    pub fn brush_graph_disconnect(
        &self,
        from_node: u32,
        from_port: &str,
        to_node: u32,
        to_port: &str,
    ) -> JsValue {
        self.flush_if_needed();
        graph_result(self.engine.borrow_mut().brush_graph_disconnect(
            from_node as u64,
            from_port,
            to_node as u64,
            to_port,
        ))
    }

    pub fn brush_graph_set_param(
        &self,
        node_id: u32,
        param_index: u32,
        kind: &str,
        value: JsValue,
    ) -> JsValue {
        let pv = match kind {
            "float" => ParamValue::Float(value.as_f64().unwrap_or(0.0) as f32),
            "int" => ParamValue::Int(value.as_f64().unwrap_or(0.0) as i32),
            "bool" => ParamValue::Bool(value.as_bool().unwrap_or(false)),
            "string" => ParamValue::String(value.as_string().unwrap_or_default()),
            "curve" => {
                let json_str = value.as_string().unwrap_or_default();
                let points: Vec<[f32; 2]> = serde_json::from_str(&json_str)
                    .unwrap_or_else(|_| vec![[0.0, 0.0], [1.0, 1.0]]);
                ParamValue::Curve(points)
            }
            _ => return graph_result(Err(format!("unknown param kind: {kind}"))),
        };
        self.flush_if_needed();
        graph_result(self.engine.borrow_mut().brush_graph_set_param(
            node_id as u64,
            param_index as usize,
            pv,
        ))
    }

    pub fn brush_graph_set_port_default(
        &self,
        node_id: u32,
        port_name: &str,
        value: f32,
    ) -> JsValue {
        self.flush_if_needed();
        graph_result(self.engine.borrow_mut().brush_graph_set_port_default(
            node_id as u64,
            port_name,
            value,
        ))
    }

    /// Run auto-layout. `sizes_json` is a JSON object mapping node ID
    /// strings to `[width, height]` arrays, measured from the DOM.
    /// Returns a JSON object mapping node ID strings to `[x, y]` —
    /// positions are a UI-only concern, not stored on the graph.
    pub fn brush_graph_auto_layout(&self, sizes_json: &str) -> String {
        self.flush_if_needed();
        let sizes: std::collections::HashMap<u64, [f32; 2]> =
            serde_json::from_str(sizes_json).unwrap_or_default();
        let sizes = sizes
            .into_iter()
            .map(|(id, wh)| (darkly::nodegraph::NodeId(id), wh))
            .collect();
        let layout = self.engine.borrow().brush_graph_auto_layout(&sizes);
        // Re-key by stringified id so JSON consumers see plain object keys.
        let json_layout: std::collections::HashMap<String, [f32; 2]> = layout
            .into_iter()
            .map(|(id, pos)| (id.0.to_string(), pos))
            .collect();
        serde_json::to_string(&json_layout).unwrap_or_else(|_| "{}".into())
    }

    pub fn brush_upload_image(
        &self,
        resource_name: &str,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> JsValue {
        self.flush_if_needed();
        match self
            .engine
            .borrow_mut()
            .brush_upload_image(resource_name, width, height, rgba)
        {
            Ok(()) => JsValue::NULL,
            Err(e) => JsValue::from_str(&e),
        }
    }

    // --- Brush library (direct) ---

    /// Load a brush. Returns `null` on success or an error string.
    /// The frontend always re-runs auto-layout after a load — positions
    /// are UI-only and not persisted with the brush.
    pub fn brush_load(&self, name: &str) -> JsValue {
        self.flush_if_needed();
        match self.engine.borrow_mut().brush_load(name) {
            Ok(()) => JsValue::NULL,
            Err(e) => JsValue::from_str(&e),
        }
    }

    pub fn brush_save(&self, name: &str, category: &str) -> JsValue {
        self.flush_if_needed();
        match self.engine.borrow_mut().brush_save(name, category) {
            Ok(()) => JsValue::NULL,
            Err(e) => JsValue::from_str(&e),
        }
    }

    pub fn brush_import(&self, bytes: &[u8]) -> JsValue {
        self.flush_if_needed();
        match self.engine.borrow_mut().brush_import(bytes) {
            Ok(name) => JsValue::from_str(&name),
            Err(e) => JsValue::from_str(&e),
        }
    }

    // --- Thumbnails ---

    /// Cached thumbnail bytes for any node id (raster layer or mask modifier).
    /// Format dispatch happens internally — callers just pass the node id.
    pub fn node_thumbnail(&self, node_id: f64, width: u32, height: u32) -> Vec<u8> {
        self.flush_if_needed();
        self.engine
            .borrow_mut()
            .node_thumbnail(LayerId::from_ffi(node_id as u64), width, height)
    }

    /// Monotonic counter bumped by the engine each time a thumbnail
    /// readback lands in its cache. The frontend mirrors this into a
    /// Svelte-reactive epoch so the layer panel's `$derived` re-runs
    /// after async cache updates (which would otherwise be invisible
    /// to Svelte).
    pub fn thumbnail_version(&self) -> u32 {
        self.engine.borrow().thumbnail_version()
    }

    /// Engine-side thumbnail dimension used by the auto-queue path. The
    /// frontend's `THUMB_SIZE` literal in `thumbnails.ts` must match —
    /// `app.svelte.ts` asserts equality at handle init so drift is
    /// caught loudly the first time a stale frontend talks to a fresh
    /// engine.
    pub fn engine_default_thumb_size(&self) -> u32 {
        darkly::engine::DEFAULT_THUMB_SIZE
    }

    // =======================================================================
    // Queries — immutable borrow, always safe
    // =======================================================================

    pub fn screen_to_canvas(&self, screen_x: f32, screen_y: f32) -> Vec<f32> {
        self.flush_if_needed();
        let (cx, cy) = self.engine.borrow().screen_to_canvas(screen_x, screen_y);
        vec![cx, cy]
    }

    pub fn last_picked_color(&self) -> Vec<u8> {
        self.flush_if_needed();
        self.engine.borrow().last_picked_color().to_vec()
    }

    pub fn has_pending_color_pick(&self) -> bool {
        self.flush_if_needed();
        self.engine.borrow().has_pending_color_pick()
    }

    pub fn has_selection(&self) -> bool {
        self.flush_if_needed();
        self.engine.borrow().has_selection()
    }

    pub fn has_floating(&self) -> bool {
        self.flush_if_needed();
        self.engine.borrow().has_floating()
    }

    pub fn floating_info(&self) -> Option<Box<[f32]>> {
        self.flush_if_needed();
        self.engine
            .borrow()
            .floating_info()
            .map(|(ox, oy, w, h, m)| {
                vec![ox, oy, w, h, m[0], m[1], m[2], m[3], m[4], m[5]].into_boxed_slice()
            })
    }

    pub fn floating_target_layer(&self) -> f64 {
        self.flush_if_needed();
        self.engine
            .borrow()
            .floating_target_layer()
            .map(|id| id.to_ffi() as f64)
            .unwrap_or(-1.0)
    }

    pub fn brush_node_types(&self) -> String {
        self.flush_if_needed();
        serde_json::to_string(&self.engine.borrow().brush_node_types())
            .unwrap_or_else(|_| "[]".into())
    }

    pub fn brush_graph_default(&self) -> String {
        self.flush_if_needed();
        serde_json::to_string(&self.engine.borrow().default_brush_graph())
            .unwrap_or_else(|_| "null".into())
    }

    pub fn brush_graph_active(&self) -> String {
        self.flush_if_needed();
        serde_json::to_string(&self.engine.borrow().active_brush_graph())
            .unwrap_or_else(|_| "null".into())
    }

    /// Topology version of the active brush graph. Bumps only on
    /// structural changes; exposed-port scrubs do not advance it. The
    /// frontend uses this to keep the active preset name across scrubs
    /// and clear it only when the graph actually changes shape.
    ///
    /// Returned as `f64` so JS receives a plain `number` (the engine
    /// counter is `u64`, but values up to 2^53 are exact and a wrapping
    /// counter cannot realistically reach that).
    pub fn brush_topology_version(&self) -> f64 {
        self.engine.borrow().brush_topology_version() as f64
    }

    /// Does the active brush's terminal honor erase mode? `false` for
    /// brushes whose output node declares `supports_erase = false` in
    /// its registration (smudge, liquify, watercolor). The UI uses this
    /// to hide the brush-tool erase toggle.
    pub fn brush_active_supports_erase(&self) -> bool {
        self.engine.borrow().active_brush_supports_erase()
    }

    pub fn brush_graph_validate(&self, json: &str) -> JsValue {
        self.flush_if_needed();
        match self.engine.borrow().validate_brush_graph(json) {
            Ok(()) => JsValue::NULL,
            Err(e) => JsValue::from_str(&e),
        }
    }

    pub fn brush_exposed_ports(&self) -> String {
        self.flush_if_needed();
        serde_json::to_string(&self.engine.borrow().brush_exposed_ports())
            .unwrap_or_else(|_| "[]".into())
    }

    pub fn brush_set_exposed_port(
        &self,
        node_id: u32,
        port_name: &str,
        display_value: f32,
    ) -> JsValue {
        self.flush_if_needed();
        graph_result(self.engine.borrow_mut().brush_set_exposed_port(
            node_id as u64,
            port_name,
            display_value,
        ))
    }

    pub fn brush_graph_set_port_exposed(
        &self,
        node_id: u32,
        port_name: &str,
        exposed: bool,
    ) -> JsValue {
        self.flush_if_needed();
        graph_result(self.engine.borrow_mut().brush_graph_set_port_exposed(
            node_id as u64,
            port_name,
            exposed,
        ))
    }

    pub fn brush_list(&self) -> String {
        self.flush_if_needed();
        serde_json::to_string(&self.engine.borrow().brush_list()).unwrap_or_else(|_| "[]".into())
    }

    pub fn brush_export(&self, name: &str) -> JsValue {
        self.flush_if_needed();
        match self.engine.borrow().brush_export(name) {
            Ok(bytes) => {
                let arr = js_sys::Uint8Array::new_with_length(bytes.len() as u32);
                arr.copy_from(&bytes);
                arr.into()
            }
            Err(e) => JsValue::from_str(&e),
        }
    }

    pub fn layer_tree(&self) -> String {
        self.flush_if_needed();
        serde_json::to_string(&self.engine.borrow().layer_tree()).unwrap_or_else(|_| "[]".into())
    }

    pub fn veil_list(&self) -> String {
        self.flush_if_needed();
        serde_json::to_string(&self.engine.borrow().veil_list()).unwrap_or_else(|_| "[]".into())
    }

    pub fn veil_types(&self) -> String {
        self.flush_if_needed();
        serde_json::to_string(&self.engine.borrow().veil_types()).unwrap_or_else(|_| "[]".into())
    }

    /// Return registered void types as JSON. Same shape as `veil_types()` —
    /// `[{ type, displayName, params }, ...]` — so the void picker can reuse
    /// the veil-picker modal logic.
    pub fn void_types(&self) -> String {
        self.flush_if_needed();
        serde_json::to_string(&self.engine.borrow().void_types()).unwrap_or_else(|_| "[]".into())
    }

    /// Return registered tool types as JSON: `[{ type, displayName, params }, ...]`.
    /// The UI uses `displayName` directly, so tool labels live in Rust now.
    pub fn tool_types(&self) -> String {
        self.flush_if_needed();
        serde_json::to_string(&self.engine.borrow().tool_types()).unwrap_or_else(|_| "[]".into())
    }

    /// Return registered blend modes as JSON: `[{ type, displayName, category }, ...]`.
    /// The layer-properties dropdown is populated entirely from this list.
    pub fn blend_mode_types(&self) -> String {
        self.flush_if_needed();
        serde_json::to_string(&self.engine.borrow().blend_mode_types())
            .unwrap_or_else(|_| "[]".into())
    }

    /// Return registered modifier kinds as JSON: `[{ type, displayName }, ...]`.
    /// UI resolves `ModifierInfo.kind` → label via this table.
    pub fn modifier_types(&self) -> String {
        self.flush_if_needed();
        serde_json::to_string(&self.engine.borrow().modifier_types())
            .unwrap_or_else(|_| "[]".into())
    }

    /// Return registered layer kinds as JSON: `[{ type, displayName }, ...]`.
    /// UI resolves a layer's `type` discriminator → label via this table.
    pub fn layer_kind_types(&self) -> String {
        self.flush_if_needed();
        serde_json::to_string(&self.engine.borrow().layer_kind_types())
            .unwrap_or_else(|_| "[]".into())
    }

    pub fn overlay_hit_test(&self, screen_x: f32, screen_y: f32) -> i32 {
        self.flush_if_needed();
        match self.engine.borrow().overlay_hit_test(screen_x, screen_y) {
            Some(i) => i as i32,
            None => -1,
        }
    }

    // =======================================================================
    // Rendering — the ONE method that uses try_borrow_mut
    // =======================================================================

    /// Render the current frame. Returns true if animations need another frame.
    ///
    /// Drains the command queue first, then renders.  All GPU work
    /// (dab generation, compositing, presentation) happens in this single
    /// engine borrow — no other method needs the engine during a stroke.
    ///
    /// If the engine is busy (re-entrant call from WebGPU event pumping),
    /// returns false — the outer render call is already in progress and will
    /// handle everything.  Returning true here would cause the JS side to
    /// schedule another rAF, which Chromium's event pump fires immediately,
    /// creating an infinite loop that freezes the UI.
    pub fn render(&self, time_secs: f32) -> bool {
        let Ok(mut e) = self.engine.try_borrow_mut() else {
            return false;
        };

        // Frame-level perf probe. The per-event `[stab-perf]` log captures
        // `gpu_stroke_to` only; the full rAF frame also pays for command
        // draining (which may process multiple BrushStroke events at high
        // input rate) and `engine.render` (composite + veils + overlays +
        // present). The slow-frame log fires only when a frame exceeds
        // ~1.5× one 60Hz budget so healthy frames cost nothing.
        let frame_start = web_time::Instant::now();
        let drain_start = web_time::Instant::now();
        drain_commands(&self.commands, &mut e);
        let drain_us = drain_start.elapsed().as_micros() as u64;

        let render_start = web_time::Instant::now();
        let result = e.render(time_secs);
        let render_us = render_start.elapsed().as_micros() as u64;

        let frame_us = frame_start.elapsed().as_micros() as u64;
        // 16667 µs is one 60Hz frame. Threshold at 25 ms (~1.5×) so we
        // surface only the frames the user actually perceives as slow.
        if frame_us > 25_000 {
            let p = e.last_render_phases();
            log::warn!(
                "[frame-perf] slow frame={:.2}ms drain={:.2}ms render={:.2}ms \
                 [render breakdown: poll={:.2}ms thumb={:.2}ms anim={:.2}ms composite={:.2}ms]",
                frame_us as f32 / 1000.0,
                drain_us as f32 / 1000.0,
                render_us as f32 / 1000.0,
                p.poll_us as f32 / 1000.0,
                p.thumb_us as f32 / 1000.0,
                p.anim_us as f32 / 1000.0,
                p.compositor_us as f32 / 1000.0,
            );
        }
        result
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

/// Serialize `engine.brush_preview_info()` as a JS `{ halfExtent, rotation }`
/// POJO, or `null` when the active graph has no preview source.
fn brush_preview_info_as_js(engine: &DarklyEngine) -> JsValue {
    #[derive(serde::Serialize)]
    struct Info {
        #[serde(rename = "halfExtent")]
        half_extent: [f32; 2],
        rotation: f32,
    }
    match engine.brush_preview_info() {
        Some(info) => {
            let payload = Info {
                half_extent: info.half_extent_canvas_px,
                rotation: info.rotation_rad,
            };
            serde_wasm_bindgen::to_value(&payload).unwrap_or(JsValue::NULL)
        }
        None => JsValue::NULL,
    }
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
    let mode_param = js_f32(obj, "modeParam").unwrap_or(0.0);
    let rotation = js_f32(obj, "rotation").unwrap_or(0.0);

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
        mode_param,
        rotation,
    })
}
