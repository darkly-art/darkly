# Compositor revision registry: one validity mechanism

Written against the `better-veils` working tree (post `unified-effect-scale`,
uncommitted). Every line reference below was verified against the tree at the
time of writing; symbol names will outlive the numbers. Input:
`handoff-compositor-revision-registry.md`.

## Independent Review

Reviewed against the `better-veils` working tree by a fresh agent. Every
inventory claim was re-verified independently; findings below cite my own
greps and reads, not the plan's.

### Inventory verification: accurate

- Call-site counts confirmed exactly: 74 `mark_dirty()` (excluding the
  definition), 25 `.mark_node_pixels_dirty(`, 13 `.mark_needs_present()`.
- `mark_effect_dirty` (`gpu/compositor.rs:3631`): confirmed zero callers,
  definition only.
- `cache_valid_through`: confirmed dead: decl `:290`, init `:933`, `None`
  resets `:2210`, `:3894`, `:4289`; never `Some`, never read.
- `target_generation`: confirmed five bump sites (`:2024`, `:2085` [inside a
  per-group loop], `:3624`, `:3872`, `:4663`), one comparison reader
  (`:4782`), one stamp write (`:4879`).
- `dirty_procedural_scratch` correction is right: cleared, filled, drained
  entirely inside `encode_dirty_layer_content` (`:3292-:3331`); pure
  retained-capacity scratch, correctly dropped from the inventory.
- `ScreenRun::needs_present` (`gpu/screen_run.rs:38`) is read by exactly one
  consumer, `has_pending_work` (`compositor.rs:5667`): the twin flag really
  is just a third input to one predicate; deleting it is sound.
- Minor overstatement: `HistogramPass` has no per-layer `invalidate`, only
  `invalidate_all` (`gpu/histogram.rs:165`) and `remove_layer` (`:175`); the
  plan's "invalidate / invalidate_all push APIs" describes only
  `ContentBoundsPass` (`gpu/content_bounds.rs:139,145`). Immaterial to the
  design.

### Finding 1: §3.3's bypass-site enumeration is incomplete (must fix)

Three direct `self.needs_present = true` sets are absent from the conversion
list: `compositor.rs:3436` (`update_animations`, screen/overlay fires),
`:3474` (`set_viewport_bg`), `:3491` (`set_pixel_filter`). All three map
cleanly to `bump_present_inputs()` and are covered *conceptually* by §3.1's
`present_inputs` row, but the plan presents §3.3 as the verified-complete
site list; as written, PR 1 would silently drop re-presents for background
color, pixel-filter, and screen-side animation changes. Enumerate them.

### Finding 2: the `Compositor::new` first-frame argument is wrong as stated (must fix)

§3.3 claims a fresh registry has "`clock ≥ 1` from construction-time bumps"
so "the first frame is stale by construction". Staleness of the composite is
`latest_composite_input() > composite_built`, and `latest_composite_input`
deliberately excludes `targets` (§3.1). If the only construction-time bumps
are `targets` bumps (`ensure_group_state`, `:2024`, is the plausible one),
then `document`/`node_pixels_any`/`animation` are all 0 = `composite_built`
and the first frame **never composites**: a blank canvas until the first
edit. Today's `needs_composite: true` at `:1294` must be replaced by an
explicit `bump_document()` in construction (or sources initialized to 1 with
stamps at 0), and §6.1 needs a fresh-engine-first-frame readback test to pin
it. Most existing tests would mask this because their first action bumps
`document` anyway.

### Finding 3: "byte-for-byte semantics-preserving" is overclaimed in two scheduling edges

Both are improvements, but the plan pledges identical behavior and should
own the deltas explicitly:

- **Lost/Outdated after a `mark_dirty`-only change.** Today
  `render_offscreen` clears `needs_composite` (`:3998`) before the acquire;
  a failed acquire then leaves all three flags false (`needs_present` was
  never set on this path), so `has_pending_work` (`:5667`) and
  `frame_needs_more` (`engine/rendering.rs:761-763`, which reads *only*
  `needs_present()`) go false and the stale surface persists until the next
  unrelated mark. The `:2259-2263` doc comment's resilience story only holds
  when `needs_present` was set. Under the plan `presented` never advances,
  so the frame retries: strictly better, but a behavior change, and one
  that headless tests cannot exercise.
- **`frame_needs_more`'s gate widens.** Today it consults only
  `compositor.needs_present()`; under the plan `needs_present()` becomes
  `latest_visual > presented`, which includes `document`/`node_pixels`/
  `animation`. Post-present the two are equivalent; in failure paths they
  are not. State this.

### Finding 4: double `sync_effect_scale` breaks the present stamp for one frame (should fix)

`sync_effect_scale` runs twice per rendered frame: `render` (`:5691`) and
`render_offscreen` (`:3968`). Today double-marking bools is idempotent.
Under §3.4, `render` captures `frame_tick` after *its* sync; the drift is
still unresolved when `render_offscreen`'s sync runs (instances rebuild
later, during the walk), so `document` is bumped a second time **after**
`frame_tick`. `finish_present` then stamps `presented = frame_tick` below
the second bump and schedules one spurious extra frame (it settles (drift
resolves in the walk) but "no behavioral drift" it is not). Simplest fixes:
capture `frame_tick` after `render_offscreen` returns (safe, only `targets`
moves mid-walk and it is excluded from `latest_visual`), or make the
composite path's sync the only one. Decide and record.

### Finding 5: §6.2's `bump_targets`-alone test contradicts §3.3

§3.3 has `resize_screen_run` bump `targets` **and** `present_inputs` on a
real resize, so calling it "schedules nothing by itself" is false under the
plan's own design: it schedules a present. Testing the `targets` exclusion
needs a test-only registry accessor (`test_bump_targets()` or similar), not
`resize_screen_run`. Reword the test.

### Verified sound (challenged and upheld)

- **`targets` exclusion from the frame gates.** Every out-of-band
  `target_generation` bump is already paired with a separate scheduling mark
  (`set_canvas_rect` `:2117`; engine `resize` → `mark_needs_present`,
  `engine/rendering.rs:778`; `bake_subtree_to_layer` → `mark_dirty`,
  `:3954`), and I verified nothing inside `render_offscreen`'s walk bumps
  document or node pixels: `encode_dirty_layer_content` (`:3285`) and
  `realize_dirty_vector_layers` (`:3053`) write textures without marks. The
  mid-frame liveness argument holds on today's tree.
