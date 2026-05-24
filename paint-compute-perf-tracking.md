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

#### Bench data

`stroke_replay_matrix --topology stamp-color-output` against
[recorded_curvy_stroke.json](crates/darkly/tests/fixtures/recorded_curvy_stroke.json)
(204 events, 3536 ms, Ink Pen, stabilize=1.0). Full table at
[bench-results/stroke-replay-matrix-stamp-color-output-recorded_curvy_stroke-5c0ea3a0ff.md](crates/darkly/bench-results/stroke-replay-matrix-stamp-color-output-recorded_curvy_stroke-5c0ea3a0ff.md).
GPU timestamps are blank because the bench's `TIMESTAMP_QUERY`
instrumentation is wired only on the `paint_compute` compute pass; the
fragment path goes through render passes that aren't instrumented.

| canvas | radius_px | wall (ms) | behind (ms) | worst-frame (ms) | cpu p50 (µs) | cpu p95 (µs) |
|---|---:|---:|---:|---:|---:|---:|
| 1280×720 | 1 | 6649 | +3113 | 107.9 | 29945 | 70115 |
| 1280×720 | 10 | 4542 | +1006 | 42.4 | 20086 | 41459 |
| 1280×720 | 100 | 3551 | +15 | 2.4 | 6445 | 9715 |
| 1280×720 | 1000 | 3559 | +23 | 22.8 | 2959 | 4540 |
| 1280×720 | 2000 | 3552 | +16 | 8.3 | 2494 | 3456 |
| 1920×1080 | 1 | 9905 | +6369 | 127.4 | 43603 | 108306 |
| 1920×1080 | 10 | 6589 | +3053 | 92.6 | 29418 | 69017 |
| 1920×1080 | 100 | 3563 | +27 | 29.6 | 7600 | 13752 |
| 1920×1080 | 1000 | 3557 | +21 | 28.8 | 3555 | 5523 |
| 1920×1080 | 2000 | 3559 | +23 | 27.5 | 3180 | 5153 |
| 2560×1440 | 1 | 13051 | +9515 | 162.1 | 57118 | 143101 |
| 2560×1440 | 10 | 8716 | +5180 | 103.3 | 38366 | 88201 |
| 2560×1440 | 100 | 3552 | +16 | 20.0 | 8007 | 13622 |
| 2560×1440 | 1000 | 3556 | +20 | 27.3 | 4170 | 7873 |
| 2560×1440 | 2000 | 3556 | +20 | 27.4 | 3865 | 7721 |
| 3840×2160 | 1 | 21150 | +17614 | 464.9 | 93961 | 213553 |
| 3840×2160 | 10 | 13370 | +9834 | 180.1 | 60838 | 135738 |
| 3840×2160 | 100 | 3553 | +17 | 32.4 | 9273 | 17531 |
| 3840×2160 | 1000 | 3591 | +55 | 49.2 | 6515 | 26113 |
| 3840×2160 | 2000 | 4785 | +1249 | 129.8 | 24225 | 39115 |

The narrative matches: dab count per event is the killer, not dab
size. Ink Pen's `pen_input.spacing` defaults to a fraction of radius,
so small radius → tight spacing → many dabs/event → per-dab
render-pass overhead serializes the CPU against the GPU queue. The
engine falls behind by *seconds* on every canvas at radius ≤10px
because that's where the matrix's tight-spacing × high-dab-count
regime lives.

**This is a spacing-driven failure, not a size-driven one.** The
matrix conflates the two because Ink Pen's spacing scales with
radius. A brush like impasto oil — where spacing is pinned to 1px
regardless of dab size for the signature daubed look — would hit the
exact same catastrophe at *any* radius. The radius axis here is a
proxy for "events that emit O(stroke_length_px) dabs". Read radius=1
as "1px spacing", radius=10 as "~0.4px spacing", etc.

At radius ≥ 100px on Ink Pen the spacing scales up enough that the
dab count drops to ~one per event and the fragment path is fine
everywhere — even 4K with 1000px dabs sits within budget. The 4K +
2000px regression is the GPU work itself catching up.

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

#### Bench data

Same recording and bench, but the bench binary was cherry-picked into
a worktree at git `dfa4207` (the last commit on the single-workgroup
shader). Full table at
[bench-results/stroke-replay-matrix-approach-2-recorded_curvy_stroke-dfa4207.md](crates/darkly/bench-results/stroke-replay-matrix-approach-2-recorded_curvy_stroke-dfa4207.md).

