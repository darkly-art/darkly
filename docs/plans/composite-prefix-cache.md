# Composite prefix cache — reusing the unchanged bottom of the layer stack

> **Needs rebasing before implementation.** `docs/plans/compositor-revision-registry.md`
> has since been implemented. The `composite_epoch` and `node_revisions` map this
> plan proposes to add now exist as the registry's `document` and `node_pixels`
> sources (`gpu/revisions.rs`), so PR A's groundwork is done: both fossils are
> deleted, `mark_effect_dirty` is gone, and a per-composite counter
> (`composite_runs`) exists for testability. What remains is the per-group
> `Vec<ChildStamp>` — which becomes a fold of child-subtree ticks against the
> registry rather than new invalidation machinery. The review's argument that
> PR B should be a **per-effect output cache** instead of a per-group prefix is
> unaffected and still stands.

## Provenance — this mechanism already existed and was dropped

Established from git history after the review, and it reframes the plan: this is
not new machinery, it is a mechanism the compositor shipped with and lost.

`git log -S cache_valid_through -- crates/darkly/src/gpu/compositor.rs` gives
five commits. In the earliest three (`f05af709`, `57e1e0d2`, `9662cfff`) the
field was fully wired — read at the top of the composite and assigned `Some`:

```rust
let start_layer = match self.cache_valid_through {
    Some(valid_through) => valid_through + 1,
    None => 0,
};
let resuming_from_cache = start_layer > 0;
...
if !resuming_from_cache { /* clear accum */ }
...
if start_layer < num_layers {
    self.cache_valid_through = Some(num_layers.saturating_sub(1));
}
```

The same function also scissored to a dirty rect computed from `doc.dirty`, so
the original compositor had **both** halves of the optimization: resume-from-cache
and dirty-rect compositing.

`5badf609 "wip compositor refactor"` — the move from a flat layer list to the
tree of per-group `GroupState`s — dropped both. From that commit on, the reads
and every `Some` assignment are gone and only the field, its `None`
initializer and the `None` resets remain. `scissor` survived as the fossil of
the dirty-rect half; `cache_valid_through` as the fossil of the resume half.

Two honest qualifications, so this is not overclaimed:

- The old resume was **coarser than what this plan proposes**. The only value
  ever stored was `Some(num_layers - 1)` — "valid through the topmost layer" —
  so in practice it functioned as "skip the composite when nothing is dirty"
  rather than a mid-stack split. The `start_layer` machinery could express a
  partial prefix; nothing ever produced one.
- The refactor's reason for dropping it stands. A flat layer index has no
  meaning once passthrough groups inline into a parent's accumulator, which is
  the same conclusion §1.2 reaches independently. The mechanism was not
  rejected on merit; it was made ill-typed by the tree and never re-derived.

Bearing on the review's finding 3 ("PR B is over-built for the measured
evidence"): the reviewer's cost objection was to *new* machinery in the crate's
most intricate file. Restoring capability the compositor was designed around,
and deleting two fossils left by the refactor that removed it, is a different
trade — though it does not by itself answer whether the per-group prefix or a
per-effect output cache is the right shape for the tree.


Written against `better-veils` @ `06e808bb`. Every source claim below was verified
from the file; line numbers will drift, symbol names will not.

## Independent Review

Reviewed against `better-veils` @ `06e808bb`. Every finding below was checked
against source; citations are `path:line` at the reviewed commit.

This is a well-researched plan and it is unusually honest about its own premise
(§1.3, §9.1). Almost every factual claim it makes checks out. My objections are
about **two soundness holes in the safety argument**, **one narrowing the plan
performs without disclosing it**, and **the shape of PR B**, which I believe is
larger and more general than the measured evidence supports and is not the shape
the repo's own recorded direction points at.

### A. What I verified and accept

- **`cache_valid_through` is dead — confirmed, both halves.** Declared at
  `gpu/compositor.rs:290`, assigned `None` at `:928`, `:2205`, `:3845`, `:4235`,
  and nowhere else; `grep` over `crates/darkly/src` and `frontend/wasm/src`
  returns exactly those five hits, so it is never assigned `Some` and never read.
  The "wrong shape" argument also holds: `compose_group_arm` inlines a pure
  passthrough group's children into the **parent's** accumulator with the same
  `parent_group` (`gpu/compositor.rs:5453-5454`), and `compose_passthrough_masked`
  does the same (`:5546-5547`, `:5553-5554`), so a position in the accumulator
  sequence is not an index into `doc.children_of(group)`. `AccumPair` really is
  two ping-pong textures and `composite_cache` really holds only the final image.
  **Delete it. This part is unambiguously right.**
- **74 `mark_dirty()` call sites** — confirmed exactly (`grep -rn "mark_dirty()"`
  over `crates/darkly/src` + `frontend/wasm/src`).
- **Exactly three `needs_composite = true` sites outside `mark_dirty`** —
  confirmed: `:1289` (`new`), `:2112` (`set_canvas_rect`), `:3427`
  (`update_animations`); `:2202` is `mark_dirty` itself. `set_canvas_rect` does
  replace every `GroupState` wholesale at `:2078-2083`. Accepted.
- **Six `target_generation += 1` sites** — confirmed at `:2019`, `:2080`,
  `:3575`, `:3823`, `:4609`, `:5609`.
- **`compose_children` has no dirtiness filter** — confirmed
  (`:4307-4339`): `find_node` miss, `node.visible()`, `is_in_isolation_path`,
  `screen_run.contains`, and nothing else.
- **`scissor` is a threaded constant** — confirmed. Both call sites pass the full
  canvas (`:3835`, `:3920`). Minor: it is carried by **nine** functions plus
  `CompositionContext:332`, not eight — `compose_group:4224`,
  `compose_children:4301`, `compose_layer_through_projection:4846`,
  `compose_layer_arm:5029`, `compose_effect_arm:5183`,
  `snapshot_parent_accum:5270`, `apply_in_place:5331`, `compose_group_arm:5428`,
  `compose_passthrough_masked:5541`. I agree with deleting it.
- **Symptom B's measurements** are real and quoted correctly from
  `handoff-viewport-boundary.md:163-169`.
- **`canvas_effect_scale: 1.0` vs `screen_effect_scale: 0.7071`** — confirmed at
  `crates/darkly/presets/defaults.yaml:131-132`, and the "canvas output is
  document content" rationale is at `gpu/effect_scaling.rs:10-14`. §1.3's
  honesty about symptom A is correct and I endorse it.
- **`poll_pending`'s global `mark_dirty` (§8) is genuinely not a per-frame
  problem during a stroke** — `render` at `engine/rendering.rs:665-668` fires it
  only when `poll_pending` returns true, which needs `drain_readbacks` to land
  something; thumbnail readbacks are queued from `drain_dirty_pixels`
  (`engine/rendering.rs:651-656`), which `gpu_stroke_to` never populates because
  `painting.rs:606` is `mark_dirty()`, not `mark_node_pixels_dirty()`. The
  analysis holds. (Process recording at ~1.5 s and any stroke that triggers
  `ensure_layer_covers_dab` → `resize_node_texture` → `mark_node_pixels_dirty`
  (`:1511`) will still punch holes in reuse; harmless but worth expecting in the
  §7.1 counter assertions.)

### B. Blocking findings

#### B1. `CompositeMode::Authoritative` does not make the file right — `render_offscreen` early-returns

§3.5 rests its second line of defence on: "every path that writes pixels to disk
or into the document forces a from-scratch composite." **It does not.** Every
caller in §3.5's table reaches the walk through `Compositor::render_offscreen`,
which returns `false` at `gpu/compositor.rs:3916-3918` when `!needs_composite`.
Verified callers: `engine/export.rs:39`, `engine/save.rs:146`,
`engine/process_recording.rs:275`, `engine/painting.rs:985` (whose own comment at
`:980` says "no-op when clean"), `engine/preview.rs:262`, `engine/mod.rs:1108`.

So the common case for export/save is: the last composite was `Interactive` and
possibly prefix-resumed, `needs_composite` is already `false`, the mode parameter
is never consulted, and `composited_texture()` (`:3539-3541`) hands back that
`Interactive` result verbatim. Threading a mode through `compose_group` protects
nothing when `compose_group` is not called.