- **The inversion claim.** Genuine for every consumer the plan touches:
  `content_bounds.get(id, &Revisions)` cannot answer without comparing,
  `has_pending_work` *is* the comparison, and `render_offscreen` computes
  its gate internally. Forgetting to bump remains the old bug class, as the
  handoff demands. The thumbnail cursor preserves drain-once semantics
  (queue-at-change-time, once per frame: same cadence as
  `drain_dirty_thumbnail_readbacks`, `engine/rendering.rs:651-656`), and the
  single global clock makes LayerId reuse after `remove_node` safe (any new
  bump exceeds every stale cursor). I endorse PR 3.
- **`Document::revision` non-unification**, not a dodge. Verified different
  bump sets: `undo/mod.rs:176,228` and `engine/rendering.rs:972` (undoable
  history granularity, consumed by the recorder, must exist without a GPU)
  vs. `mark_dirty`'s transient mid-drag bumps. Registry on the `Compositor`
  is the correct owner under Document Authority; nothing flows upward.
- **Mechanism count genuinely decreases**: two frame bools + twin, two
  generation maps + their invalidate APIs, the drain set, the raw counter,
  one dead field, one dead method, and the fan-out bodies, for one registry.
  Nothing additive is masquerading as consolidation. The ~+50 net production
  LOC framing is honest; the test estimate is achievable with shared
  helpers.
- **PR sequence is real.** PR 1 keeps the invalidate calls in the mark
  bodies, so it changes scheduling mechanics only; PR 2 and PR 3 are each
  self-contained absorptions with their own tests. No hidden cross-PR
  dependency found.

### Test-story gaps (fold into §6)

- Fresh-engine first-frame render (Finding 2): mandatory.
- Add/remove of a canvas-space effect layer as a *mutation* class: the
  fixture contains effects, but no battery step adds or removes one, which
  is the path that realizes/destroys instances mid-session.
- `set_viewport_bg` / `set_pixel_filter` present-scheduling cases in §6.2:
  the exact sites Finding 1 flags as unconverted.
- Content bounds: the redesign must preserve the resolved-empty terminal
  state (`content_bounds.rs:256-262`, `cached` stores `Option<[u32;4]>`) and
  move `is_pending`/`request` dedup (`:132-136`, `:181-190`) from the
  generation map to the tick pair, plan text mentions only `get`/`poll`;
  the LOC allows for it but the words should.

Verdict: revise

## Revision

All review findings addressed in the plan body below; none rejected. The
line-verified changes:

- **Finding 1**: the three direct `needs_present = true` sites (`:3436`
  screen/overlay animation fires, `:3474` `set_viewport_bg`, `:3491`
  `set_pixel_filter`) are now enumerated in §3.3's conversion list, and §6.2
  gains scheduling cases for `set_viewport_bg` / `set_pixel_filter`.
- **Finding 2**: §3.3 now requires an explicit `bump_document()` in
  `Compositor::new` (the reviewer is right: construction-time bumps are
  `targets` bumps, which the composite gate excludes, so the old argument
  produced a never-compositing first frame). §6.1 gains a mandatory
  fresh-engine first-frame readback test.