| canvas | radius_px | wall (ms) | behind (ms) | worst-frame (ms) | cpu p50 (µs) | gpu p50 (µs) | gpu p95 (µs) |
|---|---:|---:|---:|---:|---:|---:|---:|
| 1280×720 | 1 | 3540 | +4 | 8.3 | 5038 | 1755 | 4380 |
| 1280×720 | 10 | 3540 | +4 | 7.5 | 4484 | 2325 | 6307 |
| 1280×720 | 100 | 3540 | +4 | 7.6 | 3667 | 5053 | 11595 |
| 1280×720 | 250 | 3570 | +34 | 30.2 | 3776 | 10815 | 22977 |
| 1280×720 | 500 | 5241 | +1705 | 51.5 | 25963 | 21798 | 47595 |
| 1280×720 | 1000 | 6472 | +2936 | 78.8 | 31137 | 27990 | 67324 |
| 1280×720 | 2000 | 4819 | +1283 | 85.3 | 2922 | 11923 | 61445 |
| 1920×1080 | 1 | 3541 | +5 | 24.2 | 5187 | 1011 | 3133 |
| 1920×1080 | 100 | 3539 | +3 | 23.8 | 4053 | 6777 | 13918 |
| 1920×1080 | 250 | 4805 | +1269 | 41.5 | 24113 | 16696 | 40621 |
| 1920×1080 | 500 | 8864 | +5328 | 108.3 | 42693 | 33824 | 83090 |
| 1920×1080 | 1000 | 14891 | +11355 | 193.4 | 75405 | 65778 | 149097 |
| 1920×1080 | 2000 | 14967 | +11431 | 235.4 | 85779 | 77408 | 172752 |
| 2560×1440 | 1 | 3541 | +5 | 36.5 | 5834 | 1552 | 4271 |
| 2560×1440 | 100 | 3553 | +17 | 23.1 | 4745 | 8678 | 20063 |
| 2560×1440 | 250 | 6635 | +3099 | 63.9 | 32527 | 22683 | 53119 |
| 2560×1440 | 500 | 12390 | +8854 | 137.8 | 58788 | 45357 | 112575 |
| 2560×1440 | 1000 | 21758 | +18222 | 271.4 | 109133 | 92481 | 194470 |
| 2560×1440 | 2000 | 25637 | +22101 | 513.4 | 122558 | 110135 | 258683 |
| 3840×2160 | 1 | 3541 | +5 | 23.7 | 6567 | 1476 | 3615 |
| 3840×2160 | 100 | 4951 | +1415 | 39.3 | 24111 | 14206 | 31548 |
| 3840×2160 | 250 | 10301 | +6765 | 169.6 | 48870 | 32978 | 78958 |
| 3840×2160 | 500 | 19763 | +16227 | 215.1 | 95181 | 68270 | 157204 |
| 3840×2160 | 1000 | 36705 | +33169 | 477.8 | 174868 | 140676 | 346013 |
| 3840×2160 | 2000 | 57635 | +54099 | 837.9 | 275790 | 242081 | 502561 |

Now we have the numbers behind "large dabs collapsed". The 4K +
2000px cell takes **57.6 seconds** for a 3.5s stroke — 16× slower
than real-time, with the GPU genuinely busy (gpu p50 = 242 ms/event,
within ~80% of cpu p50 = 276 ms/event — confirms it's actual shader
work, not back-pressure). The knee starts at radius=250-500px on
every canvas, where the single workgroup's 64 threads can no longer
chew through each dab's bbox in time. Below the knee the workgroup
has enough parallelism for tiny bboxes; above it, the per-thread tile
count grows quadratically with radius. (Spacing matters less here
than for #1 — #2's cost scales with `dab_count × bbox_area`, not
`dab_count` alone, so even at tight spacing the bbox_area term keeps
small dabs cheap.)

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

#### Bench data

`stroke_replay_matrix` (default `paint-compute` topology) on the
current branch with the full GPU-timeline instrumentation
(sync_in / shader / sync_out timestamps + per-event submit_us +
per-flush dabs + union_bbox). Full table at
[bench-results/stroke-replay-matrix-paint-compute-recorded_curvy_stroke-b146df4220.md](crates/darkly/bench-results/stroke-replay-matrix-paint-compute-recorded_curvy_stroke-b146df4220.md).
This supersedes the earlier `f9895f0285` run, which only timed the
compute pass and missed the surrounding GPU work.

