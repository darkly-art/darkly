# Stabilization performance: `paint_compute` attempts

Branch: `dab-compute-shader`. Running log for the stabilization
performance problem: at high stabilization the stroke engine emits
many dabs per pen event, and the per-event GPU cost balloons. The
end-to-end behavior the artist sees is **stroke lag**: frames stall,
the stroke trails the pen, the editor stops feeling responsive.

Predecessor investigation: its findings are folded into
[`brush/stabilization.md`](brush/stabilization.md) ("Lag investigation findings").
Originating plan: `~/.claude/plans/paint-compute-perf-fix.md`.

## Problem

`paint_compute` drives every Basic brush (Airbrush, Ink Pen).
Stabilization spreads ~30+ dabs across each pen event. Each dab is
small individually but they accumulate fast: the engine has to land
all of them in the scratch before the next event's commit, on every
frame. **Where** the cost sits has shifted across attempts; the
underlying constraint hasn't.

Current felt behavior on `dab-compute-shader` (post-attempt #3):
small dabs / small canvases: smooth. 4K canvas + medium dab size:
not smooth. Long fast strokes: still trails the pen.

## Attempts

### #1: Fragment-path `color_output`

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
radius. A brush like impasto oil (where spacing is pinned to 1px
regardless of dab size for the signature daubed look) would hit the
exact same catastrophe at *any* radius. The radius axis here is a
proxy for "events that emit O(stroke_length_px) dabs". Read radius=1
as "1px spacing", radius=10 as "~0.4px spacing", etc.

At radius ≥ 100px on Ink Pen the spacing scales up enough that the
dab count drops to ~one per event and the fragment path is fine
everywhere: even 4K with 1000px dabs sits within budget. The 4K +
2000px regression is the GPU work itself catching up.

### #2: Compute terminal, single workgroup serial tile-walk (v1 `paint_compute`)

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
2000px cell takes **57.6 seconds** for a 3.5s stroke (16× slower
than real-time), with the GPU genuinely busy (gpu p50 = 242 ms/event,
within ~80% of cpu p50 = 276 ms/event, confirming it's actual shader
work, not back-pressure). The knee starts at radius=250-500px on
every canvas, where the single workgroup's 64 threads can no longer
chew through each dab's bbox in time. Below the knee the workgroup
has enough parallelism for tiny bboxes; above it, the per-thread tile
count grows quadratically with radius. (Spacing matters less here
than for #1: #2's cost scales with `dab_count × bbox_area`, not
`dab_count` alone, so even at tight spacing the bbox_area term keeps
small dabs cheap.)

### #3: Compute terminal, thread-per-pixel iterate-dabs (this branch)

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
sparse: most threads in the rectangle never get hit by any dab,
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

**The shader is innocent.** At 4K + 1000px (the cell that's 3.5
seconds behind real-time), `gpu_shader_p50 = 3.2 ms`. The sync
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
small (shader time grows with `union_bbox_area` because more
threads spawn that do nothing), but the shader alone never breaks
the budget at any cell.

**The `dabs/ev` and `bbox/ev` columns confirm the spacing argument
quantitatively.** 4K + 1px = 916 dabs/event with bbox 39k px²; 4K
+ 2000px = 3.3 dabs/event with bbox 20.5M px². The two axes are
inversely correlated for Ink Pen but **architecturally
independent**: a brush like impasto oil with pinned 1px spacing
would push `dabs/ev` to ~900 *at any radius*, with bbox tracking
radius separately. #3's failure mode is the `bbox/ev` axis, not
the `dabs/ev` axis.

**One curious cell to revisit:** 1440p + 1000px (`+286 ms` behind,
`cpu_p50 = 18.2 ms` but `submit_p50 = 3.8 ms`). Unlike its 4K
neighbors, submit isn't dominant: possibly variance, possibly the
GPU has just enough slack at this resolution that submit doesn't
back up but encoding still bloats. Worth a repeat run if anything
hinges on it.

### #4: Single-pass instanced fragment (this branch, current)

**Shape:** one terminal `paint`, one render pass per phase, N
instanced draws (`pass.draw(0..6, 0..N)`). Per-instance data
(`PaintDabRecord`) lives in a storage buffer. Vertex shader computes
each instance's clip-space quad from `pos ± radius`; fragment shader
computes disc coverage + softness + selection, emits premultiplied
output. Pipeline blend state runs source-over (`One, OneMinusSrcAlpha`
on color *and* alpha) or destination-out for erase. The scratch
texture is written directly by the ROP stage: no buffer round-trip
anywhere.

**Files:** [`shaders/brush/paint.wgsl`](shaders/brush/paint.wgsl),
[`crates/darkly/src/brush/nodes/paint.rs`](crates/darkly/src/brush/nodes/paint.rs).

**Alpha note.** The scratch texture is straight-alpha for every other
consumer (color_output, watercolor, smudge, liquify). During a Basic-
brush stroke the convention is internal to the `paint` terminal:
it writes premultiplied, then the commit hook flips
`fg_premultiplied: true` on `commit_brush_dab` so the existing
composite shader interprets the scratch correctly when blitting to
the layer. See `docs/lessons-learned/compositing-lessons-learned.md` §4 for why hardware
source-over requires a premultiplied destination; per the same lesson,
straight-alpha + hardware blend would produce dark-edge artifacts on
partially transparent backgrounds. The shipped fragment terminal is
the answer that lesson was waiting for.

**What it bought:** both #1's and #3's failure modes disappear in one
move. Per-dab `begin_render_pass` overhead is gone (one pass per
phase, regardless of dab count). Buffer round-trip is gone (hardware
ROP writes the scratch directly, no `copy_texture_to_buffer` /
`copy_buffer_to_texture`). Cost scales with `Σ(dab_area)`
(actually-rasterized pixels) instead of `dab_count` (#1) or
`union_bbox_area` (#3). Neither catastrophe regime applies.

**Why this was always available but took four attempts.** The
predecessor doc claimed the scratch had been flipped to premultiplied
alpha so hardware blend would work. It hadn't (or the flip was
reverted when paint_compute landed). #3 worked around the
straight-alpha constraint with a storage-buffer compute path and
manual blend math; the resulting buffer round-trip became the
dominant cost the bench surfaced. Once we localised the premultiplied
convention to the paint terminal's own commit (via `fg_premultiplied:
true`), the rest of the codebase stays unchanged and hardware blend
works correctly.

**WebGPU spec on instance ordering**: instances within a draw call
are issued in instance-index order, and primitives within a draw are
blended in primitive-issue order. So overlapping dabs blend
deterministically with no inter-instance hazards. The earlier #3 doc
hedged this as "implementation-defined"; the spec is firmer than that
hedge suggested.

#### Bench data

`stroke_replay_matrix --topology paint` against
[recorded_curvy_stroke.json](crates/darkly/tests/fixtures/recorded_curvy_stroke.json),
same recording as the prior attempts. Full table at
[bench-results/stroke-replay-matrix-paint-recorded_curvy_stroke-cc26144cf2.md](crates/darkly/bench-results/stroke-replay-matrix-paint-recorded_curvy_stroke-cc26144cf2.md).
The 6-slot GPU-timestamp split (sync_in / shader / sync_out) is gone
from the bench because the path it instrumented (the buffer
round-trip) no longer exists.

| canvas | radius_px | wall (ms) | behind (ms) | worst-frame (ms) | cpu p50 (µs) | submit p50 (µs) | dabs/ev | bbox/ev (px²) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 1280×720 | 1 | 3553 | +17 | 1.7 | 3619 | 1664 | 286.5 | 4654 |
| 1280×720 | 10 | 3551 | +15 | 1.2 | 3394 | 1669 | 201.8 | 10854 |
| 1280×720 | 100 | 3555 | +19 | 1.2 | 3006 | 1772 | 20.2 | 197168 |
| 1280×720 | 250 | 3552 | +16 | 22.4 | 2973 | 1799 | 8.1 | 835616 |
| 1280×720 | 500 | 3553 | +17 | 24.7 | 2716 | 1597 | 4.1 | 1837229 |
| 1280×720 | 1000 | 3555 | +19 | 15.1 | 2696 | 1585 | 2.0 | 2255072 |
| 1280×720 | 2000 | 3554 | +18 | 16.2 | 2554 | 1485 | 1.0 | 1466990 |
| 1920×1080 | 1 | 3552 | +16 | 23.8 | 4203 | 1774 | 438.5 | 10081 |
| 1920×1080 | 10 | 3550 | +14 | 24.9 | 3855 | 1769 | 308.8 | 18755 |
| 1920×1080 | 100 | 3558 | +22 | 26.3 | 3328 | 1950 | 30.9 | 235482 |
| 1920×1080 | 250 | 3554 | +18 | 21.4 | 3236 | 1928 | 12.4 | 1045568 |
| 1920×1080 | 500 | 3556 | +20 | 17.7 | 3116 | 1868 | 6.2 | 2813993 |
| 1920×1080 | 1000 | 3555 | +19 | 26.7 | 2656 | 1553 | 3.2 | 5348161 |
| 1920×1080 | 2000 | 3554 | +18 | 27.0 | 2693 | 1570 | 1.6 | 5017430 |
| 2560×1440 | 1 | 3553 | +17 | 17.5 | 4781 | 1885 | 597.0 | 17572 |
| 2560×1440 | 10 | 3558 | +22 | 16.2 | 4034 | 1776 | 420.4 | 28767 |
| 2560×1440 | 100 | 3554 | +18 | 32.0 | 3353 | 1994 | 42.1 | 275881 |
| 2560×1440 | 250 | 3555 | +19 | 23.6 | 3238 | 1944 | 16.9 | 1198200 |
| 2560×1440 | 500 | 3559 | +23 | 26.8 | 3119 | 1849 | 8.4 | 3516147 |
| 2560×1440 | 1000 | 3554 | +18 | 13.5 | 3085 | 1819 | 4.2 | 7635587 |
| 2560×1440 | 2000 | 3557 | +21 | 11.1 | 2781 | 1650 | 2.1 | 8789082 |
| 3840×2160 | 1 | 3551 | +15 | 33.2 | 5224 | 1701 | 916.5 | 39237 |
| 3840×2160 | 10 | 3552 | +16 | 28.7 | 4622 | 1723 | 645.5 | 55577 |
| 3840×2160 | 100 | 3555 | +19 | 26.0 | 3368 | 1836 | 64.6 | 356606 |
| 3840×2160 | 250 | 3556 | +20 | 22.3 | 3406 | 1991 | 25.8 | 1412694 |
| 3840×2160 | 500 | 3557 | +21 | 26.5 | 3323 | 1958 | 12.9 | 4395837 |
| 3840×2160 | 1000 | 3556 | +20 | 26.0 | 3702 | 2187 | 6.5 | 11656344 |
| 3840×2160 | 2000 | 3660 | **+124** | 108.8 | 18277 | 2163 | 3.3 | 20514778 |

**Every cell of the matrix is within the recorded cadence.** 26 of 28
cells are at +15-23 ms behind: bench-noise level, identical
behaviour across small-radius and large-radius regimes. The worst
cell, 4K + 2000px, sits at +124 ms: still under 4 % of the 3.5 s
stroke, and 33× better than #3's +4151 ms.

**Cross-attempt diff on the previously-worst cells:**

| cell | #1 fragment | #3 thread-per-pixel | #4 instanced fragment |
|---|---:|---:|---:|
| 4K + 1px | +17614 ms | +5 ms | +15 ms |
| 4K + 500px | +20 ms | +1794 ms | **+21 ms** |
| 4K + 1000px | +55 ms | +3562 ms | **+20 ms** |
| 4K + 2000px | +1249 ms | +4200 ms | **+124 ms** |
| 1080p + 1000px | +21 ms | +3 ms | +19 ms |
| 1280×720 + 1px | +3113 ms | +3 ms | +15 ms |

**The catastrophe regimes are gone.** #1's small-radius cells
(thousands of dabs/event × per-pass driver overhead) caught up; #3's
large-bbox cells (buffer round-trip × union bbox bytes) caught up. The
single residual outlier (4K + 2000px) is the heavily-overlapping-
large-dabs regime called out in the plan's "where this could lose"
hedge: 3.3 dabs × ~20 M px² union with heavy overdraw produces ~60 M
shaded fragment invocations per event, borderline. Still well under
the threshold where the artist feels lag. If a future use case ever
makes this regime first-class, a per-dab tile-bin or a coarse
overdraw cull is the next lever.

**Where #4 *doesn't* close a gap and why that's OK.** Tiny radius
cells (1px) read slightly higher than #3 (+15 ms vs +5 ms in the 4K
column). The difference is bench noise and the modest CPU cost of
appending 900 dab records to the storage buffer per event vs writing
the same records as compute-buffer dabs. Neither is felt by an artist;
the matrix's noise floor is around ±20 ms.

### Watercolor: the same shapes, one extra constraint

The wet-media terminal went through the same sequence, and it is the
only terminal to have gone *back*:

- `watercolor_compute` had #3's architecture and #3's failure: sync
  bytes over the union bbox, catastrophic from 1280x720 at 250 px up.
  Nothing new was learned from it.
- `watercolor_batched` ported it to #4 with one addition: a per-dab
  *pickup* probe (the 8x8 neighbourhood average under the dab) written
  into one cell of a 128x128 atlas by a single instanced pass, which is
  legal only because the probe reads the read-only pre-stroke snapshot.
  The pattern is worth keeping: any per-dab probe whose source does
  not change mid-flush can be instanced. Result: 26 of 28 cells within
  bench noise; full table in
  [bench-results/stroke-replay-matrix-watercolor-recorded_curvy_stroke-39c5566bc3.md](../crates/darkly/bench-results/stroke-replay-matrix-watercolor-recorded_curvy_stroke-39c5566bc3.md).
- The two outliers were the 4K overdraw cells, where its fragment
  shader cost about 7x paint's (+875 ms versus +124 ms at 4K + 2000 px).
  Shader weight matters only where overdraw does.
- It was then **re-serialized**: a dab's colour reads the deposit
  earlier dabs left under it, and a batched flush gave every dab the
  same pre-flush answer, banding at the pointer-event period. Today's
  `watercolor.rs` runs one pickup pass and one composite pass per dab,
  and that version has never been benched. See section E below.

### #5, stage 1: dispatch per dab, measured in isolation

**Shape:** option F. One compute pass, one `dispatch_workgroups` per dab
sized to the dab's bounding box, one thread per pixel, against a
stroke-resident `r32uint` read-write storage texture holding packed RGBA8.
Each dispatch gets its dab through a static index buffer (slot `i` holds
`i`, written once) bound with a dynamic offset; the tight 32-byte record
array is uploaded once per pass.

**Files:** [`crates/darkly/src/bin/dispatch_cost_bench.rs`](../crates/darkly/src/bin/dispatch_cost_bench.rs)
(harness, inline WGSL). No engine, no stroke, no terminal: this stage
measures the one quantity B.1 was dismissed over without measuring, the
all-in cost of a dispatch inside a pass, barrier included.

**Bench data.** 3840x2160 target, N dabs scattered, three shapes over the
same dabs: (a) the option F shape; (b) the same dispatches with the texture
bound read-only and the store dropped, identical encode and no
inter-dispatch barrier, so (a) minus (b) is the barrier; (c) one render
pass per dab with hardware source-over, the shape the read-mirror
terminals pay today. Five untimed warm-up iterations per cell, then 20
timed iterations with the three shapes interleaved so an integrated GPU's
frequency scaling lands on all three alike (the first run, without that,
showed N = 2000 finishing faster than N = 900 purely from clock ramp).
Wall clock is encode through `poll(Wait)`; GPU time is pass timestamps.
Full file: `bench-results/dispatch-cost-bench-9d824e90e8.md`.

Adapter: Intel Raptor Lake-P iGPU, Vulkan, Mesa 26.2.3. Same machine as
the section E smudge baseline.

| radius_px | N | shape | wall p50 (ms) | wall min (ms) | wall per dab, p50 (us) | gpu p50 (ms) | gpu per dab, p50 (us) | parity max diff (LSB) |
|---:|---:|---|---:|---:|---:|---:|---:|---:|
| 1.5 | 300 | (a) dispatch, read-write | 4.65 | 3.62 | 15.5 | 2.86 | 9.5 | 1 |
| 1.5 | 300 | (b) dispatch, read-only | 5.03 | 3.52 | 16.8 | 2.77 | 9.2 | - |
| 1.5 | 300 | (c) render pass per dab | 28.12 | 19.21 | 93.7 | 11.54 | 38.5 | - |
| 1.5 | 900 | (a) dispatch, read-write | 6.46 | 5.60 | 7.2 | 4.69 | 5.2 | 1 |
| 1.5 | 900 | (b) dispatch, read-only | 6.97 | 6.07 | 7.7 | 4.56 | 5.1 | - |
| 1.5 | 900 | (c) render pass per dab | 49.59 | 43.34 | 55.1 | 19.03 | 21.1 | - |
| 1.5 | 2000 | (a) dispatch, read-write | 7.82 | 7.16 | 3.9 | 5.48 | 2.7 | 1 |
| 1.5 | 2000 | (b) dispatch, read-only | 8.54 | 7.02 | 4.3 | 5.50 | 2.7 | - |
| 1.5 | 2000 | (c) render pass per dab | 73.96 | 67.20 | 37.0 | 21.78 | 10.9 | - |
| 10 | 300 | (a) dispatch, read-write | 2.47 | 2.01 | 8.2 | 1.09 | 3.6 | 1 |
| 10 | 300 | (b) dispatch, read-only | 2.66 | 2.08 | 8.9 | 1.07 | 3.6 | - |
| 10 | 300 | (c) render pass per dab | 15.42 | 12.44 | 51.4 | 4.25 | 14.2 | - |
| 10 | 900 | (a) dispatch, read-write | 6.59 | 5.64 | 7.3 | 4.25 | 4.7 | 2 |
| 10 | 900 | (b) dispatch, read-only | 7.54 | 5.37 | 8.4 | 4.56 | 5.1 | - |
| 10 | 900 | (c) render pass per dab | 49.61 | 42.92 | 55.1 | 16.74 | 18.6 | - |
| 10 | 2000 | (a) dispatch, read-write | 8.20 | 7.34 | 4.1 | 5.67 | 2.8 | 2 |
| 10 | 2000 | (b) dispatch, read-only | 7.91 | 7.30 | 4.0 | 5.33 | 2.7 | - |
| 10 | 2000 | (c) render pass per dab | 73.72 | 66.97 | 36.9 | 21.24 | 10.6 | - |

**What it shows.**

- **The gate is met.** The gate written before the run (plan, stage 1):
  shape (a) at N = 900 within 9 ms wall, i.e. 10 us per dispatch. Measured:
  6.5 ms p50, 5.6 ms min, 7.2 us per dispatch, at both radii. N = 2000
  fits in 8 ms.
- **The barrier is free.** (b) is not cheaper than (a) in any cell; the
  differences are inside the noise. The `vkCmdPipelineBarrier` wgpu
  inserts between read-write dispatches costs nothing measurable here.
- **A dispatch is not a render pass.** Marginal cost from the 300 to 2000
  slope: about 1.5 us wall and 1.5 us GPU per dispatch, against about 27 us
  wall and 6 us GPU per single-instance render pass. The pass-per-dab
  column reproduces section E's smudge figure (about 40 us per dab all-in)
  on this machine in this harness.
- **Both shapes carry a fixed cost per submission** (roughly 4 ms wall for
  the compute shapes at the smallest N, more for the render-pass shape),
  attributed to the harness's submit-and-idle cycle and an idle GPU's clock
  ramp rather than to either shape. The engine pays that cycle once per
  event regardless of terminal, so it cancels in a comparison with #4; the
  slopes are what differ.
- **Parity:** the compute path's packed result matches hardware blending
  within 1 LSB at radius 1.5 and 2 LSB at radius 10 (stacked rounding where
  dabs overlap), so the timed shapes do equivalent work.

**Caveats.** One adapter, one driver, native Vulkan. The WebGPU backend
routes every `setBindGroup` and `dispatchWorkgroups` through the browser's
GPU process, and that per-call cost is unmeasured; a browser replay is a
separate measurement after the port. This is not a stroke: stabiliser
rewinds, checkpoints and layer growth are not in the number, but they are
terminal-independent and #4 pays them too.

**Decision: stage 2 proceeds.** Per the plan, a pass at or under 10 us per
dispatch means the throwaway terminal is built and run on the replay
matrix against a same-session `paint` run, and the outcome is recorded
here as attempt #5, stage 2.

### #5, stage 2: dispatch per dab on the real stroke path

**Shape:** the stage 1 shape as a terminal. `paint_dispatch_spike`
(`crates/darkly/src/brush/nodes/paint_dispatch_spike.rs`,
`shaders/brush/paint_dispatch_spike.wgsl`) queues one 48-byte record per
dab and, per flush, opens one compute pass and issues one
`dispatch_workgroups` per dab over the dab's layer-clamped footprint, one
thread per pixel, reading and writing a stroke-resident `r32uint` ground
under premultiplied source-over. The ground is a stroke channel of the
new `ChannelUse::Storage` kind (`scratch.rs`), so the framework
allocates, clears, checkpoints, restores and grows it with the scratch.
At commit an unpack pass rewrites the RGBA8 scratch from the ground and
the ordinary `commit_brush_dab` runs. The dab index reaches a dispatch
through a static index buffer bound with a dynamic offset. Driven by the
Ink Pen graph with its terminal swapped
(`tests/fixtures/ink_pen_dispatch_spike.yaml`), under
`--topology paint-dispatch-spike`.

**What is charged to the spike that a port would not pay:** the unpack,
one full-layer pass per event (measured by A/B below), and doubled
checkpoint traffic, since the ring snapshots both the RGBA8 write side
and the ground where `paint` snapshots one texture.

**Bench data.** Same machine and session as stage 1 (Intel Raptor Lake-P
iGPU, Vulkan, Mesa 26.2.3), `stabilize = 1.0`, real-time pacing, in this
order: `paint`, `paint-dispatch-spike`, the spike with the unpack draw
disabled by a local edit, `paint` again. The two `paint` runs agree
within 2 ms of `behind` and about 200 us of `cpu p50` on every cell, so
the session carries no drift. Files:
`bench-results/stroke-replay-matrix-{paint,paint-rerun,paint-dispatch-spike,paint-dispatch-spike-no-unpack}-recorded_curvy_stroke-746570670c.{md,tsv}`.
Flushes per event are 9 to 10 at every cell; the spike's `dispatches/ev`
equals its `dabs/ev`.

| canvas | radius_px | dabs/ev | paint behind (ms) | paint rerun behind (ms) | spike behind (ms) | spike, no unpack behind (ms) | paint cpu p50 (us) | spike cpu p50 (us) | spike, no unpack cpu p50 (us) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1280×720 | 1 | 321.4 | +5 | +5 | +19 | +14 | 5623 | 6506 | 6451 |
| 1280×720 | 10 | 321.5 | +5 | +5 | +14 | +15 | 5775 | 6482 | 6582 |
| 1280×720 | 100 | 32.3 | +6 | +6 | +18 | +17 | 3774 | 4400 | 4246 |
| 1280×720 | 250 | 13.0 | +8 | +8 | +22 | +16 | 3587 | 4039 | 4059 |
| 1280×720 | 500 | 6.5 | +9 | +8 | +23 | +18 | 3667 | 3995 | 3760 |
| 1280×720 | 1000 | 3.3 | +8 | +8 | +19 | +19 | 3457 | 3777 | 3476 |
| 1280×720 | 2000 | 1.7 | +7 | +8 | +20 | +18 | 3218 | 3617 | 3331 |
| 1920×1080 | 1 | 490.9 | +4 | +5 | +15 | +14 | 6876 | 7184 | 7124 |
| 1920×1080 | 10 | 491.0 | +4 | +5 | +12 | +13 | 6633 | 7070 | 7084 |
| 1920×1080 | 100 | 49.2 | +6 | +5 | +18 | +18 | 4106 | 4862 | 4370 |
| 1920×1080 | 250 | 19.7 | +9 | +9 | +19 | +19 | 4055 | 5062 | 4393 |
| 1920×1080 | 500 | 9.9 | +9 | +9 | +19 | +18 | 3711 | 4545 | 4377 |
| 1920×1080 | 1000 | 5.0 | +6 | +7 | +19 | +21 | 3469 | 4771 | 4606 |
| 1920×1080 | 2000 | 2.5 | +7 | +9 | +20 | +21 | 3752 | 4736 | 4377 |
| 2560×1440 | 1 | 666.8 | +5 | +4 | +17 | +16 | 6983 | 7388 | 7155 |
| 2560×1440 | 10 | 666.9 | +5 | +5 | +14 | +16 | 6969 | 7377 | 7275 |
| 2560×1440 | 100 | 66.8 | +5 | +5 | +17 | +17 | 4472 | 5262 | 5146 |
| 2560×1440 | 250 | 26.7 | +9 | +8 | +18 | +22 | 4087 | 5036 | 5135 |
| 2560×1440 | 500 | 13.3 | +8 | +7 | +18 | +20 | 4236 | 5315 | 5141 |
| 2560×1440 | 1000 | 6.7 | +10 | +8 | +20 | +20 | 4181 | 5403 | 4924 |
| 2560×1440 | 2000 | 3.3 | +8 | +8 | +80 | +40 | 3938 | 5953 | 6097 |
| 3840×2160 | 1 | 1021.4 | +6 | +6 | +16 | +17 | 7278 | 8153 | 7745 |
| 3840×2160 | 10 | 1021.5 | +7 | +6 | +23 | +16 | 7154 | 8187 | 7951 |
| 3840×2160 | 100 | 102.2 | +5 | +5 | +17 | +16 | 4372 | 6847 | 6468 |
| 3840×2160 | 250 | 40.9 | +8 | +9 | +18 | +18 | 4548 | 6488 | 6382 |
| 3840×2160 | 500 | 20.5 | +8 | +9 | +41 | +20 | 4437 | 6819 | 6683 |
| 3840×2160 | 1000 | 10.3 | +8 | +8 | +658 | +445 | 4851 | 22354 | 20986 |
| 3840×2160 | 2000 | 5.1 | +11 | +13 | +2688 | +2365 | 5926 | 32970 | 31227 |

**What it shows.**

- **The gate is met.** The gate from section F: `behind_by_ms` within
  noise of a same-session `paint` run at 3840x2160 r = 1 and at 1920x1080
  r = 1 and 10. Measured: +16 against +6 at 4K r = 1 (1021 dispatches per
  event), +15 and +12 against +4 and +4 at 1080p. The bench's noise floor
  is about +-20 ms over the 3.5 s stroke.
- **A steady 10 to 15 ms over the stroke, everywhere below 4K r = 1000.**
  The spike's `behind` sits at +12 to +23 where `paint` sits at +4 to
  +11, about 0.3% of the stroke, and `cpu p50` is 300 to 1100 us higher.
  The A/B attributes 0 to 500 us of that per event to the unpack; the
  rest is the dispatch loop (about a thousand `set_bind_group` plus
  `dispatch_workgroups` per event at r = 1) and the doubled checkpoint
  copies.
- **Where the fragment path wins: enormous dabs.** At 4K r = 1000 and
  r = 2000 (10 and 5 dabs per event, each clipped to most of the canvas)
  the spike falls behind by 0.7 s and 2.7 s where `paint` stays at +8 and
  +11. Attribution, from the A/B and from the stage 1 harness re-run with
  a 1000 px radius cell (`bench-results/dispatch-cost-bench-746570670c.md`,
  which also re-measures the stage 1 cells; they reproduce within noise):

| radius_px | N | shape | wall p50 (ms) | wall min (ms) | wall per dab, p50 (us) | gpu p50 (ms) | gpu per dab, p50 (us) | parity max diff (LSB) |
|---:|---:|---|---:|---:|---:|---:|---:|---:|
| 1000 | 5 | (a) dispatch, read-write | 7.23 | 6.81 | 1446.0 | 5.08 | 1016.6 | 3 |
| 1000 | 5 | (b) dispatch, read-only | 5.80 | 5.40 | 1160.9 | 3.74 | 748.7 | - |
| 1000 | 5 | (c) render pass per dab | 4.78 | 4.08 | 956.8 | 2.40 | 479.1 | - |
| 1000 | 10 | (a) dispatch, read-write | 12.50 | 12.27 | 1250.4 | 10.23 | 1023.5 | 4 |
| 1000 | 10 | (b) dispatch, read-only | 9.64 | 9.46 | 963.7 | 7.55 | 754.9 | - |
| 1000 | 10 | (c) render pass per dab | 7.54 | 7.31 | 754.5 | 4.80 | 480.3 | - |

  On this iGPU a dab covering about four megapixels costs the compute
  shape about 1.0 ms of GPU time against 0.48 ms for the render pass, and
  the read-only variant sits between (0.75 ms): the storage read-modify-
  write and the pack and unpack cost more than the blend unit's
  fixed-function path per pixel. Scaled to the 4K r = 2000 cell that is
  about 1 s of the 2.7 s; the unpack is about 0.3 s (A/B: +2688 against
  +2365); the remaining 1.4 s is the doubled checkpoint copying of two
  full 4K textures per checkpoint, a spike artefact. So the shape is
  genuinely about 2x slower per pixel than hardware blending on huge dabs
  on a bandwidth-bound integrated GPU, and that regime is the one a port
  must measure on a discrete GPU before committing to compute-only there.
- **Parity holds.** `tests/paint_dispatch_spike.rs`: the same stroke
  through the Ink Pen and through the spike lands the same pixels within
  4 LSB per channel in premultiplied space, 97% of painted pixels within
  1 LSB (1002 exact and 8291 at exactly 1 LSB of 9559; the pack's rounding
  against the blend unit's). Two replays of the
  recorded stroke at full stabilisation agree byte for byte, which
  exercises the `r32uint` ground through the checkpoint ring's rewinds.
- **The first spike run dispatched over the unclipped bounding box**, as
  the harness does; clipping the grid to the layer-clamped footprint (the
  same rect `paint`'s ledger publishes) changed the 4K r = 2000 cell by
  only 60 ms, because the recording's dabs mostly lie inside the canvas.
  The clipped grid is what is recorded above.

**Decision: the door is open, with one regime marked.** Below huge dabs
the dispatch-per-dab shape lands within noise of the shipped instanced
terminal on the real stroke path, at the many-dabs-per-event counts that
collapsed #1 and the smudge baseline. The port to a compute paint
terminal (every accumulation law as shader code against a live ground,
the read-mirror terminals and their copies gone) can proceed on that
basis. The cost the port must carry into its own gate: at 4K with dabs
of a thousand pixels or more, thread-per-pixel compute is about 2x the
fragment path per pixel on this iGPU. The port's plan decides whether
that is accepted, mitigated (larger workgroups, a per-row loop, or
`rgba8unorm` read-write storage where the adapter offers it, which drops
the pack and unpack), or measured again on a discrete GPU first.

**Kept:** the spike stays in the tree behind its bench topology until the
port replaces it, per the plan. `ChannelUse` and the `dispatches` counter
are permanent.

### #6: the compute paint terminal (shipped)

**Shape:** `paint` ported to the stage 2 shape with the spike's artefacts
removed (`crates/darkly/src/brush/nodes/paint.rs`). The terminal's registration
declares `dab_pass: DispatchPerDab` and `scratch_format: R32Uint`; the
ground *is* the stroke scratch, so the checkpoint ring, the clear, the
grow and the commit act on one texture and the unpack pass and the doubled
checkpoint copies are gone. The WGSL assembler emits a compute skeleton
(`cs_main`, `@workgroup_size(8, 8, 1)`) for a terminal that declares the
pass; every node body is spliced into it unchanged. The accumulation laws
are shader code in `shaders/brush/paint_accumulate.wgsl`, applied per dab
against the live ground: build-up is premultiplied source-over, wash is
the commit's deposit ceiling (`shaders/lib/deposit_ceiling.wgsl`, shared
with `composite.wgsl`), which for one pigment reproduces the `Max` blend
exactly. Inside the dial the build half gets a second packed ground as a
storage channel and the commit is unchanged. The commit reads the packed
ground through a second fragment entry point (`fs_packed`). Selection is
sampled per dab inside the law; erase stays at commit. The spike, its
shader, fixture, test and bench topology are deleted.

**Measurements before, on this machine** (Intel Raptor Lake-P iGPU,
Vulkan, Mesa 26.2.3): `paint` twice at `929928f9ab`, which agree with the
`746570670c` reference within the noise band on every cell but 4K at
2000 px (+44 and +119 against +11; that cell is noisy here). The harness
with two new variants of (a), `bench-results/dispatch-cost-bench-929928f9ab.md`:
a 16x16 workgroup and an 8x2 workgroup whose threads each walk four rows.
8x8 wins every small-dab cell (1.5 px and 10 px at 300 to 2000 dabs:
16x16 is 15 to 45% slower on wall, four rows 30 to 70% slower); at
1000 px the row loop gains about 20% of GPU time (1.0 to 0.8 ms per dab)
and 16x16 gains nothing. `DAB_WORKGROUP` is 8.

**Bench data after the port**, same session, `paint` twice
(`bench-results/stroke-replay-matrix-paint-compute-after{,-rerun}-recorded_curvy_stroke-929928f9ab`),
`dispatches/ev == dabs/ev` on every cell:

| canvas | radius_px | dabs/ev | before behind (ms) | before rerun | after behind (ms) | after rerun | before cpu p50 (us) | after cpu p50 (us) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 1280×720 | 1 | 321.4 | +7 | +5 | +5 | +4 | 6294 | 6330 |
| 1280×720 | 10 | 321.5 | +5 | +4 | +5 | +6 | 5921 | 6630 |
| 1280×720 | 100 | 32.3 | +9 | +5 | +6 | +6 | 3936 | 4272 |
| 1280×720 | 250 | 13.0 | +9 | +8 | +10 | +10 | 3728 | 4303 |
| 1280×720 | 500 | 6.5 | +9 | +8 | +10 | +10 | 3437 | 4056 |
| 1280×720 | 1000 | 3.3 | +10 | +9 | +10 | +9 | 3290 | 3418 |
| 1280×720 | 2000 | 1.7 | +9 | +9 | +5 | +8 | 2981 | 3535 |
| 1920×1080 | 1 | 490.9 | +4 | +9 | +7 | +8 | 6578 | 7198 |
| 1920×1080 | 10 | 491.0 | +8 | +5 | +9 | +6 | 6607 | 7313 |
| 1920×1080 | 100 | 49.2 | +6 | +6 | +6 | +6 | 4378 | 5048 |
| 1920×1080 | 250 | 19.7 | +10 | +9 | +11 | +9 | 3925 | 4824 |
| 1920×1080 | 500 | 9.9 | +10 | +7 | +9 | +10 | 3881 | 4706 |
| 1920×1080 | 1000 | 5.0 | +3 | +6 | +11 | +9 | 3518 | 4504 |
| 1920×1080 | 2000 | 2.5 | +7 | +10 | +10 | +10 | 3312 | 4516 |
| 2560×1440 | 1 | 666.8 | +2 | +4 | +9 | +8 | 7344 | 7552 |
| 2560×1440 | 10 | 666.9 | +6 | +4 | +9 | +7 | 7167 | 7401 |
| 2560×1440 | 100 | 66.8 | +5 | +4 | +6 | +7 | 4470 | 5812 |
| 2560×1440 | 250 | 26.7 | +7 | +9 | +8 | +8 | 4054 | 5275 |
| 2560×1440 | 500 | 13.3 | +8 | +10 | +9 | +10 | 3777 | 5301 |
| 2560×1440 | 1000 | 6.7 | +7 | +8 | +11 | +12 | 3694 | 5438 |
| 2560×1440 | 2000 | 3.3 | +9 | +8 | +11 | +11 | 3905 | 5320 |
| 3840×2160 | 1 | 1021.4 | +4 | +11 | +8 | +7 | 7654 | 7997 |
| 3840×2160 | 10 | 1021.5 | +7 | +7 | +7 | +8 | 7369 | 8097 |
| 3840×2160 | 100 | 102.2 | +2 | +8 | +6 | +9 | 5289 | 6602 |
| 3840×2160 | 250 | 40.9 | +8 | +8 | +11 | +10 | 4218 | 6153 |
| 3840×2160 | 500 | 20.5 | +11 | +8 | +9 | +9 | 4173 | 6166 |
| 3840×2160 | 1000 | 10.3 | +7 | +8 | +30 | +11 | 4869 | 6892 |
| 3840×2160 | 2000 | 5.1 | +44 | +119 | +2986 | +1355 | 5743 | 25632 |

**What it shows.**

- **The gate is met.** Every cell at a radius of 500 px or less is within
  a few ms of the baseline's `behind` (the band was +20 ms), with
  `dispatches/ev` equal to `dabs/ev`. `cpu p50` sits 0 to 1.5 ms higher
  than the instanced terminal's, which is the dispatch loop (two calls per
  dab) and the per-flush bind group; it stays well inside the 16 ms event
  budget at every cell, including 1021 dispatches per event at 4K.
- **The large-dab regime is what stage 2 said it was.** 4K at 1000 px is
  within noise (+30 and +11 against +7 and +8). 4K at 2000 px, five dabs
  per event each clipped to most of an 8-megapixel canvas, is 1.4 to 3.0 s
  behind over the 3.5 s stroke where the fragment path is +44 to +119; the
  spike was +2688 with its unpack and doubled checkpoints. That cell's
  noise is about ±800 ms here, and its `cpu p50` of 25 ms says the submit
  is blocking on GPU work: the thread-per-pixel read-modify-write is
  bandwidth-bound on this iGPU, about 2x the blend unit per pixel, as the
  harness measured.
- **The row loop, measured on the matrix and deferred.** Per the port
  plan's commitment, the four-rows-per-thread skeleton was applied and the
  matrix run once
  (`bench-results/stroke-replay-matrix-paint-compute-after-four-rows-per-thread-recorded_curvy_stroke-929928f9ab.tsv`):
  4K at 2000 px lands at +1115, 4K at 1000 px at +9, and every other cell
  is unchanged, because the small-dab cells are CPU-bound and cannot show
  the 30 to 70% GPU-time loss the harness measured there. It buys a
  fraction of a second in a regime that is unusable on this GPU either way
  and costs the regime artists paint in, so by the plan's rule it stays
  deferred; `DAB_WORKGROUP` is the one knob, and a discrete-GPU session is
  the place to revisit both.
- **Parity holds.** Before the spike was deleted,
  `spike_matches_paint_within_tolerance` measured the ported `paint`
  against the spike on the 48-sample reversing stroke: 9559 painted
  pixels, max 1 LSB in premultiplied space, none beyond 1 LSB. The
  spike's own recorded distance from the fragment terminal (4 LSB max,
  97% within 1 LSB) is the bridge to the old pixels. The exact
  accumulation suite (`tests/brush_accumulation.rs`, 17 tests, the
  within-stroke wash exactness and the mid-dial parity ladders) passes
  unchanged.

**Decision: shipped.** The instanced fragment terminal, its `Max` blend
state, the `FsOut` build channel and the spike are gone; `paint` is one
compute pass per flush, one dispatch per dab. What is deleted later, when
its last user is a graph on `paint`: the read-mirror loop, the `smudge`
and `blur` terminals, watercolor's atlas, the instanced skeleton and the
float foreground entry in `composite.wgsl` (the port plan's "deleted
later" list).

### #7: region copies in the checkpoint ring (shipped)

**Shape:** not a terminal change; the per-event fixed cost every brush
pays through the checkpoint ring (`crates/darkly/src/brush/checkpoint_ring.rs`).
Every save used to copy
the stroke's whole cumulative bbox into a slot, every restore copied the
slot's whole bbox back after a full-canvas attachment clear, and at
`stabilize = 0.6` that was about seven saves and one restore and clear
per ground per event, each the size of most of the canvas once the stroke
had crossed it. Now each slot is a layer-sized frame that records which
save point it equals and a stale rect where it does not; a save copies
the stale rect plus the footprint of the dabs placed since the slot's
save point (the per-dab footprints live on the save points), and a
restore copies back only the footprint of the dabs after the checkpoint,
resetting the part its frame does not cover (a zero fill, or a re-seed
from the pre-stroke layer) in the same submission. The reset is a
`copy_buffer_to_texture` from a zero buffer (`gpu/zero_fill.rs`), since
an attachment clear has no sub-rect form.

**Measurements, on this machine** (Intel Raptor Lake-P iGPU, Mesa
26.2.3), before at `b9aa3e94` and after with this change, the recorded
curvy stroke (204 events) at 1920x1080, Pencil, `stabilize = 0.6`,
medians of three. Browser `event_sync_ms.p50` is the harness's
`pace=sync` (one event posted, drained, rendered and waited on; serialized
CPU plus GPU per event under Dawn); native `cpu p50` is the matrix under
`--gpu-sync` (`bench-results/stroke-replay-matrix-pencil-*-b9aa3e9458*`).

| cell | browser before | browser after | native before | native after |
|---|---:|---:|---:|---:|
| 250 px, build-only (`buildup=100`) | 10.0 ms | 9.9 ms | 8.9 ms | 8.5 ms |
| 500 px, build-only | 13.4 ms | 12.2 ms | 11.6 ms | 10.7 ms |
| 250 px, mid-dial | 11.4 ms | 10.6 ms | 9.7 ms | 9.3 ms |
| 500 px, mid-dial | 15.2 ms | 13.1 ms | 13.1 ms | 11.3 ms |

The realtime 500 px mid-dial cell's `long_frames_over_33ms` went from 3,
5 and 4 to 0, 1 and 0. Full re-render fallbacks are 0 in every cell,
before and after (the matrix now prints them). Per stroke in the browser,
counted with a one-off patch of the harness: `copyTextureToTexture`
texels fell from 1.66 G to 0.42 G at 250 px build-only and from 4.58 G to
1.44 G at 500 px mid-dial, with the copy *count* unchanged (1408 and
2806); render passes fell by one per event (1025 to 823, the full-canvas
clears); submits fell by one per event (3419 to 3215, the restore
recorded into the rewind's encoder); zero fills are 4 to 10 per stroke.
The remaining copies are the divergence window's dabs, a bounding rect of
the last 65 or so points rather than of the whole stroke.

The first implementation sized a slot's frame to the cumulative bbox
rounded out to 256 px. Its medians matched these, but the worst single
event in the build-only cells rose from 21 to 27 ms (before) to 40 to
47 ms: every slot crosses a band in the same event and reallocates, a
fresh texture plus a frame-sized copy, times eight slots and two grounds.
Layer-sized frames brought the worst event to 17 to 22 ms with the same
medians, at one layer-sized copy per slot the first time a stroke uses
it, and are what shipped.

**Decision: shipped.** The gate (`event_sync_ms.p50` down 1.5 ms at
250 px and 2 ms at 500 px mid-dial) is met at 500 px (2.1 ms, and 1.2 ms
build-only) and missed at 250 px (0.8 ms, and 0.1 ms build-only). The
texel sums say why: the copies did fall by the expected factor there too,
so what remains at 250 px is not the ring. That cell runs 116 dabs per
event as 116 dispatches (`dispatches/ev` in the matrix) and its time
tracks the dispatch count, not the bbox; the handoff's remaining items
(replay length, item 3; dab footprints, item 4) and the per-event commit
(a full-layer pass that could be scissored to the rewound plus dirtied
region, which the ring now computes) are where the 250 px residual lives.

### #8: checkpoint saves in the segment's submission (shipped as stage one; the full fold is not worth it)

**Shape:** not a terminal change; the submit count of the stabilized
rewind-and-replay path in `crates/darkly/src/engine/painting.rs`. At
`stabilize = 0.6` an event replays 5.85 segments on average and used to
submit each segment's dabs and then its checkpoint save separately, with
the rewind and the commit in submissions of their own: about fourteen
`queue.submit` calls per event. The handoff expected 1 to 3 ms per event
in the browser from folding them into one. The plan staged the work so
the submit cost could be measured before the invasive part: stage one
records each save into its segment's encoder (a copy recorded after a
pass reads the pass's result), removing the 5.85 save submits per event
with no other change and keeping every segment's CPU/GPU overlap; stage
two (one context per event, with the dab-batching terminals' per-flush
`write_buffer` uploads turned into per-submission ring slots) would
remove the rest.

**Measurements, on this machine** (Intel Raptor Lake-P iGPU, Mesa
26.2.3), before at `b6155333` and after stage one, the recorded curvy
stroke (204 events) at 1920x1080, Pencil, `stabilize = 0.6`, browser
medians of three, native single runs
(`bench-results/stroke-replay-matrix-pencil-*-b615533324-saves-folded*`).

| cell | browser before | browser after | native before | native after |
|---|---:|---:|---:|---:|
| 250 px, build-only (`buildup=100`) | 9.8 ms | 9.8 ms | 8.8 ms | 9.1 ms |
| 500 px, build-only | 12.4 ms | 12.6 ms | 10.6 ms | 10.6 ms |
| 250 px, mid-dial | 10.4 ms | 10.9 ms | 9.2 ms | 9.2 ms |
| 500 px, mid-dial | 12.8 ms | 12.9 ms | 11.0 ms | 11.1 ms |

Submits per stroke in the browser fell from 3215 to 2021 (1194 fewer:
5.85 per event, exactly the saves), `dispatches` unchanged (23770 and
11886), full re-render fallbacks 0, the realtime cell's
`long_frames_over_33ms` 1 before and after and `behind_by_ms` within
noise. The individual runs scatter by about 0.5 ms around each median in
both directions; the medians moved by -0.0, +0.2, +0.5 and +0.1 ms.

**Decision: stop at stage one.** Twelve hundred submits per stroke cost
nothing measurable, so the per-submit cost under Dawn on this machine is
below the noise floor (under about 0.05 ms, against the 0.07 ms the
earlier instrumentation in `docs/brush/stabilization.md` booked for a
submit) and the remaining eight submits per event are worth well under
the plan's 1 ms gate; the plan's decision rule (stage one under 0.3 ms:
do not build stage two) applies. Stage one stays as a simplification:
the saves share their segment's submission, the two hand-bumped submit
counters and the two `self.gpu.encode("checkpoint-save")` blocks are
gone, and the stroke frame and channels come from the live context
(`StrokeResources::scratch_frame`, `channel_textures`). The
`tests/stroke_rewind.rs` oracle gates the fold and
`checkpoint_saves_share_their_segment_submission` asserts it. What this
rules out: submit count is not where the stabilized event's time goes.
What is left of the handoff's list is item 2 (prediction rendered as an
overlay rather than committed paint, which removes replayed segments
outright) and item 4 (several dabs per dispatch, since the 250 px cell's
time tracks its 116 dispatches per event), plus the commit scissor
(item 3) as its own small plan.

### #9: the live canvas sampler on `paint` (shipped)

**Shape:** not a change to `paint`'s own regime but a second dispatch per
dab for a graph that samples the stroke at other pixels. Before each
dab's dispatch, in the
same compute pass, an appearance snapshot dispatch lays the grounds on the
pre-stroke snapshot through the commit law into a layer-sized
`rgba8unorm` mirror over the dab's read region (footprint plus `|motion|`
plus a texel); the dab samples the mirror. Per dab: two `set_pipeline`,
two `set_bind_group(1)` and two dispatches, against one bind and one
dispatch for a brush that samples nothing (group 0 is shared by the two
pipelines' layouts and stays bound; groups 2 and 3 are outside the
snapshot's layout and stay bound). `dispatches/ev` counts both, so it reads
`2 * dabs/ev` for the Smudge.

**Measurements, on this machine** (Intel Raptor Lake-P iGPU, Mesa
26.2.3), the recorded curvy stroke at `stabilize = 1`, native, realtime
pacing, one session at `ce9622df` plus the change
(`bench-results/stroke-replay-matrix-{paint,pencil,dry-smudge}-recorded_curvy_stroke-ce9622df0e.md`).
The Pencil is the fair baseline: the Dry Smudge as measured ran its dial
(`buildup 0.1`, two grounds) and its spacing (0.03). That brush has since
become the shipped Smudge at full build-up (one ground, the dial hidden),
which does strictly less work per thread than these rows. The two brushes' pressure-to-size
curves differ, so at 100 px and above their dab counts differ and the rows
compare per event only.

| cell | brush | dabs/ev | dispatches/ev | cpu p50 | submit p50 | behind |
|---|---|---:|---:|---:|---:|---:|
| 1920x1080, 1 px | Ink Pen | 490.9 | 490.9 | 7.0 ms | 3.0 ms | +2 ms |
| | Pencil | 490.9 | 490.9 | 7.5 ms | 2.3 ms | +7 ms |
| | Dry Smudge | 490.9 | 981.9 | 7.5 ms | 3.8 ms | +8 ms |
| 1920x1080, 10 px | Pencil | 491.0 | 491.0 | 7.7 ms | 2.3 ms | +3 ms |
| | Dry Smudge | 491.0 | 982.1 | 7.3 ms | 3.8 ms | +6 ms |
| 1920x1080, 100 px | Pencil | 301.9 | 301.9 | 7.3 ms | 2.6 ms | +8 ms |
| | Dry Smudge | 121.4 | 242.8 | 6.0 ms | 3.4 ms | +6 ms |
| 2560x1440, 2000 px | Pencil | 20.5 | 20.5 | 10.5 ms | 5.4 ms | +5672 ms |
| | Dry Smudge | 8.3 | 16.5 | 6.9 ms | 4.5 ms | +2323 ms |

At the small-dab rows, where the stroke is CPU-bound, the doubled
dispatch count is inside the noise of `cpu p50` (7.3 to 7.5 ms against the
Pencil's 7.5 to 7.7 ms); `submit p50` rises by about 1.5 ms, the GPU work
the snapshots add showing up as back-pressure, and nothing falls behind.
At 2000 px on 1440p the compute terminal's large-dab regime is already
over budget for the Pencil; the Dry Smudge falls behind by less only
because its curve places fewer dabs.

**Not measured:** the browser. Every production pixel goes through the
`webgpu` backend, where a `setPipeline`, `setBindGroup` or
`dispatchWorkgroups` costs whatever the browser's GPU process charges, and
this shape doubles the dispatches and adds two pipeline switches per dab.
The browser replay harness the paint port listed as a follow-up is where
that is answered.

**Correctness note** worth carrying to the next live reader: the sampler
filters the mirror by hand from four texel loads, splitting the sample
point into the pixel's integer texel and a fraction taken from `motion`
alone. Through `graph_smp` the hardware filter's fixed-point weights depend
on the layer's size, so a dab re-rendered after the layer grew (the full
re-render path) read a different LSB than the same dab rendered before
the grow, and `tests/stroke_rewind.rs`'s oracle caught it.

### #10: windowed relaxation and one stroke flush per frame (shipped)

**Shape:** not a terminal change; the stabilizer and the per-event cycle in
front of every brush. The stabilization rework raised the Laplacian's
sweeps at strength 1 from 10 to 160 and resampled the polyline to a vertex
every 6 CSS px, while each push still relaxed the whole polyline from its
raw points, so a push cost `sweeps x stroke length` and grew as the stroke
did. Now a push relaxes only the `N + 2` vertices its influence bound says
can move, replaying the recorded per-sweep values of the vertex left of the
window, which is bit-identical to the from-scratch result
(`brush/stabilizers/laplacian.rs`; `docs/brush/stabilization.md` has the
argument). Separately, `stroke_to` only feeds the stabilizer, and the
rewind, replay and commit run once per frame in `flush_stroke`, from
`render` and `end_stroke`: the brush tool forwards every coalesced pointer
sample, so a tablet faster than the display ran the whole cycle several
times per frame.

**Measurements, on this machine** (Intel Raptor Lake-P iGPU, Vulkan, Mesa
26.2.3), `stroke_replay_bench --brush Sponge --dab-size 256` over the
recorded curvy stroke at `stabilize = 1` (the Sponge's default), prediction
at the config default of 15 ms, one run of two shown (the other agrees
within 0.3 ms on every column). Non-submit CPU is the median of
`cpu_us - submit_us` over the first and last 40 events.

| build | canvas | dabs | cpu p50 | cpu p95 | non-submit, events 0-40 / 160-204 |
|---|---|---:|---:|---:|---:|
| dev `c933fd27` | 1280x720 | 6419 | 3.7 ms | 5.7 ms | 1.3 / 1.3 ms |
| before | 1280x720 | 9493 | 8.1 ms | 12.9 ms | 4.6 / 9.8 ms |
| windowed relaxation only | 1280x720 | 9493 | 6.4 ms | 8.6 ms | 4.3 / 5.6 ms |
| both | 1280x720 | 9416 | 6.1 ms | 7.9 ms | 3.8 / 5.1 ms |
| both | 1920x1080 | 14130 | 6.8 ms | 9.1 ms | 4.1 / 6.1 ms |

The identical dab count of the windowed row is the exactness, seen from the
bench. The last rows' smaller count is the resampler's window losing the
per-event commit cap it no longer needs (169 to 161 vertices at strength
1), which spaces the checkpoint ring one vertex tighter. 1080p is new: the
bench now runs on `bench_device`, whose limits admit a 1920 px layer.

**What the native bench cannot show:** the per-frame flush. `replay`
flushes after every event, because the recording carries one event per
frame, so its per-event numbers are a frame's. On a tablet that samples at
several times the display rate the browser ran the rewind and replay once
per sample; it now runs them once per frame. Not measured in the browser.

**What is left, measured:** with startup asset decoding set aside,
`LaplacianStabilizer::push` is still the largest single cost per event
(38% of the profile's samples). Each relaxation is now constant, but the
resampler commits about ten vertices per event on this stroke and each is
its own push, so an event relaxes ten windows of `N x (N + 1)` updates
where one window widened by the commits would do. That, not the window, is
why the non-submit column still sits above dev's.

## Background changes that are NOT competing attempts

These landed for different reasons over the same time window. Listed
so we don't accidentally re-litigate them; they're orthogonal to
the per-event compute structure.

- **Premultiplied scratch + fixed-function blend**: eliminated a
  per-dab `copy_texture_to_texture` mirror copy in the fragment path.
  Helped (#1); irrelevant to (#2)/(#3).
- **Deferred composite batching**: collapsed N per-dab fragment
  passes into one pass with N draws. Optimization on (#1); obsoleted
  by (#2).
- **One stroke flush per frame**: the bridge no longer coalesces
  `BrushStroke` events, and every pen sample reaches the stabilizer.
  The engine renders them once per frame (`flush_stroke`, from
  `render` and `end_stroke`), so the number of events between two
  frames changes what the stabilizer sees, not how many times the
  stroke rewinds and replays. See #10.

## Options to explore next

> **Status as of #4 shipping.** The hybrid-router framing was rejected
> in favour of pure C (single-pass instanced fragment, shipped as #4).
> The "fragment + compute hybrid with bbox_density routing" would have
> required every brush node to grow two implementations and a generic
> dispatch system for a speculative regime (heavy overdraw on huge
> overlapping dabs) we have no evidence of in practice. The 4K +
> 2000px cell at +124 ms is the closest #4 comes to that regime and
> it's still well within real-time. If a use case ever makes heavy
> overdraw first-class, the next lever is a per-dab tile-bin or a
> coarse overdraw cull, not a fragment+compute hybrid.
>
> Options A, B, D below are kept for reference. None of them are
> active work.

### A. Tile-binning (Forward+ for dabs): *demoted*

CPU bins each queued dab's bbox into 64×64 (or 32×32) tile
coordinates. GPU dispatches one workgroup **per non-empty tile**:
dispatch scales with the *actually painted* area, not the union
bbox. Each workgroup reads its tile's dab-id slice and runs
thread-per-pixel iterate-dabs **only over those dabs**.

Wins: kills the wasted-reject problem from (#3) directly. No
cross-workgroup ordering hazard (different tiles ↔ different
pixels). Per-thread inner loop shrinks (3-5 relevant dabs vs ~30).

Costs: CPU bin construction (dab count × tiles-per-dab), two extra
storage buffers (`dab_ids[]`, `tile_offsets[]`), one extra pass to
build them. Well-trodden in tiled lighting renderers.

**Why this is demoted:** attacks shader lane-waste, which the data
now shows is the smaller component. At 4K + 1000px the shader is
3.2 ms of a 37 ms cell: even a perfect tile-bin leaves ~34 ms of
sync + submit work untouched. Worth doing eventually but won't close
the felt-experience gap on its own.

### B. Per-dab workgroup: the "one pass without per-pixel threads" shape

ONE dispatch with N workgroups (N = dab count). Each workgroup
tile-walks its own dab's bbox; threads inside are per-pixel within
that bbox.

**Fatal as-stated:** WebGPU does not order workgroups within a
dispatch. Overlapping dabs race on scratch.

Salvage paths:
- **B.1: one dispatch per dab.** Restores ordering via implicit
  pass-to-pass sync. Brings back the per-dispatch overhead we paid
  (#2) to eliminate. Probably worse than (#3).
- **B.2: non-overlap groups.** CPU sorts dabs into groups where no
  two members overlap; dispatch one group at a time, workgroup-per-dab
  inside. Helps when dabs *don't* overlap much (fast strokes); does
  nothing when they do (slow / dense strokes). Strictly weaker than (A)
  in the dense case.
- **B.3: per-pixel atomic ordering.** Atomic compare-exchange against
  the highest-id dab that has touched this pixel. Almost certainly a
  loss vs even (#3).

This is the shape the user proposed. The defensible variant is B.2;
(A) covers the same intuition more robustly.

### C. Instanced-quad render pass with fixed-function blending: *promoted as hybrid candidate*

ONE draw call, N instances (one per dab). Vertex shader emits the
per-instance bbox quad in clip space; fragment shader computes
coverage and emits premultiplied source; pipeline blend state does
source-over in hardware.

Wins: rasterizer handles thread layout, no wasted lanes; hardware
blend stage is the fastest source-over path on the GPU. **Crucially,
writes the scratch texture directly: no buffer round-trip.** This
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
eliminating the sync round-trip (the actual dominant cost there).
Ship as a sibling terminal; the brush graph picks the terminal at
compile time based on a `bbox_density` heuristic. The matrix shows
the two approaches' failure regimes are disjoint, so a hybrid would
dominate cell-by-cell.

### D. Adaptive mid-phase flush: *promoted*

CPU heuristic: if `pending_dabs_bbox` area exceeds a threshold,
force a flush mid-phase. Trades one giant sparse dispatch for a few
small dense ones.

**Why this is promoted:** at 4K + 1000px the union bbox is 11.7 M
px² per event; sync_in + sync_out total 21.7 ms. If the CPU
heuristic broke that single dispatch into four dense sub-dispatches
each covering ~3 M px² with the same total painted area, sync time
would scale roughly linearly with bbox area: the four sub-flushes
would do ~22 ms of sync work total (same), BUT the per-flush sync
cost would be ~5.4 ms each, fitting inside the 17 ms/event budget
with submit able to interleave shader work for the next sub-flush
while the GPU completes the prior one. This is the fastest path to
unblocking the 4K + ≥500px regression *without* changing the
architecture: bench-driven CPU heuristic only.

Cheap, partial; ships behind the heuristic. **Rejected in favour of
(C)**: the mechanism actually depends on intermediate
`queue.submit()` calls to pipeline CPU/GPU work, and once you accept
that complexity the buffer round-trip is still the dominant cost. (C)
removes the round-trip entirely.

### E. Chained-read terminals: answered on `paint` by the appearance mirror

Smudge, blur, liquify and watercolor all have a *semantic* dependency
between consecutive dabs: dab `n+1` reads what dab `n` wrote (the
scratch read mirror for the first three, the deposit channel for
watercolor). Neither #3 nor #4 can express it: a thread cannot read a
neighbour's post-dab value inside one dispatch, and instances of one draw
cannot see each other's writes.

The dispatch-per-dab terminal (#6) can, because dispatches in a pass are
ordered and a dispatch's stores are visible to the next. Attempt #9 is the
shape: a graph that samples the live stroke requests
`LiveSource::StrokeAppearance`, and `paint` runs a snapshot dispatch
before each dab that renders the stroke's appearance under the dab's read
region into a mirror the dab samples. That is one extra dispatch per dab,
about 1.5 us of marginal cost on this machine, against the old framing's
one render pass and one copy per dab (about 40 us, the read-mirror
terminals' shape). The Smudge is built this way, on every `paint` law at
full build-up, and the `smudge` terminal is gone.

What remains is the follow-up: delete the `blur` terminal in favour of a
sampler on `paint`, and move watercolor's pickup onto the same mechanism.
Liquify warps rather than deposits, so it is a different question. The browser's per-call cost for the doubled dispatches is
unmeasured (#9).

### F. Dispatch-per-dab on a resident storage scratch: B.1 revisited, *stages 1 and 2 passed*

Section E established that chained terminals cannot use #4 and are stuck
choosing between #1 and #2. This option asks whether a third compute shape
serves *every* terminal, chained or not, and so removes the choice.

**The shape.** One compute pass per flush. One `dispatchWorkgroups` per dab,
sized to that dab's bounding box, one thread per pixel. The stroke scratch
is a `read_write` storage texture that lives for the whole stroke, packed
RGBA8 in `r32uint` (the only read-write storage formats in core WebGPU are
32-bit single-channel). Within one compute pass, dispatches execute in
order and each one's storage writes are visible to the next, so dab `n+1`
reads dab `n`'s output through the same texture with no copy and no pass
boundary.

**Why it was dismissed before, and why that was not a measurement.** This is
option B.1 above, written off as "brings back the per-dispatch overhead we
paid (#2) to eliminate; probably worse than #3". Two premises under that
line were never tested:

1. That a dispatch inside one compute pass costs what a render pass costs.
   #2 removed per-*render-pass* overhead (attachment load/store, pass
   begin/end, bind-group rebinding); per-dispatch overhead inside a pass
   was never measured anywhere in this doc. The smudge baseline in section
   E puts one pass plus one copy at roughly 40 us per dab on the test iGPU;
   a dispatch plus the barrier wgpu inserts between dispatches should be a
   different order of magnitude, but "should" is the word to remove.
2. That the scratch must be a buffer, so the shape pays #3's texture-to-
   buffer round trip over the union bbox. A read-write storage texture
   removes the round trip: no `sync_in`, no `sync_out`, the scratch is
   simply resident. The cost model above did not consider that format.

**What it would unify if it holds.** Every accumulation law becomes shader
code applied per dab against the live ground: the wash ceiling that
`composite.wgsl` already runs across strokes would run per dab within the
stroke too (for one pigment it reproduces the max blend exactly, for
varying colour it does not fringe), build-up is source-over, a smear is a
read at an offset, watercolor's deposit is a field the shader reads. The
build channel, the max blend, the read-mirror loop and its copies, and
the "must stay on build-up" caveats all go. The instanced-versus-serialized
branch goes with them, because there is one path.

**Cost axes, predicted (to be replaced by measurement):** per-event cost
scales with `dab_count` (one dispatch each) plus `sum(dab_area)` (threads),
the same area term as #4. The catastrophe regime, if any, is the same as
#1's: many dabs per event, if per-dispatch overhead is not small.

**Decision gate, written before the run.** The spike keeps up (`behind_by_ms`
within noise of #4) on 3840x2160 at radius 1 (about 916 dabs per event) and
on the 1920x1080 rows at radius 1 and 10. If it does, the paint terminal
moves to this shape and the terminals above consolidate onto it. If it
does not, the fallback is the hybrid from section E with #4 kept only for
the one law fixed-function blending does exactly, and this section records
the numbers that closed the door.

**Spike shape.** A throwaway terminal, source-over only, with a hand-written
compute shader for a soft disc (no per-brush WGSL assembly: that is the
expensive part of the real port and is not what the gate measures), the
`r32uint` scratch, and an unpack pass at commit into the existing RGBA8
scratch so the normal commit runs unchanged. The unpack is one full-layer
pass per event that the real port would fold into `composite.wgsl`; the
bench reports it separately so the gate is judged on the dispatch cost.
Driven through `stroke_replay_matrix` under its own topology on the same
cells and the same recorded stroke as attempts #1 to #4. Result recorded
here as attempt #5, kept or removed.

**Status:** both stages passed their gates; see "#5, stage 1" and "#5,
stage 2" under Attempts, and the port of `paint` to this shape is attempt
#6. The one regime where the fragment path wins, enormous dabs on an
integrated GPU, is recorded there and under #6.

## What the user proposed

> "We can do all the dabs in one pass without assigning each pixel a thread."

**Shipped as #4.** The closest literal match, option (C) instanced-
quad render pass, turned out to also be the closest robust-perf
match, because it eliminates the buffer round-trip that was the
dominant cost in #3 *and* the per-dab `begin_render_pass` overhead
that was the dominant cost in #1. The two failure regimes from the
prior attempts disappear in one move.

## Instrumentation status

> **Historical.** The 6-slot GPU-timestamp split
> (`sync_in` / `shader` / `sync_out`) was specific to the
> compute path's buffer round-trip and was removed alongside
> `paint_compute`. The bench harness now surfaces `cpu_us`,
> `submit_us`, and the per-flush dab + union-bbox workload
> vectors. CPU/submit alone proved sufficient for #4: both
> failure axes that motivated the timestamp split disappeared
> structurally, so there's no per-flush GPU sub-cost left to
> attribute.

**Still present (active):**

- ✅ Deterministic bench stroke: captured via the `?_RECORD_STROKES=1`
  recorder ([crates/darkly/tests/fixtures/recorded_curvy_stroke.json](crates/darkly/tests/fixtures/recorded_curvy_stroke.json))
  and re-runnable across approaches via `stroke_replay_matrix`.
- ✅ Per-event `submit_us` exposed via `BrushPerfDelta`: the
  back-pressure waterfall that was hiding inside `cpu_us` is a
  first-class bench column.
- ✅ Per-event `dabs_total` and `union_bbox_area_total` surfaced from
  `BrushPerfCounters` so cells can be read by the actual workload the
  engine fed the GPU rather than the nominal radius axis. Markdown
  carries the per-event averages; TSV carries the per-flush vectors
  if a future analysis wants them.

**Removed with `paint_compute`:**

- ❌ `PaintComputeTimestamps` (6-slot query set per flush, gated on
  `TIMESTAMP_QUERY_INSIDE_ENCODERS`).
- ❌ `EventTiming::gpu_shader_ns` / `gpu_sync_in_ns` / `gpu_sync_out_ns` /
  `gpu_samples`: the bench's `EventTiming` no longer carries these.

**Notes from the prior instrumentation pass (kept for context):**

A smoke run on `dab-compute-shader` against #3 had read
`gpu_shader_p50 = 3166 µs`, `gpu_sync_in_p50 = 12133 µs`,
`gpu_sync_out_p50 = 9595 µs`, `submit_p50 = 28835 µs` at the 4K + 1000px
cell (canonical `b146df4220` run). The "12× CPU-bound" reading from
the first synthesis was wrong: the sync copies *alone* were 6.8× the
shader pass, and `queue.submit()` back-pressure on top of that brought
`cpu_p50` to 11.8× the shader. This is what motivated #4 (eliminate
the round-trip rather than optimise within it).

**Still missing:**

- Driver dispatch-grid construction / scoreboard cost. The 6-slot
  timeline above brackets every encoder command we issue, but the
  driver's per-dispatch CPU work (workgroup scheduling, descriptor
  validation) is still folded into `submit_us` rather than measured
  separately. Probably fine (at 4K+1000px the sync columns dominate
  by an order of magnitude over what driver overhead could plausibly
  be), but worth revisiting if a future attempt shrinks the sync
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
read-write store. But the scratch is canonically a *texture*:
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
| **#1 fragment** | `dab_count` (per-pass driver overhead) + `Σ(dab_area)` (rasterized pixels) | many dabs/event: high stabilizer × tight spacing |
| **#2 1-wg compute** | `union_bbox_area × dab_count` (64 threads chew tiles serially) + sync round-trip | large dabs anywhere: workgroup parallelism is the bottleneck |
| **#3 thread-per-pixel** | `union_bbox_area` (sync copies + dispatch grid) | few large dabs spread over a big bbox |
| **#4 instanced fragment** | `Σ(dab_area)` (rasterized pixels): no per-pass overhead, no round-trip | heavy overdraw on huge overlapping dabs (theoretical; not reached in the matrix) |
| **#6 dispatch per dab** *(shipped)* | `dab_count` (one dispatch each, a few us) + `Σ(dab_area)` (one thread per pixel, a storage read-modify-write each): no pass per dab, no round-trip, the scratch is resident | huge dabs on a bandwidth-bound integrated GPU: about 2x the blend unit per pixel at 4K with 1000 px dabs |

A 1px dab on a 4K canvas: #1 pays 1 render pass + ~1 pixel
rasterized. #3 pays a sync round-trip of ~4 KB. #1 wins handily,
but only because the dab count stays low. 916 dabs/event on the
same canvas (4K + 1px Ink Pen, tight spacing) is where #1
catastrophically loses to its own driver overhead.

A few-dabs-over-a-big-bbox event: #1 pays a few render passes,
each rasterizing the dab footprint. #3 pays sync copies of the
entire union bbox regardless of where dabs land. #3 loses because
the round-trip cost is proportional to the *bounding rectangle*,
not the painted area.

**Implication for #4.** Eliminating *both* the per-pass driver
overhead axis (#1's failure) and the buffer round-trip axis (#3's
failure) collapses the design space: no routing decision is needed
because one approach dominates everywhere a Basic brush actually
runs. The hybrid framing earlier in this doc was the right shape
*given* #1 and #3's failure regimes; once #4 closed both, the
hybrid became unnecessary infrastructure.

## Bench synthesis

Cross-approach `behind_by_ms` on the same recording. Negative or
near-zero = the engine kept up with the recorded cadence; positive =
the engine fell behind by that many ms over a 3.5s stroke. **#4 wins
every cell.**

| canvas | radius_px | #1 fragment | #2 1-wg compute | #3 thread-per-pixel | #4 instanced fragment |
|---|---:|---:|---:|---:|---:|
| 1280×720 | 1 | **+3113** | +4 | +7 | **+17** |
| 1280×720 | 10 | **+1006** | +4 | +7 | **+15** |
| 1280×720 | 100 | +15 | +4 | +5 | **+19** |
| 1280×720 | 250 | +18 | +34 | +3 | **+16** |
| 1280×720 | 500 | +18 | **+1705** | +4 | **+17** |
| 1280×720 | 1000 | +23 | **+2936** | +3 | **+19** |
| 1280×720 | 2000 | +16 | **+1283** | +4 | **+18** |
| 1920×1080 | 1 | **+6369** | +5 | +4 | **+16** |
| 1920×1080 | 100 | +27 | +3 | +4 | **+22** |
| 1920×1080 | 250 | +16 | **+1269** | +3 | **+18** |
| 1920×1080 | 500 | +18 | **+5328** | +4 | **+20** |
| 1920×1080 | 1000 | +21 | **+11355** | +3 | **+19** |
| 1920×1080 | 2000 | +23 | **+11431** | +4 | **+18** |
| 2560×1440 | 1 | **+9515** | +5 | +4 | **+17** |
| 2560×1440 | 100 | +16 | +17 | +4 | **+18** |
| 2560×1440 | 250 | +16 | **+3099** | +5 | **+19** |
| 2560×1440 | 500 | +20 | **+8854** | +4 | **+23** |
| 2560×1440 | 1000 | +20 | **+18222** | +295 | **+18** |
| 2560×1440 | 2000 | +20 | **+22101** | +102 | **+21** |
| 3840×2160 | 1 | **+17614** | +5 | +5 | **+15** |
| 3840×2160 | 100 | +17 | **+1415** | +4 | **+19** |
| 3840×2160 | 250 | +17 | **+6765** | +36 | **+20** |
| 3840×2160 | 500 | +20 | **+16227** | **+1794** | **+21** |
| 3840×2160 | 1000 | +55 | **+33169** | **+3562** | **+20** |
| 3840×2160 | 2000 | **+1249** | **+54099** | **+4200** | **+124** |

#5 (dispatch per dab) and #6 (the port of `paint` to that shape) were
measured on a different machine and are not columns here; their
same-session comparisons against #4 are the tables under "#5, stage 2" and
"#6" above. In short: within noise of #4 everywhere except 4K with dabs of
1000 px or more, where the compute shape loses on per-pixel cost.

**Important framing: read radius as a spacing proxy.** Ink Pen's
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
  shader can't change the cells where #3 falls behind: those are
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
  collapses. This is on a different axis from #3's failure; neither
  is "GPU-slow", both are architectural.
- **The two failure axes are not the same.** `dabs/ev` ranges from
  916 (4K + 1px) to 3.3 (4K + 2000px). #3 keeps up at 916 and lags at
  3.3: failure axis is `bbox_area`, not `dab_count`. For Ink Pen the
  two correlate negatively, but a brush like impasto oil with pinned
  1px spacing would have 916 dabs/event *at any radius*, exposing the
  axes as independent.
- **#2 is dominated almost everywhere.** Loses to #3 on every cell
  with radius ≥ 250, never beats #1 at small dabs. Its only reason
  for existing was to escape #1's per-dab overhead, which #3 also
  escapes, without #2's single-workgroup serialization.
- **#4 dominates without a hybrid.** Once #1's per-pass overhead and
  #3's buffer round-trip are *both* eliminated in one architecture
  (one render pass per phase + N instanced draws with hardware blend),
  there's no cell where the trade-off between #1 and #3 still
  matters. The hybrid framing earlier in this doc was the right call
  given #1's and #3's actual failure modes; once the shipped #4
  removed both, the hybrid became unnecessary infrastructure.

## Working agreement

- Each new attempt is its own commit. Shader + CPU evolve together.
- New attempts append a row here: what we did, what we measured, why
  we kept it or moved on.
- Watercolor's history is under Attempts above. Smudge, blur, liquify
  and the per-dab watercolor have never been benched; see section E
  under Options to explore next before touching any of them.
