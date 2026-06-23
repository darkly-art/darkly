//! [`EngineHost`] — the inverted frame loop that unifies request transport and
//! readback completion under one frame-driven executor.
//!
//! The architectural unlock: `render`/composite is **one scheduled burst among
//! many**, not the outer borrow everything nests inside. The host wraps
//! `(EngineCell, Executor)`; every burst acquires the engine through the scoped
//! [`EngineCell::with`] and releases it before any `.await`, so a deferred op
//! (copy / cut / …) is an ordinary `async` block that re-acquires the engine
//! between readback `.await`s instead of a `pending_*` field + a central resume
//! `match`.
//!
//! Two entry points mirror the two TS drain triggers so request latency is
//! preserved:
//!
//! - [`tick`](EngineHost::tick) — the **frame** path (rAF). Full orchestration
//!   including composite.
//! - [`pump`](EngineHost::pump) — the **macrotask** path (`MessageChannel`).
//!   Drains + drives tasks but skips composite, so simple requests resolve
//!   sub-frame rather than ~16 ms later.
//!
//! ## Host-owned transport + image FIFO
//!
//! The host owns the request [`Transport`] (the FIFO + dispatch registry) and
//! the `OffscreenCanvas` image side-FIFO, so `tick`/`pump` orchestrate the whole
//! pointer-to-pixel path — drain → drive tasks → composite — with no
//! platform-supplied closures. A wasm bridge or a future Tauri backend supplies
//! only device acquisition and an event-loop binding; it `enqueue`s requests and
//! calls `tick`/`pump`, and the same core orchestration runs on every platform.

pub mod cell;
pub mod combinators;
pub mod executor;

use std::cell::RefCell;
use std::rc::Rc;

use cell::EngineCell;
use executor::Executor;
use serde_json::Value;

use crate::engine::protocol::{RequestOutcome, Transport};
use crate::engine::DarklyEngine;
use crate::gpu::context::GpuContext;
use crate::gpu::void::ExternalImageSource;
use crate::layer::LayerId;

/// Cap on the in-frame readback re-poll in [`drive_tasks`](EngineHost::drive_tasks).
/// A chain of already-landed readbacks drains in one frame instead of one stage
/// per frame; newly-*kicked* readbacks still land next frame (a kicked readback
/// isn't ready the same frame regardless).
const MAX_REPOLL: u32 = 4;

/// Result of a [`tick`](EngineHost::tick). Mirrors the TS `{ busy, needsMore,
/// results }` contract the rAF loop consumes; the bridge layers `state` on top
/// from a scoped read.
pub struct FrameOutcome {
    /// The engine was already borrowed (re-entrant frame reached through the
    /// event pump). Nothing ran — the caller must not reschedule.
    pub busy: bool,
    /// Animations, in-flight readbacks, or in-flight tasks need another frame.
    pub needs_more: bool,
    /// Terminal outcomes for requests dispatched (or resolved) this frame.
    pub outcomes: Vec<RequestOutcome>,
}

impl FrameOutcome {
    fn busy() -> Self {
        FrameOutcome {
            busy: true,
            needs_more: false,
            outcomes: Vec::new(),
        }
    }
}

/// Result of a [`pump`](EngineHost::pump) — the macrotask path. No composite, so
/// no `needsMore`.
pub struct PumpOutcome {
    pub busy: bool,
    pub outcomes: Vec<RequestOutcome>,
}

/// A deferred upload of a live `OffscreenCanvas` frame into a camera-style
/// void's GPU texture. This is the one engine operation that can't cross the
/// serialized protocol boundary (an `OffscreenCanvas` isn't JSON), so it rides
/// its own typed side-FIFO — still *deferred* (no synchronous borrow at enqueue
/// time), applied under the next `tick`/`pump` burst.
struct PendingImage {
    layer_id: LayerId,
    source: ExternalImageSource,
}

/// Wraps the engine cell, the frame-driven executor, the request transport, and
/// the image side-FIFO. `&self` throughout: every piece is interior-mutable and
/// the cell is try-acquired, so the host needs no outer `RefCell` and a
/// re-entrant call yields instead of panicking.
pub struct EngineHost {
    cell: Rc<EngineCell>,
    executor: Executor,
    /// Deferred request FIFO + dispatch registry. Enqueue-only from the
    /// platform edge; drained into the engine under `tick`/`pump`'s borrow.
    transport: Transport,
    /// Side-FIFO for the `OffscreenCanvas` upload exception. Enqueue-only;
    /// applied at drain/render time before the protocol FIFO.
    pending_images: RefCell<Vec<PendingImage>>,
}

impl EngineHost {
    /// Build a fresh engine and wrap it. The same entry for browser, Tauri, and
    /// headless tests.
    pub fn new(gpu: GpuContext, doc_width: u32, doc_height: u32) -> Self {
        Self::adopt(DarklyEngine::new(gpu, doc_width, doc_height))
    }

