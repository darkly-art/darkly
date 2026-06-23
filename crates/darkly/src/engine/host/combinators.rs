//! Task-authoring combinators — thin async helpers over [`EngineCell::with_async`]
//! so a migrated multi-step op reads as a linear sequence of bursts.
//!
//! These add no new borrow semantics: each is sugar over `with_async` (a scoped
//! engine burst) plus an `.await` on a readback slot or a yield. They exist so a
//! deferred op's body alternates short `cell.with_async(|e| …)` bursts with
//! `.await`s top-to-bottom, instead of every op re-deriving the same
//! warm-the-selection-cache dance.
//!
//! [`EngineCell::with_async`]: super::cell::EngineCell::with_async

use std::rc::Rc;

use super::cell::EngineCell;
use super::executor::yield_now;

/// Ensure the selection CPU cache is warm before an op that reads it (copy /
/// cut / flip / adjustment / transform all derive their region from it).
///
/// No-op when there is no active selection or the cache is already populated.
/// Otherwise kicks a selection readback and awaits the cache being filled by
/// the sink scheduler's `SelectionReadback` handler when a frame poll lands it.
pub async fn ensure_selection_cache_warm(cell: &Rc<EngineCell>) {
    let cold = cell
        .with_async(|e| e.has_selection() && e.selection_cpu_cache().is_none())
        .await;
    if !cold {
        return;
    }
    cell.with_async(|e| e.kick_selection_readback()).await;
    loop {
        if cell.with_async(|e| e.selection_cpu_cache().is_some()).await {
            break;
        }
        yield_now().await;
    }
}
