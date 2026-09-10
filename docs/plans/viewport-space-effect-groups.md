# Viewport-space effect groups: render them, and refuse illegal placements loudly

## Revision (step 3): **this section governs**

The independent review below returned `revise`. This section records the
disposition of every finding and **replaces §4 (Stage A) wholesale**; §5 (Stage B)
is amended in place by the deltas listed here. Where this section and the body
disagree, this section wins. The review is preserved verbatim underneath.

Three review claims were independently re-verified before revising, because they
are the ones that change the design:

- A passthrough group's opacity and blend mode are already ignored in canvas
  space: `compose_group_arm` inlines children and returns
  (`gpu/compositor.rs:5534-5539`). So flattening a screen-space group loses
  nothing.
- `CompoundAction::undo` iterates **in reverse** (`undo/compound.rs:25`), so the
  original plan's undo justification for not enforcing in `Document::link` is
  false.
- `LayerMoveAction` stores only two `TreeSlot`s (`undo/layer.rs:96-124`) and
  never captures `screen_space_count`, which `Document::move_layer` overwrites
  destructively via `set_screen_space_members` (`document/mod.rs:1091`). The move
  path's damage really is unrecoverable by undo.

### R.1 Stage A is now **R4: flatten the run**, not R1

The original Stage A routed the screen-space chain through `compose_children` by
parking a screen accumulator in `Compositor::group_state` under a sentinel key.
Review finding D2 showed the option set was a two-way framing between that and a
strawman, omitting a materially cheaper third shape. **R4 is adopted.**

The reasoning, in the order it matters:

1. **A screen-space group carries no compositing semantics.** To be eligible it
   must be passthrough and unmasked and non-empty
   (`layer.rs:740-758`). Passthrough means no accumulator; unmasked means no
   projection; and passthrough opacity/blend are already discarded in canvas
   space (`compositor.rs:5534-5539`). Visibility is transitive through
   `effective_visible`, which the screen path already calls
   (`compositor.rs:3748`). A flattened list of the group's effect descendants
   therefore produces **identical pixels** to a recursive walk.
2. **Prior art flattens too.** Krita's `KisProjectionLeaf` splices a pass-through
   group's children into the parent's sibling chain
   (`krita/libs/image/kis_projection_leaf.cpp:196-221`) precisely so the walker
   never sees the group. Darkly's run is already a flat sibling chain; R4 keeps
   it one.
3. **R1's DRY argument does not survive contact.** `compose_children` exists to
   handle isolation paths, masks, group accumulators, blend and scissor: every
   branch of which is inapplicable above the divider. Reusing it means reusing a
   walk none of whose decisions apply, and then papering over the isolation
   filter it drags along (the original §4.4 bullet). That is reuse of a shape,
   not of a rule.
4. **It eliminates four risks and two defects outright**: the `set_canvas_rect`
   trap (§8.1.1), the borrow-splitting gamble (§8.1.2), the scratch pool (B6) and
   the blit bind-group re-owner (B7) all become moot, along with review finding
   D1's `AccumId` counter-proposal.

Stage A production cost drops from ~+252/−276 to roughly **+50/−15**.

#### R.1.1 The one new document query

Add to `crates/darkly/src/document/mod.rs`, beside `screen_space_run`:

```rust
/// The effect layers realized in screen space, bottom-to-top: the run,
/// flattened.
///
/// A group above the divider is passthrough, unmasked, and holds only nodes
/// that themselves qualify, so it contributes no compositing of its own
/// (`LayerNode::supports_screen_space`). Its effects are therefore siblings of
/// the run's leaves as far as the present chain is concerned, and the chain
/// consumes this list rather than the run's root-level members.
pub fn screen_space_effects(&self) -> Vec<LayerId>
```

Depth-first over `screen_space_run()`, descending into groups, collecting nodes
that are `Layer::Filter`. Structure only: **no visibility filtering here**; the
compositor keeps its existing `effective_visible` filter, which already handles
an invisible ancestor group transitively.

This is the whole of the document-side change. `screen_space_run()`,
`renders_in_screen_space()`, `slot_of()` and `TreeSlot.screen_space` keep their
current root-child meaning, untouched, which is what stops undo from trying to
restore a *nested* effect as a root-level run member.

#### R.1.2 The two compositor call sites

- `present_and_screen_run` (`gpu/compositor.rs:3744`): derive `run` from
  `doc.screen_space_effects()` instead of `doc.screen_space_run()`. The
  `effective_visible` filter, the `effect_instances.contains_key` filter, and the
  entire encode loop at `:3805-3828` are unchanged.
- `sync_effect_instances` (`gpu/compositor.rs:4727`): build the screen set from
  `doc.screen_space_effects()` and keep the `contains` test. Because
  `all_filter_layers` yields only filters and `screen_space_effects` yields
  exactly the filters in screen space, membership is now exact, and the nested
  effect stops being mis-tagged `Canvas { parent: root }`.

The original §4.2's `render_space` query is **dropped**: it existed to serve a
walk that no longer exists, and `screen_space_effects()` answers the same
question in the only place that asks it.

#### R.1.3 `group_layers` must preserve the run (C-finding, confirmed)

`engine/layers.rs:694` hardcodes `screen_space: false` when reinserting the new
group, so grouping run members (the obvious gesture for building a viewport
effect group) silently drops the whole arrangement to canvas space. Set it from
whether the topmost source was a run member.

**Ordering wrinkle, flagged for implementation:** `supports_screen_space` answers
`false` for an *empty* group (`layer.rs:735-738`), and `reinsert_entity` restores
through `restore_to_screen_space`, which clamps against
`qualifying_screen_space_suffix`. The group is created empty and filled
afterwards, so restoring it into the run before its children move in will clamp
to zero. The restore has to happen after the children are attached, or the
reinsert has to be re-ordered. Verify with the test in R.4(e) rather than by
inspection.

### R.2 Stage B amendments

Stage B's shape stands. Five deltas:

1. **Delete §5.3's undo justification (B1).** It is false: `CompoundAction::undo`
   runs in reverse (`undo/compound.rs:25`), `reinsert_entity` already falls back
   to root for an absent parent (`document/mod.rs:1128-1131`), and no compound
   undo passes through a state a refusing `link` would corrupt. The refusal still
   belongs at the engine handlers, on a different and honest justification:
   `Document::link` is infallible and sits beneath eight `add_*` entry points that
   all return `LayerId` unconditionally, and (the substantive reason) refusal is
   about the *user's intent to move*, which only the handler knows. `link` sees
   insertions it must never reject, such as undo putting a node back.

2. **`enforce_boundary_on_insert` gains a cross-parent redirect (B2 accepted).**
   B2 is correct that the function cannot express one today:
   `document/mod.rs:1224-1256` returns `Option<usize>` and `link` captures
   `parent` beforehand. Change the return to `(LayerId, Option<usize>)` and have
   `link` take both.

   The rule it enforces, stated once: **an insertion lands at the nearest
   position to the one requested that does not violate the screen-space rules.**
   For a root-level insert that is already what the function does, an ineligible
   node requested at an index inside the run is redirected to `Some(floor)`, the
   topmost canvas-space slot (`:1249-1253`). The extension is that a *parent*
   inside the screen-space subtree is redirected too: every position inside a run
   group is above the divider, so for a child that does not qualify the nearest
   legal slot is `(root, floor)`. Walk the requested parent up until it leaves
   the screen-space subtree; a child that *does* qualify is inserted exactly
   where it was asked to go, unredirected.

   This is why inserts do not refuse. A move states an intent about *placement*,
   so refusing it is honest and is what the report asked for. An add or a paste
   states an intent to *have a new layer*; where it lands is incidental, and
   failing a common action because of where the selection happened to be is
   hostile. One rule, two honest outcomes.

   Nested inserts are, separately, recoverable: verified: an insert whose parent
   is a run group early-returns at `:1231` today, never touches
   `screen_space_count`, and `unlink` decrements only for direct run members
   (`:1266-1270`). So this redirect prevents a transient collapse rather than
   permanent damage; it is correctness of behaviour, not of data.

3. **The loud/silent line is refounded on recoverability (B3).** The original
   claim that "the stored count is left alone, so undoing the disqualifying
   change restores the run" is false for the move path specifically: the one the
   user reported. The rule becomes:

   | Path | Behaviour | Why |
   |---|---|---|
   | `move_layer` / `move_layers` into viewport space, or into a viewport group | **Refuse, `Err(String)` → toast** | A move states placement intent. Also the only path that destroys `screen_space_count` beyond undo's reach |
   | add / paste / duplicate into a run group | Redirect to the nearest legal slot (R.2.2) | An add states no placement intent; failing it would be hostile. Stored intent untouched, and undo restores it either way (verified above) |
   | `set_group_passthrough(false)`, `add_mask` on a run member | Silent clamp | Same: recoverable, and the clamp is scheduled for deletion by the divider-as-a-node redesign |
   | `set_screen_space_boundary` (divider drag) | Clamp, stays infallible | Dragging past the last eligible node means "as far as it goes", not an error |
   | undo / redo | Never refuses | Enforcement is in the engine handlers; `UndoAction`s call `Document` directly |

   This also closes open question §8.2.1: leave `add_mask` and
   `set_group_passthrough` clamping.

4. **Name the registration flag honestly (D3).** `renders_after_view_transform`
   on `LayerKindRegistration` reads as a lie on a group, which *can* render after
   the view transform while the flag says `false`. Name it
   `leaf_renders_after_view_transform` and document at the field that groups
   answer by recursion, not by the flag.

5. **Record the two frontend notes (D4).** `SpaceDivider.svelte:36` has no error
   surface at all: un-awaited, un-caught, `reportEngineError` → `console.error`
   only. That is harmless *because* the boundary setter stays infallible, which
   makes item 3's divider row load-bearing; say so. And dropping a layer onto the
   divider row is a silent no-op (`LayerPanel.svelte:22-24, :47`): adjacent to
   the complaint, out of scope here, fixed by the divider-as-a-node redesign.
   Record, do not act.

Also: add the delete-last-effect-in-a-run-group path to §5.5 (C1), and fix the
drifted citations; `any_animated_effect` is at `compositor.rs:3229`, Krita's
`correctNewNodeLocation` at `kis_mimedata.cpp:427-446` (B8). §8.2.4 is closed
**no**: `docs/gpu-passes.md` is a generic WebGPU primer with no reference to the
screen run (B9).

### R.3 Findings dispositions at a glance

| Finding | Disposition |
|---|---|
| B1 undo justification false | Accepted, justification replaced (R.2.1) |
| B2 `enforce_boundary_on_insert` signature | Accepted; the redirect it enables is how inserts avoid refusing (R.2.2) |
| B3 move path destroys the count irreversibly | Accepted, loud/silent line refounded on it (R.2.3) |
| B4 untested behaviour changes | Partly moot: the `is_in_isolation_path` change dies with R1. `group_layers` test added (R.4e) |
| B5 walk-space resolved per call | Moot under R4: there is no walk |
| B6 scratch pool | Moot under R4 |
| B7 blit bind-group re-owner | Moot under R4 |
| B8 / B9 citation drift, `gpu-passes.md` | Accepted |
| C1 delete-last-effect path | Accepted, added to §5.5 |
| D1 `AccumId` enum | Moot under R4: `group_state` is not touched |
| D2 R4 omitted | **Accepted and adopted** (R.1) |
| D3 registration flag naming | Accepted (R.2.4) |
| D4 frontend gaps | Accepted as notes (R.2.5) |
| D5 prior-art departure justified | No change needed |