- **Finding 3**: §2 no longer claims byte-for-byte scheduling equivalence;
  the two owned deltas (Lost/Outdated retry after a `mark_dirty`-only change,
  `frame_needs_more`'s widened gate) are stated in §2 and carried as a risk
  note in §7. Both are strict improvements in failure paths and identical in
  steady state.
- **Finding 4**: decided and recorded in §3.4: `render` captures
  `frame_tick` *after* `render_offscreen` returns, so the second
  `sync_effect_scale`'s drift bump lands before the capture. Safe because
  only `targets` moves during the walk and it is excluded from
  `latest_visual`.
- **Finding 5**: §6.2's `targets`-exclusion test now uses a test-only
  `test_bump_targets()` registry accessor instead of `resize_screen_run`
  (which legitimately schedules a present under this design).
- **Minor overstatement**: §1 now states `HistogramPass` has only
  `invalidate_all` (plus `remove_layer`); the per-layer `invalidate` exists
  only on `ContentBoundsPass`.
- **Test gaps**: §6.1 adds add/remove of a canvas-space effect layer as a
  mutation class; §3.4 and §6.3 now name the content-bounds
  resolved-empty terminal state and the `is_pending`/`request` dedup as
  behavior the tick-pair redesign must preserve.

## Implementation outcome

All three PRs are implemented in the working tree. Every gate in `CLAUDE.md`
passes: `cargo fmt --check`, both clippy invocations, `svelte-check` (0 errors),
`tsc`, `vite build`, `npm test` (756), and the Rust suite at **1387 passing, 0
failing** (1354 before, plus 33 new).

**Actual LOC against the estimate.** Counting *code* lines (excluding comments
and blanks, which is how the estimate was framed): production **+273 / −236**,
net **+37**, within the estimated ~+275/−225, net ~+50. Raw line counts are
higher (+532 production) because roughly 150 of the added lines are doc
comments, matching the density of the code around them. `gpu/revisions.rs` is
204 lines total, 92 of them code.

The **test battery overran**: 828 lines / 38 tests against an estimated ~395.
The excess is coverage breadth and multi-line assertion messages, not
machinery; §6.1's mutation-class list is simply long.

A DRY pass over the battery brought it from 871 lines / 33 tests to 828 / 38.
The three duplication clusters behaved differently, which is worth recording:

- The **byte-equality battery** is a table of mutations and now reads as one:
  a macro generates each case from a name, a label, and its mutation. Density
  went from ~12 lines per case to ~9 while the case count rose from 18 to 23,
  because self-contained rows replaced five chained add-then-remove tests.
- The **scheduling truth table** compressed best: five near-identical 17-line
  tests became an `assert_schedules(label, owes_frame, composites, mutate)`
  harness plus four-line rows, and it now actually reads as a truth table. 34
  lines saved.
- The **content-bounds cluster refused to compress.** Three tests sharing a
  nine-line poll loop is below the threshold where abstraction pays: a free
  function needed eight parameters, and a harness struct cost more in methods
  than the duplication cost in repetition. Both attempts made the section
  *larger* than the inline original (215 → 223). The harness is kept because
  it removes genuine copy-paste and reads better, but the honest lesson is
  that three repetitions did not justify the extraction on size grounds.

**Four findings from implementation, all folded in above:**

0. **A third vacuous test, found during the DRY pass.** The filter-param case
   changed params on the fixture's `invert` effect, which declares no
   parameters (`gpu/effects/invert.rs:54`, `params: &[]`), so its `if let
   Some(first) = defs.first()` guard never fired and the test asserted
   nothing. It now uses `brightness_contrast`, which has two. Worth noting as
   a pattern: all three vacuous tests hid behind something that silently did
   nothing (a conditional, an empty document, an unrelated `mark_dirty`).
1. **Two of the tests as specified were vacuous** and were rewritten after
   being checked against deliberately broken code. The first-frame test
   originally asserted on pixels, which cannot distinguish "composited
   nothing" from "never composited" for an empty document: it passes with
   the construction bump removed. It now asserts on `composite_runs`. The
   animation test called a helper whose `settle()` runs a frame, and a frame
   that lands async work calls `mark_dirty()`, recompositing for a reason
   other than the animation source; it now quiesces first and measures the
   tick in isolation. Both now fail with their bump removed and pass with it
   restored.
2. **The byte comparison is weaker than §6.1 implies, today.** The compositor
   has no partial caching, so the walk rebuilds the whole tree and *any*
   recomposite yields correct pixels: only "never recomposited" is
   observable, which is what the `composite_runs` counter measures. The
   comparisons start catching partially-stale results when the held caching
   plans land, which is the argument for writing them now. Recorded in the
   test file's module doc.
3. **One existing test needed the widened present gate absorbed.**
   `effect_space.rs::canvas_space_animated_effect_animates` asserted that
   hiding an animated effect quiesces `frame_needs_more`. Hiding it is a
   document change that now legitimately owes a present, and a headless engine
   has no surface to discharge that on, so the test absorbs it with
   `test_clear_needs_present()`: the idiom it already uses twice earlier for
   the same reason. Production quiesces on its own, because a real
   `finish_present` advances `presented`.

The `debug_assert_eq!` pinning "only `targets` moves during the walk" is live
across all 1387 tests and never fired, which is direct evidence for §3.1's
exclusion argument.

## LOC estimate: stated first

Lines added / removed (not touched):

| area | added | removed |
|---|---|---|
| `gpu/revisions.rs` (new) | ~110 | 0 |
| `gpu/compositor.rs` | ~70 | ~110 |
| `gpu/screen_run.rs` | ~5 | ~18 |
| `gpu/content_bounds.rs` | ~30 | ~40 |
| `gpu/histogram.rs` | ~25 | ~35 |
| `engine/rendering.rs` (thumbnail cursor, test accessors) | ~30 | ~12 |
| `engine/` misc (`layers.rs`, `mod.rs` accessors) | ~5 | ~8 |
| **production total** | **~275** | **~225** |
| `tests/compositor_revisions.rs` (new) | ~380 | 0 |
| existing test adjustments | ~15 | ~10 |
| **tests total** | **~395** | **~10** |
| this plan | ~540 | 0 |
| `docs/gpu-passes.md` note | ~8 | 0 |

**Honest framing:** net production is roughly **+50 lines**, not a large
deletion. The win is mechanism count, not raw LOC: ten independent validity
mechanisms (two boolean frame flags plus a twin, a drain-once work set, two
hand-rolled generation maps with push-invalidate APIs, a raw generation
counter, a dead field, a dead method, and the `mark_dirty` fan-out body)
collapse into one registry plus two deliberately-retained per-object encode
gates. The held caching work (`composite-prefix-cache`, the per-effect output
cache) then becomes stamp comparisons against this registry instead of each
introducing its own epoch/revision machinery, which is the point of the
consolidation. If the owner weighs raw LOC over mechanism count, PR 2 and PR 3
are individually skippable (each is a self-contained absorption); PR 1 alone
is roughly +160/−95 and delivers the registry, the frame-gate unification, and
both dead-code deletions.

## 1. Problem

The compositor answers "is this derived thing still valid?" in many
independent ways, and every queued performance plan wanted to add another.
Verified inventory of the current tree (`crates/darkly/src/gpu/compositor.rs`
unless noted):

**Push-style boolean work flags**

- `needs_composite: bool` (`:705`): set at `:1294` (`new`), `:2117`
  (`set_canvas_rect`), `:2207` (`mark_dirty`), `:3432` (`update_animations`);
  cleared at `:3998`; read at `:3970` (`render_offscreen` gate) and `:5667`
  (`has_pending_work`).
- `needs_present: bool` (`:707`) plus a **second** `needs_present` on
  `ScreenRun` (`gpu/screen_run.rs:38`) with its own `mark_needs_present` /
  `clear_needs_present` / internal set in `resize` (`:121`). Both cleared
  together in `finish_present` (`:5671`); `engine/layers.rs:1742-1743` has to
  set both by hand after a screen-boundary move.

**Push entry points**

- `mark_dirty()` (`:2206`): sets `needs_composite`, loops every `GroupState`
  nulling the dead `cache_valid_through`, calls
  `content_bounds.invalidate_all()`. **74 call sites** (24 in `compositor.rs`,
  22 in `engine/layers.rs`, the rest spread over 15 files).
- `mark_node_pixels_dirty(id)` (`:2237`): inserts into `dirty_node_pixels`,
  calls `histogram.invalidate_all()`, then `mark_dirty()`. **25 call sites.**
- `mark_needs_present()` (`:2257`): ~13 call sites across engine and
  compositor.
- `mark_effect_dirty(doc, id)` (`:3631`): **confirmed zero callers** in the
  current tree (definition only). Dead.

**Hand-rolled stamp mechanisms: the pattern worth generalizing**

- `target_generation: u64` (`:764`): bumped at `:2024`
  (`ensure_group_state`), `:2085` (`set_canvas_rect`), `:3624`
  (`resize_screen_run`), `:3872` (`bake_subtree_to_layer`), `:4663`
  (`ensure_canvas_apply_scratch`); consumed by exactly one reader, the
  `EffectInstance` fingerprint compare in `structural_match` (`:4778-4782`).
- The `EffectInstance` fingerprint itself (`:550-568`): `pipeline_id`,
  `space`, `render_size`, `target_generation`, `applied_scale`, `params`,
  validity checked at the point of consumption (`sync_effect_instances`,
  `:4714`), no push-side invalidation call anywhere. `applied_scale` was added
  this session and required **zero new invalidation call sites**; that is the
  ergonomics this plan generalizes.
- `ContentBoundsPass` (`gpu/content_bounds.rs`) and `HistogramPass`
  (`gpu/histogram.rs`): structurally identical
  `cached` / `generation: HashMap<LayerId, u64>` / `pending` triples with
  push-invalidate APIs (`ContentBoundsPass` has per-layer `invalidate` plus
  `invalidate_all`; `HistogramPass` has only `invalidate_all` and
  `remove_layer`) and a "discard results whose generation moved" poll rule,
  two hand-rolled copies of a per-node revision counter.

**Dead**

- `GroupState::cache_valid_through: Option<usize>` (`:290`): initializer at
  `:933`, `None` resets at `:2210`, `:3894`, `:4289`; never assigned `Some`,
  never read. A fossil of the pre-tree flat compositor (see
  `docs/plans/composite-prefix-cache.md` Provenance; `git log -S
  cache_valid_through` confirms it was fully wired before `5badf609`).

**Corrections to the handoff inventory**

- `dirty_procedural_scratch: Vec<LayerId>` (`:846`) is **not a validity
  mechanism**. It is a retained-capacity scratch buffer: cleared, filled, and
  drained entirely within one call to `encode_dirty_layer_content` (`:3285`),
  kept on `self` purely to avoid per-frame `Vec` allocation. The actual
  per-void work state is the `DirtyFlag` on each void (`gpu/void.rs:115`).
  Nothing to consolidate; it drops out of the inventory.
- The handoff counts 75/26 call sites for the two marks; the current tree has
  74/25 (line drift from the unified-effect-scale merge). Immaterial.
- The review of `composite-prefix-cache.md` cites six `target_generation`
  bumps; the current tree has five: the sixth (the old
  `sync_resolution_scale` block in `render`) was removed when
  `sync_effect_scale` (`:3580`) replaced it.

The root problem: **validity is answered partly by push (flags, sets,
invalidate calls) and partly by pull (fingerprints)**. The push half requires
every mutation site to remember every consumer (`mark_node_pixels_dirty`
literally knows thumbnails and histograms exist), invalidates far more than it
must (`mark_dirty` nukes all content bounds for an opacity change), and forces
every new cache to add its own channel: the prefix-cache plan's review caught
it accidentally taking ~25 call sites off its own epoch, the canonical
bolting-on failure. The pull half (`EffectInstance`) has none of these
problems. This plan moves everything to pull.

## 2. Feature semantics

One registry (`Revisions`) owns a single monotonic clock and a small fixed
set of named **sources of truth**. Mutations bump the source they changed
(same call sites as today's marks; the mark methods survive as one-line
bodies). Every **derived artifact** records the clock value it was built at
and owns an explicit list of the sources it depends on; validity is computed
by comparison **at the point of consumption**: the read path *is* the check.

The failure-mode inversion the handoff demands, achieved:

- **Forgetting to bump** a revision remains exactly the class of bug that
  forgetting `mark_dirty` is today: stale output until the next coarse bump.
  Unchanged.
- **Forgetting to check becomes impossible**, because there is no flag to
  consult and no invalidate call to omit. `render_offscreen` computes its own
  staleness from the registry inside the function; `content_bounds.get()`
  compares ticks before returning a value; `has_pending_work` is a
  comparison. No caller-side check exists to forget.

v1 is **semantics-preserving**: every existing mechanism maps onto a source or
a dependency list at its current granularity. No narrowing, no new caching.
What recomposites when and what invalidates when is identical to today on
every steady-state path: the equivalence battery in §6 pins this. Two
failure-path scheduling edges change, deliberately, and both are strict
improvements:

- **Lost/Outdated acquire after a `mark_dirty`-only change.** Today
  `render_offscreen` clears `needs_composite` (`:3998`) before the acquire; a
  failed acquire then leaves every flag false (`needs_present` was never set
  on this path), `frame_needs_more` (`engine/rendering.rs:761-763`, which
  reads only `needs_present()`) goes false, and the stale surface persists
  until the next unrelated mark. Under the plan `presented` never advances on
  a failed acquire, so the frame retries. Headless tests cannot exercise
  this; it is owned here as a known, wanted delta.
- **`frame_needs_more`'s gate widens.** It consults only
  `compositor.needs_present()` today; under the plan `needs_present()` is
  `latest_visual > presented`, which also covers `document` / `node_pixels` /
  `animation`. Equivalent immediately after a successful present; not
  equivalent in failure paths, per the previous bullet.

Granularity
improvements (per-node animation bumps, narrowing `update_filter_params`,
the audit's §3.3 over-invalidation fixes) become **one-line dependency-list
edits** afterwards, each individually reviewable, which is precisely what the
prefix-cache review demanded and could not have under the push model.

## 3. Design

### 3.1 Question 1: what is a source of truth, and what is derived

A **source of truth** is a fact whose change can make derived GPU state wrong,
and which no other tracked fact implies. Five, at v1 granularity:

| source | meaning | today's push equivalent |
|---|---|---|
| `document` | any document-shaped change: tree structure, layer properties, filter/void params, canvas geometry, isolation, selection edits, undo/redo, load | `mark_dirty()` (74 sites) |
| `node_pixels: HashMap<LayerId, Tick>` (+ maintained aggregate `node_pixels_any: Tick`) | the bytes of one node's GPU texture changed, paint, fill, paste, mask edit, bake, resize, upload. The principled GPU-authoritative bulk-data exception | `mark_node_pixels_dirty(id)` (25 sites) |
| `animation` | a canvas-side animated clock advanced (void tick, canvas-effect tick) | `needs_composite = true` at `:3432` |
| `targets` | a GPU render target was recreated: accumulators, screen-run pair, apply scratch. Compositor-internal identity, invisible to the document | `target_generation` (5 bump sites) |
| `present_inputs` | something downstream of the composite changed: view transform, tool overlay, screen-run resources, screen-side effect clocks, selection visuals | `mark_needs_present()` (~13 sites) + `ScreenRun::needs_present` |

Everything else is **derived** and records what it was built from:

| derived artifact | depends on | today's mechanism replaced |
|---|---|---|
| the composite (`composite_built: Tick` on `Compositor`) | `document`, `node_pixels_any`, `animation` | `needs_composite` |
| the presented frame (`presented: Tick`) | `document`, `node_pixels_any`, `animation`, `present_inputs` | `needs_present` + `ScreenRun::needs_present` + `finish_present` + `has_pending_work` |
| per-node content bounds (tick pair per entry) | `document`, `node_pixels[id]` | `ContentBoundsPass::generation` + `invalidate` / `invalidate_all` |
| per-node histograms (tick per entry) | `node_pixels_any` | `HistogramPass::generation` + `invalidate_all` |
| effect instances (fingerprint) | `targets` (plus its existing non-registry fields) | `EffectInstance::target_generation`: rename only, already the pattern |
| thumbnails (engine-side cursor per node) | `node_pixels[id]` | `dirty_node_pixels` + `drain_dirty_pixels` |

Two things deliberately remain per-object consume-on-read gates rather than
joining the registry (see §3.6): the void `DirtyFlag` and
`VectorContent::dirty`.

**Why `targets` is not a composite/present dependency.** Today a
`target_generation` bump alone schedules nothing: it is consumed only at
`sync_effect_instances` time to rebuild instances whose bind groups point at
replaced textures. `set_canvas_rect` schedules the recomposite separately
(`:2117`). Preserving that: `targets` stays fingerprint-only, and
`set_canvas_rect` bumps `document` (canvas geometry is a document fact). This
also keeps a critical liveness property: `targets` is the only source that can
bump **mid-frame** (`ensure_group_state` during the walk,
`ensure_canvas_apply_scratch` during sync), and excluding it from the frame
gates means a frame cannot re-schedule itself forever by doing its own work.
A debug assertion in `render_offscreen` will pin the invariant that
`document`/`node_pixels_any`/`animation` do not move between capture and
commit.

**Config-derived effect scale** is not a registry source. It already has the
right shape: `sync_effect_scale` (`:3580`) pull-compares each instance's
`applied_scale` against the config at frame entry and, on drift, escalates,
today via `mark_dirty()` + `screen_run.mark_needs_present()`, tomorrow via
`bump_document()` + `bump_present_inputs()`. The point-of-consumption check
stays where it is.

### 3.2 The registry

New file `crates/darkly/src/gpu/revisions.rs` (~110 lines):

```rust
pub type Tick = u64;

/// One monotonic clock, many named sources. Every bump advances the clock
/// and stamps the source with the new value, so "did any of these sources
/// change since tick T" is a max-compare, never a scan.
pub struct Revisions {
    clock: Tick,
    document: Tick,
    node_pixels: HashMap<LayerId, Tick>,
    node_pixels_any: Tick,      // maintained max of the map
    animation: Tick,
    targets: Tick,
    present_inputs: Tick,
}

impl Revisions {
    pub fn bump_document(&mut self);
    pub fn bump_node_pixels(&mut self, id: LayerId);   // also moves node_pixels_any
    pub fn bump_animation(&mut self);
    pub fn bump_targets(&mut self);
    pub fn bump_present_inputs(&mut self);
    pub fn remove_node(&mut self, id: LayerId);        // dispose path

    pub fn document(&self) -> Tick;
    pub fn node_pixels(&self, id: LayerId) -> Tick;    // 0 if never written
    pub fn node_pixels_iter(&self) -> impl Iterator<Item = (LayerId, Tick)>;
    pub fn node_pixels_any(&self) -> Tick;
    pub fn animation(&self) -> Tick;
    pub fn targets(&self) -> Tick;

    /// Latest change across every present-relevant source.
    pub fn latest_visual(&self) -> Tick;
    /// Latest change across every composite-relevant source.
    pub fn latest_composite_input(&self) -> Tick;
}
```

`node_pixels_any` is not a mirror of the map: it is the map's maintained
maximum, written by the single `bump_node_pixels` method inside the registry.
Dependency lists live with the consumers (each artifact knows what it depends
on); the registry only serves ticks. That keeps the registry ignorant of its
consumers: the placement inversion this whole plan exists for.

### 3.3 The mark methods survive as names; their bodies collapse

The 74 + 25 + 13 call sites are **not edited** in v1. The methods keep their
names and become one-liners:

```rust
pub fn mark_dirty(&mut self) { self.revisions.bump_document(); }
pub fn mark_node_pixels_dirty(&mut self, id: LayerId) {
    self.revisions.bump_node_pixels(id);
}
pub fn mark_needs_present(&mut self) { self.revisions.bump_present_inputs(); }
```

What disappears from their bodies: the `GroupState` loop nulling
`cache_valid_through` (deleted with the field), `content_bounds.invalidate_all()`
(PR 2; bounds compare ticks on read), `dirty_node_pixels.insert` +
`histogram.invalidate_all()` (PR 2/3), and the `mark_dirty()` tail call
(implied by the dependency lists: `node_pixels_any` is a composite dep). The
write-site invariant doc comment at `:2215-2231` moves onto
`mark_node_pixels_dirty` unchanged: it is about bumps, and bumps keep the
same discipline.

Sites that bypass the marks today are re-expressed as bumps. This list is the
verified-complete set of direct flag writes outside the mark bodies
(`grep -n 'needs_present = true\|needs_composite = true'`):

- `update_animations` `:3432` (canvas-side fire) → `bump_animation()`; its
  screen/overlay branch at `:3436` → `bump_present_inputs()`.
- `set_viewport_bg` `:3474` → `bump_present_inputs()`.
- `set_pixel_filter` `:3491` → `bump_present_inputs()`.
- `set_canvas_rect` `:2117-2118` → `bump_document()` (its `:2085`
  `target_generation` bump becomes `bump_targets()`).
- `Compositor::new` `:1294` → an explicit `bump_document()` in construction,
  replacing the `needs_composite: true` initializer. This is load-bearing:
  the only construction-time bumps are `targets` bumps
  (`ensure_group_state`), and `targets` is excluded from
  `latest_composite_input` (§3.1), without the explicit bump, `document` /
  `node_pixels_any` / `animation` would all equal `composite_built` at 0 and
  the first frame would never composite. Pinned by the mandatory
  fresh-engine first-frame test in §6.1; most other tests would mask it
  because their first action bumps `document` anyway.
- The five `target_generation += 1` sites → `bump_targets()`.
- `sync_effect_scale` `:3597-3599` → `bump_document()` + `bump_present_inputs()`.
- `screen_run.resize()` already returns `bool`; `resize_screen_run` (`:3621`)
  bumps `targets` + `present_inputs` on true, and `ScreenRun`'s internal flag
  is deleted (`gpu/screen_run.rs:38,60,66-76,121`). The
  `engine/layers.rs:1743` `screen_run_mut().mark_needs_present()` becomes
  `compositor.mark_needs_present()`: the manual double-mark disappears.

### 3.4 The read paths: where checking becomes structural

**Composite** (`render_offscreen`, `:3959`):

```rust
self.sync_effect_scale();                       // may bump document
let built_at = self.revisions.clock();          // capture after sync
if self.revisions.latest_composite_input() <= self.composite_built {
    return false;                               // replaces !needs_composite
}
/* realize voids/vectors, sync, walk: may bump `targets` only */
self.composite_built = built_at;                // replaces needs_composite = false
```

**Present** (`has_pending_work` `:5666`, `finish_present` `:5671`,
`needs_present()` `:2263`):

```rust
fn has_pending_work(&self) -> bool {
    self.revisions.latest_visual() > self.presented
}
// render(): let frame_tick = self.revisions.clock();  // AFTER render_offscreen returns
// finish_present(): self.presented = frame_tick;
```

`frame_tick` is captured **after `render_offscreen` returns**, not at frame
entry. `sync_effect_scale` runs in both `render` (`:5691`) and
`render_offscreen` (`:3968`); on a scale change the second sync still sees
unresolved drift (instances rebuild later, during the walk) and bumps
`document` a second time. Capturing before it would stamp `presented` stale
and schedule one spurious extra frame per scale change. Capturing after the
walk is safe because only `targets` may bump during it (§3.1's invariant,
debug-asserted) and `targets` is excluded from `latest_visual`.

The `Lost`/`Outdated` resilience documented at `:2259-2265` falls out for
free: a failed acquire returns before `finish_present`, `presented` never
advances, the loop keeps scheduling. Headless engines (no surface) likewise
never advance `presented`, exactly as they never reach `finish_present`
today; `test_clear_needs_present` becomes `presented =
revisions.latest_visual()`.

**Content bounds** (`gpu/content_bounds.rs`): each cached entry stores the
`(document, node_pixels[id])` tick pair it was computed under; `get(id,
&Revisions)` returns `None` on mismatch. Pending requests record the same
pair; `poll` discards results whose ticks moved: the existing
"generation moved" rule, now against the shared clock. Two existing behaviors
carry over onto the tick pair and are pinned by §6.3: the resolved-empty
terminal state (`cached` stores `Option<[u32; 4]>`, `:256-262`; an empty
result is a valid answer, not a miss) and the `is_pending` / `request` dedup
(`:131-136`, which must dedup against the *current* tick pair so a stale
in-flight request does not suppress a fresh one). The `generation` map,
`invalidate`, and `invalidate_all` are deleted. Depending on `document`
preserves today's exact behavior (`mark_dirty` invalidates all bounds);
narrowing to `node_pixels[id]` alone is the audit's rec #5 and is left as a
recorded one-line follow-up with its own test.

**Histograms** (`gpu/histogram.rs`): entries record `node_pixels_any` only,
matching today, where `invalidate_all` is called solely from
`mark_node_pixels_dirty` and the comment at `:2239-2241` deliberately keeps
histograms alive across `mark_dirty`-only param drags. This is also why
`animation` is its own source rather than per-node `node_pixels` bumps in v1:
folding animation ticks into `node_pixels` would invalidate a mid-Levels-drag
histogram every void tick, a regression.

**Effect instances**: `EffectInstance::target_generation` (`:563`) renames to
`built_targets: Tick` and `structural_match` (`:4782`) compares against
`revisions.targets()`. Behavior identical; the last hand-rolled counter joins
the registry.

**Thumbnails**: §3.5.

### 3.5 Question 2: the work sets

- **`dirty_node_pixels` dissolves into a consumer-owned cursor.** Its
  drain-once semantics are real, but they are the *consumer's* cursor ("which
  changes have I already queued a readback for"), not compositor state. The
  engine gains `thumbnails_synced: HashMap<LayerId, Tick>`;
  `drain_dirty_thumbnail_readbacks` (`engine/rendering.rs:651`) becomes a scan
  of `revisions.node_pixels_iter()` queueing a readback for every id whose
  tick exceeds its cursor entry, then advancing the cursor: queue semantics
  preserved exactly (queued-once per change, at queue time not landing time).
  Cost: O(nodes-ever-painted) per frame of integer compares, on a frame loop
  that already runs O(layers) scans two to three times (`needs_animation`).
  Cleanup: `dispose_node_texture` (`:2844`) calls `revisions.remove_node(id)`
  instead of `dirty_node_pixels.remove`; the cursor drops ids absent from the
  iterator. The payoff is placement: the write path stops knowing thumbnails
  exist, and any future consumer (minimap, sync) plugs in with its own cursor
  and zero write-site edits. If the reviewer judges the scan-vs-drain trade
  not worth it, PR 3 is severable: the set survives as-is with `insert`
  moving into `bump_node_pixels`'s caller. I recommend dissolving it.
- **`dirty_procedural_scratch` was never a work set** (§1 correction): it is
  a retained-allocation buffer local to one function. Untouched.
- **The real per-object gates (void `DirtyFlag` and `VectorContent::dirty`)
  survive deliberately.** Both are already pull-at-consumption: the compositor
  asks `take_dirty()` at encode time, no external invalidate exists, and the
  user has explicitly endorsed the `DirtyFlag` protocol as the model
  (`handoff-viewport-boundary.md:178-193`). They are degenerate revisions:
  a counter with exactly one consumer, folded to a bool. Converting them to
  registry ticks would churn every void implementation for zero deleted
  mechanisms; they stay. The registry interoperates: a void that re-encodes
  already recomposites via the `animation` bump (`canvas_fires`) or the
  param-change `mark_dirty`, unchanged.

### 3.6 Question 3: where the revisions live

**On the `Compositor`, as one field: `revisions: Revisions`.** The Document
Authority resolution:

- "This node's content changed" is indeed a fact *about* the document, but a
  **revision** is not that fact; it is bookkeeping about *when derived state
  last observed* the fact. The document's own answer to "what is my state" is
  its state. Change-ordinals for cache maintenance are derived-side concerns
  and belong with the caches, exactly as `target_generation` and the
  `EffectInstance` fingerprints live compositor-side today.
- The registry is rebuildable in the only sense that matters: throwing it away
  together with its artifacts (they live on the same struct and are
  constructed together) costs one full recomposite, the same recovery
  contract `mark_dirty` provides today. Nothing in it survives save/load.
- Nothing flows upward. The document is consulted for structure and
  properties; the registry is bumped by the same engine/compositor call sites
  that already call the marks: the existing contract, not a new one.
- `Document::revision` (`document/mod.rs:164`) is **deliberately not
  unified**. It looks like a sixth source but answers a different question for
  a different consumer: "has undoable session history advanced", bumped only
  at the `UndoStack::push` chokepoint and undo/redo application
  (`undo/mod.rs:176,228`, `engine/rendering.rs:972`), sampled by the process
  recorder, and it must exist even with no compositor (Document Authority:
  reasoning about the document requires no GPU). The compositor's `document`
  source moves on transient non-undoable changes too (mid-drag params, scale
  drift), so the two counters have different bump sets by design. Recorded
  here because a reviewer will reasonably ask.

### 3.7 The safe failure mode: the inversion argument

- Every read of a derived artifact goes through a comparison that the
  artifact's own accessor performs (`render_offscreen` computes its gate
  internally; `content_bounds.get` takes `&Revisions` and cannot answer
  without comparing; `has_pending_work` *is* the comparison). There is no
  public validity flag left to consult stale and no invalidate call left to
  omit. A new consumer of any artifact inherits the check because the check is
  the only way to get the value.
- A forgotten bump produces stale output until the next coarse bump: the
  same class, frequency, and blast radius as a forgotten `mark_dirty` today.
  The equivalence battery (§7) is the net under it, exactly as it would be
  under the status quo.
- The clock is monotonic and shared, so "artifact newer than source" cannot
  occur and wraparound is a non-issue (u64 at 10⁶ bumps/second lasts ~584k
  years).

### 3.8 What is deleted, outright

- `needs_composite`, `needs_present`, `ScreenRun::needs_present` and its three
  methods, `finish_present`'s double clear.
- `cache_valid_through` and its four assignment sites (`:290`, `:933`,
  `:2210`, `:3894`, `:4289`): dead since `5badf609`.
- `mark_effect_dirty` (`:3631`): zero callers, and its doc comment misstates
  the mechanism (audit §2.3). Re-introducing its narrowing later is a
  dependency-list edit, not a resurrection.
- `mark_dirty`'s `GroupState` loop and `invalidate_all` fan-out;
  `mark_node_pixels_dirty`'s knowledge of histograms and thumbnails.
- `ContentBoundsPass::generation` + `invalidate` + `invalidate_all`;
  `HistogramPass::generation` + `invalidate_all`.
- `dirty_node_pixels` + `drain_dirty_pixels` (PR 3).
- `target_generation` as a free-standing counter (absorbed as the `targets`
  source; the fingerprint field renames).

## 4. Architectural impact

- `gpu/revisions.rs` is new generic infrastructure, named for what it is, with
  zero knowledge of its consumers: dependency lists live on the artifacts.
- `gpu/compositor.rs` loses two fields, one dead field, one dead method, and
  the fan-out bodies; gains `revisions`, `composite_built`, `presented`.
- `gpu/content_bounds.rs` / `gpu/histogram.rs` shrink; their public surface
  loses the invalidate APIs and `get`/`poll` gain a `&Revisions` parameter
  (both are only called from `Compositor` methods that have `&self.revisions`
  in scope: no borrow conflict, the passes and the registry are disjoint
  fields).
- **No document change, no WASM/frontend change, no modular-registry change**
  (`gpu/veils/`, `gpu/voids/`, `blend_modes/`, `layer_kinds/` untouched; no
  generated `mod.rs` moves).
- **Held work lands on this**: `composite-prefix-cache` PR A's
  `composite_epoch` + `node_revisions` map are this registry (its epoch =
  `document`, its per-node map = `node_pixels`); the user-endorsed per-effect
  output cache's "did the accumulator below me change" input becomes a fold of
  child-subtree ticks. Neither needs new invalidation machinery afterwards.
- The `scissor` fossil is **not** touched here: it is dead dirty-rect
  scaffolding, not a validity mechanism, and the prefix-cache plan already
  owns its deletion. No overlap.

## 5. Question 4, landing sequence: three PRs, each independently verifiable

**PR 1: the registry and the frame gates (~ +160 / −95 production).**
Introduce `Revisions`; absorb `needs_composite`, both `needs_present`s, and
`target_generation`; delete `cache_valid_through` and `mark_effect_dirty`;
re-express the marks as bumps (temporarily keeping
`content_bounds.invalidate_all()` in `mark_dirty` and
`histogram.invalidate_all()` + `dirty_node_pixels.insert` in
`mark_node_pixels_dirty`, so PR 1 changes no invalidation semantics); capture/
commit ordering per §3.4 with the debug assertion. Tests: the §7 equivalence
battery, the scheduling truth table, steady-frame no-op.

**PR 2: the derived-value caches (~ +55 / −75).** Move `ContentBoundsPass`
and `HistogramPass` onto registry ticks; delete their generation maps and
invalidate APIs; drop the temporary calls from the mark bodies. Tests:
bounds staleness after paint and after a document change; histogram survival
across a param drag (the mid-drag guard); stale-result rejection when a
readback lands after its ticks moved.

**PR 3: thumbnail queue dissolution (~ +30 / −25, severable).** Delete
`dirty_node_pixels` / `drain_dirty_pixels`; engine-side cursor per §3.5;
`dispose_node_texture` → `revisions.remove_node`. Tests: thumbnail parity
(paint → exactly one readback queued; unrelated mutation → none; delete →
cursor pruned; undo → readback).

Each PR passes the full lint/CI gate from CLAUDE.md, including
`--features darkly/testing -- --test-threads=1` for the GPU integration tests.

## 6. Question 5: test story

New `crates/darkly/tests/compositor_revisions.rs`, helpers modeled on
`tests/effect_space.rs`. No blocking readback enters production code: all
readbacks go through the existing `#[cfg(any(test, feature = "testing"))]`
`test_readback_*` accessors.

**6.1 Byte-equality battery against a from-scratch render.** Add
`DarklyEngine::test_readback_canvas_from_scratch()`: bumps every source
(one call, `Revisions::bump_all_for_test()`), forces the composite, reads
back. Then:

```rust
fn assert_matches_from_scratch(engine: &mut DarklyEngine, what: &str) {
    let incremental = engine.test_readback_canvas();
    let scratch = engine.test_readback_canvas_from_scratch();
    assert_eq!(incremental, scratch, "stale composite after {what}");
}
```

Guarded against the self-comparison hazard the prefix-cache review flagged
(its C3): the helper asserts the incremental read actually recomposited when
the mutation class requires it, via a `composite_runs: u64` test counter
(three lines, incremented beside `composite_built`'s commit).

Fixture: raster / raster / masked raster / passthrough group of two /
canvas-space `invert` / non-passthrough group holding raster + `grain` /
raster on top. Mutation classes, each one line + the assert: paint · paint
into a mask · fill · opacity · blend mode · visibility toggle · reorder ·
add layer · delete layer · add/remove mask · **add and remove a canvas-space
effect layer** (the path that realizes and destroys effect instances
mid-session) · filter param change · void param change · void transform ·
vector scene push · passthrough toggle · isolation set/clear ·
screen-boundary move · selection change · undo · redo · canvas resize (crop
and grow) · canvas transform · animation tick (`test_tick_animations`) ·
flatten · merge down · paste · floating commit.

Plus, mandatory and standalone: a **fresh-engine first-frame** test, build
an engine, render once with no prior mutation, read back, assert non-blank
against the fixture's expected pixels. This pins §3.3's construction-time
`bump_document()`; every other test in the battery would mask its absence.

**6.2 Scheduling equivalence (the flags' semantics).** A truth table pinned by
tests, since the composite/present gates are now computed:

- steady frame: two `render()`s with no mutation; second does zero work
  (`composite_runs` delta 0, `has_pending_work` false via
  `frame_needs_more`).
- `mark_dirty` → composite + present both run once.
- `mark_needs_present` alone (view pan) → present runs, composite does not
  (`composite_runs` delta 0).
- `set_viewport_bg` and `set_pixel_filter` → present runs, composite does
  not: the exact sites §3.3 converts from direct `needs_present` writes.
- animation tick on a canvas void → composite runs; on a screen effect →
  present only.
- `test_bump_targets()` alone (a test-only registry accessor; NOT
  `resize_screen_run`, which legitimately bumps `present_inputs` too and so
  schedules a present) → schedules nothing by itself, and the next composite
  rebuilds effect instances (`test_effect_rebuilds` delta), pinning §3.1's
  `targets` exclusion.
- mid-frame `targets` bump does not re-schedule: paint once, render, assert
  `frame_needs_more()` settles false.

**6.3 Cache-consumer semantics (PR 2/3).** Histogram survives a param drag;
bounds invalidate on paint; stale async results discarded; bounds
resolved-empty state survives the redesign (an empty layer's bounds resolve
to a cached empty answer, not a perpetual re-request); `is_pending` dedup
holds for the current tick pair but not across a tick move; thumbnail parity
per §5 PR 3.

**6.4 Regression framing.** This is a consolidation, not a bug fix, so there
is no fail-first regression test in the CLAUDE.md sense; §6.1/§6.2 are the
feature tests, written against PR 1's tree and green before and after each
subsequent PR: any behavioral drift the consolidation introduces fails a
named assertion.

## 7. Risks and interactions

- **A missed dependency in an artifact's list is the new staleness class**:
  the analogue of a missed `invalidate_all`. Mitigated by v1's rule that dep
  lists reproduce today's semantics verbatim (§2), by the battery, and by the
  §6.2 truth table pinning both directions (stale *and* over-eager).
- **Mid-frame bumps.** Only `targets` may legally move between capture and
  commit (§3.1); the debug assertion converts a future violation into a loud
  failure instead of an infinite render loop or a silently stale stamp.
- **Two owned scheduling deltas (§2).** The Lost/Outdated retry and the
  widened `frame_needs_more` gate change behavior only in failure paths that
  headless tests cannot exercise, and only in the direction of retrying
  rather than stranding a stale surface. If either proves undesirable in the
  browser, the old semantics are recoverable by advancing `presented` on the
  failed-acquire path, but that would be reintroducing the bug shape the
  `:2259-2265` comment warns about, so the plan does not propose it.
- **Cold-flatten defect (handoff §"Confirmed defect")**: untouched. The
  registry changes when composites are *scheduled*, not how
  `bake_subtree_to_layer` composes or when effect instances are realized; the
  unrealized-canvas-effect flatten bug reproduces identically before and
  after. Its own plan should note that `bake`'s `target_generation` bump is
  spelled `bump_targets()` after this lands. The §6.1 flatten/merge cases
  exercise only the warm path, matching the handoff's finding that the warm
  path is correct.
- **Held caching plans** must be rebased onto the registry (they were held for
  exactly this); `composite-prefix-cache.md` §3.2's table and
  `unified-effect-scale.md`'s three references to `cache_valid_through` as
  "the natural place" for prefix validity become stale prose and should be
  annotated when this lands.
- **Over-invalidation is preserved, not fixed.** `poll_pending`'s global
  `mark_dirty` on any readback (audit §3.3), the per-dab global mark at
  `engine/painting.rs:606`, and `update_filter_params`' global mark all keep
  today's cost. Fixing them becomes a per-site bump-narrowing with a test:
  explicitly the follow-up work this plan exists to make one-line.
- **Performance**: the frame gate goes from reading two bools to comparing a
  handful of u64s; the thumbnail scan is O(painted nodes) integer compares on
  a loop that already does O(layers) scans repeatedly. No allocation is added
  anywhere per-frame.

## 8. Unresolved questions

1. **`animation` granularity.** v1 keeps it coarse to preserve histogram
   semantics (§3.4). When the per-effect output cache lands it will want
   per-node ticks for animated layers; at that point the histogram dependency
   (`node_pixels_any` vs a new distinction) must be decided consciously. Flagged
   now so it is a decision then, not an accident.
2. **PR 3's trade** (scan vs drain): recommended but severable; reviewer
   should weigh the placement win against a per-frame O(painted nodes) scan.
3. **`Document::revision` unification**: argued against in §3.6; the
   reviewer should challenge that argument.
4. Whether `latest_visual` / `latest_composite_input` should be maintained
   aggregates instead of max-of-five on read. Read-side max of five u64s per
   frame is nothing; maintained aggregates add write-side coupling. Proposed:
   compute on read.

## 9. Verdict on the handoff's null hypothesis

The mechanisms **can** be unified without loss. The two that resist
(`DirtyFlag` and `VectorContent::dirty`) resist because they are already the
target pattern (validity consumed where the value is used), and keeping them
is convergence, not failure. The consolidation deletes eight push-style
mechanisms, makes the check side structurally unforgettable, and turns the
three queued caching plans' invalidation needs into stamp comparisons. The
cost is ~+50 net production lines and a genuinely new invariant to maintain
(dependency lists): smaller machinery than any one of the held plans would
have added on its own.
