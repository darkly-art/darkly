//! Regression tests for the competing-borrow panic class (plan steps 3 + 5).
//!
//! Production panic: `RefCell already mutably borrowed` at `frame_count()`, then
//! a poisoned cell for the rest of the session. Mechanism: Chromium pumps the
//! event queue *inside* `queue.submit()` while `render()` holds
//! `engine.borrow_mut()`; a re-entrant call takes a competing borrow and panics.
//!
//! Browser event-pump re-entrancy itself is not reproducible headless — that is
//! the manual browser repro's job. But the *competing-borrow class* is: these
//! tests model render holding the engine borrow and assert the transport's two
//! invariants hold (enqueue borrows nothing; drain yields instead of panicking).
//!
//! `naive_synchronous_access_under_held_borrow_panics` pins the bug itself: the
//! pre-fix `flush_if_needed` pattern (a second `borrow_mut` while one is held)
//! *does* panic. The two `transport_*` tests prove the deferred design does not.
//!
//! Run: `cargo test -p darkly --test protocol_reentrancy --features testing`

use std::cell::RefCell;

use darkly::engine::protocol::{DrainOutcome, Transport};
use darkly::engine::DarklyEngine;
use darkly::gpu::context::GpuContext;
use darkly::gpu::test_utils::test_device;
use serde_json::json;

fn test_engine_cell(width: u32, height: u32) -> RefCell<DarklyEngine> {
    let (device, queue) = test_device();
    let gpu = GpuContext::new_headless(device, queue);
    RefCell::new(DarklyEngine::new(gpu, width, height))
}

/// The bug, reproduced: the old `flush_if_needed` took a `borrow_mut` even when
/// render already held one. With a render-style borrow held, that second
/// `borrow_mut` panics — exactly the production crash. This is what the deferred
/// transport eliminates.
#[test]
#[should_panic(expected = "already")]
fn naive_synchronous_access_under_held_borrow_panics() {
    let cell = test_engine_cell(64, 64);
    // Stand in for render()'s held borrow during the GPU event pump.
    let _render_guard = cell.borrow_mut();
    // Stand in for a re-entrant query/mutation doing what `flush_if_needed` did.
    let _competing = cell.borrow_mut(); // panics: already mutably borrowed
}

/// Invariant #1: enqueue takes no engine borrow, so it is safe to call while
/// render holds the engine — the request just lands in the FIFO. Against the
/// pre-fix synchronous model the equivalent call would `borrow_mut` and panic.
#[test]
fn transport_enqueue_under_held_borrow_does_not_panic() {
    let cell = test_engine_cell(64, 64);
    let transport = Transport::new();

    let _render_guard = cell.borrow_mut(); // render in flight
                                           // Re-entrant enqueue (as if fired inside submit()'s event pump):
    transport.enqueue(1, "add_raster", json!({ "anchor": null }), Vec::new());
    transport.enqueue(2, "layer_tree", json!(null), Vec::new());

    assert_eq!(
        transport.pending(),
        2,
        "requests queued, nothing dispatched yet"
    );
}

/// Invariant #2: try_drain yields `Busy` (reschedule) instead of panicking when
/// the engine is already borrowed by render. Nothing is dequeued on a busy
/// drain, so no request is lost.
#[test]
fn transport_try_drain_under_held_borrow_reschedules() {
    let cell = test_engine_cell(64, 64);
    let transport = Transport::new();
    transport.enqueue(1, "add_raster", json!({ "anchor": null }), Vec::new());

    let render_guard = cell.borrow_mut(); // render in flight
    match transport.try_drain(&cell) {
        DrainOutcome::Busy => {} // expected: yields, no panic
        DrainOutcome::Drained(_) => panic!("must not drain while engine is borrowed"),
    }
    assert_eq!(transport.pending(), 1, "busy drain dequeues nothing");

    // Once render releases the borrow, the same drain succeeds and dispatches.
    drop(render_guard);
    match transport.try_drain(&cell) {
        DrainOutcome::Drained(outcomes) => {
            assert_eq!(outcomes.len(), 1);
            assert!(outcomes[0].result.is_ok());
        }
        DrainOutcome::Busy => panic!("engine is free; drain should succeed"),
    }
    assert_eq!(
        transport.pending(),
        0,
        "FIFO emptied after a successful drain"
    );
}
