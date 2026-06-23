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
//! ## Injected drain/image closures
//!
//! The request transport and the `OffscreenCanvas` image side-FIFO live
//! bridge-side, not in the host, so `tick`/`pump` take them as closures
//! (`apply_images`, `drain`) rather than owning them. A backend that holds those
//! in the core instead can pass thin closures over its own state; the
//! orchestration (drain → drive tasks → composite) is identical regardless.

pub mod cell;
pub mod executor;

use std::rc::Rc;

use cell::EngineCell;
use executor::Executor;

use crate::engine::protocol::RequestOutcome;
use crate::engine::DarklyEngine;
use crate::gpu::context::GpuContext;

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

/// Wraps the engine cell and the frame-driven executor. `&self` throughout: the
/// executor is interior-mutable and the cell is try-acquired, so the host needs
/// no outer `RefCell` and a re-entrant call yields instead of panicking.
pub struct EngineHost {
    cell: Rc<EngineCell>,
    executor: Executor,
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
    pub fn tick(
        &self,
        time_secs: f32,
        apply_images: impl FnOnce(&mut DarklyEngine),
        drain: impl FnOnce(&mut DarklyEngine) -> Vec<RequestOutcome>,
    ) -> FrameOutcome {
        // Apply pending image uploads, then drain the FIFO and dispatch handlers
        // (a deferring handler spawns a task) — one burst.
        let Some(mut outcomes) = self.cell.with(|e| {
            apply_images(e);
            drain(e)
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
    pub fn pump(
        &self,
        apply_images: impl FnOnce(&mut DarklyEngine),
        drain: impl FnOnce(&mut DarklyEngine) -> Vec<RequestOutcome>,
    ) -> PumpOutcome {
        let Some(mut outcomes) = self.cell.with(|e| {
            apply_images(e);
            drain(e)
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
            let progressed = self.executor.tick();
            self.collect_spawns();
            // The sink scheduler / diff-rect can also gate a task indirectly
            // (cold selection cache lands via the sink path); run a render poll
            // so those advance too.
            self.with(|e| {
                e.render(0.0);
            });
            let idle =
                !self.executor.has_pending_tasks() && self.with(|e| !e.has_pending_readbacks());
            if idle && !harvested && !progressed {
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
