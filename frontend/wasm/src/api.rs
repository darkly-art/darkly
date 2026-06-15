//! WASM bridge for the Darkly engine — the in-process transport.
//!
//! ## The async request/response boundary
//!
//! The engine sits behind a single deferred [`Transport`] (the platform-agnostic
//! FIFO + registry in `darkly::engine::protocol`). The frontend never calls a
//! per-method wasm wrapper; instead it `enqueue`s `(id, kind, payload, bytes)`
//! requests and the FIFO is drained on a scheduled macrotask ([`drain`]) and at
//! frame time ([`render`]). Each request resolves a JS-side promise by `id`.
//!
//! ## Why this kills the re-entrancy panic
//!
//! Chromium pumps the browser event queue *inside* `queue.submit()` /
//! `device.poll()`. The old bridge held `engine.borrow_mut()` across a method
//! and a re-entrant pointer/rAF callback took a competing borrow → permanent
//! `RefCell` poison. Now there are **exactly two** engine borrow sites:
//!
//! - [`render`] — `try_borrow_mut`; drains the FIFO then composites, all in one
//!   borrow. Returns `false`-equivalent (`busy: true`) and reschedules if it
//!   can't get the borrow.
//! - [`drain`] — `Transport::try_drain`, which `try_borrow_mut`s and yields
//!   `busy` instead of panicking when render holds the borrow.
//!
//! [`enqueue`] borrows nothing (it only appends to the FIFO), so a re-entrant
//! request fired inside `submit()` is safe by construction. A CI grep gate
//! enforces that no third `engine.borrow*()` site appears.
//!
//! `frame_count` / `thumbnail_version` are no longer separate borrowing reads —
//! [`render`] returns them as a downhill projection of its single borrow.

use std::cell::RefCell;
use std::sync::Arc;

use darkly::engine::protocol::{DrainOutcome, RequestOutcome, Transport};
use darkly::engine::DarklyEngine;
use darkly::gpu::context::{GpuContext, GpuDevice};
use darkly::layer::LayerId;
use wasm_bindgen::prelude::*;

// ---------------------------------------------------------------------------
// Pure view-matrix helper (borrows no engine state)
// ---------------------------------------------------------------------------

/// Pure screen↔plane matrix constructor for the JS coordinate path. Borrows no
/// engine state, so it is safe to call inside a pointer event (no RefCell
/// aliasing with an in-flight `render()`).
///
/// Returns 12 floats `[screen→plane (6), plane→screen (6)]`, each row-major
/// `[m00, m01, m02, m10, m11, m12]`.
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub fn compute_view_matrices(
    pan_x: f32,
    pan_y: f32,
    zoom: f32,
    rotation: f32,
    mirror_h: bool,
    screen_w: f32,
    screen_h: f32,
    canvas_origin_x: f32,
    canvas_origin_y: f32,
    canvas_w: f32,
    canvas_h: f32,
) -> Vec<f32> {
    darkly::gpu::view::compute_view_matrices(
        pan_x,
        pan_y,
        zoom,
        rotation,
        mirror_h,
        screen_w,
        screen_h,
        canvas_origin_x,
        canvas_origin_y,
        canvas_w,
        canvas_h,
    )
    .to_vec()
}

// ---------------------------------------------------------------------------
// DarklySession — shared GPU device for multiple DarklyHandles
// ---------------------------------------------------------------------------

/// A process-level GPU session that owns one `wgpu::Instance` and one
/// `Arc<GpuDevice>`. Hand out `DarklyHandle`s via `createHandle(...)` to attach
/// additional canvases to the same WebGPU device — the multi-tab editor uses one
/// session and N handles, one per open document.
#[wasm_bindgen]
pub struct DarklySession {
    instance: wgpu::Instance,
    /// `None` until the first canvas is attached; `Some` thereafter.
    gpu: RefCell<Option<Arc<GpuDevice>>>,
    /// Shared tool session — generic per-tool state bag, cloned into every
    /// handle so all engines see the same tool state.
    tool_session: darkly::tool::SharedToolSession,
}

