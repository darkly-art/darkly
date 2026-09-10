# Composite work caching: making a canvas-space animated veil tick cost what it should

> **Implemented** on `better-veils`. All three PRs landed together; the
> notes below record where the implementation diverged from the plan.
>
> - **PR 1** as specified. One discovery: `ExternalImageSource` is
>   uninhabited on native (`gpu/void.rs`), so the camera-upload narrowing
>   cannot be exercised by a native test; its contract is pinned by
>   `gpu/revisions.rs` unit tests plus
>   `a_histogram_survives_an_animating_veil` instead (§7.1).
> - **PR 2** as specified. `GroupState` gained `output_index/​view/​texture`
>   accessors; `present_cache_bind_group` became a per-half pair selected at
>   draw time; the child-group blend key carries the child's half in bit 1.
> - **PR 3** as specified, with the inclusion predicate's node-kind half
>   implemented as type-owned dispatch rather than a match in the walk:
>   `LayerNode::compose_ready` / `Layer::compose_ready` (sibling of
>   `compose_into`) ask each variant whether its arm has the resources to
>   draw, answering `true` whenever unsure, recording a drawing child as
>   absent would hide a later edit to it, while the reverse only costs a
>   recomposite. The snapshot/restore live in `compose_group`;
>   `compose_children` gained one `snapshot_after` parameter rather than a
>   second recording loop.
> - Verified: full workspace suite green; `cargo fmt`, native clippy and
>   wasm clippy clean. The S1 regression
>   (`a_property_change_below_the_prefix_is_not_restored_over`) was confirmed
>   to fail with the prefix-drop removed and pass with it.

Written against `better-veils` @ `a345989a` (working tree). Every source claim
below was verified against the file at that state; line numbers will drift,
symbol names will not.

## Independent Review

Reviewed independently against the working tree at the same state. Every
source citation in the plan was re-verified; the pass ledger (section 1.1), the
five ping-pong flip sites, the frame gate, the full-canvas scissor
(`gpu/compositor.rs:2720`, which is what makes "the prefix half is intact"
a whole-texture statement rather than a per-region one), the histogram and
content-bounds stamps, the `mark_node_pixels_dirty` write-site invariant, the
`accumulator_host_of` semantics (`document/mod.rs:758-769`), and the battery
harness's anti-vacuity guard (`tests/compositor_revisions.rs:100-117`, 23
battery rows) are all as the plan states. All six Krita/GIMP citations were
verified against the checkouts, including the absence of any global
"something changed" flag in Krita's walker (state is per-node `JobItem`s in
`kis_base_rects_walker.h:509-548`); two ranges could be tightened
(N_ABOVE/BELOW labelling lives in the `kis_merge_walker.cc:86-110` helpers
that `startTripImpl` calls; rect-bounding is better cited as
`kis_base_rects_walker.h:400-424`). The `composited_texture()` consumer audit
was independently redone and confirms the plan's central claim: every
consumer copies at request time through `GpuContext::encode`
(`gpu/context.rs:150-157` submits before returning), and no bind group built
against `composited_view()` outlives its statement; the only cross-frame bind
groups over group outputs are `present_cache_bind_group`
(`gpu/compositor.rs:1019-1025`, rebuilt at `:1789-1795`, consumed at `:2640`,
`:2650`, and `gpu/compositor_test_harness.rs:49`) and the child-group blend
entries (`gpu/compose_walk.rs:1196-1213`), exactly the two sites PR 2 changes.

The zero-copy resume's safety argument was attacked adversarially across
every compose arm and every accumulator writer (the group clear, the five
advancing arms, the passthrough-masked snapshot blit which only reads, the
histogram dispatch which only reads, bake's sentinel walk, and the present
path which binds no accumulator directly). The parity invariant holds,
including the multi-frame cases: repeated resumes at a stable `d` write only
the non-prefix half; a resume whose own suffix advances twice legitimately
corrupts the prefix half but produces a correct frame, and the corruption is
recorded in `advances` so the next frame falls back; a dirty child moving
*below* the previous `d` is caught because `input_half`/`advances` entries for
un-walked prefix children are carried forward and the suffix sum then exceeds
1 exactly when intermediate children wrote the prefix half. The clear in the
full-walk branch is unaccounted as an "advance" but is only ever the content
of half 0 for child index 0, where resume-at-0 is valid precisely when the
whole-walk advance sum is at most 1, i.e. the clear survived. No corrupting
sequence was found. The findings below are therefore about guard scope,
stamp completeness, and honesty of the coverage claims, not about the core
mechanism.

### Findings

**R1 (must fix, PR 3): the histogram interlock does not survive nesting.**
The guard as specified ("the group `accumulator_host_of(t)` refuses resume")
is evaluated inside that group's own `compose_group`. But when the LUT
filter's host group sits *below* the first dirty child of an ancestor, the
ancestor's resume (or all-clean) branch skips the host group's subtree
entirely, so its `compose_group` is never entered and the guard never runs:
`compose_effect_arm`'s dispatch (`gpu/compose_walk.rs:949-959`) is
unreachable and `pump_node_histogram` starves for as long as activity stays
above the host. Concrete repro shape: LUT filter inside the fixture's nested
group, user focuses it (no cached histogram, `needs()` true), then paints on
`top` at root level. The guard must refuse reuse in the host group *and every
group on the path from the root to it*, or, simplest, refuse all reuse for
the composite while `histogram_target` is `Some` and `needs()` holds
(histograms are transient and the state is rare). Add the nested-host case to
the section 7.3 interlock test, which as written places the target where the
single-group guard happens to work.

**R2 (must fix, PR 3): `included` is missing three no-op conditions.** The
plan folds the filter arm's no-ops into `included` (review C1) but claims
completeness while omitting the same class elsewhere:
- `compose_layer_arm` returns without advancing when `node_textures` or
  `layer_cache` has no entry (`gpu/compose_walk.rs:816-829`);
- `compose_layer_through_projection` advances the parent then returns
  without writing when `projection_states` has no entry
  (`gpu/compose_walk.rs:645-647`), an advance with no write (conservative
  for the counter, but a contribution change invisible to stamps);
- `compose_group_arm` returns when the child group has no `GroupState`
  (`gpu/compose_walk.rs:1179-1181`).
Each of these is a compositor-side fact that can flip a child between
contributing and not. In practice every realization path appears to be
paired with a `document` bump, but the plan must either fold these into
`included` (uniform with the filter case, cheapest) or state the pairing
argument explicitly per condition; silently relying on it is the exact shape
review C1 flagged.

**R3 (must fix, plan text): the fallback boundary is misstated by one, and
the "painting above a veil" row overclaims.** `suffix_advances(cache, d) <= 1`
means resume engages only when the dirty child is the *topmost* advancing
child: one advancing child above it already sums to 2. Section 3.3's "a dirty
child with two or more compose steps above it" is wrong; it should read "one
or more". Consequently, for the measured handoff section 3.2 workload
(painting above an *animating* VHS veil), every canvas-tick frame has two
dirty depths (veil via `animation`, paint layer via `node_pixels`) and falls
back to a full walk; only the paint-only frames between ticks resume. The
section 4 table's row A claim for that column ("veil encode and everything
below skipped; per-dab cost is one blend") holds only for a non-animating
veil, and contradicts the plan's own section 3.3/section 9.2 admission. State
the alternate-frame reality in the table; it changes the priced win for
symptom 2 and belongs in front of the user at approval.