### R.4 Tests (supersedes §6's list)

Mandatory constraints: `--features darkly/testing`, `--test-threads=1`, and
readback only through the test-only helpers.

- **(a) Regression, must fail first.** An effect inside a passthrough group above
  the divider changes the presented image. The reviewer transcribed and ran this
  against the unfixed tree: centre pixel reads `[255,0,0,255]` (red, the effect
  never ran) instead of the expected inverted colour. This is the required
  failing-first test.
- **(b)** Moving a group that recursively contains a raster into viewport space
  returns `Err`, and the document is unchanged.
- **(c)** Moving a raster into a viewport-space group returns `Err`, and the
  document is unchanged.
- **(d)** A legal arrangement survives undo/redo without refusal, and the run is
  the same list afterwards.
- **(e)** `group_layers` over two run members leaves the new group in the run:
  pins R.1.3 including the empty-group ordering wrinkle.
- **(f)** A nested effect is tagged `EffectSpace::Screen`, not
  `Canvas { parent: root }`: pins R.1.2's second call site directly rather than
  only through pixels.
- **(g)** Adding a raster while a nested effect is the active layer lands it at
  the topmost canvas-space slot, not inside the run group, and leaves the run
  intact: pins the R.2.2 redirect. A companion case adds an *effect* with the
  same anchor and asserts it lands inside the group, unredirected, which is what
  stops the redirect from becoming "everything falls out of the group".

The existing `crates/darkly/tests/effect_space.rs` (16/16 green) is the home for
all of these.

### R.5 Revised LOC estimate

Added / removed, not touched.

| | Production | Tests | Generated + docs |
|---|---|---|---|
| Stage A (rendering) | **+50 / −15** | +120 / −0 | +10 / −0 |
| Stage B (refusal + redirect) | **+180 / −40** | +160 / −0 | +30 / −10 |
| **Combined** | **+230 / −55** | **+280 / −0** | **+40 / −10** |

Stage B grew by ~+20/−5 production and +30 tests over the first revision when the
insert redirect (R.2.2) replaced silent clamping: the
`enforce_boundary_on_insert` signature change, the ancestor walk, and `link`
threading both halves through.

Down from the original plan's +455/−329 production (reviewer's own recount:
+490/−350), almost entirely because Stage A no longer performs compositor
surgery. Stage A is independently shippable and is the half that fixes the
reported bug.

### R.6 Unchanged from the draft

The relationship to the pending divider-as-a-node redesign (§3) is unaffected:
neither stage touches `TreeSlot.screen_space`, `ScreenSpaceBoundaryAction`, or
the count representation, and R4 leaves *less* behind for that redesign to unwind
than R1 would have. The two known bugs in `handoff-viewport-boundary.md` §3.1
(canvas-space effect animation gate) and §3.2 (effect pass re-runs every frame)
remain out of scope and are neither absorbed nor invalidated.

---

## Independent Review

Step 2 of the planning workflow. Reviewed against the working tree at `06e808bb`
plus uncommitted changes. Every file:line citation in the plan was opened and
checked; the diagnosis was re-derived from the code and **empirically confirmed**
by a throwaway integration test (written, run, deleted; no production code was
modified).

### A. What was verified as correct

Verified by reading the cited code, unless marked *(empirical)*.

- **§2.1**: `LayerNode::supports_screen_space` (`crates/darkly/src/layer.rs:739-760`)
  admits a passthrough group of effects exactly as quoted.
- **§2.2**: `present_and_screen_run` (`gpu/compositor.rs:3733`), the
  `effective_visible` filter (`:3744-3749`), the `effect_instances` filter
  (`:3761-3764`), the plain-present branch (`:3766`), the flat effect loop
  (`:3809-3828`); `compose_children` (`:4348`), the hoisted `screen_run`
  (`:4360`), the run skip (`:4381-4383`). All exact.
- **§2.3**: `sync_effect_instances` (`:4714`), the direct-membership tag
  (`:4727-4738`), the space→views match (`:4836-4849`), `mark_effect_dirty`
  (`:3631-3637`), `effect_animates` (`:3220-3224`), `merge.rs:52` and
  `merge.rs:206`. `accumulator_host_of` (`document/mod.rs:659`) does stop at the
  root for a screen node, so the mis-binding is real.
- **§2.4**: `screen_space_run` (`document/mod.rs:531`),
  `qualifying_screen_space_suffix` (`:511`), `move_layer` (`:1067`),
  `set_screen_space_members` (`:1101`), `enforce_boundary_on_insert` (`:1224`)
  and its `parent != self.root` early return (`:1231`), `resolve_anchor_target`
  (`:1286`).
- **§2.5**: `group_layers` hardcodes `screen_space: false` at
  `engine/layers.rs:694` **(confirmed empirically: grouping a single run member
  drops the run to `[]`)**. The duplicated `ScalingPipelines`
  (`compositor.rs:1309-1313` vs `gpu/screen_run.rs:129-134`, both built from the
  compositor's `accum_format` via `compositor.rs:1262`) and the duplicated
  scratch (`compositor.rs:4645` vs `screen_run.rs:183`) are as described.
- **§4.3 R1 precedent**: the bake sentinel at `compositor.rs:3870-3881` driving
  `compose_children` at `:3918` is real; `create_group_state` (`:888`) is
  size-generic and `make_accum_texture` (`:876`) is `Rgba8Unorm` with a strict
  superset of `ScreenRun`'s usages (`screen_run.rs:155-156`).
- **§4.4**: the masked-passthrough branch (`compositor.rs:5525-5533`) is indeed
  structurally unreachable in screen space. `is_in_isolation_path` is at
  `:2180` (plan says 2179: immaterial).
- **§5.1 prior art**: 22 of 23 Krita/GIMP citations verified verbatim, including
  the load-bearing ones: `kis_node_manager.cpp:529` really is
  `if (parent->allowAsChild(node)) {` with no `else`; `kis_mimedata.cpp:520-522`
  really returns `false` with no message anywhere in the file (only `i18n` hit in
  that file is a layer *name* at `:357`); `kis_node_manager.cpp:1698-1706` really
  is the floating-message site with `showFloatingMessage` at `:1704`;
  `gimpitemtreeview.c:1642-1646` really discards `gimp_image_reorder_item`'s
  return value. `HIDE_SAFE_ASSERTS` really is `ON` at `krita/CMakeLists.txt:307`,
  and `kis_assert.cpp:66-67` confirms it degrades to `qWarning`. An independent
  sweep of all 35 `allowAsChild` references confirms the plan's strongest claim:
  **that floating message is the only user-visible placement refusal in Krita.**
- **§6.1 regression test, CONFIRMED TO FAIL TODAY (empirical).** Transcribed
  verbatim and run: `SCREEN CENTER = [255, 0, 0, 255]`. Red, not cyan. The canvas
  readback is also `[255, 0, 0, 255]`. The effect renders in neither space,
  exactly as §2.2 predicts. This is a valid regression test.
- **Existing test claims**: `a_group_is_eligible_exactly_when_its_contents_are`
  (`tests/effect_space.rs:307`) passes today, and its tail at `:328-335` does
  assert the silent-clamp behaviour. Full `effect_space` suite: 16/16 green.
- **Frontend §5.6**: `LayerItem.svelte:375` + catch/toast at `:381-383`,
  `LayerGroup.svelte:319` + catch/toast at `:325-327`, `LayerPanel.svelte:22`
  `preventDefault`-only, `spaceDivider.ts:44` `maxEligible`. All exact. Both
  import `toast` at `:7`; `toast.show('error', …)` is valid
  (`state/toast.svelte.ts:37`). `moveLayer` (singular) is **never called** from
  hand-written frontend code (only generated surface at `protocol_gen.ts:1379`
  and `:1566`) so its signature change is pure codegen churn.
- **§3 handoff interaction**: §3.1 really is already fixed in the tree
  (`any_animated_effect` exists, `canvas_fires` consumes it at
  `compositor.rs:3416-3417`). §3.2 is genuinely untouched. The plan's claim that
  Stage A survives the divider-as-a-node redesign holds: every consumer it
  touches depends only on `screen_space_run()`'s contract, not its
  representation.

### B. Findings: things the plan gets wrong

**B1. The stated justification for not refusing in `Document::link` is false.**
§5.3 asserts: *"a `CompoundAction` undo reassembles a tree one `reinsert_entity`
at a time, and the intermediate states are legitimately inconsistent (undoing
`group_layers` reinserts children before the group is back)."* This does not
happen. `CompoundAction::undo` iterates **in reverse** (`undo/compound.rs:25`).
`group_layers` pushes actions in the order `[EntityAddAction(group),
LayerMoveAction(child)…, LayerMoveAction(group reposition)]`
(`engine/layers.rs:657-704`), so undo runs the group's reposition first, then
pulls each child *out* of the still-present group, and detaches the group last.
There is no moment where a child is reinserted into an absent group. And if there
were, `reinsert_entity` already degrades safely: `document/mod.rs:1128-1131`
falls back to `self.root` when the recorded parent is not in the tree. I could
not construct any compound undo that a refusing `link` would corrupt.

This matters because the whole three-layer split (structural redirect / read
clamp / intent refusal) rests on that sentence. The split may still be the right
design, but it has to be re-justified on the real grounds, which are about API
shape, not undo: `link` returns `()`, and it is reached from `attach_at_target`
(`document/mod.rs:1304`) beneath `add_raster_layer` / `add_void_layer` /
`add_filter_layer` / `add_group` (`:818`, `:861`, `:891`, `:980`), every one of
which returns `LayerId` unconditionally. Making `link` fallible would make ~8
creation entry points fallible to deliver an error nobody wants surfaced, because
*redirect is the desired behaviour on those paths*. Say that instead. Delete the
undo claim.

**B2. `enforce_boundary_on_insert` cannot do what §5.3 asks it to do.** The
plan's central Stage-B mechanism is: generalize it from "parent is the root" to
"the resolved parent renders in screen space", after which an ineligible node
targeted at a screen-space parent is *"redirected to the first canvas-space slot
at the root"*. But the function returns `Option<usize>` (a **position only**)
and `link` (`document/mod.rs:1198-1211`) has already captured `parent` before
calling it:

```rust
let position = self.enforce_boundary_on_insert(child, parent, slot, position);
let Some(node) = self.find_node_mut(parent) else { return; };
```

There is no way to redirect *across parents* with this signature. Moving a node
from "inside a run group" to "the root's canvas floor" is exactly a cross-parent
redirect. The signature has to become `-> (LayerId, Option<usize>)` and `link`
has to honour the returned parent. That is a small change, but it is a change to
the shape of the one function the plan calls "the only place the rule is written",
and it is not in the plan or its LOC line (`document/mod.rs … 55 / 18`). Add it.

**B3. The move path's data loss is not recoverable by undo, and the plan says
the opposite.** §5.3's table says of the read clamp: *"The stored count is left
alone, so undoing the disqualifying change restores the run."* That is true for
`add_mask` and `set_group_passthrough(false)`, and is pinned by
`masked_or_isolated_nodes_cannot_be_above_the_boundary`
(`tests/effect_space.rs:294-302`). It is **false for the move path**, which is
the path the user actually reported.

`Document::move_layer` calls `set_screen_space_members`
(`document/mod.rs:1091`), which *destructively writes* `screen_space_count`
(`:1101-1109`). `LayerMoveAction` (`undo/layer.rs:98-124`) records only two
`TreeSlot`s and never captures `screen_space_count`; `move_layer_inner`
(`engine/layers.rs:1273-1285`) constructs it from `slot_of` alone. **Empirically
confirmed:** move a raster into a viewport group, then undo; the run stays `[]`.
The user's viewport arrangement is gone permanently.

Two consequences:

1. §2.4 must be corrected. It currently frames the move failure as "the intent is
   discarded" / a silent no-op. It is worse than that: it silently and
   *irreversibly* destroys unrelated state.
2. This is a **better** justification for the plan's own loud/silent line than
   the one §5.1 gives. The plan currently defends the toast with a general
   appeal to "silence destroys your arrangement". The sharp version is:
   *refuse where the damage is unrecoverable (moves, because
   `set_screen_space_members` is destructive and unrecorded); clamp where it is
   recoverable (`add_mask`, `set_group_passthrough`, because the stored count
   survives and undoing the disqualifier restores the run).* That is a principled
   line rather than a judgement call, and it also answers open question §8.2.1
   directly: **leave them clamping, because they are recoverable.** Use it.

**B4. Two behaviour changes ship with no test.** CLAUDE.md: *"Every feature must
have a test."*

- §4.4's `group_layers` space preservation: the fix that makes the whole feature
  reachable by the obvious gesture (select veils → Group). Broken today
  (confirmed empirically). §6 lists no test for it. It needs one:
  *grouping run members leaves the new group in the run*.
- §4.4's `is_in_isolation_path` change (screen nodes always in the isolation
  path). This is a semantic change to isolation introduced *by* R1 to avoid a
  regression R1 creates. Untested, it is exactly the kind of thing that silently
  rots. Needs a test: *isolating a canvas layer does not hide a viewport veil.*