    /// Build a fresh engine sharing a `DarklySession`-owned tool session, then
    /// wrap it (the multi-tab path).
    pub fn new_with_tool_session(
        gpu: GpuContext,
        tool_session: crate::tool::SharedToolSession,
        doc_width: u32,
        doc_height: u32,
    ) -> Self {
        Self::adopt(DarklyEngine::new_with_tool_session(
            gpu,
            tool_session,
            doc_width,
            doc_height,
        ))
    }

    /// Wrap an already-constructed engine and wire its `self_cell`
    /// back-reference so deferring handlers can build futures that re-acquire
    /// the engine between awaits.
    pub fn adopt(engine: DarklyEngine) -> Self {
        let cell = EngineCell::new(engine);
        let weak = Rc::downgrade(&cell);
        cell.with(|e| e.set_self_cell(weak))
            .expect("freshly-built cell is not borrowed");
        EngineHost {
            cell,
            executor: Executor::new(),
            transport: Transport::new(),
            pending_images: RefCell::new(Vec::new()),
        }
    }

    /// Append a request to the FIFO. **Borrows no engine state** — safe to call
    /// re-entrantly inside a `tick`'s `queue.submit` event pump (the request
    /// just lands in the FIFO; no competing borrow, so no panic). `payload` is
    /// the decoded JSON value; `bytes` is the optional binary side-channel.
    pub fn enqueue(&self, id: u64, kind: &str, payload: Value, bytes: Vec<u8>) {
        self.transport.enqueue(id, kind, payload, bytes);
    }

    /// Queue an `OffscreenCanvas` upload for a camera-style void. Borrows no
    /// engine state; applied at the next `tick`/`pump`. See [`PendingImage`].
    pub fn enqueue_image(&self, layer_id: LayerId, source: ExternalImageSource) {
        self.pending_images
            .borrow_mut()
            .push(PendingImage { layer_id, source });
    }

    /// Read-only access to the transport's dispatch registry (kind enumeration).
    pub fn registry(&self) -> &crate::engine::protocol::RequestRegistry {
        self.transport.registry()
    }

    /// Apply any queued `OffscreenCanvas` uploads under an already-held engine
    /// borrow. Run by both drain paths before the protocol FIFO so a frame's
    /// camera upload lands the same frame.
    fn apply_pending_images(&self, engine: &mut DarklyEngine) {
        let pending: Vec<PendingImage> = self.pending_images.borrow_mut().drain(..).collect();
        for p in pending {
            engine.upload_void_external_image(p.layer_id, p.source);
        }
    }

    /// The engine cell — for scoped reads the host doesn't orchestrate (e.g. the
    /// bridge reading `engine_state` after a tick).
    pub fn cell(&self) -> &Rc<EngineCell> {
        &self.cell
    }

    /// Move any handler-spawned tasks from the engine's `pending_spawns` into the
    /// executor. Run after every dispatch burst and after each task tick (a task
    /// could, in principle, spawn another).
    fn collect_spawns(&self) {
        if let Some(spawns) = self.cell.with(|e| e.take_pending_spawns()) {
            for task in spawns {
                self.executor.spawn(task);
            }
        }
    }

    /// Drive in-flight tasks: poll readbacks (fill slots) **before** ticking the
    /// executor so a slot landing this frame is observed this frame, with a
    /// bounded re-poll so a chain of already-landed readbacks drains in one
    /// frame. The executor tick happens outside any cell borrow — each task
    /// self-acquires per burst.
    fn drive_tasks(&self) {
        for _ in 0..MAX_REPOLL {
            self.collect_spawns();
            let harvested = self.cell.with(|e| e.poll_readbacks()).unwrap_or(false);
            let progressed = self.executor.tick();
            self.collect_spawns();
            if !harvested && !progressed {
                break;
            }
        }
    }

    /// The **frame** path (rAF). Drains the FIFO, drives tasks, composites, and
    /// collects terminal outcomes — each step a scoped burst released before the
    /// next; `.await`s happen only inside the executor's tasks, between bursts.
    pub fn tick(&self, time_secs: f32) -> FrameOutcome {
        // Apply pending image uploads, then drain the FIFO and dispatch handlers
        // (a deferring handler spawns a task) — one burst.
        let Some(mut outcomes) = self.cell.with(|e| {
            self.apply_pending_images(e);
            self.transport.drain_with(e)
        }) else {
            return FrameOutcome::busy();
        };

        // Move spawned tasks into the executor and drive them between bursts.
        self.drive_tasks();

        // Composite. `render` also polls the sink scheduler / diff-rect /
        // content-bounds and returns the animation keepalive; OR in the executor
        // so a deferred op in flight keeps frames coming (see
        // `DarklyEngine::render`).
        let needs_more = self.cell.with(|e| e.render(time_secs)).unwrap_or(false)
            || self.executor.has_pending_tasks();

        // A task may have resolved its request during drive_tasks or the render
        // poll — flush terminal outcomes for the JS id→promise table.
        if let Some(completed) = self.cell.with(|e| e.take_completed_requests()) {
            outcomes.extend(completed);
        }

        FrameOutcome {
            busy: false,
            needs_more,
            outcomes,
        }
    }

