//! [`EngineCell`] — the sole sanctioned engine borrow.
//!
//! The whole engine is one actor cell: a `RefCell<DarklyEngine>` exposed *only*
//! through a scoped accessor that hands out `&mut DarklyEngine` for the duration
//! of a synchronous closure and never returns a guard. Two invariants fall out
//! of that shape, and they are the reason this type exists:
//!
//! - **No held guard ⇒ no borrow across `.await`.** A caller cannot stash the
//!   `&mut` and suspend; the borrow is released when the closure returns, before
//!   any `.await` in the surrounding task. This makes the borrow-across-await
//!   deadlock (a task holding the engine while the frame loop's `device.poll`
//!   needs it) *structurally unrepresentable*.
//! - **Try-acquire, never panic.** [`with`](EngineCell::with) uses
//!   `try_borrow_mut` and yields `None` on contention instead of panicking. The
//!   re-entrancy eruption (`queue.submit` pumping the browser event queue while a
//!   borrow is held, commit `219a879`) becomes a yield, not a poisoned cell.
//!
//! A CI grep gate enforces that this is the *only* place the engine cell is
//! borrowed — any raw `.borrow()` / `.borrow_mut()` on a `RefCell<DarklyEngine>`
//! outside this file fails the build.

use std::cell::RefCell;
use std::rc::Rc;

use crate::engine::DarklyEngine;

/// One actor cell wrapping the whole engine. Tasks hold a cheap `Rc<EngineCell>`
/// clone; every engine access goes through [`with`](Self::with).
pub struct EngineCell(RefCell<DarklyEngine>);

impl EngineCell {
    /// Wrap an engine. Returns an `Rc` so tasks (and the engine's own
    /// `self_cell` back-reference) can share it.
    pub fn new(engine: DarklyEngine) -> Rc<Self> {
        Rc::new(EngineCell(RefCell::new(engine)))
    }

    /// Run `f` with exclusive access to the engine for the duration of the
    /// closure. Returns `Some(f(...))`, or `None` if the engine is already
    /// borrowed (the re-entrancy yield). The borrow is released when `f`
    /// returns — you cannot carry it across an `.await`.
    pub fn with<R>(&self, f: impl FnOnce(&mut DarklyEngine) -> R) -> Option<R> {
        self.0.try_borrow_mut().ok().map(|mut e| f(&mut e))
    }

    /// Async variant of [`with`](Self::with): if the engine is busy, yield and
    /// retry on the next executor tick rather than skipping the burst. The
    /// task-facing combinator — keeps a migrated op's body a linear sequence of
    /// `cell.with_async(|e| …).await` bursts that each observe the engine even
    /// across a transient re-entrant borrow. `FnOnce` (the closure runs at most
    /// once, only on the attempt that wins the borrow), so a burst can move
    /// owned data — e.g. readback pixels — into the engine.
    pub async fn with_async<R>(self: &Rc<Self>, f: impl FnOnce(&mut DarklyEngine) -> R) -> R {
        let mut f = Some(f);
        loop {
            if let Ok(mut e) = self.0.try_borrow_mut() {
                let f = f.take().expect("with_async closure runs exactly once");
                return f(&mut e);
            }
            super::executor::yield_now().await;
        }
    }

    /// Try to reclaim the wrapped engine, consuming the cell. Succeeds only when
    /// this is the last `Rc` — i.e. every task that captured a clone has been
    /// dropped. Used by teardown (and tests) to recover the engine.
    pub fn into_engine(self: Rc<Self>) -> Result<DarklyEngine, Rc<Self>> {
        Rc::try_unwrap(self).map(|cell| cell.0.into_inner())
    }

    /// `true` if more than one `Rc<EngineCell>` is alive — i.e. at least one
    /// task still holds a strong reference. Teardown asserts this is `false`
    /// after the executor's task list is dropped.
    pub fn has_outstanding_tasks(self: &Rc<Self>) -> bool {
        Rc::strong_count(self) > 1
    }
}