This is not fatal to the design, but it invalidates the plan's headline safety
claim and changes what `Authoritative` has to mean. The fix is small and must be
in the plan explicitly: `Authoritative` has to **force** the composite (set
`needs_composite = true`, or bypass the gate) as well as declining to read/write
prefix state. Note the cost that then becomes real and is currently unpriced:
`SavePurpose::Snapshot` autosaves and the process recorder fire on a timer
(`unified-effect-scale.md:115` cites `defaults.yaml:120-121` and
`recording.minIntervalSeconds: 1.5`), so every one of those would become an
unconditional full-canvas from-scratch composite where today it is frequently a
no-op. That is a regression the plan does not currently account for, and it
argues for making `Authoritative` mean "drop prefix caches and force" only on the
paths that truly persist (export, save-to-file, flatten/merge, clone source),
with recording/preview left `Interactive`.

Until this is resolved, §3.5's "defence in depth with two independent layers" is
**one** layer — the epoch. Which makes B2 more serious than it looks.

#### B2. The plan narrows ~25 more call sites than it admits, and drops an invalidation while doing it

§3.5 states: "Only two narrowed paths exist in v1 (paint dab, animation tick),
each with a byte-equality test." §3.2's table and §5 step 2 contradict this. The
table defines `mark_node_pixels_dirty` as "**the above** + `dirty_node_pixels`
+ `histogram.invalidate_all()`", where "the above" is `mark_node_content_dirty` —
and step 2 says "re-express `mark_node_pixels_dirty` on top of it". Today
`mark_node_pixels_dirty` ends in `self.mark_dirty()` (`gpu/compositor.rs:2238`).
Re-expressing it on `mark_node_content_dirty` therefore **narrows every
`mark_node_pixels_dirty` call site off the epoch**, silently, as a refactor.

That is not two narrowings. Actual call sites (excluding the definition and doc
comments): `gpu/compositor.rs:1413, 1511, 1575, 1666, 1702, 1742, 1898, 2004,
2660, 2692, 3904`; `engine/bake_common.rs:114`; `engine/clipboard.rs:273`;
`engine/rendering.rs:932`; `engine/filters/mask.rs:376, 435, 579`;
`engine/floating.rs:965, 1055`; `engine/painting.rs:164, 285, 1533, 1540, 1557,
1566`. **That is 25, not the "16 today" the table claims.** Several of them
(`:1511`, `:1575`, `:1666`, `:1702`, `:1742`, `:1898`, `:2004`) happen to be
followed by an explicit `mark_dirty()` and would survive; the rest —
`ensure_raster_layer:1413`, all three mask sites, both floating sites,
`bake_common:114`, `clipboard:273`, `rendering:932`, four `painting.rs` sites —
would not. Each of those becomes a fresh staleness surface with no test named for
it.

Second, concrete defect in the same change: **`content_bounds.invalidate_all()`
(`gpu/compositor.rs:2208`) is listed only under `mark_dirty`.** Narrowing
`mark_node_pixels_dirty` and `painting.rs:606` off `mark_dirty` therefore stops
invalidating content bounds after a paint, a texture swap, a mask edit, a paste,
or a floating commit. That cache is consumed by the transform/floating setup path
(`engine/floating.rs:392-394, 421`) and gates `handle_transform_setup_outcome`
(`engine/rendering.rs:307, 318-324`), so the observable symptom is a transform
gizmo sized to pre-paint bounds. The plan does not mention it.

The fix is available and is already the audit's recommendation #3/#5 —
`ContentBoundsPass::invalidate(layer_id)` exists at
`gpu/content_bounds.rs:139-142` with **zero callers today**. `mark_node_content_dirty`
should call it, and `HistogramPass` should get the same treatment rather than
keeping `invalidate_all()` in the narrow path. Fold that into PR A; it makes the
narrowing a strict improvement rather than a silent regression.

**Required revision:** either (a) keep `mark_node_pixels_dirty` implying
`mark_dirty` (epoch bump) in v1, so the plan's "only two narrowings" claim
becomes true, or (b) enumerate all 25 sites, route per-node `content_bounds` /
`histogram` invalidation through the new mark, and extend §7.2 to cover the
classes they represent. Do not ship it as an undisclosed refactor.

#### B3. PR B is the wrong size for the evidence — the measured win is the effect *encode*, and a per-effect output cache reaches all of it

§1.3 already concedes the prefix cache is not the bulk of symptom A. Work through
what it buys on symptom A concretely: for `[r0, r1, r2, grain]` with `grain`
animating, `first_diff = 3`, `split = 2`, and the saving is **three fullscreen
blend draws minus one full-canvas copy**. The plan says this out loud (§1.3) and
correctly refuses to claim the 10×.

So the whole of PR B rests on symptom B. But look at what symptom B's numbers
actually attribute the cost to (`handoff-viewport-boundary.md:167-169`): `invert`
+1.1 ms, `grain` +3.7 ms, `painting` **+43.5 ms**. These scale with *shader
cost*, not with stack depth — the handoff says so directly at `:172-175`
("`painting` is 169 taps per pixel and it executes every dirty frame"). The blend
draws below the effect are a rounding error inside the 8.3–10.2 ms baseline. **The
entire measured win is one `inst.scaled.encode(...)` call at
`gpu/compositor.rs:5221-5231`.**

A mechanism that reaches all of it, and nothing more, is: cache that effect's
output and skip the encode when the effect's *input* is unchanged. Concretely —
`compose_effect_arm` already computes `src` (the accumulator holding everything
below, `:5196-5204`), already has an `apply_in_place` that reads `(before, after)`
(`:5246-5257`), and already writes `after` into a scratch. Give each effect
instance its own output texture instead of the shared `canvas_apply_scratch`, and
skip lines `:5221-5231` when the running "nothing below me changed" bit is set.
Everything below still re-blends — cheap — and the apply pass still runs, so
opacity/blend/mask behaviour is untouched.

Compared to PR B this drops: the per-group prefix texture and its VRAM/eviction
open question (§9.2), the resume copy, the snapshot copy, the split-index
bookkeeping, the `Vec<ChildStamp>` diff over every child of every group, the
"convergence takes two composites" property (§9.3), the whole restructure of
`compose_group`, and most of the pressure behind B1 (an effect that skips its
encode when its input is byte-identical is far easier to argue correct than a
resumed accumulator). It keeps everything PR A builds — the epoch, the revisions,
`subtree_revision`, the narrowed marks — because it needs exactly the same "did
anything below me change" signal.

Three further reasons this shape deserves a real defence rather than §6's single
sentence:

1. **It is the recorded direction in this repo, attributed to the user.**
   `handoff-viewport-boundary.md:178-193`: "this should be the mechanism voids
   already use, not a special case … A void owns a `DirtyFlag`, marks itself from
   its own state-mutating methods, and `encode_dirty_layer_content` re-encodes
   only the dirty ones … An effect should work the same way … So it needs the
   same flag plus one more input to it: *did the accumulator below me change*."
   `docs/compositor-caching-audit.md:410-419` (rec #3) repeats it as
   "**Direction resolved**". §6 dismisses this as "anchored per effect rather
   than per group … the group-level split subsumes it" without noting that it is
   the endorsed direction, without pricing the two options against each other,
   and without observing that "subsumes" here means "costs ~2× the lines to reach
   the same measured milliseconds".
2. **The prior art the plan itself gathered points away from the general
   version.** §2.1 establishes that Krita re-blends the whole stack every merge
   and reuses only *per-node projections*, bounded by a dirty rect
   (`kis_async_merger.cpp:246`, `kis_base_rects_walker.h:413-424`). §2.3
   establishes GIMP reuses the upstream chain by topology, with no index. Neither
   maintains a prefix accumulator. The plan reports this and then builds the
   prefix accumulator anyway. A per-effect output cache is much closer to Krita's
   `N_ABOVE_FILTHY` + `dependsOnLowerNodes()` reuse
   (`kis_async_merger.cpp:226-234`, `kis_projection_leaf.cpp:276-279`) — reuse
   attached to the expensive node, not to the group's running buffer.
3. **The generality PR B buys is thin in practice.** §3.8's four cases reduce to:
   effect below the change (covered by the per-effect cache), effect above the
   change (nothing helps — the effect must re-encode either way), unchanged
   nested group (which already has its own `composite_cache` at
   `gpu/compositor.rs:4266-4284`; skipping its walk is a separate and much
   smaller change to `compose_group_arm`), and param scrubbing (deferred to §8
   anyway).

**Required revision:** §6 must price the per-effect output cache against the
group prefix cache in LOC and in expected milliseconds on the measured cases, and
justify the delta — or adopt it. Right now the plan rejects the cheaper option
in one line and the more expensive option is ~210 added lines in the crate's most
intricate file whose payoff §9.1 admits is unmeasured.

### C. Substantive findings on the mechanism as designed

- **C1. `ChildStamp` does not cover effect-instance existence.**
  `compose_effect_arm` returns early with no work when
  `!self.effect_instances.contains_key(&filter.id)` (`gpu/compositor.rs:5188-5190`)
  and when `canvas_apply_scratch` is `None` (`:5191-5193`). Both are compositor-side
  facts that are invisible to `(id, included, revision)`. If an effect below the
  split composes as a no-op on frame N because its instance did not exist, and the
  instance appears on frame N+1 without any `mark_dirty` in between, the stale
  prefix is reused and the effect never appears. `ensure_group_state:2019` and
  `ensure_canvas_apply_scratch:4609` both bump `target_generation` (which PR A
  routes into the epoch), which probably closes it — but "probably" is the wrong
  standard for a plan whose thesis is that staleness is impossible. State the
  argument, or fold instance-existence into the stamp.
- **C2. §3.7's histogram guard names the wrong group.** "force a full walk for the
  group containing `t`" — the group that owns the accumulator `t` composes into is
  `doc.accumulator_host_of(t)` (`document/mod.rs:659-670`), not `parent_of(t)`,
  because a passthrough group inlines into its nearest non-passthrough ancestor.
  `compose_effect_arm` dispatches the histogram against
  `group_state[&parent_group]` (`:5211-5219`), where `parent_group` is exactly
  that host. Say `accumulator_host_of` in the plan.
- **C3. `test_readback_canvas_from_scratch` has an ordering hazard that can turn
  the §7.2 battery green while broken.** `test_readback_canvas`
  (`engine/mod.rs:1106-1119`) calls `render_offscreen`, which clears
  `needs_composite` (`:3944`). If `from_scratch` forces a composite (it must —
  see B1) it also clears the flag. So in `assert_matches_from_scratch`, if the
  *next* case's mutation fails to set `needs_composite` — which is precisely the
  bug class being hunted — the incremental read no-ops and returns the previous
  case's from-scratch bytes, and the assertion compares a texture with itself and
  passes. Guard it: assert inside the helper that the incremental readback
  actually recomposited (`composite_encodes` delta > 0, or `needs_composite` was
  set on entry). Without that guard the battery's failure mode is silent.
- **C4. §7.2's mutation list has real gaps** given the stated fear is wrong pixels
  in an exported file. Missing classes: transform preview start/update/commit
  (`effective_mask_bind_group_fields` consults `transform_session` and
  `transform_pass.paste` per draw, `:5128-5135`); floating-layer drag and commit
  (`engine/floating.rs:965`, `:1055` — both `mark_node_pixels_dirty` sites that
  B2 would narrow); selection change; a sample-merged clone stroke
  (`engine/painting.rs:985` reads the composite as paint source); paint into a
  layer *inside* a nested non-passthrough group below the split (exercises the
  nested group's own prefix cache, which §3.8 claims a win for and no listed case
  touches); an **animated void tick** as distinct from "void param change"
  (`encode_dirty_layer_content:3280` is one of the three revision-bump sites);
  a masked *passthrough* group straddling the split (`compose_passthrough_masked`
  is the one path that still snapshots the parent accumulator, `:5551`); and at
  least one **negative** case — a mutation that must NOT invalidate (a rename, a
  screen-space effect param drag) asserted via a zero `composite_encodes` delta,
  since a plan whose safety comes from over-invalidating needs a test that the
  reuse exists at all.
- **C5. `composite_encodes` is too coarse to prove the thing being sold.**
  Incrementing once per `node.compose_into` (`:4338`) counts a cheap raster blend
  and a 169-tap `painting` encode as 1 each. §7.1's assertions work, but the
  metric that would actually pin the +43.5 ms is a separate counter on
  `compose_effect_arm`'s `inst.scaled.encode` (`:5223`). Add it; it is three
  lines and it is the number the plan's justification rests on.
- **C6. Threading `CompositeMode` on `CompositionContext` (`:332`) puts a caching
  policy flag into the modular dispatch carrier** that `LayerNode::compose_into`
  (`layer.rs:699`) crosses — the seam whose whole point is that node kinds see
  only what they need. It is unavoidable if `compose_group_arm` must recurse with
  the mode, and it is still better than a scoped field on `Compositor` (which
  `unified-effect-scale.md:108` rejects on save/restore grounds, correctly). Just
  say why in the plan rather than presenting it as a free swap for `scissor`.
- **C7. `compose_children` also skips a child when `doc.find_node` returns `None`
  (`:4308-4311`).** Fold that into `ChildStamp::included` or state why it cannot
  change without a `mark_dirty`.
- **C8. §9.5's `subtree_revision` cost is understated as O(n·depth).** It is
  recomputed for every child of every group on every composite, and it is
  re-walked from scratch for a nested group both as a child stamp of the parent
  and again inside its own `compose_group`. For deep trees that is superlinear.
  Memoize per composite from the start rather than "if a large document shows it".

### D. Prior-art spot-check

I opened every cited location in `/mega/ARTEXP/darkly/krita` and
`/mega/ARTEXP/darkly/gimp`. **All eight files exist and 16 of 18 claims are
supported at or within a few lines of the citation.** Specifically verified as
exact: `kis_merge_walker.cc:28-41` (`startTripImpl`), `kis_async_merger.cpp:172`,
`:219-225`, `:226-234`, `:241-244` (`/* nothing to do */`), `:246`
(`compositeWithProjection` unconditional after the position chain),
`kis_projection_leaf.cpp:276-279` (`qobject_cast<const KisAdjustmentLayer*>`),
`kis_base_rects_walker.h:413-424`, `gimpfilterstack.c:217` and `:261-264`,
`gimpprojection.c:934` and `:894`, `gimpdrawablestack.c:192`. Two corrections:

- **§2.2's "four *disjoint* rects" is wrong.** In
  `kis_async_merger_test.cpp`, `testRect4(580,381,40,40)` lies entirely inside
  `testRect3(500,0,140,441)`. The merges are at `:91-101` (cited `:87-100`) and
  the commented-out "old style merging … artifacts at x=100 and x=500" block is
  at `:103-113` (cited `:104-114`). The *point* the plan draws from the test —
  incremental compositing had this exact failure mode and the defence is
  equality against a stored reference — stands.
- **§2.1's "`setupProjection` cleared the parent projection first" is
  conditional.** `parentOriginal->clear(rect)` is at `kis_async_merger.cpp:289`
  exactly, but only in the non-temp-projection branch; with
  `useTempProjection` it does `prepareClone(parentOriginal)` (`:281-287`)
  instead. Minor, but the plan states it unconditionally.

Line numbers for `visitHigherNode` (:86-99, cited :84-97) and `visitLowerNode`
(:101-110, cited :99-108) and GIMP's chunk snapping (:640-643, cited :639-642)
drift by 1-2. Immaterial. **Prior art is not a reason to reject this plan** — but
see B3: what the prior art actually *shows* is that neither editor built the
mechanism this plan builds.

### E. Collision with `docs/plans/unified-effect-scale.md`

Read at `06e808bb`. The collision is smaller than feared but real:

- That plan's `sync_effect_scale` signals a scale change by calling
  `Compositor::mark_dirty` (`unified-effect-scale.md:56`), **not** by touching
  `cache_valid_through` directly. Since PR A bumps `composite_epoch` inside
  `mark_dirty`, the prefix cache is invalidated by a scale change for free.
  `unified-effect-scale.md:176-182` asks for exactly this ("Recommend the prefix
  cache reuse `mark_dirty`'s existing sweep rather than adding a parallel
  invalidation channel") and this plan complies. **No functional collision.**
- Three prose references in that plan (`:56`, `:113`, `:180`) describe
  `mark_dirty` as "clears every group's `cache_valid_through`" and name the field
  as "the natural place" for prefix validity. Whichever plan lands second must
  update those three lines; if `unified-effect-scale` lands second, it will read
  as referring to a field that no longer exists.
- **Textual conflict is certain in two functions.** That plan inserts
  `sync_effect_scale` at the top of `render_offscreen` *above* the
  `!needs_composite` gate (`compositor.rs:3916`) and replaces the
  `sync_resolution_scale` block in `render` (`compositor.rs:5605-5610`) — the
  same `render` block whose `target_generation += 1` at `:5609` PR A step 4 routes
  through `bump_target_generation()`. Small, mechanical, but land them in a known
  order.
- **Worth reading before writing `Authoritative`:** `unified-effect-scale.md:167`
  records an undiagnosed bug — `bake_subtree_to_layer` composes into the sentinel
  `GroupState` (`compositor.rs:3821-3833`, `:3869`) while effect instances are
  realized against `doc.accumulator_host_of(id)` (`:4681-4683`), i.e. the *root's*
  accumulator, so `compose_effect_arm` during flatten/merge encodes from the wrong
  accumulator. That means flatten/merge output is *already* suspect, independent
  of this plan, and §3.5's row for flatten/merge is resting on a path with a
  known open defect. Not this plan's to fix; do not let `Authoritative` be read as
  a guarantee about it.

### F. Scope judgement

PR A (~175 added / ~95 removed as the plan splits it) is worth doing **on its own
merits and regardless of PR B**: it deletes two dead optimization scaffolds
(`cache_valid_through`, `scissor`), fixes a real write-site-invariant violation at
`engine/painting.rs:606`, adds the counter that makes any future caching work
testable, and — with B2's fix — converts two `invalidate_all()` sweeps into the
per-node invalidations the audit asked for at rec #5. Land it.

PR B at ~210 added lines is not justified by what is currently measured. §9.1
already gates it on a profiling measurement; I would go further and require the
gate to compare *two* candidate mechanisms, not just to decide go/no-go on one.
If the measurement confirms what `handoff-viewport-boundary.md:172-175` already
says — that the cost is the effect encode, not the walk — then the per-effect
output cache in B3 is the answer and PR B as written is over-built.

Two smaller items the plan defers to §8 that are worth pulling forward, since
they are ~1 line each and the audit ranks them above this work (rec #2,
`docs/compositor-caching-audit.md:406-409`): wiring the already-written
`Compositor::mark_effect_dirty` (`compositor.rs:3582`, zero callers) into
`update_filter_params` (`engine/layers.rs:885`, which unconditionally calls
`mark_dirty`), so a slider drag on a screen-space effect stops recompositing the
whole canvas. That is a bigger user-visible win per line than anything in PR B.

### Verdict: revise

Revise, not rethink: the diagnosis is correct, the dead-machinery deletion is
correct, the epoch-based invalidation model is the right ownership answer, and
PR A should proceed largely as written. But the plan cannot go to approval until
**B1** (the `Authoritative` guarantee is currently false), **B2** (25 undisclosed
narrowings and a dropped `content_bounds` invalidation), and **B3** (PR B's shape
priced honestly against the per-effect output cache that the repo's own recorded
direction and the measured numbers both point at) are addressed. C1-C8 are
substantive but individually small.

## 1. Problem

Two symptoms, one missing mechanism.

**Symptom A (the reported one).** An animated canvas-space effect (`rainy_glass`,
`grain`, `vhs` — the three that answer `needs_animation()`) costs roughly 10× the
GPU of the same effect above the screen-space divider.

**Symptom B (measured, `handoff-viewport-boundary.md` §3.2).** Painting on a
raster stacked *above* an effect layer costs the effect's full pass every dirty
frame, even though paint above the effect cannot change the effect's input:

| effect | baseline | with effect below | cost/frame |
|---|---|---|---|
| `invert` | 8.3 ms | 9.4 ms | +1.1 ms |
| `grain` | 17.6 ms | 21.3 ms | +3.7 ms |
| `painting` | 10.2 ms | 53.7 ms | **+43.5 ms** |

### 1.1 Root cause

`Compositor::render_offscreen` (`crates/darkly/src/gpu/compositor.rs:3910`) is
all-or-nothing. It early-returns on `!needs_composite` (`:3916`), otherwise calls
`compose_group(root, …)` (`:3940`) and clears the flag (`:3944`).
`compose_group` (`:4218`) unconditionally clears accumulator slot 0 (`:4236-4248`),
walks every child through `compose_children` (`:4256`, `:4294`), and copies the
final accumulator into the group's `composite_cache` (`:4266-4284`).

`compose_children` (`:4294`) filters children on `node.visible()` (`:4312`),
`is_in_isolation_path` (`:4319`) and the screen-space run (`:4327`) — and on
nothing else. There is no dirtiness filter anywhere in the walk. Every visible
layer re-blends and every effect re-encodes on every dirty frame.

The dirty signal itself is coarse. `mark_dirty()` (`:2201`) is global; 74 call
sites in `crates/darkly/src/` reach it. The hot ones for symptom B are
`gpu_stroke_to` (`engine/painting.rs:606`, one global `mark_dirty()` per dab,
even though the function's own signature carries the painted `layer_id` —
`engine/painting.rs:471`) and `poll_pending` → `mark_dirty()`
(`engine/rendering.rs:665-668`, fired by *any* completed readback).

For symptom A, `update_animations` sets `needs_composite = true` directly
(`:3427`) after `tick_animated_effects(…, false)` (`:3234`) has written one
uniform per animated instance. Everything below that effect in the stack is
provably unchanged and is rebuilt anyway.

### 1.2 `cache_valid_through` is dead — and the wrong shape

`GroupState::cache_valid_through: Option<usize>` (`:290`) is documented as
"Child index through which the cache is valid." Verified: it is assigned `None`
at `:928` (`create_group_state`), `:2205` (`mark_dirty`), `:3845` (bake), and
`:4235` (`compose_group`), is **never assigned `Some`**, and is **never read**.

It is also the wrong shape, independently of being dead. A "child index" cannot
name a resume point, because `compose_children` recurses into a *passthrough*
group with the **same** `parent_group` (`:5453-5454`), inlining that group's
children into the parent's accumulator. Positions in the accumulator sequence are
therefore not indices into `doc.children_of(group)`. There is also no accumulator
state to resume *from*: `AccumPair` (`:272-276`) is two textures that ping-pong,
and `composite_cache` holds only the *final* image, not any intermediate.

**Decision: delete the field and build the right thing.** The right thing keeps
split points at child boundaries of the group that owns the accumulator, and
makes a nested group atomic with respect to splitting (§3.3).

### 1.3 Be honest about symptom A

The prefix cache does **not** remove the effect's own pass when the *effect*
is what changed — an animated effect must re-encode by definition. It removes
the blend passes below it and the recomposite of unchanged sub-groups. For a
three-layer document with the veil on top, that is two fullscreen blend draws.

The 10× in symptom A is dominated by two other terms, both verified:

- **Resolution.** `rendering.canvas_effect_scale` is `1.0`
  (`crates/darkly/presets/defaults.yaml:132`) versus
  `rendering.screen_effect_scale` `0.7071` (`:131`), and canvas resolution is
  the document (2048², 4096²) while the screen run is the viewport. That is a
  ~4–8× texel-count difference before anything else. The 1.0 default is
  deliberate — `gpu/effect_scaling.rs:10-14` states canvas output is document
  content and a reduced round-trip would bake loss into the export.
- **The composite happens at all.** A screen-space animation tick sets only
  `needs_present`; a canvas-space tick sets `needs_composite`, which adds the
  entire walk, the per-group `composite_cache` full-canvas copy (`:4266`), and
  the per-frame `sync_effect_instances` churn (`:4660`, which clones a `String`
  + `Vec<ParamValue>` per filter layer per frame and calls
  `doc.all_filter_layers()` twice).

So: **this plan is worth building, and it is the general mechanism, but it is
not the bulk of the reported 10×.** §9 requires a measurement gate before
implementation, and §8 records the two cheaper levers that are *not* in scope
here. The justification that carries this plan on its own is symptom B's
measured +43.5 ms, which the prefix cache reduces to zero.

## 2. Prior art

Read from the checkouts under the project root. No claim below is unsourced.

### 2.1 Krita — recompute above the change, reuse below

Krita's incremental recomposite is a walker + merger pair. `KisMergeWalker`
(`krita/libs/image/kis_merge_walker.cc`) builds a job stack from the changed
("filthy") node:

- `startTripImpl` (`:28-40`) marks the start node `N_FILTHY`, then calls
  `visitHigherNode` upward and `visitLowerNode` on `startLeaf->prevSibling()`.
- `visitHigherNode` (`:84-97`) marks every later sibling `N_ABOVE_FILTHY` and,
  at the top of a group, recurses into the parent.
- `visitLowerNode` (`:99-108`) walks *down* marking each earlier sibling
  `N_BELOW_FILTHY`.

`KisAsyncMerger::startMerge` (`krita/libs/image/kis_async_merger.cpp:172`) then
pops the stack and, per position:

- `N_FILTHY` (`:219-225`) — recalculate the node's own projection.
- `N_ABOVE_FILTHY` (`:226-234`) — recalculate **only if**
  `currentLeaf->dependsOnLowerNodes()`, which is true exactly for adjustment
  layers (`krita/libs/image/kis_projection_leaf.cpp:276-279`). Krita's
  adjustment layer is Darkly's effect layer.
- `N_BELOW_FILTHY` (`:241-244`) — **"nothing to do"**. A node below the change
  never re-runs its filters.

The crucial detail, and the reason Krita does *not* need a prefix accumulator:
after the per-node decision, **every** leaf on the stack — including
`N_BELOW_FILTHY` — is still `compositeWithProjection(currentLeaf, applyRect)`
(`:246`), because `setupProjection` cleared the parent projection first
(`:277-303`, `parentOriginal->clear(rect)` at `:289`). Krita re-blends the whole
stack every time.

That is affordable because the merge is **rect-limited**. `applyRect` is the
dirty rect propagated by `registerNeedRect`
(`krita/libs/image/kis_base_rects_walker.h:358-428`); the `N_BELOW_FILTHY`
branch (`:413-424`) still grows `m_lastNeedRect` through each node's
`needRect()` so a blur below the change contributes its halo, but the rect stays
bounded by the crop rect. A brush dab re-blends the whole stack over a few
hundred texels.

**Reading for Darkly:** Krita's answer to symptom B is dirty-rect compositing
plus per-node projection reuse, not prefix reuse. Darkly has the scaffolding for
the first (the `scissor` parameter, §3.6) and none of it wired. For symptom A
the dirty rect is the whole canvas — a full-canvas animated veil dirties
everything — so rect limiting buys nothing there, and prefix reuse is the only
lever on the walk. The two mechanisms are complementary, not alternatives; this
plan builds prefix reuse and explicitly defers dirty rects (§8).

### 2.2 Krita — how they keep it from going stale

`KisAsyncMergerTest::testMerger`
(`krita/libs/image/tests/kis_async_merger_test.cpp:51-121`) builds a document
with a paint layer, a group, and a **blur adjustment layer** inside the group,
then runs four *separate* incremental merges over four disjoint rects
(`:87-100`) and asserts the accumulated result matches a stored reference image
(`:121`, `TestUtil::compareQImages`). The commented-out block at `:104-114`
records what the "old style merging" produced: "has artifacts at x=100 and
x=500". The test exists because incremental compositing had exactly this failure
mode, and the defence is an equality assertion against a from-scratch reference.
§7.2 copies that shape.

### 2.3 GIMP — invalidation follows the dataflow, and regions are the unit

GIMP builds the layer stack as a **linear GEGL chain**:
`gimp_filter_stack_get_graph` (`gimp/app/core/gimpfilterstack.c:188-226`) starts
from the input proxy and calls `gegl_node_link(previous, node)` per filter
(`:217`); `gimp_filter_stack_add_node` (`:233-264`) splices a new node between
its `node_below` and `node_above` (`:261-264`). "Everything below me" is
literally the upstream of the chain, so invalidation propagates downstream only —
the unchanged prefix is reused by construction, with no index to maintain.

The projection is region-invalidated, never wholesale.
`gimp_projection_projectable_invalidate`
(`gimp/app/core/gimpprojection.c:916-934`) turns a layer-stack update into
`gimp_projection_add_update_area` (`:625-654`), which snaps the rect to the
`GIMP_PROJECTION_UPDATE_CHUNK_*` grid (`:639-642`) and unions it into
`update_region`. `gimp_projection_paint_area` (`:860-900`) then either validates
that rect now or calls `gimp_tile_handler_validate_invalidate` (`:894`) so only
those tiles are re-rendered on the next read. Layer edits reach this through
`gimp_drawable_stack_drawable_update`
(`gimp/app/core/gimpdrawablestack.c:193-224`).

**Reading for Darkly:** GIMP's per-node result reuse costs nothing to maintain
because it is a property of the graph topology, not of a remembered index. That
is the standard this plan's invalidation model is held to in §3.2 — the
compositor must not carry a hand-updated "lowest dirty child".

## 3. Design

### 3.1 The invariant

For a group `G` with children `c_0 … c_{n-1}` in document order (bottom to top),
define the **prefix state through `k`** as the contents of `G`'s accumulator
after clearing and composing `c_0 … c_k` under the current walk.

> **Invariant.** The prefix state through `k` is byte-identical to the previous
> composite's prefix state through `k` iff, for every `i ≤ k`, the *child stamp*
> of `c_i` is unchanged, and the composite epoch and target generation are
> unchanged.

A child stamp is a plain-equality value (no hashing — see §3.4):

```
struct ChildStamp {
    id: LayerId,
    included: bool,   // survives visible / isolation / screen-run filters
    revision: u64,    // max content revision over the child's whole subtree
}
```

`revision` is folded over the child's subtree: the node itself, its `filters()`
list (masks — a mask is not in `children_of`, so it must be folded explicitly),
and recursively every descendant's node and filters.

The composite epoch is the piece that makes this safe by default. §3.2.

### 3.2 Where invalidation lives — the ownership answer

The rejected shape is a per-group "lowest dirty child index" that every mutation
must remember to update. The shape adopted here is a **three-level dirty
hierarchy, strictly ordered, where the coarsest level is the default** and the
finer levels are opt-in per call site:

| method | effect | sites |
|---|---|---|
| `mark_dirty()` | `composite_epoch += 1`, `needs_composite = true`, `content_bounds.invalidate_all()` | 74 today, unchanged |
| `mark_node_content_dirty(id)` **(new)** | `node_revisions[id] += 1`, `needs_composite = true` | opt-in |
| `mark_node_pixels_dirty(id)` | the above + `dirty_node_pixels.insert(id)` + `histogram.invalidate_all()` | 16 today |

Every group's cached prefix records the `composite_epoch` it was built under. A
mismatch means no reuse. **Therefore any mutation that goes through the existing
global `mark_dirty()` invalidates every prefix cache in the compositor, exactly
as it invalidates everything today.** Opacity, blend mode, visibility, reorder,
add/remove, mask add/remove/edit, effect params, isolation
(`Compositor::set_isolated_node`, `:2155-2158`, calls `mark_dirty`), undo/redo
(`engine/rendering.rs:973`), load, canvas transform — all unchanged and all safe
without anyone auditing them.

This is what makes correctness structural rather than remembered: **you cannot
create staleness by forgetting to add a call; only by deliberately narrowing an
existing one.** Narrowing is a reviewable, testable, per-site act.

Exactly three places set `needs_composite` without `mark_dirty()`, and all three
are accounted for:

1. `Compositor::new` (`:1289`) — no cache exists yet.
2. `set_canvas_rect` (`:2112`) — replaces every `GroupState` wholesale
   (`:2078-2083`), so prefix textures and stamps are destroyed with them.
3. `update_animations` (`:3427`) — handled by the effect revision bump below.

The narrowings in scope (each one a line, each one covered by a §7.2 test):

- `engine/painting.rs:606`: `mark_dirty()` → `mark_node_content_dirty(layer_id)`.
  The enclosing `gpu_stroke_to(&mut self, layer_id: LayerId, …)` (`:471`) already
  carries the id, and `mark_node_pixels_dirty`'s own write-site invariant
  ("if your signature carries a LayerId, you mark it", `:2215-2231`) says this
  site should have been narrow all along. The `active_stroke_layer` is the node
  actually written — mask editing sets it to the mask id — so the id is exact.
- `tick_animated_effects` (`:3241-3246`): bump `node_revisions[id]` beside
  `update_time`. This is what makes symptom A's tick a *narrow* change instead
  of a global one.
- `encode_dirty_layer_content` (`:3280`): a void that actually re-encoded bumps
  its own revision. This incidentally fixes the cost the audit records at §4 —
  an unfrozen camera void with no fresh upload currently recomposites the whole
  tree every canvas tick.
- `realize_dirty_vector_layers` (`:3048`): same, for vector scenes.
- `target_generation` (`:2019`, `:2080`, `:3575`, `:3823`, `:4609`, `:5609`):
  route the six sites through one `bump_target_generation()` that also bumps the
  epoch. Accumulators or screen-run textures were recreated; effect instances
  will be rebuilt against new views (`:4723-4728`); reuse must stop.

**Ownership check.** The composite epoch and the per-node revision map are
compositor state — derived, non-serializable, rebuildable. The prefix texture,
stamps and split index live in `GroupState`, which the compositor already owns
and already destroys on canvas resize. Nothing flows upward: the document is
never consulted for a dirty bit, only for structure and properties, and the
compositor never writes to it. The one datum that is genuinely document-side —
"has this layer's content changed?" — is answered by the same call sites that
already answer "does the canvas need recompositing?", which is the existing
contract, not a new one.

**Why not generation counters on document nodes, or a per-node content hash of
the document?** Considered and rejected in §6.

### 3.3 The algorithm

Split points are child boundaries of the group that owns the accumulator. A
nested group — passthrough or not — is **atomic** for splitting; its stamp folds
its whole subtree. This is what sidesteps the passthrough-inlining problem in
§1.2 without a flattened cursor, and it costs nothing: a group nested inside `G`
gets its own prefix cache when `compose_group` recurses into it (`:5464`), so an
effect deep inside an isolated group still benefits within that group.

`compose_group(G, mode)`:

```
children = doc.children_of(G)
fresh[i]  = ChildStamp { id, included: passes the three filters, revision: subtree_revision(id) }

valid = mode == Interactive
     && cache.epoch == composite_epoch
     && cache.target_generation == target_generation
     && !histogram_pending_below(G)              // see §3.5

first_diff = first index where fresh[i] != cache.stamps[i]  (or len mismatch);
             None if the whole vector matches
resume     = cache.prefix.filter(|p| valid && first_diff.is_none_or(|d| d > p.through))

if let Some(p) = resume {
    copy_texture_to_texture(prefix -> accum.textures[0])   // replaces the clear
    start = p.through + 1
} else {
    clear accum.views[0]
    start = 0
}
gs.current_accum = 0

split = first_diff.and_then(|d| d.checked_sub(1))          // one below the change

match split {
    Some(s) if s >= start => {
        compose_children(&children[start ..= s])
        copy_texture_to_texture(accum.textures[current] -> prefix)
        compose_children(&children[s + 1 ..])
        cache.prefix = Some(Prefix { through: s })
    }
    _ => compose_children(&children[start ..])
}
cache.stamps = fresh
cache.epoch = composite_epoch
cache.target_generation = target_generation
copy final accum -> composite_cache                        // unchanged
```

Notes on why this shape:

- **`compose_children` is untouched.** Slicing the child list in `compose_group`
  means the walk's passthrough recursion, isolation filter, screen-run filter,
  masked-projection detour and ping-pong bookkeeping all stay exactly as they
  are. That matters: `compose_effect_arm` (`:5176`), `compose_group_arm`
  (`:5421`) and `compose_passthrough_masked` (`:5534`) each advance
  `current_accum` a different number of times, and the plan must not have an
  opinion about how many.
- **`current_accum` is restorable by construction.** The restore always lands in
  slot 0 and sets `current_accum = 0` — the same state `compose_group` leaves
  after its clear today (`:4234`). Nothing downstream can tell the difference.
- **The restore is not an extra pass.** It replaces the full-canvas clear render
  pass (`:4236-4248`) with a full-canvas copy. Marginal cost over today is one
  texture read.
- **Steady state costs one copy and zero snapshots.** With one thing changing
  repeatedly, `first_diff` is constant, so `split` is constant, so `split == p.through`
  and the snapshot branch does not re-fire. (An implementation detail: skip the
  snapshot when `split == p.through` and we resumed — the prefix texture is
  already correct.)
- **Convergence takes two composites.** The first composite after a change has
  no prior stamps to diff, so it snapshots nothing; the second knows `first_diff`
  and snapshots; the third resumes. Irrelevant for a continuous animation or a
  stroke.
- **`copy_texture_to_texture`, not a blit pass.** Exact, no sampler, no format or
  premultiplication question, and it is the idiom already used for
  `composite_cache` (`:4266`) and `snapshot_parent_accum` (`:5289`).

**VRAM.** One extra canvas-sized `Rgba8Unorm` per group that actually snapshots —
16 MB at 2048², 64 MB at 4096². Allocate **lazily**, only on the first snapshot,
so a static document pays nothing. Eviction is an open question (§9).

### 3.4 Why exact stamps and not hashes

The audit records that the compositor's six existing fingerprint sites use plain
equality and never hashing (`docs/compositor-caching-audit.md` §1.3), the
canonical one being `EffectInstance`'s five-field compare in
`sync_effect_instances` (`:4723-4734`). Following that idiom here is not just
consistency: a hash would introduce a 2⁻⁶⁴ chance of a *silently wrong exported
file*, and this plan's whole safety argument is that staleness is impossible
rather than unlikely. `Vec<ChildStamp>` of `(LayerId, bool, u64)` is ~24 bytes
per child; a 200-layer document stores under 5 KB and compares it with a slice
`==`. That is free next to any GPU work.

### 3.5 What must never reuse

`CompositeMode { Interactive, Authoritative }` is threaded through
`compose_group` / `compose_group_arm` / `CompositionContext`. Under
`Authoritative` the walk **neither reads nor writes** prefix state, which keeps
the cache consistent across an interleaved authoritative pass rather than
merely correct during it.

Callers that must pass `Authoritative` — every path whose pixels are persisted
or become document content:

| path | site |
|---|---|
| Export | `engine/export.rs:39` → `composited_texture()` `:43` |
| Save (thumbnail / flatten-on-save) | `engine/save.rs:146`, `:165` |
| Process recording (embedded in the `.darkly` file) | `engine/process_recording.rs:275`, `:328` |
| Sample-merged clone source (becomes painted pixels) | `engine/painting.rs:985`, `:1039-1040` |
| Flatten / Merge Down | `bake_subtree_to_layer` `:3801`, whose `compose_children` at `:3869` recurses into nested groups' `compose_group` |
| Test canvas readback | `engine/mod.rs:1108` — plus a separate `Interactive` accessor for the reuse tests (§7.1) |

`Interactive`: `Compositor::render` → `render_offscreen` (`:5618`), and picker
previews (`engine/preview.rs:262`, transient display).

This is the answer to "what makes staleness impossible, not merely unlikely."
It is defence in depth with two independent layers:

1. The epoch makes every non-narrowed mutation invalidate everything. Only two
   narrowed paths exist in v1 (paint dab, animation tick), each with a
   byte-equality test.
2. Even if layer 1 had a bug, **the file is still right**: every path that
   writes pixels to disk or into the document forces a from-scratch composite.
   The worst reachable failure is a transient on-screen artifact that the next
   `mark_dirty()` clears.

`bake_subtree_to_layer` deserves a specific note: it takes `isolated_node`
(`:3817`) and restores it (`:3903`) *without* marking dirty, so a walk under
`Authoritative` that wrote prefix state would poison every nested group's cache
with a non-isolated composite. `Authoritative` = no write closes that. (The
trailing `mark_dirty()` at `:3905` would also cover it; not relying on ordering.)

### 3.6 Deleting `scissor` pays for the new parameter

`scissor: (u32, u32, u32, u32)` is threaded through eight compose functions and
stored on `CompositionContext` (`:332`). Both call sites pass the full canvas —
`(0, 0, canvas_width, canvas_height)` at `:3835` (bake) and `:3920`
(`render_offscreen`). It is derivable from `self` at every point of use. It was
shaped for dirty-rect compositing, which this plan does not build (§8), and the
audit's recommendation #3 is explicit that the choice is "delete both, or
implement the region-level caching they were built for."

Deleting `scissor` and adding `CompositeMode` is net-zero threading and a net
line reduction, and it removes the second piece of dead optimization scaffolding
alongside `cache_valid_through`. If a reviewer prefers to keep `scissor` against
future dirty-rect work, the plan still stands — it just costs ~50 more lines.
Recommend deleting: the parameter is a false promise today, and re-adding it
with a real consumer is cheaper than maintaining an unused one.

### 3.7 Histogram interlock

`compose_effect_arm` (`:5211-5219`) dispatches the node histogram from *inside*
the compose walk, because the effect's input only exists mid-composite. If that
effect is skipped inside a resumed prefix, `histogram.needs(id)` never clears and
`pump_node_histogram` re-arms forever. Guard: when `histogram_target` is `Some(t)`
and `histogram.needs(t)`, force a full walk for the group containing `t`. One
predicate, stated as a dependency rather than discovered as a hang.

### 3.8 Generality — it comes free

The mechanism keys on "the lowest child whose subtree changed", not on effects.
Three cases fall out of one implementation:

- **Animated canvas effect** (symptom A): everything below the effect is skipped;
  the effect and everything above re-run, which is correct.
- **Painting with layers above** (symptom B, +43.5 ms): the effect is *below* the
  paint target, so the effect encode itself lands in the reused prefix and does
  not run at all.
- **Param scrubbing near the top of the stack**: `update_filter_params`
  (`engine/layers.rs:885`) calls the global `mark_dirty()`, so a slider drag
  bumps the epoch and gets no reuse in v1. Narrowing it to
  `mark_node_content_dirty(filter_id)` is a one-line follow-up that the design
  already supports; listed in §8 rather than v1 so each narrowing ships with its
  own test.
- **Unchanged sub-groups** get a fourth win for free: a non-passthrough group
  child whose subtree is unchanged and which sits *above* the split still gets
  blended from its `composite_cache`, but `compose_group` on it returns
  immediately with `first_diff == None` and `start == len` — no children walked.
  This is Krita's `N_BELOW_FILTHY` "nothing to do" (`kis_async_merger.cpp:241-244`)
  reached from the other direction.

## 4. Architectural impact

- `crates/darkly/src/gpu/compositor.rs` — all structural change. `GroupState`
  loses `cache_valid_through` and gains a lazily-allocated `PrefixCache`;
  `Compositor` gains `composite_epoch`, `node_revisions`, and a
  `composite_encodes` telemetry counter; `compose_group` gains the resume/split
  logic; `mark_dirty` / `mark_node_pixels_dirty` gain a sibling.
- `crates/darkly/src/engine/` — six call sites choose a `CompositeMode`, one
  paint site narrows its mark, and two test accessors are added.
- **No document change.** No new field survives save/load; the document is
  consulted, never written.
- **No modular-registry change.** `gpu/veils/`, `gpu/effects/`, `gpu/voids/`,
  `gpu/blend_modes/`, `document/layer_kinds/` are untouched, and no generated
  `mod.rs` moves. This was a deliberate design constraint: an earlier variant
  that pinned the prefix in a third accumulator slot (zero copies both ways)
  would have widened `Effect::create_cache`'s `ping_pong_views: &[TextureView; 2]`
  (`gpu/effect.rs:293`) and `Reduced::downscale_bgs: [BindGroup; 2]`
  (`gpu/effect_scaling.rs:94`) to three, rippling through all seven effect
  implementations. Rejected — see §6.
- **No WASM or frontend change.**

## 5. Implementation steps

Two PRs. PR A is behaviour-preserving groundwork and can land and bake alone.

### PR A — dirty-marking hierarchy and walk plumbing (~120 net lines)

1. Add `composite_epoch: u64` and bump it in `mark_dirty()`.
2. Add `node_revisions: HashMap<LayerId, u64>` and
   `mark_node_content_dirty(id)`; re-express `mark_node_pixels_dirty` on top of
   it; document the three-level ordering next to the existing write-site
   invariant at `:2215`.
3. Add `subtree_revision(doc, id) -> u64` (node + `filters()` + descendants).
4. Route the six `target_generation += 1` sites through
   `bump_target_generation()`, which bumps the epoch too.
5. Narrow `engine/painting.rs:606` to `mark_node_content_dirty(layer_id)`.
6. Bump revisions in `tick_animated_effects`, `encode_dirty_layer_content`,
   `realize_dirty_vector_layers`.
7. Delete the `scissor` parameter from the eight compose functions and
   `CompositionContext`; derive it from `self` at the two points of use.
8. Add `CompositeMode` and thread it in `scissor`'s place; set the six callers
   per §3.5.
9. Delete `cache_valid_through` and its four assignments.
10. Add `composite_encodes: u64`, incremented once per `node.compose_into` in
    `compose_children` (`:4338`), with a `#[cfg(any(test, feature = "testing"))]`
    accessor modelled exactly on `effect_rebuilds` (`:3554-3559`) and
    `DarklyEngine::test_effect_rebuilds` (`engine/mod.rs:1167-1171`).
11. Add `DarklyEngine::test_readback_canvas_from_scratch()` — drops all prefix
    caches, bumps the epoch, composites `Authoritative`, reads back.

After PR A the counter reports "every child, every dirty frame" and the §7.2
equality battery passes trivially. That is the baseline the §7.1 tests are
written against.

### PR B — the cache (~220 net lines)

12. `ChildStamp` + `PrefixCache { texture, view, through, stamps, epoch, target_generation }`
    on `GroupState`, lazily allocated, sized from `padded_width/height`.
13. Restructure `compose_group` per §3.3.
14. Add the §3.7 histogram guard.
15. Write the §7 tests; demonstrate the counter dropping and the equality
    battery staying green.

## 6. Alternatives considered

**Do nothing to the walk; fix symptom A by lowering `canvas_effect_scale` while
animating.** This is the largest single lever on the reported 10× (§1.3) and it
is ~5 lines. Rejected as the *answer*: it changes what the user sees, and
`gpu/effect_scaling.rs:10-14` is explicit that canvas output is document content.
A variant — run canvas effects reduced during interaction and full on
`Authoritative` composites — is genuinely interesting and is recorded in §8 as a
separate plan, because it is a product decision about output quality, not a
caching fix, and it does nothing for symptom B.

**Make `cache_valid_through` real as declared.** Rejected: a child index cannot
name a resume point across passthrough inlining (§1.2), and there is no stored
accumulator state for it to point at. The field is deleted.

**A per-group "lowest dirty child index" updated by every mutation.** Rejected
explicitly. It is the hand-maintained coupling CLAUDE.md refuses, and it inverts
the failure mode: a forgotten call site produces silent wrong pixels. The epoch
inverts it back — a forgotten call site produces a full recomposite.

**A per-node content hash of the document (`Hash` derived on `Layer` /
`BlendProps`, folded into the stamp).** Attractive because adding a document
field would automatically join the comparison. Rejected for v1 on three counts:
`f32` opacity is not `Hash` so every site needs `to_bits()`; hashing reintroduces
the collision-into-an-exported-file risk §3.4 exists to eliminate; and it buys
granularity only for mutations that currently go through the global
`mark_dirty()` and therefore already recomposite correctly. It is the right
mechanism *if* per-property granularity is ever needed; §8.

**A third accumulator slot, pinning the prefix (zero copies both ways).**
Genuinely elegant: `AccumPair` becomes three textures, `1 - src` becomes
`gs.advance()`, `blend_bind_groups`' existing `src_accum_idx: u8` key
(`:5116`) already generalises, and both the restore copy and the snapshot copy
disappear. Rejected because `Effect::create_cache` takes
`ping_pong_views: &[wgpu::TextureView; 2]` (`gpu/effect.rs:293`) and
`ScaledEffect`/`Reduced` are built on the same 2-array
(`gpu/effect_scaling.rs:94, 112, 212`), so the change ripples through all seven
files in `gpu/effects/` — a modular-boundary violation for a saving of roughly
one full-canvas copy per group per composite. Revisit if profiling shows the
copy matters.

**Dirty-rect compositing (Krita's actual answer, §2.1).** Not rejected —
deferred (§8). It is strictly better than prefix reuse for symptom B and does
nothing for symptom A, and it is a larger change (every effect needs a
`needRect` equivalent, per `kis_base_rects_walker.h:400-424`).

**Cache each effect's output texture instead of the group's accumulator.**
Equivalent in effect to the prefix cache but anchored per effect rather than per
group, which needs one texture per effect layer and cannot express "the whole
group below is unchanged". The group-level split subsumes it.

## 7. Tests

`crates/darkly/tests/composite_prefix.rs`, run under
`--features darkly/testing -- --test-threads=1`. Helpers (`test_engine`,
`fill_layer`, `settle`, `px`, `effect`) follow `tests/effect_space.rs:18-60`.
No blocking readback enters production code — `test_readback_canvas` and the new
`test_readback_canvas_from_scratch` are both `#[cfg(any(test, feature = "testing"))]`,
like every other `test_readback_*` on `DarklyEngine`.

### 7.1 The reuse actually happens

Modelled on `effect_instances_are_not_rebuilt_every_frame`
(`tests/effect_space.rs:509-550`) — settle, snapshot the counter, act, assert
the delta.

- `animated_canvas_effect_skips_the_layers_below_it` — four rasters, `grain` at
  speed 1.0 above them, all canvas-space. Settle; snapshot
  `test_composite_encodes()`; `test_tick_animations` across two
  `canvas_divisor` boundaries; force the composite. Assert the delta equals the
  count of children **at and above** the effect, not the full child count.
  Fails before PR B (delta = full count).
- `painting_above_an_effect_does_not_re_encode_it` — raster, `invert`, raster on
  top. Settle; snapshot; paint eight dabs on the top raster, compositing each
  frame. Assert the per-frame delta is 1 (the painted layer only). This is the
  +43.5 ms case. Fails before PR B.
- `an_unchanged_group_is_not_walked` — a nested non-passthrough group with three
  children below a painted top-level raster; assert the group's children
  contribute zero encodes.
- `export_does_not_reuse_the_cache` — after a narrow paint mark, assert the
  export path's encode delta equals the full child count, and that the exported
  bytes equal `test_readback_canvas_from_scratch()`.

### 7.2 The composite is still byte-identical

The anti-staleness battery, in the shape of Krita's `testMerger`
(`kis_async_merger_test.cpp:51-121`): drive the cache into a hot state, mutate,
then assert the incremental composite equals a from-scratch one.

```
fn assert_matches_from_scratch(engine: &mut DarklyEngine, what: &str) {
    let incremental = engine.test_readback_canvas();          // Interactive
    let scratch     = engine.test_readback_canvas_from_scratch();
    assert_eq!(incremental, scratch, "stale composite after {what}");
}
```

A shared fixture builds: raster (red) / raster (green) / masked raster /
passthrough group containing two rasters / `invert` effect / non-passthrough
group containing a raster and a `grain` / raster on top. Prime the cache by
painting a dab on the top raster twice (so a prefix is snapshotted and then
resumed from), then run every case below, re-priming between them:

paint below the split · paint above the split · paint into a mask · opacity ·
blend mode · visibility toggle (on a layer below, and on one above) ·
reorder across the split · add a layer below the split · delete a layer below
the split · add a mask · remove a mask · effect param change · group
passthrough toggle · isolation set and cleared · screen-space boundary move ·
undo · redo · canvas resize (crop and grow) · animation tick · void param
change · vector scene push · flatten · merge down.

Each is one line plus `assert_matches_from_scratch`. Any of them regressing is
the failure this plan is most afraid of, and each names its own mutation in the
assertion message.

Cheap extra coverage, not a substitute: assert `composite_encodes` deltas for
the mutation classes that *must* force a full walk (reorder, visibility,
isolation) — a silent narrowing shows up as a too-small delta before it shows up
as wrong pixels.

### 7.3 Regression framing

This is a performance defect, not a bug fix, so §7.1 is the feature test. §7.2's
`paint below the split` and `reorder across the split` cases are written first
and will pass against PR A's from-scratch walk — they are the guard rails that
must not break, and they are the ones to run against every subsequent narrowing
in §8.

## 8. Out of scope, recorded

- **Dirty-rect compositing** (§2.1, audit §2.2). Strictly better than prefix
  reuse for painting; needs a `needRect` per effect. Separate plan. If it is
  built, `scissor` comes back with a real consumer.
- **Interactive-vs-authoritative effect resolution** (§6). Probably the largest
  single lever on the reported 10×. Product decision; separate plan.
- **Removing the per-group `composite_cache` copy** (audit §2.4, rec #6). Would
  offset this plan's VRAM cost exactly and remove a full-canvas copy per group
  per composite.
- **Narrowing `poll_pending`'s global `mark_dirty`** (`engine/rendering.rs:667`,
  audit §3.3). Not required — during a stroke, `drain_dirty_pixels` is empty
  until `end_stroke`, so no thumbnail readback is in flight and `poll_pending`
  returns false — but §7.1's counter test will expose it immediately if that
  analysis is wrong on some path.
- **Narrowing `update_filter_params`** to `mark_node_content_dirty`, and wiring
  the already-unwired `mark_effect_dirty` (`:3582`, zero callers today, audit
  §2.3). One line each; each wants its own §7.2 case.
- **Per-property document hashing** (§6), if per-property granularity is ever
  wanted.

## 9. Risks and unresolved questions

1. **The premise.** The prefix cache is not the bulk of symptom A (§1.3).
   **Gate: before writing PR B, measure.** `cargo test --features profile
   --test profile_render` plus the `perf::time` spans already in
   `render_offscreen` and `render` (`:3908`, `:5603`) can attribute the canvas
   tick between walk, effect encode, `composite_cache` copy, and present.
   If the walk is under ~15 % of the tick, PR B's justification rests entirely
   on symptom B — which is a good justification, but the plan should say so out
   loud rather than claim a 10× fix.
2. **Prefix VRAM.** +1 canvas-sized texture per snapshotting group. Lazy
   allocation covers static documents; a document with many groups all being
   edited does not benefit. No eviction policy in v1 — **open question**: drop a
   group's prefix after N composites without a hit, or cap total prefix bytes?
   Deferring this is only safe because the textures are freed with `GroupState`
   on canvas resize.
3. **Convergence.** Two composites before the first hit (§3.3). Harmless for
   animation and strokes; means a single isolated edit never hits. Acceptable.
4. **Alternating dirty depths.** With an animating veil *and* painting above it,
   the effect is genuinely dirty every frame, so `first_diff` sits at the effect
   and the effect re-encodes. Correct, and it means symptom B's win evaporates
   in the presence of an animated veil. Worth stating in the plan so it is not
   discovered as a "regression".
5. **`subtree_revision` cost.** O(subtree) per child per composite, i.e. O(n) per
   group and O(n·depth) overall — CPU only, on a codebase that already clones a
   `String` + `Vec<ParamValue>` per filter layer per frame (audit §6). Memoize
   per composite if a large document shows it; not expected.
6. **Interaction with the divider-as-a-node redesign**
   (`handoff-viewport-boundary.md` §2). The screen-run skip is folded into
   `ChildStamp::included`, so the cache follows whatever the filter becomes.
   Independent.
7. **`bake_subtree_to_layer`'s sentinel `GroupState`** (`LayerId::from_ffi(0)`,
   `:3821`) has a child list that is not `doc.children_of`. Under
   `Authoritative` it never touches prefix state, so this is inert — but it is
   the kind of thing a later "let's allow reuse in bake too" change would break
   silently. Worth an assertion.
8. **Test-file helper duplication.** `test_engine` / `fill_layer` / `settle` are
   redefined in most files under `crates/darkly/tests/`. Adding a ninth copy is
   a DRY violation the reviewer may want addressed with a shared `tests/common`
   module; that is a ~60-line refactor touching many test files and is called
   out here rather than folded in silently.

## 10. LOC estimate

Lines added / removed, not touched.

| area | added | removed |
|---|---|---|
| `gpu/compositor.rs` — PR A (epoch, revisions, marks, `CompositeMode`, `scissor` deletion, `cache_valid_through` deletion, counter) | ~130 | ~85 |
| `gpu/compositor.rs` — PR B (`ChildStamp`, `PrefixCache`, `compose_group` restructure, `subtree_revision`, histogram guard) | ~210 | ~20 |
| `engine/` (six `CompositeMode` sites, one narrowed paint mark, two test accessors) | ~45 | ~10 |
| **production total** | **~385** | **~115** |
| `tests/composite_prefix.rs` (new) | ~440 | 0 |
| **tests total** | **~440** | **0** |
| `docs/plans/composite-prefix-cache.md` (this file) | ~520 | 0 |
| `docs/gpu-passes.md` (resume/snapshot passes) | ~25 | 0 |
| generated `mod.rs` | 0 | 0 |
| **docs total** | **~545** | **0** |

**This is a large change.** ~385 added / ~115 removed of production code in the
single most intricate file in the crate, touching the compose walk, the dirty
protocol, and six persistence call sites. The honest split is that PR A is
~175/~95 and mostly deletion and renaming — low risk, independently valuable
(it removes both dead optimization scaffolds and fixes a write-site-invariant
violation in the paint path) — and PR B is ~210/~20 of genuinely new machinery
whose payoff should be measured before it is written (§9.1).