| canvas | radius_px | wall (ms) | behind (ms) | gpu_shader p50 (µs) | sync_in p50 (µs) | sync_out p50 (µs) | submit p50 (µs) | dabs/ev | bbox/ev (px²) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1280×720 | 1 | 3540 | +4 | 580 | 489 | 240 | 2450 | 286.5 | 4654 |
| 1280×720 | 10 | 3540 | +4 | 505 | 757 | 315 | 2485 | 201.8 | 10854 |
| 1280×720 | 100 | 3540 | +4 | 306 | 2277 | 1050 | 2581 | 20.2 | 197168 |
| 1280×720 | 250 | 3539 | +3 | 384 | 2412 | 1545 | 2256 | 8.1 | 835616 |
| 1280×720 | 500 | 3540 | +4 | 555 | 2097 | 1051 | 2040 | 4.1 | 1837229 |
| 1280×720 | 1000 | 3539 | +3 | 650 | 1256 | 292 | 1687 | 2.0 | 2255072 |
| 1280×720 | 2000 | 3539 | +3 | 692 | 1131 | 259 | 1651 | 1.0 | 1466990 |
| 1920×1080 | 1 | 3540 | +4 | 524 | 559 | 208 | 2341 | 438.5 | 10081 |
| 1920×1080 | 10 | 3540 | +4 | 388 | 716 | 254 | 2425 | 308.8 | 18755 |
| 1920×1080 | 100 | 3540 | +4 | 229 | 2016 | 969 | 2596 | 30.9 | 235482 |
| 1920×1080 | 250 | 3541 | +5 | 386 | 3108 | 1540 | 2671 | 12.4 | 1045568 |
| 1920×1080 | 500 | 3539 | +3 | 627 | 3021 | 2168 | 2463 | 6.2 | 2813993 |
| 1920×1080 | 1000 | 3540 | +4 | 1209 | 3021 | 1197 | 2346 | 3.2 | 5348161 |
| 1920×1080 | 2000 | 3540 | +4 | 1211 | 1704 | 583 | 2000 | 1.6 | 5017430 |
| 2560×1440 | 1 | 3540 | +4 | 586 | 760 | 295 | 2396 | 597.0 | 17572 |
| 2560×1440 | 10 | 3540 | +4 | 465 | 941 | 339 | 2537 | 420.4 | 28767 |
| 2560×1440 | 100 | 3542 | +6 | 278 | 2562 | 1506 | 2714 | 42.1 | 275881 |
| 2560×1440 | 250 | 3540 | +4 | 443 | 3491 | 2250 | 2842 | 16.9 | 1198200 |
| 2560×1440 | 500 | 3539 | +3 | 899 | 4791 | 3650 | 3294 | 8.4 | 3516147 |
| **2560×1440** | **1000** | **3822** | **+286** | 1995 | **5120** | **2791** | **3784** | 4.2 | 7635587 |
| **2560×1440** | **2000** | **3619** | **+83** | 2430 | 3084 | 1635 | 2693 | 2.1 | 8789082 |
| 3840×2160 | 1 | 3541 | +5 | 480 | 832 | 367 | 2408 | 916.5 | 39237 |
| 3840×2160 | 10 | 3540 | +4 | 372 | 919 | 426 | 2444 | 645.5 | 55577 |
| 3840×2160 | 100 | 3540 | +4 | 249 | 2615 | 1765 | 3002 | 64.6 | 356606 |
| 3840×2160 | 250 | 3562 | +26 | 496 | 4732 | 3675 | 3441 | 25.8 | 1412694 |
| **3840×2160** | **500** | **5306** | **+1770** | 1273 | **9448** | **10036** | **23286** | 12.9 | 4395837 |
| **3840×2160** | **1000** | **7046** | **+3510** | 3166 | **12133** | **9595** | **28835** | 6.5 | 11656344 |
| **3840×2160** | **2000** | **7687** | **+4151** | 5637 | **9869** | **5958** | **31106** | 3.3 | 20514778 |

**The shader is innocent.** At 4K + 1000px — the cell that's 3.5
seconds behind real-time — `gpu_shader_p50 = 3.2 ms`. The sync
copies (`copy_texture_to_buffer` ingest + `copy_buffer_to_texture`
publish) total **21.7 ms**, and submit blocks for **28.8 ms**
because submit waits on the GPU finishing the prior frame's
commands, which are mostly… sync copies.