Also, §8.1.1's `set_canvas_rect` resize test is written as "should be added if
the reviewer agrees". I agree, and it should be non-optional: it is the plan's
own top-rated risk, and its failure mode is "mostly works".

**B5. A per-frame cost regression in the hot compose walk.** §4.3 step 5 replaces
`compose_children`'s run-membership skip with `doc.render_space(child)`
*per child*. Today the run is resolved **once per group call**: `compositor.rs:4360`,
with a comment saying exactly that. `render_space` as specified walks the parent
chain and then consults `screen_space_run()`, which calls
`qualifying_screen_space_suffix()` (`document/mod.rs:511`), which calls
`supports_screen_space` (itself recursive over group subtrees and calling
`has_mask` per node) over the whole trailing suffix. Per child, per group, per
frame. Given `handoff-viewport-boundary.md` §3.2 already measures the compose
walk as a live performance problem (+43.5 ms/frame for `painting`), the plan must
state that the space is resolved once per `compose_children` call, not per child.

**B6. `compose_effect_arm` hard-codes the canvas scaffolding, so §4.3 step 6 is
mandatory, not cleanup.** The plan lists unifying `canvas_apply_scratch` /
`canvas_scaling_pipelines` as step 6 under the heading "unify the duplicated
scaffolding (§2.5)", which reads as opportunistic DRY. It is not optional:
`compose_effect_arm` reaches for `self.canvas_apply_scratch` at
`compositor.rs:5274` and `self.canvas_scaling_pipelines` at `:5310`
unconditionally. Route a screen effect through it with a 16×16 canvas and a 64×64
viewport and the effect writes a 16×16 scratch that the apply pass reads against a
64×64 accumulator. The size-keyed pool is a **prerequisite** for R1, and its
absence would produce a subtly-wrong image rather than a crash. Re-file it as
step 2.

**B7. `ScreenRun`'s blit bind groups are built from the textures R1 takes away,
and the plan doesn't say who rebuilds them.** §4.3 step 7 says `ScreenRun` keeps
"the blit-to-surface pipeline and its two bind groups". Those bind groups are
constructed from `v0`/`v1` inside `ensure_resources`
(`gpu/screen_run.rs:166-186`) and dropped by `drop_textures` (`:189-194`). Once
the views live in `group_state[SCREEN_ACCUM]`, something has to rebuild them
every time that entry is recreated, which is at least `resize_screen_run`
(`compositor.rs:3622`, also driven by the test harness at `:4127`) and any
`target_generation` bump. Name the owner and the trigger. Add it to the LOC line.

**B8. Minor citation drift.** `any_animated_effect` is at `compositor.rs:3229`,
not `:3216` (§3). Krita's `correctNewNodeLocation` is `kis_mimedata.cpp:427-446`
with the walk-up at `:436-443`, not `:437-445` (§5.1). Neither affects an
argument. Two small prior-art additions worth absorbing: the `quickUngroup` site
emits **two** message variants, not one (`kis_node_manager.cpp:1699` and `:1702`,
the latter `"Cannot move layer \"%1\" into the root layer"`); and
`KisGroupLayer::allowAsChild` calls `checkNodeRecursively` redundantly twice
(`kis_group_layer.cc:116` and `:138`).

**B9. §8.2.4 is answerable now: no.** `docs/gpu-passes.md` is a 158-line general
WebGPU tutorial (what a pass is, the read+write rule, a hello-world shader). It
contains **zero** references to the screen run, the present pass, veils, or the
divider. It needs no pass. Delete the open question.

### C. Completeness of the path enumeration

I independently grepped every caller of `resolve_anchor_target`,
`Document::move_layer`, `move_layer_inner`, `reinsert_entity`, and `link`. The
plan's §5.5 table is nearly complete. Verified: `clipboard.rs:508`,
`floating.rs:275`, `duplicate.rs:84`/`:355`/`:360`, `load.rs:372`,
`flatten.rs:23`, `merge.rs:52`/`:206`, `engine/layers.rs:611`/`:1258`/`:1293`/
`:1669`/`:1732`. Missing:

**C1. Deleting the last effect out of a viewport group.** Not in the table at
all. `LayerNode::supports_screen_space` returns `false` for an empty group
(`layer.rs:751-758`), so removing a run group's only child silently collapses the
whole run above it via the read clamp. It is recoverable (the count survives:
`unlink` only decrements for direct run members, `document/mod.rs:1268-1270`), so
by B3's rule it belongs in the "clamp, recoverable" bucket. But it is a distinct
door and the table should say so, because it is the one case where an operation
on a node *below* the boundary changes what is above it.

**C2. Merge / flatten result placement.** `merge.rs:144-150` and
`flatten.rs:96-101`/`:253` reinsert their baked result with `screen_space:
false`. Benign today (both refuse/exclude run members first), but they are
`reinsert_entity` call sites carrying a hardcoded side, in the same family as the
`group_layers:694` bug the plan does fix. Worth one line acknowledging they were
checked.

**C3. Add-with-anchor is confirmed broken (empirical).** §2.4's add-path hole
reproduces: `add_raster_layer(Some(effect_inside_run_group))` lands the raster
**inside** the viewport group and empties the run. The plan's proposed test
`adding_a_raster_with_a_viewport_group_anchored_does_not_break_the_run` fails
today, as claimed. Note it goes through `attach_at_target` directly
(`document/mod.rs:905`, `:987`), not `Document::move_layer`, so unlike paste and
duplicate it never touches `set_screen_space_members`, which is why the plan's
"silent redirect is lossless here" reasoning is sound for `add_*`. State that;
it is the reason the silent/loud line is defensible on this path.

### D. Design opinions (not defects)

**D1. On `SCREEN_ACCUM` (self-flagged decision 1): the reuse is fine, the
sentinel is not.** Putting a screen accumulator in `Compositor::group_state` is
*not* an ownership or Document-Authority violation: `group_state` is
compositor-owned derived state, the document is never asked about it, and the
bake sentinel already establishes the pattern. The problem is narrower and the
plan half-sees it: a `HashMap<LayerId, GroupState>` that holds two non-document
keys is lying about its key type, and the consequence is §8.1.1, a hand-written
`filter` in `set_canvas_rect` (`compositor.rs:2083-2088`) that a future edit can
forget, guarding a failure mode that "mostly works".

There is a cleaner way that makes the trap unrepresentable rather than guarded:
key the map by an enum.

```rust
enum AccumId { Group(LayerId), Bake, Screen }
impl From<LayerId> for AccumId { … }
```

`set_canvas_rect` then matches instead of filtering: `AccumId::Screen` is
excluded by the type, not by vigilance. `compose_children`'s walk-space query
becomes `matches!(parent, AccumId::Screen)`, which is honest, instead of
comparing against a magic id. The `From` impl keeps most of the ~25
`group_state[&id]` sites textually unchanged. This is the plan's own §7 "one
wart" and §8.1 risk 1, both dissolved for maybe 25 lines. Recommended, not
required. (Related: `create_group_state` writes
`BlendUniforms { layer_offset: canvas_origin, layer_size: canvas_size }` at
`compositor.rs:909-916`, which is meaningless for a screen accumulator. Harmless
(`SCREEN_ACCUM` never blends into a parent) but note it.)

**D2. The R1/R2/R3 option set is incomplete, and the omitted option is the
cheapest correct one.** R2 as written ("a purpose-built recursive screen walk")
is a strawman: it is the worst of both, and the plan is right to reject it. But
there is a third shape neither considered nor rejected:

> **R4: flatten the run.** Make the screen path consume a *flat list of effect
> ids* derived from the document: `Document::screen_space_effects()`, a DFS of
> `screen_space_run()` collecting `Filter` ids and skipping invisible subtrees.
> `present_and_screen_run`'s existing flat loop (`compositor.rs:3809-3828`) then
> works unchanged; `sync_effect_instances` tags those ids `Screen` because they
> *are* in the list; `compose_children`'s skip still keys on the run's root-level
> members. `EffectSpace` and `ScreenRun` survive intact.