**R4 (must fix, plan text, PR 1): the content-bounds claim for narrowing #2
is factually wrong.** `ContentBoundsPass`'s stamp is
`(document, node_pixels(id))` (`gpu/content_bounds.rs:23-36`), and
`upload_void_external_image` bumps `document` today
(`gpu/void_content.rs:395`), so today every camera frame *does* invalidate
every layer's cached bounds; the plan's "they already do today [stay quiet]"
is backwards. The narrowing is still an improvement (other layers' bounds
stop churning at camera rate), but it creates a new permanent staleness: the
camera void's *own* bounds stamp never moves again (neither `document` nor
`node_pixels(id)` is bumped by an upload), so any consumer of that void's
content bounds sees the bounds of whichever frame was live when they were
computed, forever. Probably acceptable for live video, but it is a behavior
change the plan claims does not exist; disclose it, decide it, and correct
the verification claim. The section 7.1 battery row as written (asserts
another layer's bounds are undisturbed) tests the improvement, not this.

**R5 (should fix, plan text, PR 3): the histogram claim for narrowing #3 is
inaccurate, and the two narrowings compound.** Today the per-dab mark is a
`document` bump; `node_pixels_any` moves once per stroke at `end_stroke`
(`engine/painting.rs:1570`), so the LUT histogram (stamp =
`node_pixels_any`, `gpu/histogram.rs:49-51`) goes stale once per stroke, not
"as today ... invalidated by paint" per dab. After narrowing #3 it goes stale
per dab, so with a LUT filter focused, `needs()` is true on every mid-stroke
frame: the histogram re-dispatches per frame instead of per stroke, and via
the R1 interlock every such frame also refuses reuse, cancelling the paint
resume win while the modal is open. Correct the claim and note the
interaction; it is acceptable behavior (the modal is transient) but must be
stated, and it slightly weakens the section 4 row A pricing in that state.

**R6 (should fix, PR 3 spec): the advance counter must be per-`GroupState`,
not per-walk-global.** The plan says "a per-walk counter" and measures deltas
around each top-level `compose_into`. A single walk-global counter would fold
a non-passthrough child group's *internal* advances into the parent's delta
(the recursion happens inside its `compose_into` just like a passthrough's),
inflating `advances` and permanently disabling resume whenever any nested
group sits at or above the dirty child. Counting on the `GroupState` being
advanced gives exactly the right answer for both group kinds (a passthrough
child advances the parent's state; an isolated child advances its own and
contributes exactly its one blend advance to the parent). One sentence in the
plan pins this; getting it wrong is silent (conservative, never unsafe) and
would eat much of PR 3's win in grouped documents.

**R7 (should fix, PR 3 design, DRY): the stamp fold's `included` predicate
and the walk's skip logic must be one function.** `included` reproduces
`compose_children`'s skip chain (`find_node` miss, `visible()`, isolation
path, screen-run membership, `gpu/compose_walk.rs:399-421`) plus the arm
no-op conditions (R2), and the plan additionally moves recording into a
`compose_group`-owned child loop while `compose_children` survives for
passthrough recursion and bake. That is two loops and two copies of the same
per-child predicate, a textbook "keep in sync with" hazard where drift means
staleness. Extract a single shared per-child inclusion predicate (or an
iterator yielding included children) consumed by the stamp fold, the
recording loop, and `compose_children`.

**R8 (minor, corrections to record):**
- `engine/rendering.rs:950` is the undo pixel-restore mark, not a stroke-end
  mark; the stroke-end sites are `engine/painting.rs:1570` (`end_stroke`) and
  `:1561` (flood-fill commit). Substance of narrowing #3 unaffected.
- Section 3.2's consumer audit should record two facts the re-audit
  surfaced: `pick_color` (`engine/rendering.rs:178-213`, `PickSource::Merged`)
  is the one consumer that never forces a composite (it reads between
  composites; safe under PR 2 since the re-pointed accessor returns the
  current half and the 1x1 readback copies immediately), and `engine/save.rs`
  keeps a handle-clone of the root cache texture in `SaveJob.pinned_textures`
  for the async job's life (content already copied in the same submit; under
  PR 2 the pinned handle becomes the root accum texture, still benign).
- `bake_subtree_to_layer` bumps `targets` twice and `mark_dirty` on exit
  (`gpu/bake.rs:49,124,127`), so every merge/flatten drops all walk caches
  and re-walks nested groups mid-bake under fresh ticks; correct and
  self-healing, worth one sentence in section 3.3 so the full walk after a
  merge is not misread as a cache bug.
- Section 7.3's two anti-vacuity pitfalls share one `walk_resumes` counter,
  so the all-clean test cannot prove it exercised the all-clean branch rather
  than the resume branch. Either split the counter by branch or accept and
  say so.
- The `~70 mark_dirty() call sites` figure verifies at 68; `animation()`'s
  accessor is currently dead outside `latest_composite_input`, confirming
  the PR 1 blast-radius claim.

**Scope, ownership, simplicity.** No violations found. All new state is
compositor-owned and rebuildable (`WalkCache`, the per-node animation map);
nothing flows document-ward; no blocking readbacks; no modular-registry
surface is touched and effects/voids stay ignorant of the cache. The
rejection of the per-effect output cache (section 4 row C) is honestly
priced: an animated effect cannot reuse its output and the ledger's clear +
blend costs are untouched by it. PR 2 is a genuine deletion (state and a
per-composite copy) and PR 1 removes a taxonomy carve-out; the added
complexity is concentrated in PR 3's ~175 lines, which is plausible though
likely to run over in the compose walk; the plan already commits to stopping
if it materially exceeds the estimate. With R1, R2, R6, and R7 folded in, the
mechanism is the simplest general shape that covers the measured symptom;
the honest coverage statement after R3 is narrower than the plan currently
advertises (topmost-dirty-child only), which the user should weigh at
approval, but the primary measured symptom (animated veil as the top child)
sits squarely inside it.

**Verdict: revise.** R1 and R2 are correctness holes in the specified design
(liveness and stamp completeness); R3, R4, R5 are misstatements that
materially affect the priced benefit and the disclosed behavior changes; R6
and R7 are one-sentence and one-function design commitments respectively.
None of them undermine the core resume invariant, which survived adversarial
analysis; no rethink is warranted.

### Revision response

Every finding addressed in place; none rejected.

- **R1**: adopted the total guard, while a histogram is owed, the composite
  refuses all reuse (§3.3 interlock); nested-host starvation test added
  (§7.3).
- **R2**: all three no-op conditions folded into `included`, which now
  enumerates every arm's no-ops (§3.3).
- **R3**: fallback boundary corrected to "one or more advancing children
  above"; the "topmost advancing child only" coverage stated in §3.3 and the
  §4 table; alternate-frame reality for painting-above-an-animating-veil
  disclosed in both; boundary test added from both sides (§7.3).
- **R4**: content-bounds claim corrected in §3.1 and §3.4; the camera void's
  own frozen bounds disclosed and decided (accepted); §7.1 row now pins it.
- **R5**: histogram cadence corrected (per-stroke today, per-dab after) in
  §3.4, with the interlock compounding stated there and in §3.3.
- **R6**: advance counter specified per-`GroupState` with the
  nested-group rationale (§3.3).
- **R7**: single shared inclusion predicate committed (§3.3).
- **R8**: stroke-end citations fixed, 68-site count adopted, bake
  self-healing sentence added, `walk_resumes`/`walk_all_clean` split per
  branch (§6, §7.3), consumer-audit facts recorded (§3.2).

**Post-review reshape (user-directed).** After this review, the user judged
PR 3's zero-copy parity resume benchmark-fitted (its coverage (topmost
advancing child only) encloses exactly the measured workloads and nothing
else) and directed the general shape instead: a per-group prefix texture,
§3.3 as it now stands. This moots R3's coverage caveat (coverage is now
"below the first dirty child, anywhere") and R6 entirely (no advance
counters exist), while R1's total histogram guard, R2/R7's shared inclusion
predicate, and all PR 1 / PR 2 findings carry over unchanged. The parity
trick is preserved in §4 row B and §9 as a possible copy-elision follow-up.
This is a material mechanism change relative to the reviewed draft; a second
independent review pass follows below.

Related documents, read and reconciled here:

- `docs/compositor-caching-audit.md` (partially superseded; tier analysis and
  recommendations #3, #5, #6 still stand)
- `docs/plans/composite-prefix-cache.md` (reviewed: verdict `revise`, findings
  B1-B3, C1-C8; never implemented; this plan is its rebase onto the landed
  revision registry, reshaped per its own review)
- `docs/plans/compositor-revision-registry.md` (implemented; `gpu/revisions.rs`)
- `docs/plans/effect-invalidation-wiring.md` (reviewed, not implemented; see §8)
- `handoff-viewport-boundary.md` §3.2 (measured numbers, user-endorsed direction)

## Second Independent Review (post-reshape)

Independent second pass over the reshaped PR 3 (per-group prefix texture,
§3.3), performed against the working tree at the same state. Every rewritten
section's source citation was re-verified: the pass ledger and the walk
(`gpu/compose_walk.rs:335-375` compose_group, `:353` clear, `:363-374` cache
blit), the full-canvas scissor (`gpu/compositor.rs:2720`), the gate and
commit (`:2714-2718`, `:2749-2753`, including the `debug_assert` at
`:2744-2748` that only `targets` moves mid-composite), accumulator usages
(`make_accum_texture`, `gpu/compositor.rs:684-701`: Rgba8Unorm with
`COPY_SRC | COPY_DST`, so both prefix copies are legal
`copy_texture_to_texture` via `blit_region`, `gpu/mod.rs:57-67` (and the
mask-snapshot precedent at `gpu/compositor.rs:2245` allocates through the
same helper), the histogram stamp and `needs()`
(`gpu/histogram.rs:49-51,165-167`), the bake ticks (`gpu/bake.rs:49,124,127`)
and bake's `targets` bump at `:49` is also what makes nested groups
full-walk *inside* the bake's sentinel walk, so the bake can never consume a
live-walk prefix), `ensure_group_state`'s creation bump
(`gpu/compositor.rs:1717`), `sync_effect_scale`'s drift bump
(`gpu/effect_layers.rs:99-102`), the stroke-end marks
(`engine/painting.rs:1561,1570`), the undo restore marks
(`engine/rendering.rs:950,991`), and `set_isolated_node`'s `mark_dirty`
(`engine/layers.rs:1584-1585`).

The prefix mechanism was attacked arm by arm. The snapshot invariant (after
any child's `compose_into` returns, `accum[current_accum]` holds the complete
composite of `children[0..=that child]`) holds for every advancing arm: the
unmasked leaf blend (`gpu/compose_walk.rs:831-892`, full-coverage triangle
into `dst`), the effect arm (`:934-997`, apply writes `dst`), the isolated
child group (`:1185-1233`, blend into parent `dst`), the masked leaf
projection (`:747-757`, pass 3 writes `parent_dst`), and the masked
passthrough group (`:1284-1307`, the apply's final flip lands the result in
the new `current`). Every draw fully overwrites its `dst` within the
full-canvas scissor, so restoring only `accum[0]` and leaving `accum[1]`
stale is sound. The restore branch depends on no skipped-walk side state:
projection uniforms, mask-snapshot ensures, and effect instances are all
synced pre-walk (`gpu/compositor.rs:2733-2737`,
`gpu/compose_walk.rs:506-620`), and the histogram is covered by the total
guard. The `last_d` walkthrough in §3.3 was replayed frame by frame and is
what the pseudocode produces, including the self-limiting snapshot (child
`d-1` is only in the suffix when `through < d-1`, so a converged prefix
re-fires no copies) and the alternating-depth steady state (d flips between
the two depths, `d == last_d` never holds, the prefix stays pinned below the
lower depth). Index-vs-identity for `prefix.through` is doubly safe:
structural mutations bump `document` (invalidating the cache), and the
resume precondition (fresh stamps equal cached stamps through `d-1`,
*including ids*) independently guarantees the prefix content corresponds to
the same children in the same order. One finding survived the attack, and it
is a real staleness hole:

### Findings

**S1 (must fix, PR 3 spec): a stale prefix survives an invalid full walk
whose stamps did not move.** Any `document` or `targets` bump that moves no
per-child stamp (layer opacity or blend mode, `update_filter_params`
(`engine/layers.rs:895`), effect-scale drift
(`gpu/effect_layers.rs:99-102`), selection edits, canvas-geometry-neutral
property changes) makes `valid` false with `fresh == cache.stamps`, so
`d == None`. The full-walk branch runs (correct frame), but the snapshot
rule is defined only for `d ≥ 1`: it never fires, and nothing drops the
prefix. The cache then records the *new* ticks over the *old* prefix
texture. The next per-node-stamp-only change (a veil tick) finds
`valid ∧ prefix.through < d` and restores pre-change pixels. Concrete repro:
drag the bottom layer's opacity slider while a veil animates, every drag
frame full-walks correctly, then the first post-release tick visibly reverts
everything below the veil. Fix (three lines): the full-walk branch sets
`prefix = None` before composing; the `d ≥ 1` snapshot then re-establishes
it, and a property-only walk pays two extra composites of re-convergence,
which is the correct price. §7.3 must gain the regression row: establish the
prefix with two ticks, change a below-`through` child's opacity, tick again,
`assert_matches_from_scratch`, no existing row produces this sequence (the
chained row's *paint* below the veil moves `node_pixels`, lands `d ≤
through`, and legitimately refreshes the prefix via the full-walk snapshot,
so it cannot catch this).

**S2 (should fix, plan text): "the root group can never be all-clean here"
(§3.3) is false, and the plan's own test exercises the counterexample.**
Painting a hidden layer bumps `node_pixels_any` (the gate opens,
`gpu/compositor.rs:2714-2718`) while the root's stamps record the child as
`included = false, rev = 0`, unchanged, `d == None`, root all-clean. A
stroke on a screen-run member is the same shape. Both are *correct*
(the canvas composite genuinely excludes them; `composite_built` still
stamps, `composite_runs` still increments so the §7.3 hidden-subtree row's
anti-vacuity holds), and the screen-run case is even a free win. But the
sentence reads like an invariant an implementer would `debug_assert`, and
that assert would trip on the plan's own hidden-subtree pitfall test.
Reword: the root is *rarely* all-clean but must support the branch.

**S3 (should fix, PR 3 spec): passthrough children's stamps must fold the
inner children's `included` verdicts recursively.** A passthrough group
inlines its children into *this* group's accumulator
(`gpu/compose_walk.rs:1145-1174`), so a compositor-side no-op fact flipping
on an *inner* child (effect instance realized late, projection state,
`node_textures` entry) changes this group's output exactly as a direct
child's flip would, but §3.3 specifies `included` per child of the group
and `rev` as a fold of `node_pixels`/`animation` only, which does not carry
inner `included` flips. In practice every such flip appears bump-paired
(instance create/retain follows the document, `gpu/effect_layers.rs:216-232`;
deferred instance creation is gated on a `GroupState` whose creation bumps
`targets`, `gpu/compositor.rs:1717`, `gpu/effect_layers.rs:242-247`;
texture allocation is covered by the `mark_node_pixels_dirty` write-site
invariant, `gpu/compositor.rs:1863-1879`), but silently relying on pairing
is the exact shape R2 was raised to eliminate. One sentence pins it: the
shared inclusion predicate is applied recursively through passthrough
children when building their stamp, mirroring the inlining.

**S4 (should fix, plan text): §4's table double-counts PR 2.** Row A's cell
(3C + 2R + present) is only reachable *with* PR 2's cache-copy deletion:
PR 1+3 alone is 4C (clear→restore is net zero, the skipped blend is −1C, the
`composite_cache` copy remains). Row B's 2C likewise presumes PR 2. The last
row then claims an "additional −1C" composable with any row. The prose total
("A + PR 2 lands ... 3C") is right; either relabel rows A/B as including
PR 2 and scope the last row's −1C to row C, or correct the cells to 4C/3C.

**S5 (minor, plan text): §5's "five ping-pong flips unify into
`GroupState::advance()`" is orphaned parity-draft machinery.** No §6 step or
§10 line implements it, and the reshaped mechanism needs no flip chokepoint
(the snapshot reads `current_accum` after the child returns, however many
flips it made). Drop it, or move it to §6/§10 as an explicit PR 2 cleanup.

**S6 (minor, corrections and notes to record):**
- The snapshot point must be a *loop position*, not a dispatch: when child
  `d-1` is a not-included child, nothing "lands", the copy fires after the
  loop passes index `d-1`, dispatched or skipped (`accum[current]` is
  unchanged by a skip, so the content is still `children[0..=d-1]`).
- Folding the masked-leaf no-op into the walk-level skip *fixes a latent
  bug*: today a missing projection state advances the parent ping-pong
  without writing (`gpu/compose_walk.rs:637-647`), leaving later children
  blending over a stale half; a walk-level skip removes the advance. A
  behavior improvement, but an undisclosed one: state it, and it deserves
  its own row if the state is reachable in the fixture.
- Histogram guard, two notes. (a) If the target effect never dispatches:
  no realized instance (`:926-928`), or a hidden/off-path host: `needs()`
  stays true and the total guard forces full walks for the modal's whole
  life: same visible behavior as today (the histogram never lands), just
  zero reuse; acceptable, worth a sentence. (b) `needs()` flips false
  *mid-walk* at dispatch (the pending push, `gpu/histogram.rs:200-206`), so
  groups entered after the host may re-enable reuse within the same guarded
  composite. This is safe (bottom-to-top depth-first order means everything
  feeding the effect's input composes before the dispatch) but the plan
  should either state that argument or sample the guard once per composite.
- Under an invalid full walk, `last_d = d` records against stamps about to
  be replaced (possibly `None` despite a real change). With S1's prefix
  drop this only delays convergence by one composite; fine as is.
- Two loops (compose_group's recording loop and `compose_children`) will
  share the inclusion predicate but still duplicate the
  ctx-construction + `compose_into` dispatch scaffolding. Acceptable at
  this size; an optional per-child observer on `compose_children` would
  collapse them if the duplication grows.

**Scope, LOC, principles.** The reshaped PR 3 estimate (~160 added / ~15
removed) remains plausible; S1's fix adds single-digit lines and one
battery row (~25 test lines). All new state stays compositor-owned and
rebuildable; nothing flows document-ward; no registry or effect surface
learns the cache exists; no blocking readbacks. The reshape is a genuine
generalization, not added machinery: it deletes the parity/advance
bookkeeping and buys "below the first dirty child, anywhere" for one lazy
texture that PR 2 simultaneously frees. The revision responses to R1-R8
were each re-verified against the rewritten sections and carry through
consistently; no contradiction between rewritten and untouched sections was
found beyond S4/S5.

**Verdict: revise.** S1 is a user-visible staleness hole in the specified
snapshot rule with a common repro (property edit below an animating veil)
and no covering test; its fix is small and does not disturb the mechanism.
S2-S4 are specification and pricing corrections; S5/S6 are one-line pins.
The core prefix invariant, the restore's independence from skipped-walk
state, the `last_d` hysteresis, and index-vs-identity safety all survived
adversarial analysis; no rethink is warranted.

### Second revision response

All findings addressed in place; none rejected.

- **S1**: the full-walk branch now drops the prefix before composing, and
  the snapshot rule fires on that branch only when the walk was `valid`
  (§3.3 pseudocode); the opacity-below-an-animating-veil regression row is
  added to §7.3.
- **S2**: the "root can never be all-clean" sentence replaced with the
  correct statement (rare, but must be supported; no assert) (§3.3).
- **S3**: the inclusion predicate is applied recursively through passthrough
  children when building their stamps, so inner no-op flips move the stamp
  (§3.3).
- **S4**: §4's rows now show standalone and with-PR 2 figures, and the PR 2
  row no longer double-counts.
- **S5**: the orphaned `GroupState::advance()` mention removed from §5.
- **S6**: snapshot pinned to loop position d-1 whether the child dispatched
  or was skipped (§3.3 pseudocode); the masked-leaf latent
  advance-without-write defect disclosed as fixed by the walk-level skip
  (§3.3); both histogram-guard boundary notes recorded (§3.3 interlock);
  `last_d` under an invalid walk accepted as a one-composite convergence
  delay.

## 1. Problem

An animated canvas-space veil (VHS, grain, rainy glass) costs roughly 40% GPU
where the identical veil in screen space costs roughly 10%, in an effectively
empty document: one raster layer plus the veil, canvas roughly viewport-sized.
Moving one layer-panel slot across the divider quadruples the cost of the same
shader at nearly the same resolution. The resolution delta between canvas and
viewport explains only a fraction of that; the rest is redundant work.

### 1.1 The per-tick pass ledger (verified against source)

**Screen placement.** An animation tick calls `tick_animated_effects(.., true)`
and bumps only `present_inputs` (`gpu/frame_clock.rs:73-75, 89-91`). The
composite gate stays closed (`render_offscreen` returns at
`gpu/compositor.rs:2714-2718`), so the frame is present-only
(`present_and_screen_run`, `gpu/compositor.rs:2587-2691`):

| pass | size |
|---|---|
| present composite into run slot 0 | viewport |
| effect downscale | reduced |
| effect | reduced |
| effect upscale into run scratch | viewport |
| in-place apply into run slot 1 | viewport |
| blit run to surface (+ overlay) | viewport |

Four viewport-sized passes plus two reduced.

**Canvas placement.** The tick calls `tick_animated_effects(.., false)` and then
`self.revisions.bump_animation()` (`gpu/frame_clock.rs:81-87`), a global source
that `latest_composite_input()` folds in (`gpu/revisions.rs:180-182`), so the
whole composite is stale. `compose_group` (`gpu/compose_walk.rs:335-375`) then:

| pass | size |
|---|---|
| clear accumulator slot 0 | canvas |
| blend the (unchanged) raster layer | canvas |
| effect downscale | reduced |
| effect | reduced |
| effect upscale into `canvas_apply_scratch` | canvas |
| in-place apply into the other accumulator half | canvas |
| unconditional `composite_cache` copy (`compose_walk.rs:363-374`) | canvas |
| present to surface | viewport |

Five canvas-sized passes plus two reduced plus the present, and the pre-walk
CPU sync (`sync_projection_states` + `sync_effect_instances`,
`gpu/compositor.rs:2737`, audit §6 churn) runs as well.

Of those five canvas passes, only two (upscale target, apply) are inherent to
canvas placement. The clear, the layer blend, and the cache copy re-derive
facts that did not change.

### 1.2 Root causes

1. **The `animation` revision source is global.** `Revisions::bump_animation()`
   (`gpu/revisions.rs:117-119`) carries no layer id even though the tick loop
   holds the exact set of animated node ids
   (`tick_animated_layers` / `tick_animated_effects`, `gpu/frame_clock.rs:141-168`).
   Its single consumer is `latest_composite_input()`. The source exists as a
   separate scalar only so the histogram's `node_pixels_any` stamp
   (`gpu/histogram.rs:43-51`) does not churn during animation
   (`gpu/revisions.rs:60-63`: "Its own source rather than per-node pixel bumps,
   because histograms depend on pixels and must survive an animation frame
   mid-drag"). A one-time on-demand feature is shaping the compositor's source
   taxonomy. The per-node information exists at the call site and is discarded.

2. **The compose walk has no work skipping.** `compose_children`
   (`gpu/compose_walk.rs:385-433`) filters on `find_node` hits, `visible()`,
   isolation path, and screen-run membership, and on nothing else. Any stale
   composite means: clear, re-blend every layer, re-encode every effect, and a
   full-canvas `composite_cache` copy at the end of every `compose_group`
   (audit §2.4).

3. **The per-dab paint mark is global.** `gpu_stroke_to`
   (`engine/painting.rs:471`, mark at `:606`) calls `mark_dirty()`
   (a `document` bump) per stroke segment even though its signature carries the
   painted `layer_id`. This is what makes the measured painting-above-a-veil
   case (handoff §3.2: `painting` +43.5 ms per dirty frame at 2048²) hit the
   effect encode every frame, and it would also defeat any walk cache during a
   stroke.

The wrapper passes running at canvas resolution (upscale, apply) are inherent
to canvas placement and out of scope.

## 2. Prior art

Read from the checkouts under the project root. Citations re-verified at the
current checkouts unless marked (review-verified).

**Krita: damage starts at a node, never at a global flag.**
`KisMergeWalker::startTripImpl` (`krita/libs/image/kis_merge_walker.cc:28-42`)
builds the merge job from the changed leaf: the start node is `N_FILTHY`,
later siblings `N_ABOVE_FILTHY`, earlier siblings `N_BELOW_FILTHY`. There is no
"something changed" scalar; every invalidation names its node. That is the
model item 1 restores for animation ticks.

**Krita: nodes below the change do no per-node work.**
`KisAsyncMerger::startMerge` (`krita/libs/image/kis_async_merger.cpp:218-245`):
`N_FILTHY` recalculates its projection (`:218-224`); `N_ABOVE_FILTHY`
recalculates only when `dependsOnLowerNodes()` (`:225-233`), true exactly for
adjustment layers (`krita/libs/image/kis_projection_leaf.cpp:275-278`), Krita's
analog of Darkly's effect layer; `N_BELOW_FILTHY` is literally
`/* nothing to do */` (`:240-243`). Krita then still re-blends every leaf via
`compositeWithProjection` (`:245`) because its merges are dirty-rect-bounded
(`kis_base_rects_walker.h:413-424`, review-verified); Darkly's animated-veil
rect is the whole canvas, so rect-bounding buys nothing here and reusing the
below-the-change blend result is the lever instead. The two mechanisms are
complementary; dirty rects remain out of scope (§9).

**GIMP: reuse below the change is a property of topology, not a maintained
index.** `gimp_filter_stack_get_graph`
(`gimp/app/core/gimpfilterstack.c:213-219`) links the stack as a linear GEGL
chain; `gimp_filter_stack_add_node` (`:255-264`) splices between `node_below`
and `node_above`. Invalidation propagates downstream only, so the unchanged
upstream is reused by construction. The design below holds itself to that
standard: no hand-maintained "lowest dirty child" anywhere; validity is a
comparison of revision stamps at the point of consumption, the registry's own
model (`gpu/revisions.rs:1-27`).

**Krita: incremental compositing is defended by equality against a reference.**
`KisAsyncMergerTest::testMerger`
(`krita/libs/image/tests/kis_async_merger_test.cpp:51-121`, review-verified)
runs four incremental merges and compares against a stored reference image; the
commented-out block records the artifacts the old incremental path produced.
Darkly already has this harness shape: `assert_matches_from_scratch`
(`crates/darkly/tests/compositor_revisions.rs:100-118`) with its anti-vacuity
`composite_runs` guard, and the byte-equality battery macro (`:216-230`) with
~24 mutation rows. §7 extends it rather than inventing a parallel one.

## 3. Design

Three changes, each an independently landable PR, smallest first. A fourth
section states the interaction with `effect-invalidation-wiring.md`.

### 3.1 PR 1: per-node animation revisions (kill the global `animation` source)

`Revisions` replaces the scalar with a per-node map, exactly parallel to
`node_pixels`:

```rust
/// Per-node: this node's rendered appearance advanced without a document
/// edit or an authored pixel write. An animated void's or effect's clock
/// tick, a camera void's frame upload. Consumed by the composite (gate and
/// walk); deliberately not by thumbnails, content bounds, or histograms,
/// which depend on authored pixels.
animation: HashMap<LayerId, Tick>,
animation_any: Tick,
```

- `bump_animation(&mut self, id: LayerId)`, `animation(&self, id) -> Tick`,
  `animation_any(&self) -> Tick`; `remove_node` prunes both maps;
  `bump_all_for_test` stamps every entry plus the aggregate.
- `latest_composite_input()` becomes
  `document.max(node_pixels_any).max(animation_any)`. The frame gate's
  behavior is bit-identical to today; only the granularity behind it changes.
- `update_animations` bumps per ticked node: `tick_animated_layers` and
  `tick_animated_effects(.., false)` return (or fill a scratch with) the ids
  they actually advanced, and the canvas arm bumps each
  (`gpu/frame_clock.rs:81-87`). The predicates (`effect_animates`,
  `needs_animation`) are untouched.
- `upload_void_external_image` (`gpu/void_content.rs:374-396`) narrows its
  trailing `mark_dirty()` to `revisions.bump_animation(layer_id)`. This is the
  audit §4 cost ("an unfrozen camera void recomposites the whole tree every
  canvas tick") fixed at its source. A camera frame is derived appearance, not
  an authored pixel write, so thumbnails stay quiet exactly as today (the site
  never bumped `node_pixels`, the thumbnail cursor's source). Content bounds
  change behavior in both directions (review R4): today the site's `document`
  bump invalidates *every* layer's cached bounds on every camera frame
  (`gpu/content_bounds.rs:25-33` stamps `(document, node_pixels(id))`); after
  the narrowing, other layers' bounds stop churning at camera rate (the
  improvement), and the camera void's own bounds freeze at their last
  computation, since an upload moves neither of its stamp's sources. Accepted
  deliberately: an upload changes texel content, not the void's geometry, a
  live feed's bounds are its full frame in practice, and any `document` bump
  refreshes them. If a consumer ever needs upload-fresh bounds for a void,
  that invalidation belongs at the consumer, per-node.

**What the composite's staleness check becomes.** Unchanged in shape: the gate
compares `latest_composite_input()` against `composite_built`
(`gpu/compositor.rs:2714-2718`). What changes is that the information is no
longer destroyed on the way in: the walk (PR 3) can ask `animation(id)` and
`node_pixels(id)` per node.

**The histogram stops shaping the taxonomy.** The carve-out comment at
`gpu/revisions.rs:60-63` is deleted. The per-node `animation` source now earns
its place on its own merits (it is the signal that makes walk skipping
possible), and the histogram's stamp (`gpu/histogram.rs:43-51`) remains
`node_pixels_any` as the histogram's *own declared dependency*, documented at
the consumer: a histogram bins authored pixels; an animation tick does not
invalidate it, or it could never settle while a veil plays. That is the
registry's intended inversion (dependency lists live on the artifacts,
`gpu/revisions.rs:25-27`), not a registry-side exception. The same holds for
the other pixel consumers, which is why animation does not fold into
`node_pixels`: thumbnails (`drain_dirty_thumbnail_readbacks`,
`engine/rendering.rs:~655-676`, cursors over `node_pixels_iter`) and content
bounds (`gpu/content_bounds.rs:25-33`, stamp = `(document, node_pixels(id))`)
would otherwise churn at 30 fps under any animated layer. Three consumers need
"authored pixels", one (the composite) needs "authored pixels or advanced
appearance"; two per-node maps is the honest data model, and a merged map
would force those three consumers to grow filtering machinery instead.

A finer histogram stamp (fold of `node_pixels` over only the nodes below the
target, so painting *above* a Levels filter stops discarding its histogram) is
recorded as a follow-up in §9; it reuses PR 3's fold helper but is not needed
by this bug.

### 3.2 PR 2: delete the `composite_cache` copy (audit rec #6)

`GroupState::composite_cache` (`gpu/compositor.rs:258-259`) is filled by an
unconditional full-canvas `blit_region` at the end of every `compose_group`
(`gpu/compose_walk.rs:363-374`). Its only cross-frame role is providing a
*stable view* for consumers that must not care which ping-pong half the walk
ended on. Stability by copy is replaced with stability by index, the exact
pattern `blend_bind_groups` already uses for children
(`gpu/compositor.rs:499-506`):

- `GroupState` loses `composite_cache` / `composite_cache_view` (and their
  allocation in `create_group_state`). `current_accum` after the walk is the
  group's output half; `compose_group` stops resetting it outside the walk.
- `present_cache_bind_group` (`gpu/compositor.rs:534-539`) becomes a per-half
  pair `[wgpu::BindGroup; 2]`; the present pass and the test harness
  (`gpu/compositor_test_harness.rs:48-50`) select by the root's output half.
- `compose_group_arm`'s child-group blend (`gpu/compose_walk.rs:1196-1233`)
  reads the child's `accum.views[child_out]`; the `blend_bind_groups` key
  widens from `(parent, child, src)` to carry the child's half as well (still
  one `u8`: `src | child_out << 1`). Eviction rules are unchanged.
- `composited_texture()` / `composited_view()` (`gpu/compositor.rs:2505-2516`)
  return the root accumulator half. Consumer audit, all verified:
  - `engine/export.rs:38-47`, `engine/save.rs:145-169`,
    `engine/process_recording.rs:274-332`, `engine/preview.rs:261-279`,
    `engine/mod.rs:1159-1180` (test readbacks), `engine/painting.rs:984-1044`
    (sample-merged clone snapshot), `engine/rendering.rs:~40-60`
    (`PickSource::Merged`). Every one calls `render_offscreen` first or reads
    between composites, and every readback encodes its texture-to-buffer copy
    at request time into an immediately submitted encoder
    (`engine/rendering.rs`, `request_readback` inside `gpu.encode`), so no
    consumer can observe a half mid-overwrite: accumulators are written only
    inside `compose_group`, and command-queue ordering serializes the copy
    against any later composite. Two facts from the review's re-audit,
    recorded (review R8): `pick_color` (`engine/rendering.rs:178-213`,
    `PickSource::Merged`) is the one consumer that never forces a composite;
    it reads between composites, safe here because the re-pointed accessor
    returns the current output half and the 1×1 readback copies immediately;
    and `engine/save.rs` keeps a handle clone of the root texture in
    `SaveJob.pinned_textures` for the async job's life; content is already
    copied out in the same submit, so the pinned handle simply becomes the
    root accumulator texture, still benign.
  - `gpu/bake.rs:33-125` composes through its own sentinel `GroupState` and
    copies from the final accum already; only its doc comment ("its
    `composite_cache` must already be current", `:18`) needs rewording, since
    the walk it invokes recurses through `compose_group` either way.

Saves one full-canvas copy per group per composite and one canvas-sized
texture per group of VRAM. This is also what makes PR 3's "an unchanged nested
group is not re-walked" branch coherent: a group's output lives in its own
accumulators, which nothing but its own `compose_group` writes.

### 3.3 PR 3: walk-level work skipping (prefix resume)

This is the composite-prefix-cache plan rebased and reshaped per its review,
plus one scope decision made at revision time with the user: an earlier draft
resumed in the ping-pong accumulator itself (zero copies, no texture), but
its coverage condition (the dirty child must be the *topmost advancing*
child of its group) is a special case fitted to the benchmarks, not a
general mechanism. The adopted shape is the general one: **per-group child
stamps plus a prefix texture holding the composite of everything below the
first dirty child**, so the stack below the lowest change is reused no
matter where the change sits or what else sits above it. The zero-copy
parity trick is recorded in §9 as a possible later copy-elision layered on
this mechanism, not as the mechanism.

#### The mechanism

Per `GroupState`, compositor-owned, rebuilt freely (ownership: derived GPU
bookkeeping, exactly like `blend_bind_groups`):

```rust
struct WalkCache {
    /// One entry per child of this group, in document order.
    stamps: Vec<ChildStamp>,          // (id, included: bool, rev: Tick)
    /// Ticks the stamps were built under. Any drift means no reuse.
    built_document: Tick,
    built_targets: Tick,
    /// Composite of children[..=through], captured mid-walk. Lazily
    /// allocated on first snapshot, accumulator-sized, dropped with the
    /// GroupState on canvas resize.
    prefix: Option<Prefix>,           // { texture, view, through: usize }
    /// First-dirty index of the previous composite; the snapshot
    /// advance-rule's hysteresis (below).
    last_d: Option<usize>,
}
```

This is the texture PR 2 frees, re-earned: `composite_cache` was one canvas
texture per group copied on *every* composite and read only for view
stability; `prefix` is one canvas texture per group, lazy, copied only when
the split point moves. Net against today: the same texture count and
strictly fewer copies.

`ChildStamp.rev` is the memoized fold, over the child's subtree including its
filter nodes (masks), of `max(node_pixels(n), animation(n))`. `included`
captures every reason the walk skips *or no-ops* a child: both
`compose_children`'s skip chain (`find_node` miss (review C7), `visible()`,
isolation path, screen-run membership) and every arm's own no-op conditions,
which are compositor-side facts invisible to document revisions (reviews C1,
R2): a filter child without a realized instance or apply scratch
(`gpu/compose_walk.rs:926-931`), a leaf without a `node_textures` /
`layer_cache` entry (`:816-829`), a masked leaf without a projection state
(`:645-647`; an advance with no write, so folding it in also keeps the
advance counts honest), and a group child without a `GroupState`
(`:1179-1181`). The inclusion predicate is **one shared function** (an
iterator yielding each child with its verdict) consumed by the stamp fold,
the recording loop, and `compose_children` itself, never a second copy of the
skip chain (review R7). For a passthrough group child the predicate is
applied **recursively** when building its stamp, mirroring the inlining: a
compositor-side no-op fact flipping on an inner child changes this group's
output exactly as a direct child's flip would, so the inner verdicts are
part of the stamp's equality rather than left to the bump that usually
accompanies them (second review S3). Folding the masked-leaf no-op into the
walk-level skip also fixes a latent defect: today that arm advances the
parent ping-pong *without writing* (`:645-647`), handing every later child a
stale half to blend over; under the shared predicate the child is excluded
and never advances (second review S6, disclosed as a behavior improvement).
`rev` is recorded as 0 when `included` is false, so mutating an invisible
subtree never forces a recompose (the later visibility toggle is a
`document` bump and invalidates everything anyway).

`compose_group` becomes:

```text
fresh   = build stamps (memoized fold, one map per composite)
valid   = cache exists
        ∧ cache.built_document == revisions.document()
        ∧ cache.built_targets  == revisions.targets()
        ∧ histogram guard not forcing this group     (below)
d       = first index where fresh differs from cache.stamps
          (length mismatch = index of divergence; equal = None)

if valid ∧ d == None:
    nothing below this group changed: leave the accumulators alone,
    keep current_accum, walk nothing.                 (nested-group skip)
elif valid ∧ prefix exists ∧ prefix.through < d:
    copy prefix → accum.textures[0]; current_accum = 0
    compose children[prefix.through+1 ..]              (prefix resume)
else:
    prefix = None       (S1: a coarse bump can change output below any
                         stamp move, a prefix never survives a full walk;
                         the snapshot below re-establishes it)
    clear accum[0]; current_accum = 0; compose all children   (full walk)

snapshot rule (after the child loop passes position d-1 (d ≥ 1), whether
that child dispatched or was skipped (a skip leaves accum[current]
unchanged, so it still holds children[0..=d-1]) second review S6):
copy accum[current] → prefix, prefix.through = d-1.
On the full-walk branch: only when the walk was `valid`; an invalid walk's
d was computed against untrustworthy stamps, so it drops the prefix above
and takes no snapshot; the next, valid composite re-establishes it.
On the resume branch: only when d == cache.last_d (the same depth was dirty
twice running).

record fresh stamps + last_d = d + ticks
```

Both copies are `copy_texture_to_texture`: exact, no sampler, no format or
premultiplication question, the idiom `snapshot_parent_accum` already uses
(`gpu/compose_walk.rs:1005-1028`). The restore replaces the full-canvas clear
pass, so a resumed composite costs one copy over the walk it skips. The
`last_d` hysteresis is what keeps a single prefix from thrashing: a stable
edit depth (an animating veil, a stroke on one layer) advances the prefix to
just below it within two composites and then re-fires nothing, while
alternating dirty depths (painting above an *animating* veil, where `d` flips
between the veil and the paint layer every frame) never advance the prefix,
the walk settles into resuming from below the lower depth every frame instead
of oscillating between full walks and re-snapshots. Convergence is at most
two composites for any newly stable depth; the fallback is always today's
full walk.

Stamp recording lives in `compose_group`'s own child loop, driven by the
shared inclusion predicate above; `compose_children` keeps serving the
passthrough recursion and `bake_subtree_to_layer` untouched. There are no
advance counters and no parity bookkeeping: the review's R6 finding is moot
under this shape, and the snapshot point ("after child `d-1` lands") is
well-defined regardless of how many ping-pong advances any child performs,
because it reads `current_accum` after the child's `compose_into` returns.
Bakes need no special handling: `bake_subtree_to_layer` bumps `targets`
twice and `mark_dirty()` on exit (`gpu/bake.rs:49, 124, 127`), so every merge
or flatten drops all walk caches and the next composite is a full walk,
correct, self-healing, and expected rather than a cache bug (review R8).

The root group is rarely all-clean but must support the branch: a bump on
an excluded subtree (painting a hidden layer, a stroke on a screen-run
member) opens the frame gate while every root stamp holds, and the resulting
all-clean composite is correct, since the canvas composite genuinely excludes
those nodes (second review S2; no assert may assume the root walks). A
nested non-passthrough group is all-clean frequently, and the `d == None`
branch is Krita's `N_BELOW_FILTHY` "nothing to do" reached from the other
side: the parent blends the child's existing output half (PR 2) and the
child's subtree contributes zero passes.

#### The safety model (review B1, B2)

There is **no `CompositeMode`**. The prior plan threaded an
`Interactive`/`Authoritative` mode so persisted pixels never came from a
resumed walk; its review (B1) showed the guarantee was already false, because
every persistence path enters through `render_offscreen`, which early-returns
on a clean gate and hands back the previous interactive composite verbatim
(`engine/export.rs:38`, `engine/save.rs:145`, and the gate at
`gpu/compositor.rs:2714-2718`). That is today's shipped semantics: **the
composite in the accumulators is the composite.** Forcing exports and timed
autosaves (`SavePurpose::Snapshot`, recording at ~1.5 s intervals) to
recomposite from scratch would be a real regression for zero structural
safety. Instead:

1. **Coarse-by-default invalidation.** All 68 `mark_dirty()` call sites
   (count verified by the review) bump `document`, and `built_document`
   mismatch disables all reuse. Nobody has to audit those sites; forgetting to narrow one costs a
   full walk, never a stale pixel. Only three narrow paths exist after this
   plan (§3.4), each with battery coverage.
2. **The byte-equality battery** (`tests/compositor_revisions.rs`) is the
   executable form of the invariant, extended in §7 with the mutation classes
   its review flagged (C4). Its harness already defends against the C3
   ordering hazard: `assert_matches_from_scratch` asserts the incremental read
   actually recomposited (`composite_runs` delta == 1) before trusting the
   comparison (`:100-118`).
3. `test_readback_canvas_from_scratch` / `test_invalidate_all`
   (`engine/mod.rs:~1245`, `gpu/compositor.rs:1933-1936`) already bump every
   source; `bump_all_for_test` gains the per-node animation stamps, so the
   from-scratch reference also drops every `WalkCache` via the `document`
   mismatch.

The worst reachable failure, if a stamp fold missed a dependency, is a stale
composite that the next `document` bump clears, and the battery exists to make
that unreachable, not merely rare.

#### Histogram interlock (reviews C2, R1)

`compose_effect_arm` dispatches the LUT histogram against the effect's live
input mid-walk (`gpu/compose_walk.rs:949-959`); a skipped prefix would starve
`pump_node_histogram` forever, and a per-host guard is not enough, because
when the host group sits below an ancestor's first dirty child, the
ancestor's resume or all-clean branch never enters the host's `compose_group`
at all (review R1). Guard, the total form: while `histogram_target` is
`Some(t)` and `histogram.needs(&revisions, t)`, the composite refuses **all**
reuse, every group full-walks. A histogram is owed only while a LUT-style
modal is focused and its result is one async readback away, so the state is
rare and transient; computing the root-to-host path to guard more narrowly is
not worth the code. One compounding consequence, from narrowing #3 (review
R5): while such a modal is focused, a stroke's per-dab `node_pixels` bumps
keep `needs()` true on every mid-stroke frame, so painting with a LUT modal
open re-bins per frame and gets no walk reuse for the modal's duration,
accepted as transient modal behavior, stated so §4's pricing is read
correctly. The finer below-the-target histogram stamp recorded in §9 removes
both effects. Two boundary notes (second review S6): the guard samples
`needs()` once at walk entry; the mid-walk flip to false at dispatch is
safe by bottom-to-top walk order (everything feeding the effect's input
composes before the dispatch), but sampling once keeps the composite's
branches consistent regardless; and a target that can never dispatch (no
realized instance, a hidden or off-path host) keeps `needs()` true and
therefore forces full walks for the modal's whole life, the same visible
behavior as today (the histogram never lands), just with zero reuse, ended
when the engine clears `histogram_target` on modal close.

#### Coverage, stated honestly

Everything below the first dirty child is reused wherever that child sits:
editing a middle layer, an animated veil in the middle of the stack, a
filter-slider drag once its mark is narrowed (§8), all resume, and the
layers below never re-blend. What one prefix per group cannot do is serve
two alternating dirty depths perfectly: painting above an *animating* veil
flips `d` between the veil (canvas-tick frames) and the paint layer
(paint-only frames), the `last_d` rule pins the prefix below the veil, and
the steady state is: every frame resumes from below the veil (base layers
never re-blend), the veil re-encodes every frame (necessarily on tick
frames, redundantly on paint-only frames), and the paint layer blends. With
a *static* veil below, `d` is stable at the paint layer, the prefix advances
above the veil, and every dab skips the veil encode: the measured +43.5 ms.
No regression anywhere; the fallback is today's full walk. Closing the
alternating case fully needs either a second prefix per group or the
zero-copy parity elision from the earlier draft layered on top (resume the
upper depth in the intact ping-pong half when its condition happens to
hold); both are deferred to §9 until a measured workload justifies them.

### 3.4 The complete list of narrowed invalidation paths (review B2)

The prior plan's review found ~25 undisclosed narrowings. This plan has
exactly three, each disclosed, each with battery rows:

1. **Animation ticks** (`gpu/frame_clock.rs:81-87`): global
   `bump_animation()` becomes per-node bumps. Consumers unaffected by
   construction (nothing but `latest_composite_input` read the scalar).
2. **Camera/external void upload** (`gpu/void_content.rs:395`):
   `mark_dirty()` becomes `bump_animation(layer_id)`. Thumbnails and content
   bounds see exactly what they saw before (the site never bumped
   `node_pixels`); the composite still wakes via `animation_any`.
3. **Per-dab paint mark** (`engine/painting.rs:606`): `mark_dirty()` becomes
   `mark_node_pixels_dirty(layer_id)`. The signature carries the id
   (`gpu_stroke_to`, `:471`), and `mark_node_pixels_dirty`'s own write-site
   invariant (`gpu/compositor.rs:1859-1879`) says this site should have been
   narrow all along. Verified consequences:
   - content bounds: stamp is `(document, node_pixels(id))`
     (`gpu/content_bounds.rs:25-33`), so the painted layer's bounds still go
     stale, and *other* layers' bounds now survive a stroke (an improvement;
     the dropped-invalidation defect the prior review's B2 feared does not
     exist under the registry, where the per-node stamp moves with the bump).
   - histogram: today the per-dab mark is a `document` bump and
     `node_pixels_any` moves only at stroke end, so a focused LUT histogram
     goes stale once per *stroke*; after the narrowing it goes stale per
     *dab* (review R5), and combined with the total interlock (§3.3) this
     means painting with a LUT modal focused re-bins per frame and gets no
     walk reuse for the modal's duration. Accepted: the modal is transient,
     and the finer below-the-target stamp in §9 removes both effects.
   - thumbnails: `drain_dirty_thumbnail_readbacks` queues once per revision
     change, so a per-dab bump would queue a thumbnail readback per frame
     mid-stroke, which the site's comment deliberately avoids
     (`engine/painting.rs:602-605`). Mitigation, engine-side session logic:
     the drain skips the engine's `active_stroke_layer` while a stroke is in
     flight; the stroke-end marks (`engine/painting.rs:1570` (`end_stroke`,
     `:1561`) flood-fill commit; `engine/rendering.rs:950` is the undo
     pixel-restore mark, not a stroke-end site (review R8)) land the one
     panel update per stroke, preserving today's cadence exactly.

Every other `mark_dirty()` site, including `update_filter_params`
(`engine/layers.rs:895`), undo/redo (`engine/rendering.rs:991`), transform
preview publishes (`gpu/floating_preview.rs:159, 300, 448, 653`), and
isolation changes, keeps its coarse `document` bump and therefore full
invalidation.

## 4. Priced against the alternatives (review B3)

Per-tick cost on the two measured cases; "C" = canvas-sized pass, "R" =
reduced, "V" = viewport-sized. Today's canvas tick: 5C + 2R + present (§1.1).

| mechanism | animated canvas veil (this bug) | painting above a veil (handoff §3.2) | new textures | steady-state copies |
|---|---|---|---|---|
| **A. per-group prefix texture (this plan, PR 1+3)** | clear becomes a restore copy, blends below skipped: 4C + 2R + present standalone; **3C + 2R + present with PR 2** | static veil below: veil encode and everything below skipped, per-dab cost one blend + one restore copy; *animating* veil below: base blends skipped every frame, veil re-encodes every frame (§3.3 coverage) | 1 canvas texture per resuming group (lazy; net zero against PR 2's deletion) | 1 restore copy per resumed composite; snapshot copies only on stable split moves |
| **B. zero-copy parity resume (earlier draft of this plan)** | 3C + 2R + present standalone; 2C with PR 2 | topmost-child edits only: resume engages solely when the dirty child is the topmost advancing child; middle-layer edits and all two-depth workloads full-walk | none | none |
| **C. per-effect output cache (prior review's B3 proposal)** | **no help**: an animated effect must re-encode, and the walk below still re-clears and re-blends every layer (5C + 2R unchanged except the encode's input) | veil encode skipped, but every layer still re-blends per dab | 1 texture per effect layer | none |
| PR 2 (composite_cache removal) | −1C for every composite (rows A/B's "with PR 2" figures already include it) | same | −1 canvas texture per group | −1 per composite |

B was this plan's first draft and is rejected as benchmark-fitted (a user
call at revision time): its coverage condition happens to enclose exactly
the two measured workloads and nothing else (any middle-layer edit
full-walks) and its parity invariant proved brittle to reason about (the
draft itself misstated the boundary; review R3). It survives in §9 as a
possible copy-elision on top of A, saving A's one restore copy when the
condition happens to hold. C is rejected: it does not touch this bug at all
(the prior review's B3 argument was scoped to symptom B and does not survive
contact with the animated-veil ledger), it needs the same "did anything
below me change" stamp machinery as A to know when to skip, and once A
exists it adds nothing: whenever an effect's below-input is unchanged, the
effect sits above the first dirty child only if something *above* it
changed, in which case its own encode is in the reused prefix already.
A + PR 2 lands the canvas tick at 3C + 2R + present against the screen
path's 4V + 2R: at similar canvas/viewport sizes the canvas tick stops being
the expensive one, and the remaining delta is the honest resolution
difference plus the pre-walk sync churn (audit §6, separate work).

## 5. Architectural impact

- `gpu/revisions.rs`: `animation` scalar becomes a per-node map + aggregate.
  No consumer outside the compositor reads the scalar today (verified by
  grep: `frame_clock.rs`, `latest_composite_input`, `bump_all_for_test`).
- `gpu/compositor.rs` / `gpu/compose_walk.rs`: `GroupState` loses two texture
  fields (PR 2) and gains `WalkCache` (PR 3); `compose_group` gains the three
  branches and the snapshot/restore copies; present bind groups become
  per-half.
- `engine/`: one narrowed paint mark, the thumbnail-drain stroke guard, and
  test accessors. No engine API changes.
- **Document**: untouched. No new field survives save/load; the walk consults
  the document for structure only. Ownership check per state item:
  `animation` map is compositor (rebuildable: worst case one extra composite);
  `WalkCache` is compositor (rebuildable: next walk repopulates); the
  thumbnail guard reads existing session state (`active_stroke_layer`).
  Nothing flows upward.
- **Modularity**: no registry, node-kind, or effect-trait surface changes.
  `LayerNode::compose_into` dispatch is untouched; effects and voids never
  learn the cache exists (the review's C6 mode-threading concern is moot since
  there is no mode). The rejected third-accumulator variant (which would have
  widened `Effect::create_cache`'s `[TextureView; 2]` through all effect
  files) stays rejected for the same modular-boundary reason as before.
- **No WASM or frontend changes.**

## 6. Implementation steps

### PR 1: per-node animation revisions (~+55 / −20 production)

1. `gpu/revisions.rs`: map + aggregate + accessors; prune in `remove_node`;
   extend `bump_all_for_test`; rewrite the source's doc comment; delete the
   histogram carve-out comment.
2. `gpu/frame_clock.rs`: canvas arm bumps per ticked id (both loops report the
   ids they advanced).
3. `gpu/void_content.rs`: `upload_void_external_image` narrows to
   `bump_animation(id)`.
4. `gpu/histogram.rs`: rewrite `stamp()`'s comment as the consumer's declared
   dependency.
5. Tests (§7.1).

### PR 2: composite_cache removal (~+75 / −65 production)

6. Per-half present bind groups; harness updated; `composited_texture()` /
   `composited_view()` re-pointed; child-group blend key widened; the
   `compose_group` blit and both `GroupState` fields deleted; `bake.rs` and
   stale doc comments reworded.
7. Tests (§7.2).

### PR 3: walk skipping (~+160 / −15 production)

8. The shared per-child inclusion predicate/iterator (extracted from
   `compose_children`'s skip chain plus the arm no-op conditions, consumed by
   the walk and the stamp fold: review R7).
9. `WalkCache` (stamps, `prefix`, `last_d`), the memoized stamp fold, the
   three-branch `compose_group` with the restore copy, the snapshot rule and
   its `last_d` hysteresis; histogram guard.
10. Narrow `engine/painting.rs:606`; add the thumbnail-drain stroke guard.
11. Two testing counters beside `composite_runs`
    (`gpu/compositor.rs:565-568`): `walk_resumes` (resume branch taken) and
    `walk_all_clean` (all-clean branch taken), split per branch so each §7.3
    anti-vacuity test pins the branch it claims to exercise (review R8), plus
    engine passthroughs.
12. Tests (§7.3).

## 7. Tests

Per the project's direction for this plan: no test asserts a performance
number or a pass count as its purpose. The budget goes to correctness of the
caching logic: byte-equality against from-scratch references across every
mutation class, and targeted tests for each staleness pitfall found in the
prior review. Counters (`composite_runs`, `walk_resumes`, `effect_rebuilds`)
appear only as instruments inside correctness tests, proving a comparison was
not vacuous (a reuse test that silently full-walks proves nothing, and an
equality test that silently skips proves nothing; the existing harness already
guards the second, `tests/compositor_revisions.rs:100-118`).

All in `crates/darkly/tests/compositor_revisions.rs`, extending the existing
fixture, harness, and `stale_composite_battery!` macro.

### 7.1 PR 1

- The existing `an_animation_tick_leaves_no_stale_composite` (`:362-404`)
  must keep passing unchanged; it is the regression net for the source
  swap.
- Narrowing #2's contract, pinned at the registry rather than through the
  upload call. **Discovered during implementation:** `ExternalImageSource`
  has exactly one variant and it is `#[cfg(target_arch = "wasm32")]`
  (`gpu/void.rs:37-45`), so on native the enum is uninhabited and
  `upload_void_external_image` is unreachable from any test; the
  battery-shaped case this section originally specified cannot exist. What
  the narrowing actually asserts is covered instead by
  `an_animation_bump_leaves_the_pixel_consumers_alone` (`gpu/revisions.rs`
  unit tests): an animation bump moves neither `node_pixels(id)` nor
  `node_pixels_any`, which is precisely what keeps thumbnails, content
  bounds and histograms quiet, including the disclosed consequence that the
  void's own bounds stamp does not move (review R4). The composite-staleness
  half of the same path is covered end to end by the animated-void and
  animation-tick rows, which are natively reachable.
- Unit tests in `gpu/revisions.rs`: per-node animation bumps name their node
  and move `animation_any` and `latest_composite_input`; they leave the pixel
  consumers' stamps alone; `remove_node` prunes both maps;
  `bump_all_for_test` covers the map.
- `a_histogram_survives_an_animating_veil` (engine level): the consumer-side
  form of the same claim, and the executable statement that the deleted
  carve-out was not load-bearing.

### 7.2 PR 2

- The whole existing battery must pass unmodified (it reads through
  `composited_texture`, so it exercises the re-pointed accessor everywhere).
- New: two consecutive composites that end on *opposite* halves (one layer,
  then two, forced by an added layer) each present correctly, pinning the
  per-half bind-group selection; a present-only frame after a composite
  (view-transform change, no document change) still shows the last
  composite (pins accumulator stability between composites).
- New: merge-down / flatten / export byte-checks after a composite that ended
  on half 1 (the consumers audit, executable).

### 7.3 PR 3

**Battery extension**, closing the prior review's C4 gaps; every row is one
mutation plus `assert_matches_from_scratch`:

- transform preview: start, drag update, commit; and the paste (floating)
  drag + commit paths (`mark_node_pixels_dirty` sites
  `engine/floating.rs:968, 1058`)
- fill inside an active selection
- a sample-merged clone stroke (paints *from* the composite, so it consumes
  `composited_texture` mid-session)
- paint into a layer inside a nested non-passthrough group (exercises the
  nested group's own `WalkCache` and the parent's stamp fold across a group
  child)
- a masked passthrough group's child painted (the snapshot+apply path,
  `compose_passthrough_masked`)
- void param change; void animation tick (fixture gains an animated void row)
- vector scene push
- an effect added inside a newly created group in the same mutation (review
  C1: instance existence folded into `included`; this is the "instance appears
  a frame late" shape)
- chained: animation tick, then paint below the veil, then tick again (two
  dirty depths across consecutive composites; must stay byte-correct through
  prefix restores and the `last_d` rule)
- flatten

**Staleness pitfalls, individually pinned:**

- Reuse actually engages (anti-vacuity for the whole battery): drive the
  animated-veil fixture across two canvas ticks with readbacks between;
  assert bytes match from-scratch *and* `walk_resumes` advanced. Same shape
  for the all-clean nested-group branch against `walk_all_clean`, so each
  test pins its own branch rather than sharing one counter (review R8).
- The resume boundary: a mutation at or below `prefix.through` must full-walk
  (bytes match, `walk_resumes` does *not* advance); one above it must resume;
  and alternating dirty depths across consecutive composites stay
  byte-correct while the prefix holds still (pins the `last_d` hysteresis,
  no snapshot advance on the resume branch unless the depth repeats).
- The S1 regression (second review): establish the prefix with two veil
  ticks, change a below-`through` child's *opacity* (a `document` bump that
  moves no per-child stamp), tick again, `assert_matches_from_scratch`,
  fails without the full-walk branch dropping the prefix, because the
  post-change tick would restore pre-change pixels. No other row produces
  this sequence (paint below the veil moves a stamp and legitimately
  refreshes the prefix).
- The histogram interlock: focus a LUT filter (set `histogram_target`), let
  the veil animate, and assert the histogram result lands (would hang/starve
  without the guard) and the composite still matches from-scratch. A second
  case with the LUT filter inside the nested group and the mutation at root
  level above its host (review R1's starvation shape: the case a per-host
  guard would pass on a flat fixture while starving here).
- Hidden-subtree paint: paint a hidden layer, assert no stale composite and
  (via `composite_runs`) that making it visible again produces the correct
  image (pins the `included`-gated `rev: 0` rule).
- Thumbnail cadence: a stroke of several dabs queues no thumbnail readback for
  the active layer until stroke end, then exactly one lands (pins narrowing
  #3's mitigation; asserts on the thumbnail cache/queue, not on timing).

## 8. Interaction with `docs/plans/effect-invalidation-wiring.md`

That plan (reviewed `revise`, revised, never implemented) predates the
revision registry, and its text needs mechanical rebasing regardless of this
plan: `mark_effect_dirty` and the flags it cites no longer exist; its Problem
A maps today to "route screen-space property edits to
`bump_present_inputs` instead of `bump_document`", and its Problem B blanket
still exists verbatim (`engine/rendering.rs:682-685`,
`if pending_completed { self.compositor.mark_dirty(); }`).

- **No contradiction, no duplication.** This plan touches none of its call
  sites (`update_filter_params` keeps its coarse `document` bump here;
  `poll_pending` is untouched). Textual collision is limited to independent
  lines in `engine/rendering.rs`.
- **Ordering: either order works; landing it before PR 3 is preferable.** Its
  Problem B fix matters more once walk caches exist: today the readback
  blanket merely forces a recomposite; under PR 3 each blanket `mark_dirty`
  also discards every `WalkCache`, so e.g. a thumbnail landing mid-animation
  degrades reuse for a frame. Not a correctness issue (coarse bumps are always
  safe), purely lost wins.
- **A follow-up that plan unlocks after PR 3, recorded, not folded in
  silently:** its canvas arm ("a canvas-space effect param edit still marks
  globally") could later narrow to a per-node bump of the filter's id, making
  slider drags on a canvas effect reuse the prefix below it. That belongs to
  that plan's revision cycle, with its own battery row.

## 9. Risks and unresolved questions

1. **A mutation that changes composite output without bumping any source.**
   Under the registry such a path is already a bug today (the frame gate would
   skip the composite entirely, visible as a stuck canvas). Walk skipping adds
   a new exposure only for *per-node* narrowness in the three §3.4 paths; the
   battery rows are the defense. Residual risk accepted and stated.
2. **One prefix per group.** Two alternating dirty depths share one prefix:
   the `last_d` rule pins it below the lower depth, so the upper depth's
   frames redundantly re-run everything between the two (§3.3 coverage). If
   profiling shows this matters, the recorded extensions are a second prefix
   per group or the zero-copy parity elision (§4 row B) layered on the
   resume branch. VRAM is one lazy canvas-sized texture per resuming group,
   net zero against PR 2's deletion of `composite_cache`; no eviction policy
   in v1, the same lifecycle class as the texture it replaces (dropped with
   the `GroupState` on canvas resize).
3. **Thumbnail cadence guard** (narrowing #3): skipping the active stroke
   layer in the drain is session-state logic in the engine; the alternative (a
   frame-divisor throttle in the drain) is simpler but changes cadence for all
   callers. Plan proposes the skip; reviewer may prefer the throttle.
4. **Pre-walk CPU churn per tick** (`sync_projection_states` +
   `sync_effect_instances` clone strings and params per filter per composite,
   audit §6): untouched here, still on every resumed composite. Separate
   cleanup; noted so the remaining canvas/screen delta after this plan is not
   misread as a caching failure.
5. **`walk_resumes` counter placement**: it counts a branch, not a pass; kept
   `#[cfg(any(test, feature = "testing"))]` beside `composite_runs`.
6. **Finer histogram stamp** (fold of `node_pixels` below the target instead
   of `node_pixels_any`): recorded follow-up; reuses PR 3's fold; would stop a
   paint above a Levels target from discarding its histogram.
7. **Multi-engine sessions**: all new state is per-`Compositor`, one per
   handle; no cross-handle interaction.

## 10. LOC estimate (added / removed, not touched)

| area | added | removed |
|---|---|---|
| PR 1: `gpu/revisions.rs`, `gpu/frame_clock.rs`, `gpu/void_content.rs`, `gpu/histogram.rs` comments | ~55 | ~20 |
| PR 2: `gpu/compositor.rs`, `gpu/compose_walk.rs`, harness, `gpu/bake.rs` | ~75 | ~65 |
| PR 3: `gpu/compose_walk.rs` / `gpu/compositor.rs` (shared inclusion predicate, `WalkCache`, fold, branches, snapshot/restore, guard), `engine/painting.rs`, `engine/rendering.rs` (drain guard), counters + accessors | ~160 | ~15 |
| **production total** | **~290** | **~100** |
| tests: `compositor_revisions.rs` battery rows + pitfall tests + `revisions.rs` units + PR 2 half-selection tests | ~315 | ~10 |
| **tests total** | **~315** | **~10** |
| docs: this plan | ~430 | 0 |
| docs: `docs/gpu-passes.md` (pass-ledger update), audit banner note | ~25 | ~5 |
| **docs total** | **~455** | **~5** |

The honest split: PR 1 and PR 2 are small, mostly mechanical, and
independently valuable (PR 2 alone removes a full-canvas copy and a texture
per group). PR 3 is ~160 added lines in the compose walk, the crate's most
intricate area. This is under half the production LOC of the prior plan's
equivalent scope (~385/~115), with the reduction coming from dropping
`CompositeMode` (mooted by its review's B1), the parity/advance bookkeeping
(dropped with the earlier draft's mechanism), and the prior plan's
`composite_epoch`/`node_revisions` groundwork (already landed as the
revision registry).