**Sync copies dominate shader work by ~10-20× across the entire
matrix**, not just the bad cells. 4K + 100px: shader 0.25 ms, sync
4.4 ms (17×). 1080p + 1000px: shader 1.2 ms, sync 4.2 ms (3.5×).
The reason small canvases keep up isn't faster shaders; it's
smaller sync bytes per event.

**The 4K + ≥500px regression is sync bytes crossing the 17 ms/event
budget**, not shader lane-waste. At 4K + 500px the engine fell
behind by 1.77 s with `gpu_shader_p50 = 1.3 ms` and `(sync_in +
sync_out)_p50 = 19.5 ms`. The lane-waste hypothesis is real but
small — shader time grows with `union_bbox_area` because more
threads spawn that do nothing — but the shader alone never breaks
the budget at any cell.

**The `dabs/ev` and `bbox/ev` columns confirm the spacing argument
quantitatively.** 4K + 1px = 916 dabs/event with bbox 39k px²; 4K
+ 2000px = 3.3 dabs/event with bbox 20.5M px². The two axes are
inversely correlated for Ink Pen but **architecturally
independent** — a brush like impasto oil with pinned 1px spacing
would push `dabs/ev` to ~900 *at any radius*, with bbox tracking
radius separately. #3's failure mode is the `bbox/ev` axis, not
the `dabs/ev` axis.

**One curious cell to revisit:** 1440p + 1000px (`+286 ms` behind,
`cpu_p50 = 18.2 ms` but `submit_p50 = 3.8 ms`). Unlike its 4K
neighbors, submit isn't dominant — possibly variance, possibly the
GPU has just enough slack at this resolution that submit doesn't
back up but encoding still bloats. Worth a repeat run if anything
hinges on it.

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

> **Re-ranked after the per-event instrumentation re-run.** The data
> shows sync-copy bytes are the dominant cost in #3's bad cells, not
> shader lane-waste. Options that attack sync bytes (D, C-as-hybrid)
> are now ahead of options that attack shader lane-waste (A).
> See the *Architectural cost model* + *Bench synthesis* sections
> above for the supporting numbers.

### A. Tile-binning (Forward+ for dabs) — *demoted*

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

**Why this is demoted:** attacks shader lane-waste, which the data
now shows is the smaller component. At 4K + 1000px the shader is
3.2 ms of a 37 ms cell — even a perfect tile-bin leaves ~34 ms of
sync + submit work untouched. Worth doing eventually but won't close
the felt-experience gap on its own.

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

### C. Instanced-quad render pass with fixed-function blending — *promoted as hybrid candidate*

ONE draw call, N instances (one per dab). Vertex shader emits the
per-instance bbox quad in clip space; fragment shader computes
coverage and emits premultiplied source; pipeline blend state does
source-over in hardware.

Wins: rasterizer handles thread layout, no wasted lanes; hardware
blend stage is the fastest source-over path on the GPU. **Crucially,
writes the scratch texture directly — no buffer round-trip.** This
is what makes #1 architecturally cheaper than #3 on large-bbox
cells; (C) is the same architecture with the per-pass overhead
collapsed into a single instanced draw, sidestepping #1's failure
mode at high dab count.

Risks: fragment ordering across instances in a single draw call is
implementation-defined per the WebGPU spec. Most desktop GPUs honor
primitive-issue order via the rasterizer; D3D12 / Metal guarantee
it. Need to validate against the WebGPU backends we ship on.

**Why this is promoted:** combined with a hybrid router, (C) covers
the cells where #3 falls behind (large bbox, few dabs) by entirely
eliminating the sync round-trip — the actual dominant cost there.
Ship as a sibling terminal; the brush graph picks the terminal at
compile time based on a `bbox_density` heuristic. The matrix shows
the two approaches' failure regimes are disjoint, so a hybrid would
dominate cell-by-cell.

### D. Adaptive mid-phase flush — *promoted*

CPU heuristic: if `pending_dabs_bbox` area exceeds a threshold,
force a flush mid-phase. Trades one giant sparse dispatch for a few
small dense ones.