This is legitimate because a screen-space group carries **no** compositing
semantics that flattening would lose. It must be passthrough
(`layer.rs:751-752`), so it has no accumulator; it cannot carry a mask
(`:740-742`); visibility is already handled transitively by `effective_visible`;
and Darkly does **not** apply a passthrough group's opacity or blend mode today
in canvas space either: `compose_group_arm`'s passthrough branch
(`compositor.rs:5534-5538`) inlines children and ignores `group.blend` entirely.
So the flattened run and the recursive walk produce the same pixels. (Krita
reaches the same conclusion from the other direction: `KisProjectionLeaf`
*splices* a pass-through group's children into the parent's sibling chain
(`kis_projection_leaf.cpp:196-221`) precisely so the walker never sees the
group. Darkly's run is a flat sibling chain already.)

R4 also *avoids* the isolation regression R1 creates (§4.4's first bullet exists
only because R1 routes screen effects through a filter that was written for
document content), and it does not need the scratch pool (B6), the blit
bind-group re-owner (B7), the `set_canvas_rect` guard (§8.1.1), or the
borrow-splitting gamble (§8.1.2). Rough size: **+35 / −10 production**, versus
R1's +252 / −276.

I am **not** saying R1 is wrong. R1's case is genuinely strong: it is net
*negative* production LOC, it collapses `EffectSpace`, and it makes both spaces
one mechanism, which is the DRY outcome CLAUDE.md wants. And R4 leaves §2.3's
four mis-tagged consumers to be fixed separately by `render_space` (which the
plan wants anyway and which is independent of R1/R4). What I am saying is that
the plan presents a two-way choice between "the right thing" and a strawman, and
the user is being asked to approve +252/−276 of compositor surgery on that
framing. **R4 must be written up and explicitly rejected with reasons, or
adopted.** The four risks in §8.1 that R4 eliminates are not a small thumb on the
scale.

**D3. The `LayerKindRegistration` bool (self-flagged decision 2) is fine, but
name it honestly.** §5.2 adds `renders_after_view_transform: bool` to
`document/layer_kind.rs:54` and keeps the group recursion as a `match self` in
`LayerNode`. That means `group.rs` sets the flag `false` while a group *can*
render after the view transform: the flag silently means "…if this kind is a
leaf". A consumer reading `registration.renders_after_view_transform` on a group
gets a wrong answer. The precedent the plan cites (`composites_in_place`,
`layer.rs:713`) does not have this problem because it is not on the registry. Fix
by naming (`leaf_renders_after_view_transform`, or document the caveat at the
field) rather than by moving to a function pointer: the plan's instinct to avoid
three identical bodies is right.

**D4. The loud/silent line is defensible (see B3) but the frontend has one
uncovered gap.** §5.6's "no new code is expected" is correct *for the paths the
plan changes*: `LayerItem` and `LayerGroup` both funnel through `moveLayers` and
both already toast, and `LayerPanel.svelte:22` issues no move. Independent sweep
found no context-menu, keyboard, or command-palette reorder anywhere in the
frontend. Two things the plan should record:

- **`SpaceDivider.svelte:36` has no error surface at all.** It calls
  `app.setScreenSpaceBoundary(landed)` un-awaited and un-caught;
  `app.svelte.ts:968-973` routes through `protocol_gen.ts:1623`'s `postFF`, whose
  failure path is `reportEngineError` → `console.error` only
  (`engine/protocol.ts:16-19`, `:130-131`). `SpaceDivider.svelte` does not import
  `toast`. This is harmless *given the plan keeps `set_screen_space_boundary`
  infallible*, which it does, correctly (§5.5's divider row). Worth one
  sentence, because "the divider clamps" is now load-bearing for the frontend
  having no bug.
- **Dropping a layer onto the divider row itself is a silent no-op.** The
  `SpaceDivider` row is a plain sibling in `.layer-list`
  (`LayerPanel.svelte:47`) with no `ondrop`; the event bubbles to
  `LayerPanel.svelte:22-24`, which only calls `preventDefault()`. Since dropping
  *on the divider* is the most literal way a user expresses "put this in viewport
  space", this is adjacent to the reported complaint. Out of scope for this plan,
  and it is squarely what `handoff-viewport-boundary.md` §2 fixes by making the
  divider a row. Note it and leave it.

**D5. The departure from prior art is adequately justified.** The plan is honest
that neither editor toasts on a refused drop and states why Darkly should: under
the count representation the failure mode is not "nothing happened" but "your
unrelated arrangement was destroyed". B3 turns that from an assertion into a
demonstrated fact (unrecoverable by undo). The plan also correctly took prior
art's *other* two mechanisms (type-owned recursive predicate, drop-affordance
suppression) rather than only the toast. On the closer analogue the review brief
asked about: Krita has **no** placement rule specific to adjustment/filter layers,
neither `KisLayer` nor `KisAdjustmentLayer` overrides `allowAsChild`, and
`KisAdjustmentLayer` inherits `KisSelectionBasedLayer`'s masks-only rule for its
*children* only. Krita expresses adjustment-layer semantics through composition
order, not placement constraints. So there is no missed analogue; §5.1's mapping
onto `supports_screen_space` is the right one.

### E. Stage split and scope

The A/B split is right, and neither stage smuggles in the divider-as-a-node
redesign: nothing in §4 or §5 touches `TreeSlot.screen_space`,
`Manifest::screen_space_count`, `ScreenSpaceBoundaryAction`, or
`SpaceDivider.svelte`'s drag. §3's claim that Stage A is representation-independent
holds. Stage B's overlap with the pending redesign is real and the plan states it
accurately; given B3, Stage B is not merely cosmetic (it is what stops
unrecoverable state loss) so deferring it should be a deliberate choice, not the
default.

One scope observation: §4.2's `render_space` helper and the four consumer fixes
in §2.3 are independent of *both* R1 and R4 and fix three live bugs
(`mark_effect_dirty`, `effect_animates`, `merge`). If the user wants the smallest
correct increment, that plus R4 plus `group_layers` is it.

### F. LOC estimate

Sanity-checked against the enumerated changes. **The estimate holds**, with two
adjustments:

- Stage A's compositor line (+170 / −175) is ~15-20% light once B6 (size-keyed
  scratch pool, ~+30/−20) and B7 (blit bind-group re-owner, ~+25) are counted.
  Realistic: **+195 / −175**.
- Stage B's `document/mod.rs` line (+55 / −18) needs B2's signature change on
  `enforce_boundary_on_insert` and `link`: **+70 / −22**.

Net: Stage A production ≈ **+280 / −290** (plan: +252/−276); combined production
≈ **+490 / −350** (plan: +455/−329). Both land inside the plan's quoted approval
range of **+400 to +520 / −280 to −380**. I have no correction to make to the
headline number the user approves on. Tests at +340 are plausible and should grow
by ~40 for B4's two missing cases.

If R4 is adopted instead of R1, Stage A production collapses to roughly
**+70 / −20** (R4's +35/−10, plus `render_space` and the four consumer fixes,
plus `group_layers`), with the same regression test passing.

### G. Required revisions

1. Delete §5.3's undo justification; re-justify the three-layer split on `link`'s
   infallible-API shape (B1).
2. Change `enforce_boundary_on_insert` to return `(parent, position)` and update
   `link`; add it to the LOC table (B2).
3. Correct §2.4 and §5.3's clamp row: the move path destroys `screen_space_count`
   irreversibly. Re-found the loud/silent line on recoverability, and resolve
   §8.2.1 with it (B3).
4. Add tests for `group_layers` space preservation and for the
   `is_in_isolation_path` change; make §8.1.1's `set_canvas_rect` test
   non-optional (B4).
5. State that the walk's space is resolved once per `compose_children` call (B5).
6. Re-file §4.3 step 6 as a prerequisite, not cleanup (B6).
7. Name the owner and trigger for `ScreenRun`'s blit bind-group rebuild (B7).
8. Write up **R4** and either adopt it or reject it with reasons (D2). This is
   the finding that most affects what the user is approving.
9. Add the delete-last-effect path to §5.5; note merge/flatten's hardcoded
   `screen_space: false` were checked (C1, C2).
10. Fix the drifted citations; absorb the two prior-art corrections; close
    §8.2.4 with "no" (B8, B9).

Verdict:

revise

---

Status: **draft (step 1 of the planning workflow).** No production code has
changed. Written against branch `better-veils`, working tree at `06e808bb` plus
the uncommitted changes listed in `git status`. Every line number below was read
from that tree.

---

## 0. Vocabulary

| Term | What it is in the code |
|---|---|
| **veil / effect layer** | `Layer::Filter(FilterLayer)`: `crates/darkly/src/layer.rs:236`. Kind file: `crates/darkly/src/document/layer_kinds/filter.rs`. |
| **viewport space** | "screen space" in the code: the run of the root group's trailing children realized *after* the view transform, on the presented image. |
| **the divider / the boundary** | `Document::screen_space_count`: `crates/darkly/src/document/mod.rs:207`. A count of the root's trailing children. |
| **the run** | `Document::screen_space_run()`: `crates/darkly/src/document/mod.rs:531`. The clamped slice of root children above the divider. |
| **canvas space** | Everything below the divider. What export / flatten / merge see. |

---

## 1. The report

> Veils in viewport space don't work when inside a group. This is a code smell.
> Groups should be allowed above the viewport boundary as long as they contain
> recursively only effect layers. Moving a group recursively containing any
> non-effect layers into viewport space, or moving a non-effect layer into a
> group in viewport space, should fail with a toast notification.

Two separate defects are bundled here:

1. **A rendering defect.** A passthrough group of effects placed above the
   divider is already legal in the document model, and renders nothing at all.
2. **A silence defect.** Every way of making a viewport arrangement illegal
   degrades silently: it un-makes the user's arrangement rather than refusing
   the operation.

---

## 2. Root cause

### 2.1 The document model is not the problem

`LayerNode::supports_screen_space` (`crates/darkly/src/layer.rs:739`) already
admits a group:

```rust
LayerNode::Group(g) => {
    g.passthrough
        && !g.children.is_empty()
        && g.children.iter().all(|c| {
            doc.find_node(*c).is_some_and(|n| n.supports_screen_space(doc))
        })
}
```

and an existing test pins it:
`a_group_is_eligible_exactly_when_its_contents_are`
(`crates/darkly/tests/effect_space.rs:307`) asserts
`run_ids(&engine) == vec![group]` after grouping an effect and setting the
boundary to 1. That test passes today. It asserts membership and **never renders
anything**, which is precisely why the bug shipped.

So the group *is* in the run. The failure is entirely downstream, in the
compositor.

### 2.2 The rendering defect: the group is dropped by both walks

**Screen walk.** `Compositor::present_and_screen_run`
(`crates/darkly/src/gpu/compositor.rs:3733`) builds its member list as a **flat
list of root children**, then filters it down to ids that have a realized effect
instance:

```rust
// compositor.rs:3744-3749
let run: Vec<LayerId> = doc
    .screen_space_run()
    .iter()
    .copied()
    .filter(|id| doc.effective_visible(*id))
    .collect();
...
// compositor.rs:3761-3764
let members: Vec<LayerId> = run
    .into_iter()
    .filter(|id| self.effect_instances.contains_key(id))
    .collect();
```

A group id never has an entry in `effect_instances` (instances are minted only
for `doc.all_filter_layers()`: `compositor.rs:4728`). So `members` is empty,
and `compositor.rs:3766` takes the "no effects" branch: a plain present straight
to the surface. **There is no recursion here at all**: the screen path is a
flat `for id in members` loop over effect ids (`compositor.rs:3809-3828`).

**Canvas walk.** `Compositor::compose_children`
(`crates/darkly/src/gpu/compositor.rs:4348`) skips the group too:

```rust
// compositor.rs:4360
let screen_run = doc.screen_space_run();
...
// compositor.rs:4381-4383
if screen_run.contains(&child_id) {
    continue;
}
```

Net: the group is skipped in canvas space *because* it is in the run, and
skipped in screen space *because* it is not an effect. **The effects inside it
render in neither space.** That is the reported symptom exactly.

### 2.3 The nested effect is also mis-tagged

`Compositor::sync_effect_instances` (`crates/darkly/src/gpu/compositor.rs:4714`)
decides each effect's space by **direct membership in the run**:

```rust
// compositor.rs:4727-4738
let screen_run: Vec<LayerId> = doc.screen_space_run().to_vec();
let live: Vec<(LayerId, EffectSpace, String, Vec<ParamValue>)> = doc
    .all_filter_layers()
    .iter()
    .filter_map(|f| {
        let space = if screen_run.contains(&f.id) {
            EffectSpace::Screen
        } else {
            EffectSpace::Canvas { parent: doc.accumulator_host_of(f.id)? }
        };
        ...
```

An effect *inside* a screen-space group is not itself a run member, so it takes
the `Canvas` arm. `Document::accumulator_host_of`
(`crates/darkly/src/document/mod.rs:659`) walks up past passthrough groups and
stops at the root, so the instance is tagged
`EffectSpace::Canvas { parent: root }` and its bind groups are prepared against
the **root group's canvas-sized accumulator** (`compositor.rs:4838`). Even if
the walk reached it, it would be bound to the wrong textures at the wrong
resolution.

The same "direct membership only" mistake appears in three more consumers, and
each is a live bug for a nested node:

| Site | Consequence for a node nested inside a screen-space group |
|---|---|
| `Compositor::mark_effect_dirty`: `compositor.rs:3631-3637` | A parameter edit marks the *canvas* dirty instead of requesting a re-present. |
| `Compositor::effect_animates`: `compositor.rs:3220-3222` | The instance ticks on the canvas animation divisor rather than the screen one, and `needs_composite` is set every tick for something that is not in the composite. |
| `DarklyEngine::merge_*`: `engine/merge.rs:52` and `engine/merge.rs:206` | A layer nested inside a viewport group is **not** refused by merge, though a direct run member is. |

### 2.4 The silence defect: every illegal placement degrades quietly

`Document::screen_space_run` (`document/mod.rs:531`) clamps on read against
`qualifying_screen_space_suffix` (`document/mod.rs:511`). So the moment a run
member stops qualifying, the run shrinks: silently, and often by more than one
entry, because the suffix stops at the *first* disqualified child.

Two concrete user-visible failures:

- **Move a raster into a viewport group.** `move_layers(vec![raster],
  IntoGroupTop(group))` succeeds. `enforce_boundary_on_insert`
  (`document/mod.rs:1224`) does nothing, because it only guards inserts whose
  parent is the root (`document/mod.rs:1231`). The group stops qualifying, and
  the user's entire viewport arrangement above it evaporates with no message.
  The tail of `a_group_is_eligible_exactly_when_its_contents_are`
  (`crates/darkly/tests/effect_space.rs:328-335`) currently *asserts* this
  silent behaviour.
- **Move a group containing a raster above the divider.**
  `Document::move_layer` (`document/mod.rs:1067`) sets `wants_screen` from the
  reference node, then `set_screen_space_members` (`document/mod.rs:1101`)
  applies `.min(self.qualifying_screen_space_suffix())` and the intent is
  discarded. The move "succeeds" and nothing happens.

The add / paste path has the same hole through a different door: `add_raster`
with a run-group anchor resolves to `MoveTarget::IntoGroupTop(group)`
(`Document::resolve_anchor_target`, `document/mod.rs:1286`), and
`enforce_boundary_on_insert` again does not fire because the parent is not the
root.

### 2.5 Two adjacent facts worth recording

- **`group_layers` drops the group out of the run.** `engine/layers.rs:689-696`
  reinserts the freshly-created group with a hardcoded `screen_space: false`.
  So the most natural way to *make* a viewport effect group (select your veils,
  hit Group) takes them out of viewport space. Fixed in this plan (§4.4).
- **The two spaces own duplicate GPU scaffolding.**
  `Compositor::canvas_scaling_pipelines` (`compositor.rs:1309-1313`) and
  `ScreenRun::scaling_pipelines` (`gpu/screen_run.rs:129-134`) are
  `ScalingPipelines::new(device, accum_format, label)` with the *same* device
  and the *same* `accum_format` (`compositor.rs:1262` passes the compositor's
  `accum_format` straight into `ScreenRun::new`), differing only in the debug
  label. Likewise `Compositor::canvas_apply_scratch` (`compositor.rs:4645`) and
  `ScreenRun::scratch` (`gpu/screen_run.rs:183`) are the same accumulator-format
  scratch texture at two different sizes.

---

## 3. Relationship to the pending "divider as a node" redesign

`handoff-viewport-boundary.md` §2 records a **decided but not started** redesign:
replace `screen_space_count` with a divider node among the root's children, so
that crossing the boundary is an index change. That redesign deletes
`screen_space_count`, `clamp_screen_space_count`,
`qualifying_screen_space_suffix`, `set_screen_space_members`,
`restore_to_screen_space`, `enforce_boundary_on_insert`, the read clamp,
`ScreenSpaceBoundaryAction`, `TreeSlot.screen_space`,
`Manifest::screen_space_count`, and `MoveTarget::reference()`
side-inheritance.

**This plan does not do that redesign, and must not.** Its interaction:

- **§4 (the rendering fix) is fully independent.** Every consumer it touches
  depends only on the contract "`Document::screen_space_run()` yields the root
  children realized after the view transform, bottom-to-top." That contract
  survives §2 verbatim: under §2 the run is simply "children after the
  divider's index." The rendering fix should be done **before** §2, because it
  is the reported bug, it is cheap to verify, and it removes a whole class of
  space-tagging mistakes that §2 would otherwise inherit.
- **§5 (the refusal rule) partially overlaps §2.** §2 deletes the clamp
  machinery this plan retains as a safety net. The refusal design below is
  deliberately shaped so §2 subsumes it cleanly: the *predicate* lives on
  `LayerNode` (untouched by §2), the *destination-space query* lives on
  `Document` (§2 changes only its implementation, not its signature), and the
  *enforcement* lives in the engine's move handlers (§2 does not move those).
  What §2 will delete is only the structural clamp underneath, which is exactly
  what §2 is for.
- Recommended sequence: **this plan (both stages) → §2 → PR 5 (vocabulary).**
  Doing §2 first would mean rewriting the rendering fix against a moving
  representation for no benefit.

Other open items in that handoff, for the record:

- §3.1 (canvas-space effect animation gate) **appears already fixed** in the
  working tree: `any_animated_effect(doc, screen)` exists at
  `compositor.rs:3216` and `canvas_fires` includes it at `compositor.rs:3415`.
  `docs/plans/canvas-effect-animation-gate.md` is its plan. Nothing here
  conflicts; §4.5 only changes *how* the space is determined, not the gate.
- §3.2 (effect pass re-runs every frame) is out of scope and untouched. Its
  plans are `docs/plans/effect-invalidation-wiring.md` and
  `docs/plans/composite-prefix-cache.md`.

---

## 4. Stage A: the rendering fix

### 4.1 The shape of the problem

The screen run must stop being *a flat list of effect ids* and become *a walk of
a subtree*, with exactly the rules the canvas walk already has: skip invisible
nodes, inline passthrough groups into the parent accumulator, run an effect's
pipeline over the accumulator in place.

`Compositor::compose_children` already is that walk
(`compositor.rs:4348-4394`), and it already dispatches per node kind through
`LayerNode::compose_into` (`layer.rs:699`) so it never branches on kind. Writing
a second recursion beside it would be a textbook stop-sign ("mirrors
`compose_children`"). **The screen run must reuse it.**

Prior art agrees this is the right altitude. Krita's compositing walker,
`KisBaseRectsWalker::visitSubtreeTopToBottom`
(`krita/libs/image/kis_base_rects_walker.cpp:72-110`), has **zero knowledge of
pass-through groups**: it navigates purely through `KisProjectionLeafSP`
(`lastChild()` at `:82`, `prevSibling()` at `:98`, recursion gated on
`currentLeaf->canHaveChildLayers()` at `:108`). The pass-through elision happens
one level below, inside `KisProjectionLeaf`: `firstChild()`/`lastChild()`
(`krita/libs/image/kis_projection_leaf.cpp:122-139`) report *no children* for a
pass-through group, and `nextSibling()`
(`krita/libs/image/kis_projection_leaf.cpp:196-221`) descends into a
pass-through sibling's first child instead, splicing the group's children into
the parent's sibling chain. `parent()`
(`krita/libs/image/kis_projection_leaf.cpp:105-119`) climbs past pass-through
ancestors, and `opacity()`
(`krita/libs/image/kis_projection_leaf.cpp:300-311`) merges the pass-through
parent's opacity into the child's, confirming children are composited as if
flattened into the parent, never as a nested sub-composite.

Darkly reaches the same end by inlining directly in `compose_group_arm`
(`compositor.rs:5534-5538`) rather than through a leaf-graph indirection. That
is a legitimate different mechanism with the same property, and it is not worth
adopting `KisProjectionLeaf` wholesale. What the prior art *does* establish is
the principle this plan follows: **one walker, ignorant of the space/pass-through
distinction, fed by a graph view that has already resolved it.**

### 4.2 One document query for "what space does this render in"

Both the tagger and the walker need the same answer, and four consumers today
each ask a different, wrong version of it (§2.3).

Add to `Document`:

```rust
/// Which walk realizes this node: the canvas-space tree walk, or the
/// screen-space walk that runs after the view transform.
///
/// Derived by finding the node's root-level ancestor and asking whether it is
/// in the run: a node inherits its space from whatever crossed the boundary,
/// which for a group is the whole subtree at once.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RenderSpace { Canvas, Screen }

pub fn render_space(&self, id: LayerId) -> RenderSpace
```

Then:

- `Document::renders_in_screen_space` (`document/mod.rs:555`) becomes
  `self.render_space(id) == RenderSpace::Screen`, and stops meaning "is a direct
  run member". Its three existing callers (`compositor.rs:3632`,
  `engine/merge.rs:52`, `engine/merge.rs:206`) all *want* the transitive
  meaning; two of them are silently wrong today.
- `Document::slot_of` (`document/mod.rs:560`) must keep the **direct-membership**
  meaning for `TreeSlot.screen_space`, because that field records "was this a
  root child above the divider" for undo. Use an explicit
  `screen_space_run().contains(&id)` there rather than the renamed helper, with
  a comment saying why. **This is a trap: a blanket find-and-replace of
  `renders_in_screen_space` breaks undo.**

### 4.3 One accumulator abstraction, so `compose_children` serves both spaces

`compose_children` writes into `self.group_state[&parent_group]`. The screen run
owns an equivalent ping-pong pair on `ScreenRun` (`gpu/screen_run.rs:22-24`).
These are the same thing at different sizes.

**Recommended (R1): give the screen run a `GroupState` under a sentinel id.**