    /// The **macrotask** path (`MessageChannel`). Same as [`tick`](Self::tick)
    /// minus the composite, so simple requests resolve sub-frame and a deferred
    /// task can still progress between frames. Armed at pointer frequency, so it
    /// early-outs cheaply when idle (the readback poll only calls `device.poll`
    /// when a scheduler has work, and an empty-executor tick is O(0)).
    pub fn pump(&self) -> PumpOutcome {
        let Some(mut outcomes) = self.cell.with(|e| {
            self.apply_pending_images(e);
            self.transport.drain_with(e)
        }) else {
            return PumpOutcome {
                busy: true,
                outcomes: Vec::new(),
            };
        };

        self.drive_tasks();

        if let Some(completed) = self.cell.with(|e| e.take_completed_requests()) {
            outcomes.extend(completed);
        }

        PumpOutcome {
            busy: false,
            outcomes,
        }
    }

    /// Tear down the host: cancel every in-flight readback slot, reject every
    /// still-pending task's request so no JS promise dangles, flush the terminal
    /// outcomes one last time, then **drop** the executor's task list. The drop
    /// is mandatory — each task captured an `Rc<EngineCell>`, so a task left
    /// alive keeps the engine (and its GPU resources) alive past dispose.
    /// Returns the final outcomes (the rejections) for the caller to flush to the
    /// promise table.
    pub fn dispose(&self) -> Vec<RequestOutcome> {
        // `drain_pending_requests` empties the executor — dropping every task
        // releases the `Rc<EngineCell>` clones each captured, so the engine is no
        // longer kept alive behind them.
        let mut pending_ids = self.executor.drain_pending_requests();
        self.cell
            .with(|e| {
                // A task enqueued this dispatch but not yet moved into the
                // executor still sits in `pending_spawns` — drain and reject it
                // too, dropping the task (and its cell clone).
                for task in e.take_pending_spawns() {
                    if let Some(id) = task.request() {
                        pending_ids.push(id);
                    }
                }
                e.cancel_async_readbacks();
                for id in pending_ids {
                    e.reject_request(
                        id,
                        crate::engine::protocol::ProtocolError::engine("handle disposed"),
                    );
                }
                e.take_completed_requests()
            })
            .unwrap_or_default()
    }

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    /// Scoped engine access for test setup/inspection. Panics on contention
    /// (tests are single-threaded with no live frame), unlike the production
    /// try-acquire paths.
    #[cfg(any(test, feature = "testing"))]
    pub fn with<R>(&self, f: impl FnOnce(&mut DarklyEngine) -> R) -> R {
        self.cell
            .with(f)
            .expect("test host engine should not be borrowed")
    }

    /// Drive tasks + readbacks to quiescence (blocking `device.poll(Wait)` so
    /// native readbacks land deterministically). Test stand-in for the rAF loop
    /// flushing a deferred op to completion.
    #[cfg(any(test, feature = "testing"))]
    pub fn pump_until_idle(&self) {
        for _ in 0..256 {
            self.collect_spawns();
            let harvested = self.with(|e| e.test_poll_readbacks_blocking());
            // A no-selection `begin_transform` task awaits the content-bounds
            // compute, which rides its own pass (not the readback schedulers);
            // force-flush it so the task can observe the result this iteration.
            let bounds_done = self
                .with(|e| e.has_pending_content_bounds() && e.test_flush_content_bounds_blocking());
            let progressed = self.executor.tick();
            self.collect_spawns();
            // The sink scheduler / diff-rect can also gate a task indirectly
            // (cold selection cache lands via the sink path); run a render poll
            // so those advance too.
            self.with(|e| {
                e.render(0.0);
            });
            let idle = !self.executor.has_pending_tasks()
                && self.with(|e| !e.has_pending_readbacks() && !e.has_pending_content_bounds());
            if idle && !harvested && !bounds_done && !progressed {
                break;
            }
        }
    }

    /// `true` if any executor task is still in flight. Test-only.
    #[cfg(any(test, feature = "testing"))]
    pub fn has_pending_tasks(&self) -> bool {
        self.executor.has_pending_tasks()
    }

    /// Recover the engine, asserting no task still holds the cell. Test-only —
    /// lets a test resume direct `&mut engine` use after a deferred op.
    #[cfg(any(test, feature = "testing"))]
    pub fn into_engine(self) -> DarklyEngine {
        self.cell
            .into_engine()
            .unwrap_or_else(|_| panic!("a task still holds the engine cell"))
    }
}
