//! Deferred request transport — the platform-agnostic FIFO + drain that makes
//! the engine boundary async and re-entrancy-safe.
//!
//! This is the core half of the in-process / Worker / Tauri transports. It owns
//! the [`RequestRegistry`] and a FIFO of pending requests. The two load-bearing
//! invariants the production panic fix depends on live here (see
//! `distributed-spinning-parnas.md` "Why P1 fixes the panic"):
//!
//! 1. **[`Transport::enqueue`] never borrows the engine.** It only appends, so a
//!    request fired re-entrantly inside `render`'s `queue.submit()` event pump
//!    takes no competing borrow — it just lands in the FIFO. No borrow ⇒ no
//!    panic.
//! 2. **[`Transport::try_drain`] yields instead of blocking.** It `try_borrow_mut`s
//!    the engine; if `render` (or anyone) already holds it, it returns
//!    [`DrainOutcome::Busy`] so the caller reschedules rather than panicking.
//!
//! The JS-side `{ resolve, reject }` correlation table is *not* here — it is
//! request-agnostic plumbing that lives at the transport edge (TS for
//! in-process/Worker). This struct only routes `(id, kind, payload, bytes)` to
//! handlers and hands back per-request results keyed by `id`.

use std::cell::RefCell;

use serde_json::Value;

use super::{ProtocolError, RequestRegistry, Response};
use crate::engine::DarklyEngine;

/// One queued request awaiting dispatch. `id` correlates the eventual result
/// back to the caller's pending promise (minted by the transport edge).
pub struct QueuedRequest {
    pub id: u64,
    pub kind: String,
    pub payload: Value,
    pub bytes: Vec<u8>,
}

/// The result of dispatching one queued request, tagged with its `id`.
pub struct RequestOutcome {
    pub id: u64,
    pub result: Result<Response, ProtocolError>,
}

/// Outcome of a non-blocking drain attempt.
pub enum DrainOutcome {
    /// The engine was already borrowed (e.g. `render` is in flight, reached
    /// re-entrantly via the GPU event pump). Nothing was dequeued — the caller
    /// must reschedule the drain. This is the panic-avoidance yield.
    Busy,
    /// The FIFO was drained (possibly empty); per-request results follow.
    Drained(Vec<RequestOutcome>),
}

/// Engine-side request transport: the registry plus the pending FIFO.
pub struct Transport {
    registry: RequestRegistry,
    queue: RefCell<Vec<QueuedRequest>>,
}

impl Default for Transport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport {
    pub fn new() -> Self {
        Transport {
            registry: RequestRegistry::new(),
            queue: RefCell::new(Vec::new()),
        }
    }

    /// Read-only access to the registry (kind enumeration, etc.).
    pub fn registry(&self) -> &RequestRegistry {
        &self.registry
    }

    /// Append a request to the FIFO. **Borrows nothing but the queue** — safe to
    /// call re-entrantly while the engine is borrowed (invariant #1).
    pub fn enqueue(&self, id: u64, kind: impl Into<String>, payload: Value, bytes: Vec<u8>) {
        self.queue.borrow_mut().push(QueuedRequest {
            id,
            kind: kind.into(),
            payload,
            bytes,
        });
    }

    /// Number of requests waiting in the FIFO.
    pub fn pending(&self) -> usize {
        self.queue.borrow().len()
    }

    /// Drain and dispatch every queued request **under an already-held engine
    /// borrow** — the `render` path, which composites in the same borrow right
    /// after. Requests are dispatched in FIFO submission order.
    pub fn drain_with(&self, engine: &mut DarklyEngine) -> Vec<RequestOutcome> {
        let reqs: Vec<QueuedRequest> = self.queue.borrow_mut().drain(..).collect();
        reqs.into_iter()
            .map(|req| RequestOutcome {
                id: req.id,
                result: self
                    .registry
                    .dispatch(engine, &req.kind, req.payload, &req.bytes),
            })
            .collect()
    }

    /// Non-blocking drain — the scheduler path. `try_borrow_mut`s the engine and
    /// yields [`DrainOutcome::Busy`] if it is already borrowed, instead of
    /// panicking on a competing borrow (invariant #2). On success the FIFO is
    /// emptied and the per-request results returned.
    pub fn try_drain(&self, engine: &RefCell<DarklyEngine>) -> DrainOutcome {
        match engine.try_borrow_mut() {
            Ok(mut e) => DrainOutcome::Drained(self.drain_with(&mut e)),
            Err(_) => DrainOutcome::Busy,
        }
    }
}