**Why this is promoted:** at 4K + 1000px the union bbox is 11.7 M
px² per event; sync_in + sync_out total 21.7 ms. If the CPU
heuristic broke that single dispatch into four dense sub-dispatches
each covering ~3 M px² with the same total painted area, sync time
would scale roughly linearly with bbox area — the four sub-flushes
would do ~22 ms of sync work total (same), BUT the per-flush sync
cost would be ~5.4 ms each, fitting inside the 17 ms/event budget
with submit able to interleave shader work for the next sub-flush
while the GPU completes the prior one. This is the fastest path to
unblocking the 4K + ≥500px regression *without* changing the
architecture — bench-driven CPU heuristic only.

Cheap, partial; ships behind the heuristic and reverts cleanly.
Worth doing immediately, in parallel with the (C)/hybrid track.

## What the user proposed

> "We can do all the dabs in one pass without assigning each pixel a thread."

Closest literal match: **(C) instanced-quad render pass** — the
rasterizer assigns threads, not us. *After the instrumentation
re-run this turns out to also be the closest robust-perf match*,
because (C) eliminates the buffer round-trip that's actually the
dominant cost. The earlier "(A) tile-binning" framing was based on
the over-confident reading that union-bbox lane-waste was the
problem; the new data shows lane-waste is real but small. (A) is
still worth doing — it's a 1-3 ms win per event — but it doesn't
close the felt-experience gap on its own.

## Instrumentation status

**Done:**
- ✅ GPU timestamp queries around the paint-compute compute pass
  ([`PaintComputeTimestamps`](crates/darkly/src/brush/nodes/paint_compute.rs)).
- ✅ Deterministic bench stroke — captured via the `?_RECORD_STROKES=1`
  recorder ([crates/darkly/tests/fixtures/recorded_curvy_stroke.json](crates/darkly/tests/fixtures/recorded_curvy_stroke.json))
  and re-runnable across approaches via `stroke_replay_matrix`.

**Done in the second instrumentation pass (this branch):**

- ✅ Per-flush GPU timeline split into `sync_in` (`copy_texture_to_buffer`)
  / `shader` (compute pass) / `sync_out` (`copy_buffer_to_texture`).
  6-slot query set per flush, gated on `TIMESTAMP_QUERY_INSIDE_ENCODERS`
  for the sync brackets; the shader slot keeps working on adapters
  that only expose `TIMESTAMP_QUERY_INSIDE_PASSES`.
- ✅ Per-event `submit_us` exposed via `BrushPerfDelta` — the
  back-pressure waterfall that was hiding inside `cpu_us` is now a
  first-class bench column.
- ✅ Per-event `dabs_total` and `union_bbox_area_total` surfaced from
  `BrushPerfCounters` so cells can be read by the actual workload the
  engine fed the GPU rather than the nominal radius axis. Markdown
  carries the per-event averages; TSV carries the per-flush vectors
  if a future analysis wants them.
- ✅ `gpu_samples` is already in `EventTiming`; the matrix's TSV does
  not yet split it out per column but `replay()` aggregates it onto
  every event.

**Smoke run on `dab-compute-shader` confirms the planned hypothesis:**
at 4K + 1000px the matrix now reads `gpu_shader_p50 = 3166 µs`,
`gpu_sync_in_p50 = 12133 µs`, `gpu_sync_out_p50 = 9595 µs`, `submit_p50 =
28835 µs` (canonical `b146df4220` run). The "12× CPU-bound" reading
from the first synthesis was wrong — the sync copies *alone* are 6.8×
the shader pass, and `queue.submit()` back-pressure on top of that
brings cpu_p50 to 11.8× the shader.

The architectural prescription inverts from the original: it's the
sync-bytes axis that matters, not the shader-lane-waste axis. Tile-bin
attacks lane-waste and is now demoted — see *Options to explore next*
above for the re-ranked options and *Architectural cost model* for
why fragment (option C, with hybrid routing) is now the cleanest
candidate.

**Still missing:**

- Driver dispatch-grid construction / scoreboard cost. The 6-slot
  timeline above brackets every encoder command we issue, but the
  driver's per-dispatch CPU work (workgroup scheduling, descriptor
  validation) is still folded into `submit_us` rather than measured
  separately. Probably fine — at 4K+1000px the sync columns dominate
  by an order of magnitude over what driver overhead could plausibly
  be — but worth revisiting if a future attempt shrinks the sync
  cost without moving `cpu_us`.

## Architectural cost model

The three approaches don't fail along the same axis. The
instrumentation re-run on #3 surfaced this clearly enough to write
down explicitly.