There is precedent in the file: `Compositor::bake_subtree_to_layer` already
stashes a transient `GroupState` under `LayerId::from_ffi(0)` and drives the
ordinary walk through it (`compositor.rs:3870-3881`, then
`compose_children(..., bake_parent, source_ids, scissor)` at
`compositor.rs:3918`). The screen run is the same trick, made permanent.

Steps:

1. Add `pub(super) const SCREEN_ACCUM: LayerId` (a second reserved sentinel,
   distinct from the bake one) and allocate a `GroupState` for it at viewport
   size in `ScreenRun::ensure_resources`' place, keyed into
   `Compositor::group_state`. `create_group_state` (`compositor.rs:888`) is
   already size-generic and its textures are `Rgba8Unorm`
   (`make_accum_texture`, `compositor.rs:876`): the same format `ScreenRun`
   uses (`compositor.rs:1262`), with a superset of usages.
2. **`Compositor::set_canvas_rect` must skip the sentinel.** It currently
   recreates *every* `group_state` entry at canvas dimensions
   (`compositor.rs:2083-2088`), which would silently resize the screen
   accumulator to the canvas size on every canvas resize. One `filter` on that
   loop, with a comment. (The existing bake sentinel is canvas-sized, so it is
   unaffected either way.)
3. `present_and_screen_run` becomes: seed `accum.views[0]` with the present
   pass, set `current_accum = 0`, call
   `compose_children(encoder, device, doc, SCREEN_ACCUM, &run, full_viewport)`,
   then blit `accum[current_accum]` to the surface. Visibility, passthrough
   inlining, and per-effect encoding all come from the shared walk. The explicit
   `effective_visible` filter (`compositor.rs:3748`) and the flat effect loop
   (`compositor.rs:3809-3828`) are deleted.
4. `EffectSpace` (`compositor.rs:541-546`) **is deleted.**
   `EffectInstance.space` becomes `accumulator: LayerId`. The four `match space`
   sites (`compositor.rs:3583-3586`, `4752-4769`, `4836-4849`, and the
   `structural_match` compare at `4780`) collapse to plain
   `group_state` lookups. `sync_effect_instances` computes the accumulator as
   "`SCREEN_ACCUM` when `doc.render_space(id) == Screen`, else
   `doc.accumulator_host_of(id)`": total, because an isolated group can never
   be in screen space, so `accumulator_host_of` for a screen node always returns
   the root.
5. `compose_children`'s cross-space skip (`compositor.rs:4381-4383`) becomes a
   space comparison rather than a run-membership test: skip a child whose
   `doc.render_space(child)` differs from the walk's space (`Screen` iff
   `parent_group == SCREEN_ACCUM`). Behaviourally identical for the canvas walk;
   correct for the screen walk, where the run members *are* the children.
6. Unify the duplicated scaffolding (§2.5): one `scaling_pipelines` on the
   compositor replaces `canvas_scaling_pipelines` +
   `ScreenRun::scaling_pipelines`; one size-keyed apply-scratch pool replaces
   `canvas_apply_scratch` + `ScreenRun::scratch`.
7. `ScreenRun` shrinks to what it actually still owns: viewport dimensions, the
   `needs_present` flag, the blit-to-surface pipeline and its two bind groups.

**Rejected (R2): a purpose-built recursive screen walk.** ~60 lines that restate
`compose_children`'s visibility / passthrough / dispatch rules against
`ScreenRun`'s pair. Cheaper to write, and it is exactly the duplication
CLAUDE.md's stop-sign clause names. Recorded only as the fallback if R1's
borrow-splitting in `compose_effect_arm` proves intractable: in which case the
implementer should **stop and return to review**, not quietly ship R2.

**Rejected (R3): adopt `KisProjectionLeaf` wholesale.** Darkly already gets
pass-through inlining from `compose_group_arm`; a parallel graph view is
machinery without a second consumer.

### 4.4 Consequential behaviour decisions (call these out in review)

- **Isolation now reaches the screen walk.** `compose_children` filters on
  `is_in_isolation_path` (`compositor.rs:4373`, predicate at
  `compositor.rs:2179`). Under R1 a screen-space effect would be skipped
  whenever the user isolates an unrelated canvas layer: a behaviour change, and
  a bad one: isolation is about document content. **Decision:**
  `is_in_isolation_path` returns `true` for any node whose
  `render_space` is `Screen`. Putting it there (rather than branching in the
  walk) keeps the walk ignorant of space.
- **The histogram now sees screen-space effects.** `compose_effect_arm`
  dispatches the histogram over the effect's input when
  `histogram_target == filter.id` (`compositor.rs:5294-5302`). Screen effects
  never reached that code before. This is an improvement (a curves widget on a
  viewport veil gets a histogram of the presented image) and costs nothing when
  no widget is open. Accepted deliberately.
- **The masked-passthrough branch is unreachable in screen space.**
  `compose_group_arm` routes a masked passthrough group to
  `compose_passthrough_masked` (`compositor.rs:5525-5533`), which needs
  `mask_snapshot_state`. `supports_screen_space` refuses any node carrying a
  mask (`layer.rs:740-742`), so this is structurally unreachable. No guard
  needed; note it.
- **`group_layers` must preserve the sources' space.** `engine/layers.rs:694`
  hardcodes `screen_space: false`. Replace with the sources' actual side, read
  before the moves. Small, and it is what makes the feature reachable by the
  obvious gesture.

### 4.5 What is *not* changed

The animation gate's structure (`compositor.rs:3395-3438`) stays as it is; only
`effect_animates` (`compositor.rs:3220`) swaps `inst.space == EffectSpace::Screen`
for `doc.render_space(id) == RenderSpace::Screen`, which keeps the document as
the authority instead of the instance. `docs/plans/canvas-effect-animation-gate.md`
is unaffected.

---

## 5. Stage B: the refusal rule

### 5.1 What prior art actually does (and where we diverge)

Researched from the checked-out sources. Claims not backed by file:line have
been removed.

**Krita gates placement with a per-node predicate.** `KisNode::allowAsChild` is
pure virtual (`krita/libs/image/kis_node.h:92`), and every kind answers for
itself:

| Class | File:line | Rule |
|---|---|---|
| `KisMask` | `krita/libs/image/kis_mask.cc:129-133` | unconditional `return false`; masks are leaves |
| `KisPaintLayer` | `krita/libs/image/kis_paint_layer.cc:128-131` | `node->inherits("KisMask")` only |
| `KisSelectionBasedLayer` (adjustment/generator) | `krita/libs/image/kis_selection_based_layer.cpp:117-120` | masks only |
| `KisShapeLayer` | `krita/libs/ui/flake/kis_shape_layer.cc:295-298` | masks only |
| `KisCloneLayer` | `krita/libs/image/kis_clone_layer.cpp:109-112` | masks only |
| `KisGroupLayer` | `krita/libs/image/kis_group_layer.cc:114-139` | recursive descendant check (`checkNodeRecursively`), plus root-layer special cases for selection masks (`:125-127`) and `allowMasksOnRootNode` (`:131-135`) |

This is exactly Darkly's `supports_screen_space` shape (type-owned, recursive
for containers) and it validates keeping the predicate on the node.

**But Krita reports the refusal to the user almost nowhere.** The enforcement
points are:

- `KisNode::add`: `krita/libs/image/kis_node.cpp:473`,
  `KIS_SAFE_ASSERT_RECOVER_RETURN_VALUE(allowAsChild(newNode), false)`. The
  macro (`krita/libs/global/kis_assert.h:129`) is compiled to a log line only:
  `HIDE_SAFE_ASSERTS` is `ON` by default (`krita/CMakeLists.txt:307`).
- `KisNodeManager::moveNodeAt`: `krita/libs/ui/kis_node_manager.cpp:529`:
  `if (parent->allowAsChild(node)) { ... }` with **no `else`**. Silent skip.
- `KisNodeJugglerCompressed`: `krita/libs/ui/kis_node_juggler_compressed.cpp:446`:
  `continue` on failure. Silent.
- Drag and drop: `KisMimeData::correctNewNodeLocation`
  (`krita/libs/ui/kis_mimedata.cpp:437-445`) **walks up the parent chain looking
  for a legal ancestor**, and `insertMimeLayers` returns `false`
  (`krita/libs/ui/kis_mimedata.cpp:520-522`) with no message anywhere in the
  file.
