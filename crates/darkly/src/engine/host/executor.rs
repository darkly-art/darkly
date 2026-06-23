//! Frame-driven, single-threaded task executor — dependency-free.
//!
//! A migrated multi-step GPU op is an ordinary `async` block that re-acquires
//! the engine between `.await`s (each acquire is an [`EngineCell::with`] burst).
//! This executor runs those blocks. It is deliberately *not* a reactor:
//!
//! - **No-op waker.** Progress is frame-driven — a readback completes because a
//!   frame called `device.poll`, not because anything woke a task. So every task
//!   is polled once per [`tick`](Executor::tick) with a hand-rolled no-op
//!   `RawWaker`; there is no wake plumbing to get wrong.
//! - **No external runtime.** No `futures` / `async-task` /
//!   `wasm-bindgen-futures` dependency; the whole thing is a `Vec<Task>` and a
//!   poll loop.
//!
//! Each [`Task`] carries the originating protocol `request` id (when it has one)
//! so teardown can reject the right promise before dropping the task.
//!
//! [`EngineCell::with`]: super::cell::EngineCell::with

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::ptr;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

/// One in-flight op. `future` re-acquires the engine via captured
/// `Rc<EngineCell>` bursts; `request` is the protocol id to reject on teardown
/// (`None` for fire-and-forget tasks with no promise to settle).
pub struct Task {
    pub request: Option<u64>,
    pub future: Pin<Box<dyn Future<Output = ()>>>,
}

impl Task {
    pub fn new(request: Option<u64>, future: impl Future<Output = ()> + 'static) -> Self {
        Task {
            request,
            future: Box::pin(future),
        }
    }

    /// The protocol id this task ultimately resolves, if any.
    pub fn request(&self) -> Option<u64> {
        self.request
    }
}

/// The task list. Interior-mutable (`RefCell`) so the host's `tick`/`pump` —
/// and a re-entrant call reached through the event pump — can drive it through a
/// shared `&self`. A re-entrant `tick` `try_borrow`s and yields rather than
/// double-borrowing.
pub struct Executor {
    tasks: RefCell<Vec<Task>>,
}

impl Default for Executor {
    fn default() -> Self {
        Self::new()
    }
}

impl Executor {
    pub fn new() -> Self {
        Executor {
            tasks: RefCell::new(Vec::new()),
        }
    }

    /// Enqueue a task. Polled starting on the next [`tick`](Self::tick).
    pub fn spawn(&self, task: Task) {
        self.tasks.borrow_mut().push(task);
    }

    /// Poll every task once. Completed tasks are dropped. Returns `true` if any
    /// task completed this tick (a progress signal the host uses to decide
    /// whether to re-poll readbacks within the same frame).
    ///
    /// Re-entrant safety: if the task list is already borrowed (this `tick` was
    /// reached re-entrantly through a task's `queue.submit` event pump), yields
    /// `false` instead of panicking. Polling a task can re-enter — the borrow is
    /// held across `future.poll`, so the nested `tick` must not take it again.
    pub fn tick(&self) -> bool {
        let Ok(mut tasks) = self.tasks.try_borrow_mut() else {
            return false;
        };
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut completed = false;
        let mut i = 0;
        while i < tasks.len() {
            match tasks[i].future.as_mut().poll(&mut cx) {
                Poll::Ready(()) => {
                    tasks.swap_remove(i);
                    completed = true;
                    // Don't increment — swap_remove moved the last task here.
                }
                Poll::Pending => i += 1,
            }
        }
        completed
    }

    /// `true` if any task is still in flight. Drives the host's `needsMore`
    /// keepalive — without it a frame loop with a deferred op mid-flight could
    /// sleep before the op finishes.
    pub fn has_pending_tasks(&self) -> bool {
        !self.tasks.borrow().is_empty()
    }

    /// Drain every task, returning the protocol ids that were still in flight so
    /// teardown can reject their promises. Dropping the tasks releases the
    /// `Rc<EngineCell>` clones each captured — mandatory, or a disposed handle's
    /// engine (and its GPU resources) leaks behind the still-living tasks.
    pub fn drain_pending_requests(&self) -> Vec<u64> {
        self.tasks
            .borrow_mut()
            .drain(..)
            .filter_map(|t| t.request)
            .collect()
    }
}

/// A future that yields exactly once (Pending on first poll, Ready after). With
/// the no-op waker the executor re-polls every tick, so this is "give other
/// tasks a turn / retry next tick" — used by [`with_async`] when the engine is
/// transiently busy.
///
/// [`with_async`]: super::cell::EngineCell::with_async
pub fn yield_now() -> YieldNow {
    YieldNow { yielded: false }
}

pub struct YieldNow {
    yielded: bool,
}

impl Future for YieldNow {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        if self.yielded {
            Poll::Ready(())
        } else {
            self.yielded = true;
            Poll::Pending
        }
    }
}

// ---------------------------------------------------------------------------
// No-op waker — progress is frame-driven, so waking is a no-op.
// ---------------------------------------------------------------------------

fn noop_waker() -> Waker {
    fn no_op(_: *const ()) {}
    fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(ptr::null(), &VTABLE)
    }
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, no_op, no_op, no_op);
    // SAFETY: the vtable's clone/wake/drop are all no-ops over a null data
    // pointer that is never dereferenced — the canonical no-op waker.
    unsafe { Waker::from_raw(RawWaker::new(ptr::null(), &VTABLE)) }
}
