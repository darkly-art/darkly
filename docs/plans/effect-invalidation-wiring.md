# Effect Invalidation Wiring

Fix two related over-invalidation problems identified in
`docs/compositor-caching-audit.md` §2.3 and §3.3 (recommendation #2). Every
claim below was re-verified against the source on branch `better-veils`
(working tree at `ddcd909a` + uncommitted changes); line numbers cite that
state.

## Independent Review

Reviewed independently against the working tree (branch `better-veils`,
`ddcd909a` + uncommitted changes). Every load-bearing claim was re-verified
against source. Verdict at the end.

### Confirmed: diagnosis and safety claims

- `Compositor::mark_effect_dirty` (`crates/darkly/src/gpu/compositor.rs:3576-3582`)
  has zero callers (grep over `crates/darkly` and `frontend/wasm`), and
  `update_filter_params` marks globally (`engine/layers.rs:885`). The cost claim
  is accurate: `mark_dirty` nulls every group cache and calls
  `content_bounds.invalidate_all()` (`compositor.rs:2201-2209`).
- Present-only sufficiency verified end to end: `has_pending_work` ORs the three
  flags (`compositor.rs:5577-5579`); `render_offscreen` early-returns on
  `!needs_composite` (`compositor.rs:3910`) and clears the flag only when it
  actually runs (`compositor.rs:3938`); `present_and_screen_run` syncs whenever
  the run is non-empty (`compositor.rs:3702-3704`); the sync re-adopts params in
  place or rebuilds (`compositor.rs:4717-4748`) and rewrites **every** instance's
  apply uniform (blend, opacity) from the document on every call
  (`compositor.rs:4800-4825`); run membership and visibility are re-derived from
  the document at present time (`compositor.rs:3689-3694`).
- Flag choice verified: `frame_needs_more` (`engine/rendering.rs:761-769`)
  consults only `compositor.needs_present()`; `ScreenRun::needs_present`
  (`gpu/screen_run.rs:44`) is invisible to it, and a `Lost`/`Outdated` acquire
  returns without presenting (`compositor.rs:5619-5626`), so the stall scenario
  is real and setting the compositor's own flag is the correct arm. No new
  double-mark site is introduced.
- All four routed call sites mutate the document **before** the mark
  (`layers.rs:882-885`, `1364-1371`, `1403-1419`, `1446-1458`), so
  `renders_in_screen_space` answers for the post-mutation tree. The borrow shape
  (`&self.doc` + `&mut self.compositor`) is disjoint-field and compiles.
- Problem B: independently audited all 18 `ReadbackContext` variants
  (`engine/mod.rs:153-333`) against the exhaustive dispatch
  (`rendering.rs:351-593`, no wildcard arm), plus the diff-rect poll, the
  content-bounds un-park, and the histogram poll. Actively hunted for a
  completion path relying on the blanket; **found none**:
  - `FloodFill` marks on every exit path (`engine/painting.rs:1533`, `1540`,
    `1557`).
  - `SelectionReadback` → `push_merged_overlay` → `mark_needs_present`
    (`engine/filters/selection.rs:958-965`), including the `!has_selection`
    early path (`selection.rs:926-928`). Every resumed op self-marks: cut via
    `clipboard.rs:273` or `gpu_clear_layer` → `region_undo_inplace`
    (`painting.rs:164`); `flip_node` (`engine/layer_flip.rs:123`);
    `apply_filter_typed` (`engine/filters/apply.rs:134`, `237`); transform
    resume, `Active` publishes (`gpu/floating_preview.rs:447`),
    `Stale`/`NoOp`/`Rejected` clear (`floating_preview.rs:299`), `Pending`
    changes nothing visible (`engine/floating.rs:357-370`, `372-557`).
  - `MagicWand`/`AlphaToSelection` → `apply_selection_full`
    (`selection.rs:713-734`) mutates only the selection texture and ends with
    `kick_selection_readback`; the visible ants arrive with the subsequent
    `SelectionReadback` mark, and `readbacks.has_pending()` keeps the loop
    alive meanwhile.
  - `BrushCursorPreviewScale` self-marks (`rendering.rs:518-521`). Everything
    else is a pure cache/result landing: `ColorPick` (`rendering.rs:360-364`),
    `Copy` completion builds clipboard bytes only
    (`clipboard.rs:356-402`; the cut's clear + mark happen at request time),
    `complete_export` (`engine/export.rs:65-71`), save, thumbnails, brush
    previews, `UndoRegionReady`, `RecordingFrame`, `PreviewFrame`,
    `NodePreview`, `ActiveBrushDab`.
  - `any_completed` indeed feeds only the blanket (the un-park branch itself
    must stay, `rendering.rs:315-326`), and the histogram-exclusion precedent
    reads exactly as cited (`rendering.rs:308-314`).
- Test B is structurally sound: `pick_color` makes no mark at request time
  (`rendering.rs:178-213`); the scheduler's `poll` begins mapping for new
  requests (`gpu/readback.rs:228-240`), so the render → `test_wait_gpu` →
  render loop resolves it; `test_flush_readbacks` bypasses `render`'s blanket
  entirely (`engine/mod.rs:1400-1409`), so only the production path can catch
  the bug; `test_wait_gpu` is `#[cfg(test)]` today (`mod.rs:1416-1422`) and
  invisible to integration tests, so the widening is required, and the sole
  in-crate user (`engine/save.rs:640`) keeps compiling.
- LOC estimate is honest; scope is minimal; deleting the blanket (with
  ownership already present in every handler) is simpler and more principled
  than narrowing it, and matches the type-owned-dispatch rule. The audit-doc
  citations (§2.3, §3.3, recommendations #2/#7) all check out.

### Findings requiring revision

1. **Test A's difference assertion is vacuous with pixelate on a flood fill.**
   `fill_layer` flood-fills a solid colour (`tests/effect_space.rs:25-38`), and
   pixelate is a pure resampling of its input (`gpu/effects/pixelate.rs`): on
   a uniform region every block averages to the same colour, so changing the
   block-size param produces a byte-identical frame. Step 5's "readback differs
   from the step-2 frame" fails **after** the fix too; the test cannot pass as
   designed. Fix: use `brightness_contrast` (float params; a brightness change
   visibly transforms a solid fill, `gpu/effects/brightness_contrast.rs:25-28`;
   declares no `needs_animation`), or keep pixelate and make the content
   non-uniform. The two flag assertions are unaffected; only the visual proof
   needs the change.

2. **"None of them changes run membership" is wrong for `set_blend_mode`.**
   Setting a blend mode on a *passthrough group* flips it to isolated
   (`layers.rs:1399-1412`), and an isolated group fails `supports_screen_space`
   (`layer.rs:751-758`), so a run-member group **and every run member below
   it** drop out of the run. The proposed change still behaves correctly (the
   mark runs after the mutation, so the clamped run answers `false` and routes
   to full `mark_dirty()`, which covers the ex-members' canvas re-entry and the
   same-frame present) but the ordering is load-bearing here in a way the
   risk note denies. Reword the risk note, and preferably add this case to
   Test A's control section (`set_blend_mode` on a passthrough run-member
   group → `test_needs_composite()`): it is exactly the trap a future reorder
   of the mark would spring.

3. **Drop `drain_readbacks`' bool too.** After the fix nothing consumes it:
   `poll_save_result` ignores the return (`engine/save.rs:217`) and the test
   flush inlines its own dispatch (`mod.rs:1400-1409`). The plan's stated
   reason for keeping it ("poll_save_result and the test flush still use the
   dispatch path") conflates using the dispatch path with using the bool.
   Minor, but the plan reasons about it explicitly, so correct it either way.

### Notes: no action required

- The rename to `mark_node_dirty` is justified, not churn: with the property
  setters routed, callers pass arbitrary node ids and must not need to know the
  id is an effect for the routing to be legal; zero existing callers means zero
  call-site churn. The adjacency to `mark_node_pixels_dirty` is real but the
  contracts differ (pixel writes queue thumbnails); the rewritten doc comments
  must carry that distinction, as planned.
- `flip_node` uses `mark_dirty` rather than `mark_node_pixels_dirty` despite
  writing pixels (`layer_flip.rs:110-123`): a pre-existing thumbnail-staleness
  nit the blanket never fixed either (it also only called `mark_dirty`); out of
  scope and unchanged by this plan.
- Prior art is not cited, acceptably: the routing mechanism already exists
  in-repo and the audit doc is the evidence base; Krita/GIMP damage-region
  designs would not change this wiring decision.
- The stale doc-comment fix on `update_filter_params` (`layers.rs:865-869`)
  should also drop the `sync_projection_states` reference: the rebuild
  actually happens in `sync_effect_instances`.

**Verdict: `revise`**, the diagnosis, safety analysis, flag choice, and the
Problem B per-variant audit all hold up under independent verification;
revision is needed for Test A's vacuous visual assertion (finding 1) plus the
two smaller corrections.

### Revision log

All three findings accepted and folded into the plan body below:

1. Test A's routed effect is now `brightness_contrast` (a `ParamEffect`, so it
   also still exercises the in-place `set_params` adoption path); a brightness
   change visibly transforms a solid fill, so the step-5 difference assertion
   is meaningful.
2. The ordering-contract risk note is rewritten to state that `set_blend_mode`
   on a passthrough run-member group **does** change run membership and that
   the post-mutation mark ordering is load-bearing; Test A gains a control
   asserting that exact case routes to a full recomposite.
3. `drain_readbacks` drops its bool as well, nothing consumes it after the
   fix (`poll_save_result` ignores the return; the test flush inlines its own
   dispatch).

Also folded in from the review notes: `update_filter_params`'s doc-comment fix
now also drops the stale `sync_projection_states` reference (the rebuild
happens in `sync_effect_instances`).

### Post-review addendum: `handoff-viewport-boundary.md` fold-in

- The pending **divider-as-a-node redesign** (handoff §2, decided but
  unplanned) will delete `set_screen_space_boundary`,
  `enforce_boundary_on_insert`, and the read clamp. This plan survives it: the
  routing consults `doc.renders_in_screen_space(id)` post-mutation, and "the
  run" remains a document-derived fact (children after the divider's index).
  Impact is mechanical: see the risk note below.
- Handoff §3.2 (effect passes re-run every frame; per-layer dirty marking is
  partial: 16 `mark_node_pixels_dirty` sites vs ~47 global `mark_dirty`)
  records the user-endorsed direction for the *next* invalidation step: a
  void-style dirty protocol for effects plus a running changed-below-me bit in
  the compose walk. This plan is a prerequisite-shaped sibling, not that work:
  it narrows two invalidation triggers but adds no per-layer skip. Noted in
  Architectural impact.

## Problem A: fine-grained effect invalidation exists but is unwired

### Symptom

Dragging a parameter slider on a **viewport-only** (screen-space) effect
recomposites the entire canvas layer tree every drag event, for an edit whose
output is entirely downstream of the composite.

### Root cause

`Compositor::mark_effect_dirty`
(`crates/darkly/src/gpu/compositor.rs:3576-3582`) implements exactly the right
routing (screen-space edit → re-present only, canvas-space edit → full
`mark_dirty()`) but has **zero callers** (verified by grep across
`crates/darkly` and `frontend/wasm`):

```rust
pub fn mark_effect_dirty(&mut self, doc: &Document, id: LayerId) {
    if doc.renders_in_screen_space(id) {
        self.screen_run.mark_needs_present();
    } else {
        self.mark_dirty();
    }
}
```

The live path, `Engine::update_filter_params`
(`crates/darkly/src/engine/layers.rs:870`, mark at line 885), calls
`compositor.mark_dirty()` unconditionally. `mark_dirty`
(`compositor.rs:2201-2209`) sets `needs_composite`, nulls every group's
`cache_valid_through`, and calls `content_bounds.invalidate_all()`: a full
recomposite plus loss of every cached content bound, per slider event.

### Why a present-only wake is sufficient for screen-space edits (verified)

- `Compositor::render` (`compositor.rs:5589`) gates on `has_pending_work`
  (`compositor.rs:5577`), which ORs `needs_composite || needs_present ||
  screen_run.needs_present()`. On a present-only wake, `render_offscreen`
  (`compositor.rs:3904`) early-returns on `!needs_composite` (line 3910), and
  the frame proceeds straight to `present_and_screen_run`
  (`compositor.rs:3678`).
- `present_and_screen_run` calls `sync_effect_instances` whenever the run is
  non-empty (`compositor.rs:3702-3704`): the deliberate second sync site.
  `sync_effect_instances` (`compositor.rs:4654`) re-reads `f.params` from the
  document, compares against the instance fingerprint, and adopts new params
  via `Effect::set_params` in place (or rebuilds) (`compositor.rs:4717-4748`).
  So a present-only wake **does** pick up new params.
- The same sync rewrites every instance's apply uniform (blend mode, opacity)
  from the document on **every** call (`compositor.rs:4800-4825`), and the run
  membership + visibility are re-derived from the document at present time
  (`compositor.rs:3689-3694`, filtering on `doc.effective_visible`). So
  opacity, blend-mode, and visibility changes on run members are also fully
  realized by a present-only wake.

### Which flag the screen arm should set

`mark_effect_dirty` currently sets `ScreenRun::needs_present`
(`gpu/screen_run.rs:44`, the second copy of the flag: audit §1.1). That flag
is **not** consulted by `frame_needs_more`
(`crates/darkly/src/engine/rendering.rs:761-769`), which only checks
`compositor.needs_present()`. Consequence: if the surface acquire hits
`Lost`/`Outdated` (render returns without presenting, flags stay set;
`compositor.rs:5619-5626`), a screen-run-flag-only wake would stall the rAF
loop until the next interaction.

Fix: the screen arm sets the **compositor's own** flag
(`self.mark_needs_present()`, `compositor.rs:2250`). This is a `Compositor`
method, so the "ScreenRun can't reach the compositor from inside `resize`"
justification for the second flag does not apply here. This deliberately does
**not** expand the two-flag wart (no new manual double-mark site like
`layers.rs:1742-1743`), and it does not attempt the full flag merge (audit
recommendation #7: out of scope, not needed for this fix).

### Which mutation paths route through the helper

Routed (each is a one-line swap of `compositor.mark_dirty()` for the helper;
the helper degrades to `mark_dirty()` for any node not in the run, so behavior
for canvas-side nodes, rasters, groups, and mask filters is unchanged):

| Path | Site | Why present-only is correct for run members |
| --- | --- | --- |
| `update_filter_params` | `layers.rs:885` | params re-adopted by the present-path sync (above): the audit's named case |
| `set_opacity` | `layers.rs:1371` | apply uniforms rewritten from doc every sync |
| `set_blend_mode` | `layers.rs:1419` | same uniform path |
| `set_layer_visible` | `layers.rs:1458` | run membership/visibility re-derived from doc at present time |

Not routed: these keep unconditional `mark_dirty()`:

- `add_filter_layer` (`layers.rs:812`): insertion lands canvas-side by default
  (`Document::enforce_boundary_on_insert`); a fresh canvas filter changes the
  composite.
- `remove_layer` / `remove_layers`, `move_layer(s)` across the divider:
  membership changes alter which side composes; also, the document mutates
  **before** the compositor mark, so `renders_in_screen_space` would answer
  for the post-mutation tree, an ordering trap for no benefit (single-shot
  events, not drag-frequency).
- `set_screen_space_boundary` (`layers.rs:1732-1743`): both sides go stale by
  definition; already correct.
- Undo/redo: `apply_undo` ends with an unconditional `mark_dirty()`
  (`rendering.rs:973`), covered independently, unchanged.

Note the existing histogram-preservation property is untouched: `mark_dirty`
deliberately does not invalidate histograms (`compositor.rs:2234-2237`), so a
canvas-side Levels drag keeps its histogram exactly as today.

### Naming

With four generic call sites (any node kind can pass through `set_opacity`
etc.), the name `mark_effect_dirty` is no longer honest. Rename to
`Compositor::mark_node_dirty(&mut self, doc: &Document, id: LayerId)`
("invalidate for an edit to this node, as cheaply as its render space allows")
and rewrite the doc comment to distinguish it from `mark_node_pixels_dirty`
(pixels changed → thumbnails + recomposite) and to drop the stale
"keyed by the param fingerprint" phrasing in `update_filter_params`'s doc
comment (`layers.rs:865-869`: the actual mechanism is `EffectInstance`'s
`Vec<ParamValue>` equality). If the reviewer prefers minimal churn, keeping
the old name and wiring only `update_filter_params` is the fallback; the
rename is preferred because consumers should not need to know the id is an
effect for the routing to be legal.

## Problem B: every completed readback forces a global recomposite

### Symptom

Any completed async readback (color pick, thumbnail, brush preview, save
blob, recording frame) triggers a full-canvas recomposite and wipes every
layer's cached content bounds, even though none of these change a single
composited pixel.

### Root cause

`Engine::render` (`crates/darkly/src/engine/rendering.rs:664-668`):

```rust
let pending_completed = self.poll_pending();
if pending_completed {
    self.compositor.mark_dirty();
}
```

`poll_pending` (`rendering.rs:277-327`) returns `true` whenever
`drain_readbacks()` dispatched **any** completed readback (or a completed
content-bounds batch un-parked a pending transform). The blanket mark then
recomposites and calls `content_bounds.invalidate_all()`: which, for the
content-bounds readbacks themselves, throws away the very results that just
landed on the next request cycle.

### Audit of every completion path (what actually needs invalidation)

Every `ReadbackContext` variant (`crates/darkly/src/engine/mod.rs:153-333`)
dispatched by `handle_completed_readback` (`rendering.rs:351`), plus the two
non-scheduler polls, was checked for composite-affecting mutations:

**Already self-marking (blanket is redundant):**

- `FloodFill` → `complete_flood_fill` calls
  `compositor.mark_node_pixels_dirty(layer_id)` on every exit path
  (`painting.rs:1533`, `1540`, `1557`).
- `SelectionReadback` → `update_selection_overlay_from_readback` →
  `push_merged_overlay` → `mark_needs_present()`
  (`filters/selection.rs:965`). Its resumed deferred ops each self-mark:
  `start_copy_readback` cut path marks at commit (`clipboard.rs:273`);
  `flip_node` marks (`layer_flip.rs:123`); `apply_filter_typed` marks
  (`filters/apply.rs:134`, `237`); transform resume; see next item.
- Transform setup (resumed from the content-bounds poll at
  `rendering.rs:317-324` or from `SelectionReadback` at `rendering.rs:411-415`):
  every outcome of `prepare_transform_session` marks through the compositor,
  `Active` requires `publish_transform_preview` →
  `publish_transform_preview_batch` → `mark_dirty()`
  (`gpu/floating_preview.rs:452`); `Stale`/`NoOp`/`Rejected` go through
  `clear_transform_session` → `mark_dirty()` (`floating_preview.rs:299`);
  `Pending` changes nothing visible. So the `any_completed` bookkeeping in
  `poll_pending` exists only to feed the blanket and can go.
- `BrushCursorPreviewScale` → sets overlay uniform + `mark_needs_present()`
  itself (`rendering.rs:518-521`).
- `MagicWand` / `AlphaToSelection` → `apply_selection_full` mutates the
  selection texture (not part of the composite) and ends with
  `kick_selection_readback`; the visible result (marching ants) arrives with
  the subsequent `SelectionReadback` completion, which marks (above). The
  frame loop stays alive meanwhile via `readbacks.has_pending()` in
  `frame_needs_more`.

**Genuinely no invalidation needed (pure cache/result landings):**

`ColorPick` (writes `last_picked_color`), `Copy` (clipboard bytes; the cut's
GPU clear + mark happen at *request* time in
`copy_with_selection`/`copy_without_selection`), `ExportImage`,
`SaveDocument`, `Thumbnail` (cache + `thumbnail_version` bump; the panel is
DOM, not canvas), `BrushStrokePreview`, `BrushThumbnailForSave`,
`BrushDabThumbnail`, `ActiveBrushDab`, `NodePreview`, `PreviewFrame`,
`UndoRegionReady` (staging→heap flip), `RecordingFrame` (recorder queue).
Also the `diff_rect` poll (deferred stroke undo commit (creates an undo
entry, changes no pixels) and the histogram poll, which is **already**
deliberately excluded from the blanket for exactly this reason
(`rendering.rs:308-313`)) the precedent this fix generalizes.

### Fix

Delete the blanket: `poll_pending` stops returning a bool, and `render` drops
the `pending_completed` / `mark_dirty` pair. Invalidation ownership moves
entirely to the completion handlers: which, per the audit above, **already
hold it**; no handler needs a new mark added. This is the type-owned-dispatch
shape: each readback kind knows what it invalidates, and the frame loop stops
second-guessing all of them.

Concretely, in `rendering.rs`:

- `fn poll_pending(&mut self) -> bool` → `fn poll_pending(&mut self)`;
  remove the `any_completed` bookkeeping around the transform-resume branch
  (keep the branch itself); end with a bare `self.drain_readbacks();`.
  Per the review, `drain_readbacks` drops its bool too: after the fix nothing
  consumes it, `poll_save_result` ignores the return (`engine/save.rs:217`)
  and the test flush inlines its own dispatch (`mod.rs:1400-1409`).
- Rewrite `poll_pending`'s doc comment to state the ownership rule, in the
  register of the existing write-site invariant on `mark_node_pixels_dirty`:
  *a completion handler that mutates composited output must mark the
  compositor itself; landing a result in a cache never marks anything.*
- `render` (`rendering.rs:664-668`) shrinks to `self.poll_pending();`.

## Architectural impact

- No new state, no new flags, no data-model changes. Problem A wires an
  existing compositor method into existing engine call sites; Problem B
  deletes a global side effect whose per-case replacements already exist.
- Document authority is respected: routing consults
  `doc.renders_in_screen_space(id)` (`document/mod.rs:555`), which reads the
  clamped run (`screen_space_run` clamps on read, so a disqualified,
  e.g. masked: run member correctly routes to full `mark_dirty`).
- The two-`needs_present`-flags wart (audit §1.1) is neither expanded nor
  merged: the fix adds no site that sets `ScreenRun::needs_present`, and the
  screen arm uses the compositor's own flag, which `frame_needs_more` already
  consults. A full flag merge remains audit recommendation #7, separate work.
- Known cosmetic overlap: `mark_node_dirty` vs `mark_node_pixels_dirty`, two
  adjacent names for "property/space-routed" vs "pixels changed". The doc
  comments disambiguate; merging them is not possible (they have different
  contracts: pixel writes must queue thumbnails).
- This plan narrows *which events* invalidate, not *how much* is invalidated:
  `mark_node_dirty`'s canvas arm is still the global `mark_dirty()`. The
  per-layer skip work (a void-style dirty protocol for effect encodes plus a
  changed-below-me bit in the compose walk (`handoff-viewport-boundary.md`
  §3.2, audit recommendation #3)) is separate, planned work that this
  change's routing helper neither blocks nor implements.

## Implementation steps

1. **Tests first** (below): add both regression tests plus the minimal test
   accessors, run them, and record both failing against unfixed code.
2. `compositor.rs`: rename `mark_effect_dirty` → `mark_node_dirty`; screen arm
   `self.screen_run.mark_needs_present()` → `self.mark_needs_present()`;
   rewrite doc comment. Add `test_needs_composite()` accessor (cfg-gated
   `any(test, feature = "testing")`, next to `test_clear_needs_present` at
   `compositor.rs:2266`).
3. `engine/mod.rs`: add cfg-gated `test_needs_composite()` and
   `test_needs_present()` passthroughs (next to the existing
   `test_clear_needs_present` at `mod.rs:888`); widen `test_wait_gpu`
   (`mod.rs:1416`) from `#[cfg(test)]` to
   `#[cfg(any(test, feature = "testing"))]` so integration tests can resolve
   buffer mappings without the scheduler-bypassing `test_flush_readbacks`.
4. `engine/layers.rs`: swap the four call sites (table above) to
   `self.compositor.mark_node_dirty(&self.doc, id)`; fix the stale
   "param fingerprint" doc comment on `update_filter_params` and drop its
   stale `sync_projection_states` reference (the rebuild happens in
   `sync_effect_instances`).
5. `engine/rendering.rs`: delete the blanket in `render`; change
   `poll_pending` to return `()`; drop `any_completed`; rewrite the doc
   comment with the ownership rule.
6. Confirm both regression tests pass; run the full gate suite at commit time.

## Tests

Both are regression tests in the CLAUDE.md sense: written first, demonstrated
failing against unfixed code, then made to pass.

### Test A: `crates/darkly/tests/effect_space.rs`

Reuses the file's existing `test_engine` / `fill_layer` / `effect` helpers.
Uses `brightness_contrast` (param-bearing (float brightness/contrast), **not**
animated (`needs_animation` is declared only by `grain`, `vhs`, `rainy_glass`),
so `frame_needs_more` is not perpetually true for unrelated reasons, and) per
the review: visibly transforming even a uniform fill, so the difference
assertion cannot pass vacuously. (The originally proposed `pixelate` is a pure
resampling: on a solid flood fill every block averages back to the same color,
making the visual assertion unpassable even post-fix.) As a `ParamEffect` it
answers `set_params` with `true`, so step 5 also exercises the in-place
adoption path.

```text
screen_space_param_edit_re_presents_without_recompositing:
  1. raster + fill; eff = effect("brightness_contrast");
     set_screen_space_boundary(1).
  2. Build the screen instance once: test_readback_screen_run(16, 16).
  3. Baseline: test_readback_canvas() (clears needs_composite via
     render_offscreen); test_clear_needs_present();
     assert !test_needs_composite().
  4. update_filter_params(eff, params-with-changed-brightness).
  5. assert !test_needs_composite()          // FAILS before fix (mark_dirty)
     assert test_needs_present()             // a present is owed
     assert test_readback_screen_run(16,16) differs from the step-2 frame:
       proves the new params reach the screen through the present path
       alone (render_offscreen inside the readback is a no-op because
       needs_composite is still false; brightness shift changes every texel
       of the uniform fill).
  6. Control A: canvas_eff = effect("brightness_contrast") below the divider;
     re-baseline; update its params; assert test_needs_composite(),
       guards against over-correcting the routing.
  7. Control B (ordering trap, per review finding 2): group a run member
     into a passthrough group inside the run, re-baseline, then
     set_blend_mode(group, non-normal). The group flips to isolated, fails
     supports_screen_space, and drops itself (plus lower run members) out of
     the run: assert test_needs_composite(): the post-mutation
     renders_in_screen_space answer must route this to a full recomposite.
     This pins the "mark after mutation" ordering a future reorder would
     spring.
```

Pre-fix failure: step 5's first assertion, `update_filter_params`
unconditionally sets `needs_composite`. (Controls A and B pass both before
and after; they exist to pin the routing's boundaries.)

### Test B: `crates/darkly/tests/engine.rs`

```text
completed_color_pick_does_not_recomposite:
  1. raster + fill; settle until !test_frame_needs_more() (drains the
     auto-queued thumbnail readbacks so the pick is the only in-flight op).
  2. Baseline: test_readback_canvas(); test_clear_needs_present();
     assert !test_needs_composite().
  3. pick_color(4.0, 4.0, PickSource::Merged).
  4. Drive completion through the PRODUCTION poll path (test_flush_readbacks
     would bypass it by dispatching handlers directly):
       loop up to 32×: render(0.0)        // begin_mapping via scheduler poll
                       test_wait_gpu()    // resolve the map callback
                       render(0.0)        // poll_pending drains ColorPick
                       break when !has_pending_color_pick()
  5. assert last_picked_color() == fill color   // the pick really landed
     assert !test_needs_composite()             // FAILS before fix
     assert !test_needs_present()               // no present owed either
```

Pre-fix failure: step 5's second assertion, the blanket
`if pending_completed { mark_dirty() }` fires when the pick drains.
`needs_composite == false` also implies `content_bounds.invalidate_all()` was
not called (it only runs inside `mark_dirty`), covering the cached-bounds
half of the symptom without a dedicated bounds accessor.

### Accessors added (test surface only)

- `Compositor::test_needs_composite(&self) -> bool`: new.
- `DarklyEngine::test_needs_composite()` / `test_needs_present()`: new
  passthroughs (`test_needs_present` reads `compositor.needs_present()`).
- `DarklyEngine::test_wait_gpu`: cfg widened to `feature = "testing"`.

## Risks and unresolved questions

- **A handler silently relying on the blanket.** Mitigated by the per-variant
  audit above (every variant checked at its handler); residual risk is
  covered by the existing suites (thumbnails, selection, transform, save,
  clipboard all have integration tests that drive readbacks to completion).
  If one surfaces, the fix is a mark **in that handler**: never restoring
  the blanket.
- **Groups inside the screen run.** `set_opacity`/`set_blend_mode` on a
  passthrough group that is a run member routes present-only. Run rendering
  for groups is itself in flux on this branch (veils-as-layers wip); since
  everything the run shows is derived at present time, present-only marking
  is consistent with whatever that path renders. Flagged for the reviewer.
- **Ordering contract (load-bearing).** `mark_node_dirty` consults the
  document, so call sites must invoke it after the doc mutation, and this is
  not merely hygiene: `set_blend_mode` on a passthrough group that is a run
  member flips it to isolated (`layers.rs:1399-1412`), which fails
  `supports_screen_space` (`layer.rs:751-758`) and drops the group **and every
  run member below it** out of the run. The routing stays correct only because
  the mark runs after the mutation, so the clamped run answers `false` and the
  edit takes the full `mark_dirty()` arm, covering the ex-members' canvas
  re-entry. All four routed sites already mark post-mutation; Test A's
  Control B pins this case so a future reorder of the mark fails loudly. The
  structural paths (add/remove/move/divider) deliberately keep unconditional
  `mark_dirty`.
- **Unresolved:** should `remove_layer` of a run member also be present-only?
  Deliberately not: see the "Not routed" rationale. Cheap to revisit later
  if removal ever becomes drag-frequency.
- **Unresolved:** keep `mark_effect_dirty` name + single call site vs the
  proposed rename + four sites (see Naming). Plan proposes the rename.
- **Divider-as-a-node redesign** (`handoff-viewport-boundary.md` §2): when it
  lands, Test A's `set_screen_space_boundary(1)` becomes a divider move, and
  Control B's mechanism (an isolated group failing `supports_screen_space`
  and being clamped out of the run) may change shape; the redesign deletes
  the read clamp, so how a disqualified member exits the run needs rechecking
  then. The tests' *semantics* (screen edit → present-only; membership-
  changing edit → recomposite) survive; only the setup calls are mechanical
  swaps. Not a reason to wait: this plan's routing rides
  `renders_in_screen_space`, which the redesign keeps as a concept.

## LOC estimate (added/removed, not touched)

- **Production:** ~+25 / −27.
  - `compositor.rs`: helper rename + flag change + comment (~+6/−6), accessor (+5).
  - `engine/mod.rs`: two passthroughs (+12), `test_wait_gpu` cfg (±1).
  - `engine/layers.rs`: four one-line swaps (±4), doc comments (±3).
  - `engine/rendering.rs`: blanket removal (−4), `poll_pending` simplification
    and doc rewrite (~+6/−10), `drain_readbacks` bool drop (−2).
- **Tests:** ~+105 (two tests including Test A's Control B; helpers reused
  from `effect_space.rs`).
- **Generated / docs:** 0 (no modular `mod.rs` regeneration involved).
