# Stabilization performance — `paint_compute` attempts

Branch: `dab-compute-shader`. Running log for the stabilization
performance problem: at high stabilization the stroke engine emits
many dabs per pen event, and the per-event GPU cost balloons. The
end-to-end behavior the user sees is **stroke lag** — frames stall,
the stroke trails the pen, the editor stops feeling responsive.

Predecessor doc: [`darkly-stabilization-perf-investigation.md`](darkly-stabilization-perf-investigation.md).
Originating plan: `~/.claude/plans/paint-compute-perf-fix.md`.

## Problem

`paint_compute` drives every Basic brush (Round, Airbrush, Ink Pen).
Stabilization spreads ~30+ dabs across each pen event. Each dab is
small individually but they accumulate fast — the engine has to land
all of them in the scratch before the next event's commit, on every
frame. **Where** the cost sits has shifted across attempts; the
underlying constraint hasn't.

Current felt behavior on `dab-compute-shader` (post-attempt #3):
small dabs / small canvases — smooth. 4K canvas + medium dab size —
not smooth. Long fast strokes — still trails the pen.

## Attempts

### #1 — Fragment-path `color_output`

**Shape:** one render pass per dab. Stamp draws into a pool entry;
`color_output` composites that pool entry over the scratch via the
fragment pipeline + fixed-function blend.

**Why we moved off it:** per-dab driver overhead. With stabilization
on, ~30 dabs/event × per-pass setup cost dominated the frame. The
underlying GPU work was tiny; we were paying for `begin_render_pass`
and bind-group binding once per dab. Large dabs fine, small dabs collapse because of higher dab count.

### #2 — Compute terminal, single workgroup serial tile-walk (v1 `paint_compute`)

**Shape:** ONE compute dispatch per phase. One workgroup of 64
threads. The shader's outer loop iterates the queued dab list; for
each dab it tile-walks that dab's bbox in 8×8 chunks; each of the 64
threads handles one pixel per tile. `storageBarrier()` between dabs.

**What it bought:** eliminated the per-dab render pass overhead from
(#1). A 30-dab event becomes one dispatch.

**Why we moved off it:** the workgroup is fixed at 64 threads. For a
large dab (256×256 ≈ 65K pixels = ~1K tiles), those 64 threads grind
through the tiles serially while the rest of the GPU sits idle. Small
dabs fine; large dabs collapsed.

### #3 — Compute terminal, thread-per-pixel iterate-dabs (this branch)

**Shape:** one dispatch per phase, grid = `ceil(union_bbox / 8)`.
Each thread owns one pixel in the union bbox and walks the queued dab
list serially in registers. One scratch load on entry, one store on
exit (suppressed when no dab contributed). Selection sampled once per
thread.

**Files:** [`shaders/brush/paint_compute.wgsl`](shaders/brush/paint_compute.wgsl),
[`crates/darkly/src/brush/nodes/paint_compute.rs`](crates/darkly/src/brush/nodes/paint_compute.rs).

**What it bought:** large dabs at moderate canvases. Per-thread loop
is tight; AABB reject is cheap; selection early-out skips dead lanes.

**Why it isn't enough:** the dispatch grid is the union bbox. On a
4K canvas a long fast stroke makes that bbox huge (~500×500+) but
sparse — most threads in the rectangle never get hit by any dab,
they just chew through 30 AABB rejects per pixel and return. Lane
waste dominates again, just for a different reason than #2.

## Background changes that are NOT competing attempts

These landed for different reasons over the same time window. Listed
so we don't accidentally re-litigate them — they're orthogonal to
the per-event compute structure.

- **Premultiplied scratch + fixed-function blend** — eliminated a
  per-dab `copy_texture_to_texture` mirror copy in the fragment path.
  Helped (#1); irrelevant to (#2)/(#3).
- **Deferred composite batching** — collapsed N per-dab fragment
  passes into one pass with N draws. Optimization on (#1); obsoleted
  by (#2).
- **WASM-bridge stroke coalescing** — collapses consecutive
  `BrushStroke` events in a single drain. Reduces *how many* events
  hit the engine, doesn't change per-event cost. Still in effect.

## Options to explore next

### A. Tile-binning (Forward+ for dabs)

CPU bins each queued dab's bbox into 64×64 (or 32×32) tile
coordinates. GPU dispatches one workgroup **per non-empty tile** —
dispatch scales with the *actually painted* area, not the union
bbox. Each workgroup reads its tile's dab-id slice and runs
thread-per-pixel iterate-dabs **only over those dabs**.

Wins: kills the wasted-reject problem from (#3) directly. No
cross-workgroup ordering hazard (different tiles ↔ different
pixels). Per-thread inner loop shrinks (3–5 relevant dabs vs ~30).

Costs: CPU bin construction (dab count × tiles-per-dab), two extra
storage buffers (`dab_ids[]`, `tile_offsets[]`), one extra pass to
build them. Well-trodden in tiled lighting renderers.

### B. Per-dab workgroup — the "one pass without per-pixel threads" shape

ONE dispatch with N workgroups (N = dab count). Each workgroup
tile-walks its own dab's bbox; threads inside are per-pixel within
that bbox.

**Fatal as-stated:** WebGPU does not order workgroups within a
dispatch. Overlapping dabs race on scratch.

Salvage paths:
- **B.1 — one dispatch per dab.** Restores ordering via implicit
  pass-to-pass sync. Brings back the per-dispatch overhead we paid
  (#2) to eliminate. Probably worse than (#3).
- **B.2 — non-overlap groups.** CPU sorts dabs into groups where no
  two members overlap; dispatch one group at a time, workgroup-per-dab
  inside. Helps when dabs *don't* overlap much (fast strokes); does
  nothing when they do (slow / dense strokes). Strictly weaker than (A)
  in the dense case.
- **B.3 — per-pixel atomic ordering.** Atomic compare-exchange against
  the highest-id dab that has touched this pixel. Almost certainly a
  loss vs even (#3).

This is the shape the user proposed. The defensible variant is B.2;
(A) covers the same intuition more robustly.

### C. Instanced-quad render pass with fixed-function blending

ONE draw call, N instances (one per dab). Vertex shader emits the
per-instance bbox quad in clip space; fragment shader computes
coverage and emits premultiplied source; pipeline blend state does
source-over in hardware.

Wins: rasterizer handles thread layout, no wasted lanes; hardware
blend stage is the fastest source-over path on the GPU.

Risks: fragment ordering across instances in a single draw call is
implementation-defined per the WebGPU spec. Most desktop GPUs honor
primitive-issue order via the rasterizer; D3D12 / Metal guarantee
it. Need to validate against the WebGPU backends we ship on.

Could ship as a sibling terminal so we can A/B against (#3) honestly.

### D. Adaptive mid-phase flush

CPU heuristic: if `pending_dabs_bbox` area exceeds a threshold,
force a flush mid-phase. Trades one giant sparse dispatch for a few
small dense ones. Cheap, partial; ships behind the heuristic and
reverts cleanly. Worth doing in parallel with the real fix.

## What the user proposed

> "We can do all the dabs in one pass without assigning each pixel a thread."

Closest literal match: **(C) instanced-quad render pass** — the
rasterizer assigns threads, not us. Closest robust-perf match: **(A)
tile-binning** — pixels still get threads, but only inside tiles
that any dab actually touches, not the whole union bbox.

## Open instrumentation gaps

We do not know what is slow in the 4K-medium-dab case.
`BrushPerfCounters::compute_dispatch_us` measures CPU wall-clock
around the dispatch; it doesn't isolate GPU shader time from
encoder / submission cost.

Before iterating further:
- GPU timestamp queries around the compute pass.
- Log `union_bbox` size and `dab_count` per flush.
- A deterministic 4K bench stroke (fixed length, fixed dab count,
  fixed stabilization) so re-runs are comparable.

Without this any "improvement" we measure is anecdotal.

## Working agreement

- Each new attempt is its own commit. Shader + CPU evolve together.
- New attempts append a row here: what we did, what we measured, why
  we kept it or moved on.
- Watercolor / smudge / liquify stay untouched. Their compute ports
  will reuse whatever pattern wins.