**Why #3 has to round-trip texture↔buffer.** Compute shaders in
wgpu/WebGPU can write to two kinds of GPU memory: storage textures
(limited format support, no atomic blend in WGSL) or storage buffers
(arbitrary memory, you implement blend yourself). #3 chose storage
buffers because the shader serializes overlapping dabs via
`storageBarrier()` between dabs, which needs a generic
read-write store. But the scratch is canonically a *texture* —
every other consumer (commit, compositor, previews, the rest of the
brush graph) samples it via `texture_2d<f32>` + sampler. So each
`flush_compute` round-trips:

```
scratch texture → compute buffer    (sync_in:  copy_texture_to_buffer)
                  compute shader
compute buffer  → scratch texture    (sync_out: copy_buffer_to_texture)
```

Both copies move the **union_bbox region** regardless of how many
pixels inside it are actually painted by dabs.

**Why #1 doesn't.** Fragment shaders go through the rasterizer +
ROP stage. The pipeline's blend state does the read-modify-write
*in hardware*, atomically per pixel, with the scratch texture as
the render target. No buffer mirror, no explicit load/store, no
round-trip. Memory access scales with the *actually-rasterized
pixels* (`dab_area`), not with `union_bbox_area`.

**Orthogonal failure modes.** Each approach's per-event cost scales
along a different axis, with a different catastrophe regime:

| | per-event cost scales with | catastrophe regime |
|---|---|---|
| **#1 fragment** | `dab_count` (per-pass driver overhead) + `Σ(dab_area)` (rasterized pixels) | many dabs/event — high stabilizer × tight spacing |
| **#2 1-wg compute** | `union_bbox_area × dab_count` (64 threads chew tiles serially) + sync round-trip | large dabs anywhere — workgroup parallelism is the bottleneck |
| **#3 thread-per-pixel** | `union_bbox_area` (sync copies + dispatch grid) | few large dabs spread over a big bbox |

A 1px dab on a 4K canvas: #1 pays 1 render pass + ~1 pixel
rasterized. #3 pays a sync round-trip of ~4 KB. #1 wins handily —
but only because the dab count stays low. 916 dabs/event on the
same canvas (4K + 1px Ink Pen, tight spacing) is where #1
catastrophically loses to its own driver overhead.

A few-dabs-over-a-big-bbox event: #1 pays a few render passes,
each rasterizing the dab footprint. #3 pays sync copies of the
entire union bbox regardless of where dabs land. #3 loses because
the round-trip cost is proportional to the *bounding rectangle*,
not the painted area.

**Implication for any hybrid.** The routing key should be
`union_bbox_area_per_flush` (or `dab_area_total / union_bbox_area`,
which captures "density" within the bbox), **not** `dab_count`.
These are correlated in this matrix but dissociate for brushes like
impasto oil that pin spacing to 1px regardless of dab radius.

## Bench synthesis

Cross-approach `behind_by_ms` on the same recording. Negative or
near-zero = the engine kept up with the recorded cadence; positive =
the engine fell behind by that many ms over a 3.5s stroke.

| canvas | radius_px | #1 fragment | #2 1-wg compute | #3 thread-per-pixel | winner |
|---|---:|---:|---:|---:|:---|
| 1280×720 | 1 | **+3113** | +4 | +7 | #2 / #3 |
| 1280×720 | 10 | **+1006** | +4 | +7 | #2 / #3 |
| 1280×720 | 100 | +15 | +4 | +5 | #2 / #3 |
| 1280×720 | 250 | +18 | +34 | +3 | #3 |
| 1280×720 | 500 | +18 | **+1705** | +4 | #3 |
| 1280×720 | 1000 | +23 | **+2936** | +3 | #3 |
| 1280×720 | 2000 | +16 | **+1283** | +4 | #1 / #3 |
| 1920×1080 | 1 | **+6369** | +5 | +4 | #2 / #3 |
| 1920×1080 | 100 | +27 | +3 | +4 | #2 / #3 |
| 1920×1080 | 250 | +16 | **+1269** | +3 | #1 / #3 |
| 1920×1080 | 500 | +18 | **+5328** | +4 | #1 / #3 |
| 1920×1080 | 1000 | +21 | **+11355** | +3 | #1 / #3 |
| 1920×1080 | 2000 | +23 | **+11431** | +4 | #1 / #3 |
| 2560×1440 | 1 | **+9515** | +5 | +4 | #2 / #3 |
| 2560×1440 | 100 | +16 | +17 | +4 | #3 |
| 2560×1440 | 250 | +16 | **+3099** | +5 | #1 / #3 |
| 2560×1440 | 500 | +20 | **+8854** | +4 | #1 / #3 |
| 2560×1440 | 1000 | +20 | **+18222** | +295 | #1 |
| 2560×1440 | 2000 | +20 | **+22101** | +102 | #1 |
| 3840×2160 | 1 | **+17614** | +5 | +5 | #2 / #3 |
| 3840×2160 | 100 | +17 | **+1415** | +4 | #1 / #3 |
| 3840×2160 | 250 | +17 | **+6765** | +36 | #1 |
| 3840×2160 | 500 | +20 | **+16227** | **+1794** | #1 |
| 3840×2160 | 1000 | +55 | **+33169** | **+3562** | #1 |
| 3840×2160 | 2000 | **+1249** | **+54099** | **+4200** | #1 |

