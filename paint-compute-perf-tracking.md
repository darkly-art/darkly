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
current branch. Full table at
[bench-results/stroke-replay-matrix-recorded_curvy_stroke-f9895f0285.md](crates/darkly/bench-results/stroke-replay-matrix-recorded_curvy_stroke-f9895f0285.md).

| canvas | radius_px | wall (ms) | behind (ms) | worst-frame (ms) | cpu p50 (µs) | gpu p50 (µs) | gpu p95 (µs) |
|---|---:|---:|---:|---:|---:|---:|---:|
| 1280×720 | 1 | 3543 | +7 | 8.5 | 4977 | 588 | 1228 |
| 1280×720 | 10 | 3543 | +7 | 8.0 | 4919 | 470 | 1214 |
| 1280×720 | 100 | 3541 | +5 | 8.4 | 4098 | 299 | 630 |
| 1280×720 | 250 | 3539 | +3 | 29.4 | 3701 | 353 | 951 |
| 1280×720 | 500 | 3540 | +4 | 29.6 | 3351 | 507 | 1306 |
| 1280×720 | 1000 | 3539 | +3 | 19.9 | 3123 | 868 | 2272 |
| 1280×720 | 2000 | 3540 | +4 | 14.9 | 2614 | 693 | 1730 |
| 1920×1080 | 1 | 3540 | +4 | 42.4 | 5294 | 401 | 1269 |
| 1920×1080 | 100 | 3540 | +4 | 32.4 | 3938 | 189 | 547 |
| 1920×1080 | 500 | 3540 | +4 | 29.8 | 4137 | 626 | 1325 |
| 1920×1080 | 1000 | 3539 | +3 | 57.5 | 3952 | 1241 | 2611 |
| 1920×1080 | 2000 | 3540 | +4 | 37.7 | 3402 | 1244 | 2885 |
| 2560×1440 | 1 | 3540 | +4 | 24.3 | 5771 | 450 | 1622 |
| 2560×1440 | 100 | 3540 | +4 | 21.2 | 4632 | 309 | 963 |
| 2560×1440 | 500 | 3540 | +4 | 34.3 | 4986 | 885 | 1925 |
| 2560×1440 | 1000 | 3831 | +295 | 26.2 | 18961 | 2034 | 4591 |
| 2560×1440 | 2000 | 3638 | +102 | 24.5 | 5292 | 2426 | 5570 |
| 3840×2160 | 1 | 3541 | +5 | 28.8 | 7070 | 493 | 2785 |
| 3840×2160 | 100 | 3540 | +4 | 32.2 | 4909 | 253 | 835 |
| 3840×2160 | 250 | 3572 | +36 | 49.1 | 5786 | 491 | 1345 |
| 3840×2160 | 500 | 5330 | +1794 | 101.6 | 29760 | 1259 | 2383 |
| 3840×2160 | 1000 | 7098 | +3562 | 70.3 | 37271 | 3088 | 5635 |
| 3840×2160 | 2000 | 7736 | +4200 | 143.0 | 37768 | 5747 | 11200 |

Narrative confirmed: ≤1440p stays within ~100ms of real-time across
the whole radius axis. The 4K + ≥500px corner is where #3 falls
behind, but the GPU times reveal that this is *not* a shader-work
problem — `gpu p50 = 5.7 ms` at the worst cell while `cpu p50 = 37.8
ms`. The ratio is the submit/back-pressure waterfall: the engine's
queue saturates and `queue.submit` blocks, which inflates `cpu_us`
~6× over actual GPU work.

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
at 4K + 1000px the matrix now reads `gpu_shader_p50 = 3033 µs`,
`gpu_sync_in_p50 = 12133 µs`, `gpu_sync_out_p50 = 9089 µs`, `submit_p50 =
25416 µs`. The "12× CPU-bound" reading from the first synthesis was
wrong — the sync copies *alone* are 6× the shader pass, and
`queue.submit()` back-pressure on top of that is 8×. The architectural
prescription stands: changes that shrink `union_bbox_area` (tile-bin,
adaptive flush) help regardless of GPU speed because the hidden sync
cost scales with that area, not painted area.

**Still missing:**

- Driver dispatch-grid construction / scoreboard cost. The 6-slot
  timeline above brackets every encoder command we issue, but the
  driver's per-dispatch CPU work (workgroup scheduling, descriptor
  validation) is still folded into `submit_us` rather than measured
  separately. Probably fine — at 4K+1000px the sync columns dominate
  by an order of magnitude over what driver overhead could plausibly
  be — but worth revisiting if a future attempt shrinks the sync
  cost without moving `cpu_us`.

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

Key takeaways:

- **No single approach wins everywhere.** #1 wins large dabs at high
  resolution; #3 wins everywhere dab count per event is high; #2 only
  wins the small-canvas + few-dabs + small-dab corner.
- **#1 ↔ #3 crossover correlates with dab count per event, not dab
  size.** In this matrix the crossover sits between Ink Pen radius 100
  and 250 because spacing scales with radius. For a fixed-1px-spacing
  brush, expect #1 to lose at every cell where the recording emits
  >10-20 dabs per event regardless of radius — i.e., everywhere
  except very short / very slow strokes.
- **#2 is dominated almost everywhere.** It loses to #3 on every cell
  with radius ≥ 250 and never beats #1 at large dabs. Its only reason
  for existing was to escape #1's per-dab overhead — which #3 also
  escapes, without #2's single-workgroup serialization.
- **The high-dab-count cells (#3 winning at +3-+5ms, #1 falling by
  +6-17s) are the felt-experience headline.** The perceived "stroke
  lag" complaint isn't the big-dab-on-4K regression in #3 — it's the
  many-dabs-per-event regime where every architectural attempt except
  #3 melts down. This matters because impasto-class brushes live in
  that regime *by design*, independent of canvas size.
- **#3's only weakness is the opposite extreme** — few large dabs at
  high resolution, where the union-bbox lane-waste shows up as submit
  back-pressure (`cpu_p50` is ~6× `gpu_p50` in the 4K + 1000-2000px
  cells). A hybrid that picks fragment when `dabs_per_event` drops
  below a threshold (or when `dab_bbox_area / union_area` is small)
  and compute otherwise would dominate. Note this is *different* from
  the "small-dab/large-dab" axis — the routing key is the dispatch
  density, not the brush footprint.

## Working agreement

- Each new attempt is its own commit. Shader + CPU evolve together.
- New attempts append a row here: what we did, what we measured, why
  we kept it or moved on.
- Watercolor / smudge / liquify stay untouched. Their compute ports
  will reuse whatever pattern wins.