- The drop *cursor* is gated: `KisNodeModel::updateDropEnabled`
  (`krita/libs/ui/kis_node_model.cpp:885`) consults `allowAsChild` to populate a
  drop-enabled set consumed by `flags()`
  (`krita/libs/ui/kis_node_model.cpp:660-669`). Note `canDropMimeData`
  deliberately returns `true` unconditionally for item drops
  (`krita/libs/ui/kis_node_model.cpp:852-860`, with a comment that returning
  false breaks Qt5's drag handling).
- The **one** user-visible refusal message in Krita is `quickUngroup`'s floating
  message: `krita/libs/ui/kis_node_manager.cpp:1698-1706`, e.g.
  `i18n("Cannot move layer \"%1\" into new parent \"%2\"", ...)` shown via
  `m_d->view->showFloatingMessage(message, QIcon())` at `:1704`.

**GIMP is split.** The interactive path is silent-bool: `gimp_item_tree_add_item`
(`gimp/app/core/gimpitemtree.c:513-566`) and `gimp_item_tree_reorder_item`
(`gimp/app/core/gimpitemtree.c:648-...`) validate only through
`g_return_if_fail` (e.g. the parent-must-be-a-group check at
`gimp/app/core/gimpitemtree.c:533-534`, the cycle check at `:678-681`): no
`GError` parameter exists. The layer tree view calls `gimp_image_reorder_item`
without checking the return value at all
(`gimp/app/widgets/gimpitemtreeview.c:1642-1646`); the only user feedback is a
suppressed drop indicator via `drop_possible`
(`gimp/app/widgets/gimpcontainertreeview-dnd.c:697-817`, cycle rejection at
`:782-783`; `gimp/app/widgets/gimpitemtreeview.c:1447-1499`;
`gimp/app/widgets/gimplayertreeview.c:705-739`).

The **PDB** path, by contrast, returns human-readable reasons. `gimp_pdb_item_is_group`
(`gimp/app/pdb/gimppdb-utils.c:540-558`) sets
`_("Item '%s' (%d) cannot be used because it is not a group item")`;
`gimp_pdb_item_is_not_ancestor` (`gimp/app/pdb/gimppdb-utils.c:448-471`) sets
`_("Item '%s' (%d) must not be an ancestor of '%s' (%d)")`; these are consumed by
`image_reorder_item_invoker` (`gimp/app/pdb/image-cmds.c:1470-1506`) and returned
to the caller via `gimp_procedure_get_return_values`
(`gimp/app/pdb/gimpprocedure.c:642`).

**Synthesis.** Prior art supports (a) a per-kind, container-recursive predicate,
(b) suppressing the drop affordance, and (c) a human-readable reason at the API
boundary. It does **not** support toasting on every refused interactive drop:
both editors deliberately stay quiet there. The user has asked for the loud
version, and the reason it is right *here* and not there is stated in
`handoff-viewport-boundary.md` §2: under this representation the failure mode of
silence is not "nothing happened", it is "your unrelated viewport arrangement
was destroyed." That is worth a toast. The design below takes all three of
prior art's mechanisms **and** the toast, rather than the toast alone.

### 5.2 The predicate, with a reason, still type-owned

Replace `LayerNode::supports_screen_space -> bool` (`layer.rs:739`) with:

```rust
/// Why this node cannot be realized after the view transform, or `None`.
/// `supports_screen_space` is `screen_space_refusal(doc).is_none()`.
pub fn screen_space_refusal(&self, doc: &Document) -> Option<ScreenSpaceRefusal>
```

with a `Copy`, allocation-free carrier:

```rust
#[derive(Clone, Copy)]
pub enum ScreenSpaceRefusal {
    /// This node's kind is not realizable after the view transform.
    Kind { node: LayerId, kind: &'static LayerKindRegistration },
    /// It carries a mask; mask textures are canvas-space R8 sampled in plane
    /// coordinates.
    Masked { node: LayerId },
    /// An isolated group owns a canvas-space accumulator with no screen-space
    /// counterpart.
    Isolated { node: LayerId },
    /// A group with nothing in it is not a viewport effect yet.
    Empty { node: LayerId },
}
```

Group recursion returns the **descendant's** refusal unchanged, so the culprit
is always named precisely and the enum stays flat and `Copy`: no `Box`, no
`String`, nothing allocated on the hot path. `qualifying_screen_space_suffix`
(`document/mod.rs:511`) runs per `compose_children` call per frame, so this
matters.

Message rendering lives in exactly one place
(`ScreenSpaceRefusal::message(&self, doc) -> String`) and reads names out of the
document, e.g.:

> Group "Backdrop" contains "Layer 3", a Raster Layer, which cannot be shown in
> viewport space. Only effect layers can.

**Kind additivity.** The `LayerNode::Layer(Layer::Filter(_)) => true,
LayerNode::Layer(_) => false` arms (`layer.rs:746-747`) become
`self.kind().renders_after_view_transform`, a new `bool` on
`LayerKindRegistration` (`document/layer_kind.rs:54`) alongside the capability
flags it already carries (`can_have_mask`, `can_rename`, `has_thumbnail`).
`filter.rs` sets `true`; `raster.rs` / `void.rs` / `vector.rs` / `group.rs` set
`false`. A new leaf kind opts in from its own file with no edit here: the
`build.rs`-generated `registrations()` picks it up.

The container arm (`LayerNode::Group`) stays as a `match self` inside
`LayerNode`, matching the existing precedent of `composites_in_place`
(`layer.rs:713`) and `needs_before_snapshot` (`layer.rs:773`): the type
answering about itself is sanctioned; only *consumers* branching is not.

> **Open question for review:** the alternative is a full
> `fn screen_space_refusal(&LayerNode, &Document) -> Option<ScreenSpaceRefusal>`
> function pointer on the registration, moving the group recursion into
> `group.rs`. That is maximally additive but produces three byte-identical
> three-line bodies in `raster.rs` / `void.rs` / `vector.rs`. The bool is
> proposed because it trades a smaller extension point for no duplication.

### 5.3 Where refusal is enforced, and where clamping stays

The bad state to make inexpressible is: *a node whose `render_space` is `Screen`
but whose `screen_space_refusal` is `Some`.* The temptation is to refuse inside
`Document::link` (`document/mod.rs:1198`), which every structural path funnels
through. **That is wrong**, for one specific reason: a `CompoundAction` undo
reassembles a tree one `reinsert_entity` at a time, and the intermediate states
are legitimately inconsistent (undoing `group_layers` reinserts children before
the group is back). A refusing `link` would corrupt the tree mid-undo.

So the rule splits by *layer*:

| Layer | Behaviour | Why |
|---|---|---|
| **Structural** (`Document::link` → `enforce_boundary_on_insert`) | total, silent redirect | Guarantees the compositor is never handed a state it cannot render, for undo/redo/load, which have no user intent to refuse. |
| **Read** (`screen_space_run`'s clamp) | stays | Catches disqualification that insertion cannot see (`add_mask` on a run member, `set_group_passthrough(false)`). §2 of the handoff deletes it; until then it is load-bearing. |
| **Intent** (engine move handlers) | pre-check, `Err(reason)` → toast | The only layer where the user unambiguously asked to put *this* node in *that* place. |

`enforce_boundary_on_insert` (`document/mod.rs:1224`) must be **generalized from
"parent is the root" to "the resolved parent renders in screen space"**. Today
`document/mod.rs:1231` returns early unless `parent == self.root`, which is the
hole that lets a raster be added or pasted straight into a viewport group
(§2.4). After the change, a node that cannot live in screen space and is
targeted at a screen-space parent is redirected to the first canvas-space slot
at the root, which is exactly what the function's own doc comment
(`document/mod.rs:1221-1223`, "This is the only place the rule is written")
already claims it does.

### 5.4 The destination-space query, and the pre-check

Add to `Document`:

```rust
/// The space a node would render in if it landed at `target`.
///
/// Every `MoveTarget` variant names a reference node, and a node landing
/// beside a sibling (or inside a group) renders in that reference's space.
/// So this is one lookup, not four.
pub fn space_at(&self, target: MoveTarget) -> RenderSpace {
    self.render_space(target.reference())
}

/// `Ok(())` unless `node` would land in screen space and cannot be realized
/// there. Every canvas-space move is trivially `Ok`.
pub fn check_move(&self, node: LayerId, target: MoveTarget)
    -> Result<(), ScreenSpaceRefusal>
```

`MoveTarget::reference()` (`document/mod.rs:77`) already exists and already
means exactly this. For `IntoGroupTop(g)` / `IntoGroupBottom(g)` the reference
is `g`, and a child of `g` renders in `g`'s space; for `Before(x)` / `After(x)`
the reference is the sibling. One expression covers all four.

### 5.5 Every path that can reach the state, and what it does

| Path | Entry point | Disposition |
|---|---|---|
| Single move | `DarklyEngine::move_layer`: `engine/layers.rs:1258` | **Refuse.** Signature becomes `Result<(), String>`. Regenerate `protocol_gen.ts`. |
| Multi move / all panel drags | `DarklyEngine::move_layers`: `engine/layers.rs:1293` | **Refuse.** Already `Result<usize, String>`; pre-check every id against `target` before any mutation, so a refused batch is atomic. This is the path *both* `LayerItem.svelte:375` and `LayerGroup.svelte:319` use, which is what makes row-drops and group-drops one rule. |
| Divider drag | `set_screen_space_boundary`: `engine/layers.rs:1732` | **Clamp** (unchanged). Dragging the divider past a raster is a gesture that overshoots, not an illegal placement; `spaceDivider.ts:44` (`maxEligible`) already stops the handle at the first ineligible row so the user sees the limit under the cursor. Prior art agrees: this is Krita's `updateDropEnabled` affordance. |
| Group | `group_layers`: `engine/layers.rs:611` | **Structurally unable** to produce an illegal state after §4.4: the new group inherits the sources' space, and a group of effects is eligible by construction. If the selection is mixed, the sources' common space is Canvas. |
| Ungroup | (no dedicated handler found; children move out via `move_layers`) | Covered by `move_layers`. |
| Add layer | `add_raster` / `add_void` / `add_filter` / `add_group` / `add_text_layer` | **Redirect, silent.** Anchor is a convenience, not a stated intent. Covered by the generalized `enforce_boundary_on_insert` (§5.3). |
| Paste | `clipboard.rs:508`, `floating.rs:275`: both `resolve_anchor_target` then `doc.move_layer` | **Redirect, silent.** Same reason. Covered by the same generalization. |
| Duplicate | `duplicate.rs:84` (`After(source)`), `duplicate.rs:355/360` | **Structurally safe**: a duplicate of an eligible node is eligible; a duplicate of an ineligible node targets an ineligible sibling, so it lands in canvas space. |
| Undo / redo reinsert | `reinsert_entity`: `document/mod.rs:1124` → `link` | **Never refuses.** It does not call `check_move`; it goes through the structural layer only. And by induction it never *needs* to: if every user-facing path refuses, every recorded slot was legal when recorded. `TreeSlot.screen_space` + `restore_to_screen_space` (`document/mod.rs:1150`) restore the recorded side, bounded by `qualifying_screen_space_suffix` so a restore can never drag an ineligible sibling into the run. |
| Load | `engine/load.rs:372`: `doc.screen_space_count = doc.clamp_screen_space_count(manifest.screen_space_count)` | **Clamp, silent.** A hand-edited or foreign save is not a user gesture. |
| Attach a mask to a run member | `add_mask` | **Read clamp** (unchanged; pinned by `masked_or_isolated_nodes_cannot_be_above_the_boundary`, `tests/effect_space.rs:277`). |
| Isolate a run group | `set_group_passthrough(id, false)`: `engine/layers.rs:1669` | **Read clamp** (unchanged). See open question §8.2. |
| Merge / flatten | `engine/merge.rs:52,206`; `engine/flatten.rs:23` | Already refuse / exclude; both become nested-aware for free via §4.2. |

### 5.6 Frontend

No new code is expected. `LayerItem.svelte:381-383` and
`LayerGroup.svelte:325-327` already catch and toast:

```js
} catch (e: any) {
    toast.show('error', e.message ?? String(e));
}
```

Both go through `moveLayers`, so a rule enforced in `move_layers` covers row
drops, group drops, and multi-select drags. `LayerPanel.svelte:22` swallows the
list-level drop (`e.preventDefault()` only) and issues no move, so there is no
third path.

Optional, deferred: suppress the drop indicator on an illegal target using the
`screenSpaceEligible` flag already on every row (`engine/types.rs:555` and
friends). That is Krita's `updateDropEnabled`
(`krita/libs/ui/kis_node_model.cpp:885`) and GIMP's `drop_possible`
(`gimp/app/widgets/gimpcontainertreeview-dnd.c:697-817`). Not required for the
fix; listed so the reviewer can decide whether to fold it in.

---

## 6. Tests

All Rust tests go in `crates/darkly/tests/effect_space.rs`, following its
existing helpers (`test_engine`, `fill_layer`, `settle`, `px`, `effect`,
`run_ids`, `in_run`, `eligible`: `tests/effect_space.rs:18-129`).

Run with:

```bash
cargo test --workspace --exclude darkly-wasm --features darkly/testing -- --test-threads=1
```

`--features darkly/testing` is mandatory (it exposes `gpu::test_utils`,
`blocking_read`, and the `test_readback_*` accessors). `--test-threads=1` is
mandatory (GPU integration tests share a process-wide wgpu device and SIGSEGV in
parallel). Test-only readback helpers are permitted here and nowhere else; no
production code in this plan touches `device.poll(Wait)`.

### 6.1 The regression test: must fail before the fix

```
effect_inside_a_screen_space_group_reaches_the_presented_image
```

1. 16x16 engine; raster flood-filled solid red.
2. `effect(&mut engine, "invert")`; `group_layers(vec![invert])`.
3. `set_screen_space_boundary(1)`; assert `run_ids == vec![group]` (this part
   passes today: it is the setup, not the assertion).
4. `settle`.
5. **Assert `test_readback_screen_run(16, 16)` centre is cyan** (`r < 64`,
   `g > 190`, `b > 190`: the same sRGB-tolerant form as
   `screen_space_effect_is_visible_only_after_the_present_pass`,
   `tests/effect_space.rs:206-211`).
6. Assert `test_readback_canvas` centre is still `[255, 0, 0, 255]`: the
   viewport effect is not in the image.

**Expected failure today:** step 5 reads red, because `members` is empty
(§2.2) and the plain-present branch runs. Step 6 passes today for the wrong
reason (nothing renders at all), which is why step 5 is the load-bearing
assertion. Demonstrate the failure before writing any fix.

### 6.2 The rest

| Test | Asserts |
|---|---|
| `moving_a_group_containing_a_raster_into_viewport_space_is_refused` | `move_layers(vec![group], After(top_run_member))` returns `Err`, the message names the raster, and `run_ids` is **unchanged**. Fails today: the call returns `Ok` and the group silently stays in canvas space. |
| `moving_a_raster_into_a_viewport_space_group_is_refused` | `move_layers(vec![raster], IntoGroupTop(run_group))` returns `Err`; `run_ids` still contains the group; the raster is still where it was. Fails today: returns `Ok` and the run empties. **Replaces** the current silent-clamp assertion at `tests/effect_space.rs:328-335`, which encodes the behaviour being fixed. |
| `undo_redo_of_a_viewport_group_round_trips_without_refusal` | Build the legal arrangement, `remove_layers`, `undo`, `redo`, `undo`; `run_ids` and the rendered surface match at each equivalent point; no `Err` is produced. Guards §5.5's undo row. |
| `adding_a_raster_with_a_viewport_group_anchored_does_not_break_the_run` | `add_raster_layer(Some(effect_inside_run_group))` lands the raster in canvas space and leaves `run_ids` intact. Fails today (§2.4's add-path hole). |
| `a_nested_screen_effect_renders_at_viewport_resolution` | With a 16x16 canvas and a 64x64 viewport, an effect with a `perf_scale_factor` inside a run group reports a `test_effect_reduced_size` derived from **64x64**, not 16x16. This is the sharp assertion on the `EffectSpace`/`accumulator` tagging (§2.3): the strongest single guard against a regression to per-id membership. |
| `merge_refuses_a_layer_nested_in_a_viewport_group` | `merge_layers` on a node inside a run group returns the viewport-only refusal. Fails today (`engine/merge.rs:52` sees direct membership only). |
| `slot_of_records_direct_run_membership` (unit, `document/mod.rs`) | `TreeSlot.screen_space` is `true` only for a root child in the run, `false` for a node nested inside a run group, pinning the §4.2 trap. |

Existing tests expected to need edits:
`a_group_is_eligible_exactly_when_its_contents_are`
(`tests/effect_space.rs:307`, tail assertion inverted) and
`a_raster_can_never_be_placed_above_the_boundary`
(`tests/effect_space.rs:232`, the `move_layers` step at `:264-273` now expects
`Err` rather than a silent clamp).

Frontend: no new vitest. `spaceDivider.test.ts` is unaffected. Gate via
`npx tsc --noEmit`, `npm run check`, `npm test`, `npm run build`, plus
`DARKLY_REGEN_TS=1 cargo test -p darkly --test protocol --features testing,ts-export`
after `move_layer`'s signature change.

---

## 7. Architectural impact

**Strictly reduces machinery.** `EffectSpace` and one of each duplicated GPU
resource pair go away; two ways of asking "which space" collapse into one
document query; the screen run stops being a bespoke flat pipeline and becomes a
caller of the walk that already exists.

- **Document Authority:** strengthened. `render_space` is a pure document query
  with no GPU dependency; the compositor stops carrying its own answer on
  `EffectInstance.space`.
- **Modularity / type-owned dispatch:** strengthened. Per-kind screen-space
  eligibility moves onto `LayerKindRegistration`, additive from a kind's own
  file. `compose_children` gains no new kind branch.
- **DRY:** strengthened on three axes, one walk, one scaling-pipeline set, one
  apply-scratch pool.
- **Ownership:** the one wart is `SCREEN_ACCUM` living in
  `Compositor::group_state`, a map documented as holding document groups
  (`compositor.rs:623-626`). Justified by the existing bake sentinel
  (`compositor.rs:3870`) and by the alternative (threading an accessor through
  ~25 split-borrow sites) being materially worse. Called out for review.

---

## 8. Risks and unresolved questions

### 8.1 Risks

1. **`set_canvas_rect` resizing the screen accumulator.** `compositor.rs:2083-2088`
   recreates every `group_state` entry at canvas dimensions. Missing the skip
   would silently shrink the viewport accumulator to canvas size on every canvas
   resize, and it would *mostly work*, which is the dangerous kind of bug. A
   test that resizes the canvas while a viewport group is active should be added
   if the reviewer agrees; `tests/canvas_resize.rs` already touches screen-space
   state.
2. **Borrow-splitting in `compose_effect_arm`.** Today the screen path holds a
   disjoint `effect_instances` borrow across `encode_in_place_apply`
   (`compositor.rs:3810-3826`, and the field-explicit signature at
   `compositor.rs:5442` exists specifically for that). Routing screen effects
   through `compose_effect_arm` (`compositor.rs:5259`) means going through
   `apply_in_place` (`compositor.rs:5403`) instead. That path already clones the
   views it needs (`compositor.rs:5316-5325`), so it should hold, but this is
   the concrete place R1 could fail. If it does: **stop and return to review**,
   do not fall back to R2 silently.
3. **The `renders_in_screen_space` semantic change.** Three call sites want the
   new transitive meaning; `slot_of` (`document/mod.rs:565`) wants the old
   direct one. A blanket rename breaks undo. Mitigated by the unit test in §6.2.
4. **`ScalingPipelines` unification assumes the formats stay equal.** They are
   equal today (`compositor.rs:1262` passes the compositor's `accum_format`
   straight to `ScreenRun::new`, and `make_accum_texture` hardcodes
   `Rgba8Unorm` at `compositor.rs:876`). If a future HDR accumulator format
   diverges per space, the unification has to be undone. Low risk, worth a
   comment at the construction site.
5. **`move_layer`'s signature change** is a protocol change. Regenerate the TS
   client; `svelte-check` is the gate that catches component-side fallout
   (`tsc --noEmit` cannot see inside `.svelte`).

### 8.2 Unresolved questions

1. **Should `set_group_passthrough(run_group, false)` refuse instead of
   clamping?** It is not a "move", so the user's stated rule does not cover it,
   but it silently destroys a viewport arrangement exactly like the cases that
   *are* covered. Same for `add_mask` on a run member. Proposed: leave both
   clamping in this change and raise them separately, because the clamp is
   scheduled for deletion by handoff §2 anyway and refusing here would be work
   thrown away. **Wants a decision.**
2. **Bool vs. function pointer on `LayerKindRegistration`** (§5.2). Proposed:
   bool, to avoid three identical bodies.
3. **Should the screen accumulator's `composite_cache` be allocated at all?**
   `GroupState` always allocates one (`compositor.rs:900-901`); the screen
   accumulator never blends into a parent so it is dead weight: one
   viewport-sized texture (~33 MB at 4K). Making it lazy touches
   `create_group_state` for every group. Proposed: allocate it, note the waste,
   revisit if VRAM shows up in profiling.
4. **Should `docs/gpu-passes.md` be updated?** It is a new untracked file
   describing pass structure. If it documents the screen run's pass sequence, it
   needs a pass; the implementer should check.
5. **Should the drop indicator be suppressed on illegal targets** (§5.6)? Prior
   art does this and Darkly already ships the `screenSpaceEligible` flag needed
   for it. Deferred by default.

---

## 9. LOC estimate

Lines **added / removed**, not touched. Honest, and deliberately pessimistic on
the compositor.

### Stage A, rendering fix

| Area | Added | Removed |
|---|---:|---:|
| `document/mod.rs` (`RenderSpace`, `render_space`, `renders_in_screen_space`, `slot_of` comment | 40 | 8 |
| `layer.rs`) no change in Stage A | 0 | 0 |
| `gpu/compositor.rs`, `SCREEN_ACCUM` + lifecycle, `present_and_screen_run` rewrite, `EffectSpace` deletion, 4 collapsed matches, `compose_children` skip, scratch pool, unified scaling pipelines, `set_canvas_rect` skip, `is_in_isolation_path`, `effect_animates`, `mark_effect_dirty` | 170 | 175 |
| `gpu/screen_run.rs`, shrink to blit + size + flag | 30 | 90 |
| `engine/layers.rs`, `group_layers` space preservation | 12 | 3 |
| **Stage A production** | **~252** | **~276** |
| Tests (`effect_space.rs`: regression + resolution + merge + slot unit; `canvas_resize.rs` case) | 190 | 10 |
| **Stage A total** | **~442** | **~286** |

Net Stage A: roughly **-25 lines of production code** for a bug fix. That is the
signal that R1 is the right shape.

### Stage B: refusal rule

| Area | Added | Removed |
|---|---:|---:|
| `layer.rs` (`ScreenSpaceRefusal`, `screen_space_refusal`, `message` | 85 | 25 |
| `document/layer_kind.rs` + 5 kind files) one flag, five registrations | 22 | 0 |
| `document/mod.rs` (`space_at`, `check_move`, generalized `enforce_boundary_on_insert` | 55 | 18 |
| `engine/layers.rs`) pre-checks in `move_layer` / `move_layers` | 35 | 6 |
| `frontend/src/engine/protocol_gen.ts` (generated) | 6 | 4 |
| **Stage B production + generated** | **~203** | **~53** |
| Tests (three refusal cases, undo round-trip, add-path; edits to two existing tests) | 150 | 25 |
| **Stage B total** | **~353** | **~78** |

### Combined

| | Added | Removed |
|---|---:|---:|
| Production | ~455 | ~329 |
| Tests | ~340 | ~35 |
| Generated + docs (incl. this plan) | ~40 | ~4 |
| **Total** | **~835** | **~368** |

Rounded range to quote for approval: **+750 to +950 added, −300 to −430
removed**, of which production is **+400 to +520 added, −280 to −380 removed**
(i.e. production is roughly net-neutral).

### Smaller-scoped alternative, if that is too large

Stage A only, using **R2** (a bespoke recursive screen walk) instead of R1:
approximately **+95 / −25** production, **+120 / −5** tests. It fixes the
reported rendering bug and nothing else: the four mis-tagged consumers in §2.3
stay broken, the silent degradation stays, and the codebase gains a second walk
that must be kept in step with `compose_children`. Recommended only if the user
wants the visible symptom gone today and is willing to book the rest as debt.

The middle option (**Stage A with R1, defer Stage B**) is the one worth
considering seriously: **+252 / −276** production, **+190 / −10** tests. It
fixes the reported rendering bug, deletes more code than it adds, and leaves the
refusal rule (which overlaps the pending divider-as-a-node redesign) for a
follow-up.