#[wasm_bindgen]
impl DarklySession {
    #[wasm_bindgen(constructor)]
    pub fn new() -> DarklySession {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::BROWSER_WEBGPU,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
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

    /// Build a new `DarklyHandle` bound to `canvas`, sharing this session's GPU
    /// device with every other handle from this session. The first call
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

        DarklyHandle::from_engine(DarklyEngine::new_with_tool_session(
            gpu,
            self.tool_session.clone(),
            doc_width,
            doc_height,
        ))
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

/// A deferred upload of a live `OffscreenCanvas` frame into a camera-style
/// void's GPU texture. This is the one engine operation that can't cross the
/// serialized protocol boundary (an `OffscreenCanvas` isn't JSON), so it rides
/// its own typed side-FIFO — but it is still *deferred* (no synchronous borrow
/// at call time) and applied under `render`/`drain`'s borrow, preserving the
/// "exactly two borrow sites" invariant.
struct PendingExternalImage {
    layer_id: u64,
    source: darkly::gpu::void::ExternalImageSource,
}

#[wasm_bindgen]
pub struct DarklyHandle {
    engine: RefCell<DarklyEngine>,
    /// The deferred request transport (FIFO + dispatch registry). Enqueue-only
    /// from JS; drained here.
    transport: Transport,
    /// Side-FIFO for the OffscreenCanvas upload exception (see
    /// [`PendingExternalImage`]). Enqueue-only; applied at drain/render time.
    pending_images: RefCell<Vec<PendingExternalImage>>,
}

impl DarklyHandle {
    fn from_engine(engine: DarklyEngine) -> Self {
        DarklyHandle {
            engine: RefCell::new(engine),
            transport: Transport::new(),
            pending_images: RefCell::new(Vec::new()),
        }
    }

    /// Apply any queued OffscreenCanvas uploads under an already-held engine
    /// borrow. Called by both drain paths before the protocol FIFO so a frame's
    /// camera upload lands the same frame.
    fn apply_pending_images(&self, engine: &mut DarklyEngine) {
        let pending: Vec<PendingExternalImage> =
            self.pending_images.borrow_mut().drain(..).collect();
        for p in pending {
            engine.upload_void_external_image(LayerId::from_ffi(p.layer_id), p.source);
        }
    }
}

#[wasm_bindgen]
impl DarklyHandle {
    /// Create a stand-alone editor instance from a canvas (own device). Prefer
    /// `DarklySession.createHandle` for the multi-tab shared-device case.
    pub async fn create(
        canvas: web_sys::HtmlCanvasElement,
        doc_width: u32,
        doc_height: u32,
    ) -> DarklyHandle {
        let initial_width = canvas.width();
        let initial_height = canvas.height();

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::BROWSER_WEBGPU,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
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

        DarklyHandle::from_engine(DarklyEngine::new(gpu, doc_width, doc_height))
    }

    // =======================================================================
    // Transport surface — enqueue (no borrow), drain, render
    // =======================================================================

    /// Append a request to the FIFO. **Borrows nothing** — safe to call
    /// re-entrantly inside render's event pump (invariant #1). `payload` is a JS
    /// object; `bytes` is an optional `Uint8Array` binary side-channel.
    pub fn enqueue(&self, id: f64, kind: &str, payload: JsValue, bytes: Option<Vec<u8>>) {
        let value: serde_json::Value =
            serde_wasm_bindgen::from_value(payload).unwrap_or(serde_json::Value::Null);
        self.transport
            .enqueue(id as u64, kind, value, bytes.unwrap_or_default());
    }

    /// Non-blocking drain (the scheduler path). Returns `{ busy: true }` if the
    /// engine is borrowed (render in flight — caller reschedules), else
    /// `{ busy: false, results: [...] }` with one entry per dispatched request.
    pub fn drain(&self) -> JsValue {
        // Apply deferred OffscreenCanvas uploads first, under the same borrow,
        // without panicking if render holds it.
        let outcome = {
            let Ok(mut e) = self.engine.try_borrow_mut() else {
                return busy_result();
            };
            self.apply_pending_images(&mut e);
            DrainOutcome::Drained(self.transport.drain_with(&mut e))
        };
        match outcome {
            DrainOutcome::Busy => busy_result(),
            DrainOutcome::Drained(outcomes) => drained_result(outcomes),
        }
    }

    /// Render the current frame. **Seam B.** Drains the FIFO (and pending image
    /// uploads) under its one `try_borrow_mut`, *then* composites — preserving
    /// frame-coherent "mutate-then-composite-same-frame" semantics. Returns
    /// `{ busy, needsMore, frameCount, thumbnailVersion, results }`; `busy` is
    /// true when a re-entrant render couldn't get the borrow (caller must not
    /// reschedule another rAF — the outer render handles everything).
    pub fn render(&self, time_secs: f32) -> JsValue {
        let Ok(mut e) = self.engine.try_borrow_mut() else {
            return busy_result();
        };

        let frame_start = web_time::Instant::now();
        let drain_start = web_time::Instant::now();
        self.apply_pending_images(&mut e);
        let outcomes = self.transport.drain_with(&mut e);
        let drain_us = drain_start.elapsed().as_micros() as u64;

        let render_start = web_time::Instant::now();
        let needs_more = e.render(time_secs);
        let render_us = render_start.elapsed().as_micros() as u64;

        let frame_us = frame_start.elapsed().as_micros() as u64;
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

        // The frontend's synchronously-readable engine-state mirror — one struct
        // for every value the UI caches (frame/thumbnail counters + document
        // bools), built from cheap CPU reads under the borrow render already
        // holds (no extra query / per-frame poll). Grows as the UI needs more.
        let state = serde_wasm_bindgen::to_value(&e.engine_state()).unwrap_or(JsValue::NULL);
        drop(e);

        let obj = js_sys::Object::new();
        set(&obj, "busy", JsValue::FALSE);
        set(&obj, "needsMore", JsValue::from_bool(needs_more));
        set(&obj, "state", state);
        set(&obj, "results", outcomes_to_js(outcomes).into());
        obj.into()
    }

    /// Queue an OffscreenCanvas frame upload for a camera-style void. Borrows
    /// nothing; applied at the next drain/render. See [`PendingExternalImage`].
    pub fn upload_void_external_image(&self, layer_id: f64, canvas: web_sys::OffscreenCanvas) {
        let info = wgpu::CopyExternalImageSourceInfo {
            source: wgpu::ExternalImageSource::OffscreenCanvas(canvas),
            origin: wgpu::Origin2d::ZERO,
            flip_y: false,
        };
        self.pending_images.borrow_mut().push(PendingExternalImage {
            layer_id: layer_id as u64,
            source: darkly::gpu::void::ExternalImageSource::Web(info),
        });
    }

    /// Engine-side thumbnail dimension used by the auto-queue path. Returns a
    /// compile-time constant — no engine borrow. The frontend's `THUMB_SIZE`
    /// must match; `app.svelte.ts` asserts equality at init.
    pub fn engine_default_thumb_size(&self) -> u32 {
        darkly::engine::DEFAULT_THUMB_SIZE
    }
}

// ---------------------------------------------------------------------------
// JS marshalling helpers
// ---------------------------------------------------------------------------

fn set(obj: &js_sys::Object, key: &str, value: JsValue) {
    js_sys::Reflect::set(obj, &JsValue::from_str(key), &value).ok();
}

fn busy_result() -> JsValue {
    let obj = js_sys::Object::new();
    set(&obj, "busy", JsValue::TRUE);
    obj.into()
}

fn drained_result(outcomes: Vec<RequestOutcome>) -> JsValue {
    let obj = js_sys::Object::new();
    set(&obj, "busy", JsValue::FALSE);
    set(&obj, "results", outcomes_to_js(outcomes).into());
    obj.into()
}

fn outcomes_to_js(outcomes: Vec<RequestOutcome>) -> js_sys::Array {
    let arr = js_sys::Array::new();
    for o in outcomes {
        arr.push(&outcome_to_js(o));
    }
    arr
}

/// One dispatched request's result: `{ id, value, bytes? }` on success, or
/// `{ id, error }` (the `{ kind, message }` envelope) on a protocol failure.
///
/// `serde_json::Value`s are serialized with `serialize_maps_as_objects(true)` —
/// without it, `serde_wasm_bindgen` emits a JS `Map` for every JSON object
/// (so `result.id` / `layerInfo.type` would be `undefined`). A binary response
/// always emits a `bytes` field (even when empty — a readback-in-flight preview),
/// keyed off `Some`, never off emptiness.
fn outcome_to_js(outcome: RequestOutcome) -> JsValue {
    use serde::Serialize;
    let serializer = serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true);
    let obj = js_sys::Object::new();
    set(&obj, "id", JsValue::from_f64(outcome.id as f64));
    match outcome.result {
        Ok(resp) => {
            set(
                &obj,
                "value",
                resp.value.serialize(&serializer).unwrap_or(JsValue::NULL),
            );
            if let Some(bytes) = resp.bytes {
                let arr = js_sys::Uint8Array::new_with_length(bytes.len() as u32);
                arr.copy_from(&bytes);
                set(&obj, "bytes", arr.into());
            }
        }
        Err(e) => {
            set(
                &obj,
                "error",
                e.to_json().serialize(&serializer).unwrap_or(JsValue::NULL),
            );
        }
    }
    obj.into()
}