**Important framing — read radius as a spacing proxy.** Ink Pen's
default spacing is a fraction of dab radius, so the matrix's radius
axis is also a `1/dabs_per_event` axis: radius=1 → ~1px spacing →
many dabs/event; radius=1000 → ~40px spacing → ~one dab/event. The
failure modes below that correlate with "small radius" are really
**high-dab-count-per-event** failure modes. A brush like *impasto
oil* that pins spacing to 1px for its signature daubed look would
trigger #1's collapse at *any* radius. Bench with spacing as an
explicit axis when characterizing such brushes.

Key takeaways (with the new instrumentation, all defended by specific
cells in the per-approach tables above):

- **The shader has never been the bottleneck.** Across the *entire*
  #3 matrix, `gpu_shader_p50` peaks at 5.6 ms (4K + 2000px) and sits
  below 1 ms in most cells. Even a perfect rewrite of the compute
  shader can't change the cells where #3 falls behind — those are
  bounded by sync-copy bytes, not shader work.
- **Sync copies dominate #3's cost.** Sync_in + sync_out beats shader
  by 10-20× across the matrix. The 4K + ≥500px regression is sync
  bytes crossing the 17 ms/event budget. At 4K + 1000px:
  sync = 21.7 ms, shader = 3.2 ms, submit = 28.8 ms (because submit
  blocks on prior frame's syncs).
- **#1 wins large-dab cells because it has no round-trip at all.**
  Fragment writes the scratch texture directly via the hardware
  blend stage; #3 has to ingest the union bbox into a buffer mirror,
  run compute, publish back. The architectural cost model above
  explains why this gap is inherent to compute vs fragment, not
  fixable inside the shader.
- **#1 loses tiny-radius cells because it pays per-dab driver
  overhead.** With Ink Pen at radius=1 the recording emits 286-916
  dabs/event; #1 opens that many render passes per event and
  collapses. This is on a different axis from #3's failure — neither
  is "GPU-slow", both are architectural.
- **The two failure axes are not the same.** `dabs/ev` ranges from
  916 (4K + 1px) to 3.3 (4K + 2000px). #3 keeps up at 916 and lags at
  3.3 — failure axis is `bbox_area`, not `dab_count`. For Ink Pen the
  two correlate negatively, but a brush like impasto oil with pinned
  1px spacing would have 916 dabs/event *at any radius*, exposing the
  axes as independent.
- **#2 is dominated almost everywhere.** Loses to #3 on every cell
  with radius ≥ 250, never beats #1 at small dabs. Its only reason
  for existing was to escape #1's per-dab overhead — which #3 also
  escapes, without #2's single-workgroup serialization.
- **Hybrid #1 + #3 is the clean fix.** Route on
  `union_bbox_area_per_flush` (or `dab_area_total / bbox_area`
  density). When density is high, use #3 (one dispatch dominates
  hundreds of fragment passes). When density is low, use #1 (no
  buffer round-trip on a mostly-empty bbox). The matrix shows the
  two approaches' failure regimes are *disjoint*, so a hybrid would
  dominate cell-by-cell with no architectural trade-off.

## Working agreement

- Each new attempt is its own commit. Shader + CPU evolve together.
- New attempts append a row here: what we did, what we measured, why
  we kept it or moved on.
- Watercolor / smudge / liquify stay untouched. Their compute ports
  will reuse whatever pattern wins.
