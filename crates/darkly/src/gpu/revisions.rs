//! One monotonic clock, many named sources — the compositor's single answer
//! to "is this derived thing still valid?".
//!
//! # The model
//!
//! A **source of truth** is a fact whose change can make derived GPU state
//! wrong and which no other tracked fact implies. Mutating one bumps it,
//! advancing the shared clock and stamping the source with the new value.
//!
//! A **derived artifact** — the composite, the presented frame, a cached
//! content-bounds result, an effect instance's bind groups — records the
//! [`Tick`] it was built at and owns an explicit list of the sources it
//! depends on. Validity is a comparison performed **where the value is
//! consumed**, never a flag pushed from the mutation site.
//!
//! That inversion is the point. Forgetting to bump a source is the same class
//! of bug as forgetting a `mark_dirty()` call: stale output until the next
//! coarse bump. Forgetting to *check* is impossible, because there is no flag
//! to consult and no invalidate call to omit — reading a derived value runs
//! the comparison as part of getting it.
//!
//! Because every source stamps the same clock, "did any of these change since
//! tick T" is a max-compare over a handful of `u64`s, never a scan.
//!
//! The registry knows nothing about its consumers: dependency lists live on
//! the artifacts. Adding a cache means recording a tick and naming the sources
//! it reads, with no edit here and none at any mutation site.

use crate::layer::LayerId;
use std::collections::HashMap;

/// A point on the compositor's revision clock. Monotonic and shared by every
/// source, so stamps from different sources are directly comparable.
pub type Tick = u64;

/// The compositor's revision registry. One clock, five sources.
///
/// Artifact stamps start at 0 and the clock's first bump yields 1, so an
/// artifact that has never been built always compares stale.
pub struct Revisions {
    clock: Tick,

    /// Any document-shaped change: tree structure, layer properties, filter
    /// and void params, canvas geometry, isolation, selection edits,
    /// undo/redo, load. Deliberately coarse — this is what `mark_dirty()`
    /// means.
    document: Tick,

    /// Per-node: the bytes of that node's GPU texture changed. Paint, fill,
    /// paste, mask edit, bake, resize, upload. Bulk pixel data is the
    /// principled GPU-authoritative exception to document authority, so its
    /// revision lives here with the pixels.
    node_pixels: HashMap<LayerId, Tick>,

    /// Maintained maximum of `node_pixels` — not a mirror of it, and written
    /// only by [`Self::bump_node_pixels`]. Lets a consumer that cares about
    /// "any node's pixels" avoid walking the map every frame.
    node_pixels_any: Tick,

    /// A canvas-side animated clock advanced: void tick, canvas-effect tick.
    /// Its own source rather than per-node pixel bumps, because histograms
    /// depend on pixels and must survive an animation frame mid-drag.
    animation: Tick,

    /// A GPU render target was recreated — accumulators, the screen-run pair,
    /// the canvas apply scratch. Compositor-internal identity, invisible to
    /// the document, and deliberately *not* an input to the frame gates: see
    /// [`Self::latest_composite_input`].
    targets: Tick,

    /// Something downstream of the composite changed: view transform, tool
    /// overlay, screen-run resources, screen-side effect clocks, viewport
    /// background, pixel filter.
    present_inputs: Tick,
}

impl Default for Revisions {
    fn default() -> Self {
        Self::new()
    }
}

impl Revisions {
    pub fn new() -> Self {
        Revisions {
            clock: 0,
            document: 0,
            node_pixels: HashMap::new(),
            node_pixels_any: 0,
            animation: 0,
            targets: 0,
            present_inputs: 0,
        }
    }

    /// Advance the clock and return the new value. Every bump goes through
    /// here, which is what makes stamps from different sources comparable.
    fn tick(&mut self) -> Tick {
        self.clock += 1;
        self.clock
    }

    // --- Bumps ---

    pub fn bump_document(&mut self) {
        self.document = self.tick();
    }

    /// Record that one node's pixels changed. Moves the aggregate too, so no
    /// caller has to remember both.
    pub fn bump_node_pixels(&mut self, id: LayerId) {
        let t = self.tick();
        self.node_pixels.insert(id, t);
        self.node_pixels_any = t;
    }

    pub fn bump_animation(&mut self) {
        self.animation = self.tick();
    }

    pub fn bump_targets(&mut self) {
        self.targets = self.tick();
    }

    pub fn bump_present_inputs(&mut self) {
        self.present_inputs = self.tick();
    }

    /// Drop a node's revision when its GPU state is disposed. Consumers
    /// holding a per-node cursor prune ids that stop appearing in
    /// [`Self::node_pixels_iter`]; a later id reuse is safe because its first
    /// bump lands above every stale cursor value.
    pub fn remove_node(&mut self, id: LayerId) {
        self.node_pixels.remove(&id);
    }

    // --- Reads ---

    /// The current clock value. Captured by a consumer *before* doing work,
    /// then stored as that artifact's stamp once the work commits.
    pub fn clock(&self) -> Tick {
        self.clock
    }

    pub fn document(&self) -> Tick {
        self.document
    }

    /// When this node's pixels last changed; 0 if they never have.
    pub fn node_pixels(&self, id: LayerId) -> Tick {
        self.node_pixels.get(&id).copied().unwrap_or(0)
    }

    /// Every node that has ever had a pixel write, with its tick.
    pub fn node_pixels_iter(&self) -> impl Iterator<Item = (LayerId, Tick)> + '_ {
        self.node_pixels.iter().map(|(id, t)| (*id, *t))
    }

    pub fn node_pixels_any(&self) -> Tick {
        self.node_pixels_any
    }

    pub fn animation(&self) -> Tick {
        self.animation
    }

    pub fn targets(&self) -> Tick {
        self.targets
    }

    /// Latest change across every source the composite reads.
    ///
    /// `targets` is excluded on purpose. A target bump alone schedules no
    /// work — it is consumed only when rebuilding effect instances whose bind
    /// groups point at replaced textures, and every out-of-band bump is
    /// already paired with its own scheduling bump. Excluding it is also what
    /// keeps a frame from rescheduling itself forever: `targets` is the one
    /// source that can move *during* a composite, when the walk creates a
    /// group state or the apply scratch.
    pub fn latest_composite_input(&self) -> Tick {
        self.document.max(self.node_pixels_any).max(self.animation)
    }

    /// Latest change across every source a presented frame reflects — the
    /// composite's inputs plus everything downstream of it.
    pub fn latest_visual(&self) -> Tick {
        self.latest_composite_input().max(self.present_inputs)
    }

    /// Bump every source, so the next read of any derived artifact compares
    /// stale. Backs the from-scratch half of the byte-equality tests.
    #[cfg(any(test, feature = "testing"))]
    pub fn bump_all_for_test(&mut self) {
        self.bump_document();
        self.bump_animation();
        self.bump_targets();
        self.bump_present_inputs();
        let t = self.tick();
        for v in self.node_pixels.values_mut() {
            *v = t;
        }
        self.node_pixels_any = t;
    }
}
