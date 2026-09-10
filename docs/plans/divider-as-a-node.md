# Divider as a Node

## Independent Review

Independent investigation performed against the working tree (branch
`better-veils`) before reading the plan's argument. Every load-bearing claim was
re-derived from source; findings below are the deltas.

### Diagnosis — confirmed

- **Bug 1 confirmed.** The divider is not in `app.dropRows` (`indexLayerTree`,
  `frontend/src/state/layerTree.ts:79-124`, walks only tree nodes; the divider
  is count-interleaved at `LayerPanel.svelte:46-60`). The gap below the
  bottom-most screen row and above the top canvas row is one gap;
  `targetForDepth` (`dropTarget.ts:143-170`) prefers `prev` →
  `Before(draggedLayer)`. `dropTarget.svelte.ts:131` swallows the self-drop
  (`if (ids.includes(drop.target.target_id)) return`), so the gesture dies
  silently; an API-level self-target errors at `engine/layers.rs:1338-1341`.
  Both failure modes verified. Note the divider's *own* drop site has the same
  defect: `SpaceDivider.svelte:54-57` resolves through
  `rootGapIndex(app.dropRows, app.screenSpaceCount)`, whose gap also prefers
  the row above → `Before(bottomScreenRow)` — dropping directly on the divider
  reproduces both bugs. The plan's fix covers this site too (the divider
  becomes the row), but the plan doesn't mention that today's divider drop
  target is itself broken; worth stating since it's more evidence for the
  redesign.
- **Bug 2 confirmed.** `Document::move_layer` (`document/mod.rs:1172-1185`):
  `wants_screen = members.contains(&target.reference())` re-adds the mover to
  the member set; `set_screen_space_members` (`:1210-1218`) keeps it in the
  run. Verified verbatim.
- Root-cause framing is correct: every listed compensation mechanism exists at
  the cited locations and each was verified to exist *because* the boundary has
  no index (`TreeSlot.screen_space` doc comment at `document/mod.rs:34-41` says
  so explicitly).

### Deletion list — verified, with corrections

Every table row checked by grep. Corrections:

- `TreeSlot` construction in merge is at `engine/merge.rs:149`, not `:146`.
- `engine/load.rs:780` (hand-built `Manifest` in load's own unit test) also
  constructs `screen_space_count: 0` and dies with the field — not in the
  table.
- Call-site counts: `set_screen_space_boundary` appears ×37 in
  `effect_space.rs` (plan says ×40), ×3 `layer_bake.rs`, ×3 `canvas_resize.rs`,
  ×1 each `compositor_revisions.rs` / `effect_scale.rs` /
  `format/tests.rs:483` — 46 total, not ~50. Immaterial to the estimate.
- `screen_space_eligible`'s only frontend consumer is `maxEligible`
  (`spaceDivider.ts:44-51`) — confirmed; deleting the field is safe.

### Chokepoints — real

The proposed gates were verified to be actual chokepoints, not scattered sites:
`remove_layer`/`detach_for_remove`/`remove_layers` (`engine/layers.rs:1161,
1191, 1216`), `duplicate_node_inner` (`engine/duplicate.rs:66`), `group_layers`
editable-filter loop (`engine/layers.rs:617-635`), merge validation
(`engine/merge.rs:52, 206`), `add_mask` (`engine/filters/mask.rs:27`),
`check_screen_space_move` (`engine/layers.rs:1284`). The flag-on-registration
shape matches the existing `can_have_mask` precedent
(`document/layer_kind.rs:65`). Krita citation verified:
`krita/libs/image/kis_group_layer.cc:114` `allowAsChild`, root-only
selection-mask rule with the BUG 294905 comment at `:118-127`, mutual recursion
with `checkNodeRecursively` at `:79`/`:116`. GIMP citation verified:
`gimp_layer_is_floating_sel` call sites across `gimp/app/core/`
(`gimpimage-merge.c`, `gimpselection.c`, `gimpimage.c`, `gimp-edit.c`, …).
`effect-layers.md:880` rejection text verified as quoted.

- The plan's open verification item is already answered: `compose_layer_arm`
  tolerates a missing entry — `gpu/compositor.rs:5188-5199` early-returns on
  both `node_textures.get` and `layer_cache.get` misses. No
  `is_blend_content` early-return needed; drop the contingency.
- One consistency note, not a defect: divider-nesting is refused only at the
  engine chokepoint; `Document::move_layer` itself carries just a
  `debug_assert` (`document/mod.rs:1160-1163`). That is exactly the status quo
  for screen-space blockers, so it is consistent — but the plan's rule 2 should
  land in `screen_space_move_blocker`/the generalized document-side check, not
  engine-side string formatting, so the debug_assert keeps covering it.

### Findings requiring revision

1. **`resolve_anchor_target` has three `IntoGroupTop(root)` fallbacks, the plan
   re-points one.** `document/mod.rs:1445-1447` (no anchor), `:1451` (filter
   with no host), `:1457` (stale id) all answer `IntoGroupTop(root)`. Only the
   first is re-pointed to `Before(divider_id())`. Left as-is, a stale-anchor
   add of a *qualifying* node lands at the top of screen space. All three must
   become `Before(divider)` — or better, one shared fallback expression so the
   next variant can't diverge.
2. **"Today's observed behavior" claim for anchorless adds is wrong for one
   case.** Today an anchorless add of a node that qualifies *and* yields
   screen-space effects, with a non-empty run, grows the run and lands at the
   top of screen space (`enforce_boundary_on_insert`,
   `document/mod.rs:1405-1407`). Under the plan it lands below the divider.
   The new behavior is defensible (an add is not a placement statement about
   the divider), but the plan must state it as a behavior change, not
   equivalence, and check whether any existing test pins add-grows-the-run.
3. **Panel empty state never fires.** `LayerPanel.svelte:62` shows "No layers"
   when `app.layerTree.length === 0`; with the divider always in the tree the
   length is never 0. The condition must ignore the divider row. Same class of
   consumer: `LayerItem.svelte:105-109` / `LayerGroup.svelte:86-88`
   (`canMergeDown` via `topIdx < length - 1`) now count the divider as a
   below-neighbor — verified this creates no *new* hazard (a run member's
   merge-down is refused today at `engine/merge.rs:52`, and the bottom canvas
   layer stays last in the list), but the sweep of `app.layerTree` consumers
   the plan implies should be an explicit step, not incidental.
4. **The divider row's upper band is a new refuse-zone; say so.** As a
   `DropRow`, the divider's upper half resolves `After(divider)` — a request
   for screen space. A raster dropped there gets a refusal toast; today the
   same pixel region (the divider element) resolves to a canvas-space slot.
   With the run empty the divider is the topmost row, so the top edge of the
   panel gains this refuse-zone. This is the loud-refusal philosophy applied
   correctly (the two halves of the divider row *are* the two spaces — that's
   the fix), but it belongs in the UX note alongside the clamp regression.
5. **Step 1's arm list is incomplete.** Files matching on `Layer` variants also
   include `engine/clipboard.rs`, `engine/floating.rs`, `engine/painting.rs`,
   `engine/merge.rs`, `engine/flatten.rs`, `document/pixel_transform.rs`,
   `undo/mod.rs`. Spot-check: clipboard's are non-exhaustive
   `if let Layer::Raster` patterns (no arm forced, divider falls through to
   the correct "not copyable pixels" behavior), but the plan's "~15 exhaustive
   matches, clippy catches them" risk bullet is doing load-bearing work — the
   dangerous ones are precisely the *non*-exhaustive matches clippy cannot
   catch. Add an explicit grep-sweep step (`Layer::Raster|Layer::Void|...`)
   with a decision per site.

### Positions on the unresolved questions

1. **Degrade on load — agree.** Matches the stated rationale of today's load
   clamp (`engine/load.rs:367-372`: "Clamping rather than trusting"); refusing
   to open user data over a divider position would be strictly worse than the
   count design's failure mode. `CorruptManifest` for nested/duplicate dividers
   is right — those are structural corruption, not a user-expressible state.
2. **`Before(self)` no-op — agree, out of scope.** The frontend already
   suppresses self-drops (`dropTarget.svelte.ts:131`) and the divider row makes
   the legitimate gesture expressible; changing `move_layers`' contract here
   would be scope creep.
3. **Visuals — agree, keep them.** The row must stay unselectable (excluded
   from `ids`/`order`) and render no controls; `select: false` +
   `draggable: true` covers the drag half.

### Tests and estimate

- The Rust regression tests are honest: before the fix there is no divider id,
  so the *expressible* form of each gesture is asserted as failing via its
  count-era encoding (`Before(effect)` self-ref, `Before(A)` side-inheritance)
  — both verified to fail against today's code by trace. The frontend test
  (distinct gaps around a divider `DropRow`) fails today since `indexLayerTree`
  emits no divider row. Good.
- Dead tests correctly identified: the manifest-rewrite clamp test is at
  `effect_space.rs:508-524` (plan says 527-564 — the range includes the
  `save_to_zip` helper, which *survives* minus the `override_count` rewriting;
  don't delete the helper). `spaceDivider.test.ts` is 56 lines as claimed.
- LOC estimate is credible; verified call-site counts run slightly *below* the
  plan's figures, so ~+430/−400 production is if anything conservative on the
  deletion side. No material under-estimation found beyond items 1/3/5 above
  (a few dozen lines).

The superseding argument itself holds: the count design's compensations
(verified above) outnumber the node design's invariants, and the failure-mode
asymmetry (silent wrong-side export vs loud refusal) is real. No simpler
general design presented itself under challenge — an `Option<LayerId>` anchor
and index representations were already eliminated for cause in
`effect-layers.md:875-881`.

Verdict: revise

Findings 1-5 are incremental corrections — no rethink of the approach is
warranted; the design survives every structural challenge.

---

The viewport divider — the boundary between screen-space (viewport-only) and
canvas-space layers — becomes a real node among the root's children, replacing
`Document::screen_space_count` and all the machinery that compensated for a
boundary that did not exist in the tree.

Decided with the user at the end of the PR 4 session; see
`handoff-viewport-boundary.md` §2. This plan turns that decision into
implementation steps. The original effect-layers plan
(`docs/plans/effect-layers.md:880`) rejected "a marker node among the root's
children" as "more machinery than a count — it would need its own invariants
(exactly one, never inside a group, never deletable, never duplicable)". That
judgment is superseded: the count design ended up needing
`TreeSlot.screen_space`, a `link`/`unlink` invariant, a read clamp, a load
clamp, reference-side inheritance in `move_layer`, a dedicated undo action, a
dedicated wire handler, and a dedicated panel drag — strictly more machinery
than the invariants the marker node needs, and with a worse failure mode
(silently landing a layer on the wrong side of what gets exported, versus an
operation that loudly refuses).

## Problem and root cause

Two user-visible bugs, one representational root cause.

**Bug 1 — "Cannot move a layer into itself".** The dragged layer is the
bottom-most screen-space child and the user drops just below it, aiming at the
top of canvas space. The panel's drop model (`frontend/src/ui/layers/dropTarget.ts`)
resolves drops through gaps between rows, and the divider is *not* a row in
`app.dropRows` (`indexLayerTree` walks only `layerTree` nodes; the
`SpaceDivider` element is interleaved by count in `LayerPanel.svelte:46-60`).
So "the gap below the bottom-most screen-space row" and "the gap above the
top-most canvas-space row" are the *same* gap. `targetForDepth`
(`dropTarget.ts:143-170`) prefers the row above the gap, producing
`Before(draggedLayer)` — a self-reference. `move_layers`
(`crates/darkly/src/engine/layers.rs:1338-1341`) refuses it:
"Cannot move a layer into itself". (Current `dropTarget.svelte.ts:131` swallows
the self-drop silently instead — either way, the gesture the user is making is
inexpressible.)

The divider's *own* drop site shares the defect: dropping a layer directly onto
the divider element resolves through `rootGapIndex(app.dropRows,
app.screenSpaceCount)` (`SpaceDivider.svelte:54-57`), whose gap likewise prefers
the row above — `Before(bottomScreenRow)` — reproducing both failure modes at
the one place in the UI that most explicitly says "the boundary is here".

**Bug 2 — drop lands at the bottom of screen space instead of canvas space.**
Same shared gap, dragged layer elsewhere in screen space. The gap resolves to
`Before(bottomMostScreenRow)`. `Document::move_layer`
(`crates/darkly/src/document/mod.rs:1172-1185`) derives the destination side
from the *reference node's* side: `wants_screen =
run.contains(target.reference())` is true, so the moved layer is re-added to
the member set and `set_screen_space_members` keeps it in the run. It lands at
the bottom of screen space; the user asked for the top of canvas space.

**Root cause.** The boundary is `Document::screen_space_count`
(`document/mod.rs:207`) — a count of the root's trailing children. Crossing the
divider does not change a layer's index: the bottom-most screen-space child and
the top-most canvas-space child occupy the same gap, so ordering alone cannot
express which side a node lands on, and every path that touches the boundary
has to state its intent out-of-band. That produced, verified against current
code:

- reference-side inheritance in `Document::move_layer` (`document/mod.rs:1164-1185`) — the source of bug 2
- `TreeSlot.screen_space` (`document/mod.rs:43-52`) so undo can restore a side an index cannot express
- the `link`/`unlink` count bookkeeping: `enforce_boundary_on_insert` (`document/mod.rs:1365-1411`), the decrement in `unlink` (`:1424-1428`), `restore_to_screen_space` (`:1256-1268`)
- the read clamp `screen_space_run` + `qualifying_screen_space_suffix` (`:511-537`) and `clamp_screen_space_count` (`:595`)
- the load clamp (`engine/load.rs:372`) and `Manifest::screen_space_count` (`format/manifest.rs:73`)
- `ScreenSpaceBoundaryAction` (`undo/screen_space.rs`) + the `set_screen_space_boundary` handler (`engine/layers.rs:1778-1790`) + wire method
- the panel's parallel drag machinery: `SpaceDivider.svelte`'s `pointerDrag`, `spaceDivider.ts` (`gapAt`/`maxEligible`), `rootGapIndex` (`dropTarget.ts:95-103`), `app.screenSpaceCount` + `setScreenSpaceBoundary`

Give the divider a slot in `children_of(root)` and the ambiguity dies at the
source: crossing the boundary is an index change, a drag across it is an
ordinary reorder, moving the divider is an ordinary layer move, and undo of any
of it is the ordinary `LayerMoveAction`.

## Prior art

- **Krita** gates tree structure on the *parent's* capability method:
  `KisGroupLayer::allowAsChild` (`krita/libs/image/kis_group_layer.cc:114`,
  called from `checkNodeRecursively` at `:79`) decides which node kinds may
  become children — including root-only rules (a selection mask is only allowed
  under the root when no global selection exists, BUG 294905 comment at
  `:118-127`). This backs expressing "the divider may only live at root" as a
  capability consulted at the move chokepoint, not as scattered type checks.
- **GIMP** keeps a special in-tree layer — the floating selection — but gates it
  with explicit predicate calls (`gimp_layer_is_floating_sel`) sprinkled across
  `app/core` (`gimpimage-merge.c`, `gimpselection.c`, `gimpimage.c`, …). That is
  the anti-pattern our Modularity Principle forbids; it is why the gates below
  are flags on `LayerKindRegistration`, consulted at shared chokepoints, never
  `matches!` at call sites.

Neither editor has a screen-space/canvas-space divider; the feature is
Darkly-specific. The prior art informs only the *gating shape*.

## Design

### The kind

`crates/darkly/src/document/layer_kinds/divider.rs` — one file, discovered by
`build.rs` like every kind (regenerates `layer_kinds/mod.rs`):

- `TYPE_ID = "divider"`, display name "Viewport Divider".
- `DividerLayer { id, common: NodeCommon, blend: BlendProps, filters: Vec<LayerId> }`
  as a new `Layer::Divider(DividerLayer)` variant in `layer.rs`. `blend` and
  `filters` exist only to keep `Layer`'s uniform accessors total; they are
  never read (nothing composites the divider, `can_have_mask` is false).
- Serializer body is `{}`-shaped (nothing user-editable survives on it);
  `remap_ids` is a no-op; deserialize reconstructs the node.
- Registration flags: `can_have_mask: false`, `can_rename: false`,
  `has_thumbnail: false`, `leaf_renders_after_view_transform: false`, plus the
  three new capability flags below.

### New `LayerKindRegistration` flags (type-owned dispatch)

Added to `document/layer_kind.rs` and set in all six kind files (mechanical:
five existing kinds say `can_delete: true, can_duplicate: true,
screen_space_boundary: false`):

- `can_delete: bool` — consulted by `remove_layer` / `remove_layers` /
  `detach_for_remove` (refuse / skip like a locked layer) and by **merge**
  (merge consumes its sources; a source that cannot be deleted cannot be
  merged — one check in the existing merge validation loop,
  `engine/merge.rs:195-206`).
- `can_duplicate: bool` — consulted once at the top of `duplicate_node_inner`
  (`engine/duplicate.rs`); the exhaustive match gains a `Layer::Divider(_) =>
  None` arm for totality.
- `screen_space_boundary: bool` — marks the kind whose single instance *is* the
  boundary. Implies root-only placement. Consumers ask
  `node.kind().screen_space_boundary` (wrapped as
  `LayerNode::is_screen_space_boundary()`); no consumer ever compares
  `type_id`.

`group_layers` (`engine/layers.rs:612`) filters boundary nodes out of its
editable set exactly like locked layers — grouping a selection that includes
the divider groups everything else.

Property ops (`set_opacity`, `set_blend_mode`, `set_layer_visible`,
`set_node_locked`, `set_layer_name`) are deliberately *not* gated: the panel
never offers them for the divider row (it is not selectable and renders no
controls), and the divider's `blend`/`common` are never read by the compositor,
so a stray call is inert. Gating them would scatter guards for no observable
protection. `paintable` is already false (no pixel buffer);
`transform_capability` answers `None` via a new arm.

### Document invariant and queries

**Invariant: the root has exactly one divider child; the divider is always a
direct child of the root.** Established in `Document::new` (allocate + link the
divider as the root's sole initial child), defended by `can_delete: false`,
`can_duplicate: false`, and the move guard, and normalized on load (below).

No `Document::divider` field — identity is derived from the tree, so the fact
lives in one place:

```rust
pub fn divider_id(&self) -> LayerId      // scan children_of(root) for is_screen_space_boundary
fn divider_index(&self) -> usize         // its position (children are bottom-to-top)
pub fn screen_space_run(&self) -> &[LayerId]      // &children[divider_index()+1..]
pub fn canvas_space_children(&self) -> &[LayerId] // &children[..divider_index()]
pub fn renders_in_screen_space(&self, id) -> bool // root child with index > divider_index
fn in_screen_space_region(&self, id) -> bool      // root-child ancestor's index > divider_index
```

The scans are O(root children) — the same order as today's
`qualifying_screen_space_suffix` walk, and `compose_children` already resolves
the run once per group.

`screen_space_run()`, `canvas_space_children()`, `screen_space_effects()`, and
`renders_in_screen_space()` keep their signatures, so the compositor
(`gpu/compositor.rs:3767, 4393, 4762`), merge (`engine/merge.rs:52, 206`), and
flatten (`engine/flatten.rs:23`) consume them unchanged. The run no longer
clamps on read — with the count gone there is nothing to clamp; the tree *is*
the truth.

### Moves

`Document::move_layer` (`document/mod.rs:1152-1186`) becomes verbatim: refuse
filters, unlink, resolve, link. The reference-side block (`:1164-1185`) and
`set_screen_space_members` are deleted. `Before(divider)` and `After(divider)`
are now different indices, which is the whole fix.

Validation keeps its engine chokepoint (`check_screen_space_move`,
`engine/layers.rs:1284`, formats the refusal message), but both rules below
live document-side in the generalized `screen_space_move_blocker`, backed by a
new `Document::target_in_screen_space(target) -> bool` that resolves the
prospective slot (`parent != root` → `in_screen_space_region(parent)`;
`parent == root` → insertion index `> divider_index`, computed with the mover
still in place). Placing the rules in the document keeps
`Document::move_layer`'s existing `debug_assert!(screen_space_move_blocker(..)
.is_none())` covering divider moves too — an engine-side-only rule would leave
the assert blind to them:

1. **Ordinary node into the screen region** — existing rule, existing message:
   `screen_space_blocker` (`layer.rs:760`) names the offender. Unchanged in
   substance; the region test now reads indices instead of run membership.
2. **The divider itself** — destination parent must be the root
   (`screen_space_boundary` implies root-only; refusing `IntoGroup*` targets
   and any non-root parent), and every root child that would end up above the
   divider's new index must `supports_screen_space`, else refuse naming the
   first blocker. This replaces `set_screen_space_boundary`'s clamp with a loud
   refusal — consistent with the rule that *moves* refuse while *adds*
   redirect.

Multi-select drags spanning the divider need nothing new: `move_layers` already
lands the batch contiguously at the target (chaining `After(prev)`), the whole
batch is validated against the target's side before anything moves, and a batch
containing the divider plus other rows cannot occur (the divider row is never
part of the selection; if an API caller sends one anyway, the divider entry is
validated like any other id — rule 2 applies to it, rule 1 to the rest).

### Adds and pastes (placement policy)

`enforce_boundary_on_insert` (`document/mod.rs:1365-1411`) is deleted, but the
*policy* — an add or paste never fails, it lands at the nearest legal slot —
survives in a much smaller form inside `link_placed`: if the child does not
`supports_screen_space` and the requested slot is in the screen region,
redirect to `(root, divider_index())` — directly below the divider, the topmost
canvas-space slot. No count to bump, no `yields_screen_space_effects`
special-casing (the "empty group swept above the divider" hack dies with it): a
qualifying child inserted above the divider is simply above the divider.

`resolve_anchor_target` has **three** fallbacks that answer `IntoGroupTop(root)`
— no anchor (`document/mod.rs:1445`), a filter anchor with no host (`:1451`),
and a stale anchor id (`:1457`). Under the new model that slot means "top of
screen space", so all three collapse into one shared fallback expression
answering `Before(divider_id())` — the top of canvas space. One expression, so
a future fourth case cannot diverge.

This is a behavior change in one case, stated as such: today an anchorless add
of a node that qualifies *and* yields screen-space effects, with a non-empty
run, grows the run and lands at the top of screen space
(`enforce_boundary_on_insert`, `document/mod.rs:1405-1407`). Under this plan it
lands directly below the divider. That is the right semantics — an add carries
no placement statement about the boundary, so it lands in document space and
joining the run is a deliberate drag — and no existing test pins the old
behavior (verified: every `effect_space.rs` test that grows the run does so by
setting the boundary or by an explicit move, never by an anchorless add). The
new feature tests pin the new behavior. Adds anchored on a screen-space node
keep landing next to their anchor (they are effects being added to the run, or
they get the redirect).

### Disqualification in place

The count design's read clamp silently degraded a run member that acquired a
mask. With no clamp, the invariant "nothing above the divider that cannot
render there" must hold at mutation time: `add_mask`
(`engine/filters/mask.rs:27`) refuses when the host is
`in_screen_space_region`, returning `Result<(), String>` with a message in the
existing `check_screen_space_move` voice ("…has a mask, which only exists in
canvas space" is already the blocker phrase; the add-side message mirrors it).
Kind cannot change in place, and moves are already checked, so mask-attach is
the only in-place disqualifier. Loud beats silent: under the count design this
mistake changed what got exported; now it refuses.

### Save / load

- `Manifest::screen_space_count` (`format/manifest.rs:73`, `:303`) and its
  writer (`engine/save.rs:349`) are deleted. The divider serializes as an
  ordinary node entity in `nodes` and appears in the root's children list.
  Pre-release: old files without a divider entity simply load with the divider
  normalized to the top (their `screen_space_count` key is ignored by serde);
  no migration code.
- `build_staging_document` (`engine/load.rs:202`): `Document::new` pre-creates
  a divider, and the manifest root body then *replaces* the root's children
  wholesale (`load.rs:236-238`), orphaning it. After pass 3, normalize:
  - manifest tree has exactly one root-level divider → purge the orphaned
    fresh one;
  - none → link the fresh one at the top of the root's children;
  - a divider nested below root, or more than one → `CorruptManifest`.
  The load clamp (`load.rs:372`) is replaced by one load-only guard in the same
  spirit: if any root child above the loaded divider fails
  `supports_screen_space` (hand-edited file), the divider is relinked above the
  longest qualifying suffix — degrade on *load* (refusing to open user data is
  worse than degrading), refuse on *move*.

### Undo

- `undo/screen_space.rs` and its `pub use` (`undo/mod.rs:27`) are deleted;
  moving the divider records a plain `LayerMoveAction` (`undo/layer.rs:100`).
- `TreeSlot.screen_space` (`document/mod.rs:43-52`) is deleted; `slot_of`
  (`:645`) and `reinsert_entity` (`:1233-1250`, dropping the
  `restore_to_screen_space` call) become verbatim position restore — the
  position *is* the side now. Construction sites updated:
  `engine/flatten.rs:79-101`, `engine/merge.rs:146`, and `group_layers`'s
  `topmost_screen_space` capture (`engine/layers.rs:653, 700`), which becomes
  unnecessary — the group takes the topmost source's slot, and the slot carries
  the side.

### Compositor

`compose_children` (`gpu/compositor.rs:4393`) keeps its `screen_run` skip. The
divider itself is a root child on the canvas side of nothing — it reaches the
walk. It composites as nothing: `Layer::is_blend_content()` answers false (like
`Filter`), it is excluded from `all_content_layers`, `LayerKindGpu::realize_in`
(`gpu/compositor.rs:386-399`) gains an empty `Layer::Divider(_) => {}` arm, and
`compose_layer` (`:338`) routes it to `compose_layer_arm`, which already
tolerates a layer with no `layer_cache` entry — it early-returns on both the
`node_textures.get` and `layer_cache.get` misses (`gpu/compositor.rs:5188-5199`,
verified in review). No `is_blend_content` early-return is needed.

### Wire protocol and frontend

- `LayerTree` (`engine/types.rs:227-234`) drops `screen_space_count`.
  `LayerInfo` gains a `Divider { id }` variant (serde tag `"divider"`);
  `node_to_layer_info` gains the arm. The `screen_space_eligible` field on all
  five `LayerInfo` variants and `Document::screen_space_eligible`
  (`document/mod.rs:501`) are **deleted** — their only consumer was the panel's
  divider-drag clamp (`maxEligible`), which dies with the custom drag.
  `supports_screen_space` / `screen_space_blocker` (`layer.rs:739-784`) survive
  untouched engine-side; they express real rendering constraints and back every
  refusal.
- The `set_screen_space_boundary` handler (`engine/layers.rs:1778`) is deleted.
  Regenerate the client: `DARKLY_REGEN_TS=1 cargo test -p darkly --test
  protocol --features testing,ts-export`.
- `state/layerTree.ts` (`indexLayerTree`): a `type === 'divider'` node becomes
  a `DropRow` (depth 0, `isGroup: false`) — this alone makes the gaps above and
  below the divider distinct, killing both bugs — but is excluded from the
  selectable `ids` / `order` / `visibleOrder` so selection, reselection-on-
  delete, and keyboard navigation never land on it.
- `SpaceDivider.svelte` is rewritten as an ordinary row: `use:layerDropTarget`
  with `rowId: dividerId, draggable: true`; the `pointerDrag` machinery,
  `countAt`, `onEnd`, and the `spaceDivider.ts` module (`gapAt`,
  `maxEligible`) are deleted. Dragging the divider goes through the same
  `moveLayers` call as any row; an illegal divider drop surfaces the engine's
  refusal through the existing error toast (`dropTarget.svelte.ts:138-140`).
- `dropTarget.svelte.ts`: `LayerDropParams` gains `select?: boolean` (default
  true); the divider row passes `select: false` so `onDragStart` (`:84-94`)
  drags it without mutating the selection.
- `LayerPanel.svelte:46-60`: the count-interleave and the trailing conditional
  divider are deleted; the `{#each}` renders `SpaceDivider` where
  `node.type === 'divider'` appears. The "No layers" empty state
  (`LayerPanel.svelte:62`, `app.layerTree.length === 0`) never fires once the
  divider is always present — the condition changes to "no non-divider rows".
- **Explicit step: sweep every `app.layerTree` consumer** for
  divider-in-the-list assumptions. Known results from review:
  `LayerItem.svelte:105-109` / `LayerGroup.svelte:86-88` (`canMergeDown` via
  `topIdx < length - 1`) now count the divider as a below-neighbor — benign,
  because a run member's merge-down is already refused at `engine/merge.rs:52`
  and the bottom canvas layer remains last in the list, but each remaining
  consumer (`LayerFooter.svelte` `findNode` uses, `actions/index.ts` lookups,
  `TextProperties.svelte`) gets an explicit benign/fix verdict during
  implementation.
- `app.svelte.ts`: `screenSpaceCount` state (`:333`), `setScreenSpaceBoundary`
  (`:975`), and the `refreshLayerTree` parsing (`:988-990`) are deleted.
- `rootGapIndex` (`dropTarget.ts:95-103`) loses its only caller and is deleted.

UX notes, stated honestly:

- Today the divider drag clamps smoothly against eligibility; after this change
  an illegal divider drop refuses with a toast instead. That trades a live
  clamp for loud refusal and one less parallel drag system; if the clamp is
  missed, `screen_space_blocker` data can later be projected into the tree
  payload again as a purely additive change.
- The divider row's upper band becomes a refuse-zone for ineligible layers: as
  a `DropRow`, its upper half resolves `After(divider)` — a request for screen
  space — so a raster dropped there gets a refusal toast, where today the same
  pixel region resolves to a canvas-space slot. With the run empty the divider
  is the topmost row, so the panel's top edge gains this refuse-zone. This is
  the loud-refusal philosophy applied correctly — the two halves of the divider
  row *are* the two spaces — but it is a visible change in what a drop there
  does.

## Implementation steps

1. **Kind + flags.** Add the three flags to `LayerKindRegistration` and the five
   existing kind files; add `layer_kinds/divider.rs`; add `Layer::Divider` and
   its arms across `layer.rs` (`id`, `common`, `common_mut`, `blend`,
   `blend_mut`, `filters`, `modifiers_mut`, `pixels`, `kind`,
   `transform_capability` → `None`, `is_blend_content` → false,
   `owns_disposable_texture` → false), `undo/property.rs` (no new arm needed —
   its matches are non-exhaustive `if let`s), `engine/types.rs`,
   `engine/duplicate.rs`, `gpu/compositor.rs` (`realize_in`). Then an explicit
   sweep: `grep -rn "Layer::Raster\|Layer::Void\|Layer::Filter\|Layer::Vector"`
   over `crates/darkly/src` and record a per-site verdict. The exhaustive
   matches are caught by clippy; the dangerous sites are the *non-exhaustive*
   `if let` / `matches!` patterns clippy cannot flag, which silently
   misclassify a new variant — known files beyond the arm list:
   `engine/clipboard.rs` (falls through to "not copyable pixels" — correct),
   `engine/floating.rs`, `engine/painting.rs`, `engine/merge.rs`,
   `engine/flatten.rs`, `document/pixel_transform.rs`, `undo/mod.rs`. Each gets
   a verdict (fall-through correct / needs an arm) in the implementation notes.
2. **Document.** Create the divider in `Document::new`; replace the boundary
   section (`document/mod.rs:491-620`) with the index-based queries; delete
   `screen_space_count` and everything in the deletion list; simplify
   `move_layer`, `link_placed`, `unlink`, `reinsert_entity`, `slot_of`;
   re-point `resolve_anchor_target(None)`.
3. **Engine.** Generalize `check_screen_space_move` (target-side helper +
   divider-move rule); gate `remove_*`, `duplicate_*`, `group_layers`, merge
   sources on the flags; gate `add_mask`; delete `set_screen_space_boundary`;
   drop `screen_space_count` from `layer_tree`.
4. **Format + undo.** Manifest field out; load normalization in; delete
   `undo/screen_space.rs`; shrink `TreeSlot`.
5. **Wire + frontend.** Regenerate protocol; `LayerInfo::Divider`;
   `indexLayerTree`, `SpaceDivider.svelte` rewrite, `LayerPanel.svelte`
   (including the empty-state condition), `dropTarget.svelte.ts` `select`
   param, `app.svelte.ts` cleanup; delete `spaceDivider.ts`; run the
   `app.layerTree` consumer sweep with per-site verdicts.
6. **Tests** (below), then the full gate including
   `wasm-pack build` (the handoff notes a stale `frontend/wasm/pkg` bites
   manual testing otherwise).

Existing Rust tests drive the boundary through
`engine.set_screen_space_boundary(n)` at 46 call sites (`effect_space.rs` ×37,
`layer_bake.rs` ×3, `canvas_resize.rs` ×3, ×1 each `compositor_revisions.rs`,
`effect_scale.rs`, `format/tests.rs:483`). Add one test helper that expresses
"put the divider above the top n children" as a `move_layers` of the divider
and mechanically rewrite the call sites. Count-specific tests die with the
count: the manifest-rewrite clamp half of the round-trip test
(`effect_space.rs:508-524` — the `save_to_zip` helper survives, minus its
`override_count` rewriting), the stored-intent/read-clamp tests, and
`spaceDivider.test.ts` (56 lines, `gapAt`/`maxEligible`). `engine/load.rs:780`
hand-builds a `Manifest` in load's own unit test and constructs
`screen_space_count: 0` — it dies with the field.

## Tests

### Regression tests for the two bugs

The bugs are representational — the fix makes the failing gesture
*expressible* — so each regression test is written first and demonstrated
failing per the workflow, with the pre-fix failure mode noted.

The tests reproduce the user's exact report — a veil in a *subgroup* of a
viewport group ("VHS in Viewport Effects → Group 2") — because the nested shape
exercises paths the root-level shape does not: `targetForDepth`'s
escape-outward walk resolves the boundary gap to the dragged node's *ancestor*
(bug 2) or *descendant* (bug 1's visible form — `dropTarget.svelte.ts:131`
only swallows a target that is literally in `ids`, so a descendant reference
reaches the engine and surfaces the "Cannot move a layer into itself" toast the
user saw).

**Rust (`crates/darkly/tests/divider_moves.rs`, sharing `effect_space.rs`'s
helper style):**

1. *Bug 2 (report step 2):* raster in canvas space; a `vhs` effect inside
   group G2 inside group VE; VE is the run. Dragging G2 to just below the
   viewport threshold must land it in canvas space. Post-fix encoding:
   `move_layers([G2], Before(divider_id))` returns `Ok`, G2 is a root child in
   `canvas_space_children()` at the top, the run is `[VE]` alone. Pre-fix the
   gesture's only encoding is `Before(VE)` (escape-outward ancestor
   reference); reference-side inheritance re-adds G2 to the run and it lands
   at the *bottom of viewport space* — asserted as the failing baseline.
2. *Bug 1 (report step 3):* G2 (containing an effect) is the bottom-most — and
   only — run member at root. Dragging it just below the threshold must land
   it in canvas space. Post-fix: `move_layers([G2], Before(divider_id))`
   returns `Ok`, run empty, G2 canvas-side with its child intact. Pre-fix the
   gesture resolves to `Before(childEffect)` — a *descendant* of the dragged
   group — and `move_layers` errors "Cannot move a layer into itself"; the
   failing baseline pins that exact error string.

**Frontend (`dropTarget.test.ts` / `layerTree` tests):** with rows
`[VE(group), G2(group, d1), vhs(d2), D(divider), R(canvas)]`, the gap above D
resolves within screen space and the gap below D resolves to `Before(D)` — two
*distinct* targets — and resolving the below-divider gap while dragging G2
never references G2 or its descendants. Against today's model the divider is
not a row, both gestures collapse to one gap whose resolution walks into the
dragged subtree's ancestor/descendants, and the test fails — this is the
frontend half of both bugs. `indexLayerTree` gains the test that a divider
node in the wire payload becomes a `DropRow` (fails today: no divider ever
appears in the payload).

### Feature tests

- Fresh `Document::new` has exactly one divider, at the top; a new document's
  `layer_tree` carries the divider row; anchorless adds land below it.
- Save/load round-trips the divider's position (run membership identical);
  loading a manifest with no divider normalizes it to the top; nested or
  duplicate dividers → `CorruptManifest`; a hand-edited manifest with a raster
  above the divider loads with the divider relinked above the qualifying
  suffix.
- Divider refuses: delete (single and batch — batch skips it and deletes the
  rest), duplicate, merge-as-source, `IntoGroup*` targets, non-root parents;
  `group_layers` on a selection including it groups only the others.
- Moving the divider below a raster refuses and names the raster; moving it
  over effects succeeds and swaps their space (composite readback changes,
  reusing `effect_space.rs`'s `screen_space_effect_is_absent_from_the_composite`
  machinery).
- Undo/redo of a divider move restores the run exactly; undo of a cross-divider
  layer move restores the layer's side (replacing the `TreeSlot.screen_space`
  tests).
- `add_mask` on a run member refuses; on a canvas layer still works.
- Paste/floating-anchor placement while a screen-space node is active lands
  below the divider (redirect policy).
- Frontend: divider row excluded from selection order (`layerTree.test`),
  divider drag issues `moveLayers` without touching selection.

## Deletions (verified against current code)

| Item | Location |
|---|---|
| `screen_space_count` + doc comment | `document/mod.rs:191-207, 244` |
| `qualifying_screen_space_suffix`, read clamp in `screen_space_run` | `document/mod.rs:511-537` |
| `clamp_screen_space_count` | `document/mod.rs:595-597` |
| `yields_screen_space_effects` + empty-group carve-out | `document/mod.rs:574-578, 1400-1410` |
| side-inheritance in `move_layer`, `set_screen_space_members` | `document/mod.rs:1164-1185, 1210-1218` |
| `restore_to_screen_space` + `reinsert_entity` call | `document/mod.rs:1247-1268` |
| `enforce_boundary_on_insert` (count bookkeeping; redirect policy survives, simplified) | `document/mod.rs:1365-1411` |
| `unlink` run-shrink decrement | `document/mod.rs:1424-1428` |
| `TreeSlot.screen_space` + all construction sites | `document/mod.rs:43-52`; `flatten.rs:79-101`; `merge.rs:149`; `layers.rs:653, 697-701` |
| `ScreenSpaceBoundaryAction` | `undo/screen_space.rs`, `undo/mod.rs:27` |
| `set_screen_space_boundary` handler + wire method | `engine/layers.rs:1778-1790`, `protocol_gen.ts` |
| `Manifest::screen_space_count` + save/load | `format/manifest.rs:73, 303`; `save.rs:349`; `load.rs:372` |
| `screen_space_eligible` (doc query + 5 `LayerInfo` fields) | `document/mod.rs:501-507`; `engine/types.rs` |
| `LayerTree.screen_space_count` | `engine/types.rs:233`, `layers.rs:1766` |
| `spaceDivider.ts` + its test, `SpaceDivider.svelte` drag half, `rootGapIndex` | `frontend/src/ui/layers/` |
| `app.screenSpaceCount`, `setScreenSpaceBoundary` | `app.svelte.ts:333, 975-990` |

## LOC estimate

Lines added / removed (not touched):

- **Production:** ~+430 / ~-400. Added: `divider.rs` (~110), `Layer::Divider`
  arms (~60), registration flags across six kind files (~35), document queries
  + move validation + load normalization (~140), engine gates (~50),
  `LayerInfo::Divider` + frontend divider-row handling (~35). Removed: the
  deletion table above (~-330 Rust, ~-70 frontend).
- **Tests:** ~+350 / ~-260. New divider/regression suites (~+300), dropTarget
  additions (~+50); removed count-specific tests including
  `spaceDivider.test.ts` (~-260); the ~50 `set_screen_space_boundary` call
  sites are modified in place via the helper (roughly net-zero).
- **Generated/docs:** ~±150 (`protocol_gen.ts`, `layer_kinds/mod.rs`,
  this plan).

Net production code shrinks or holds even — the point of the redesign.

## Risks

- **`Layer` enum ripple.** A new variant touches ~15 exhaustive matches; all
  are mechanical (`None` / `{}` / `false` arms), and clippy `-D warnings`
  catches them. The real risk is the *non-exhaustive* `if let` / `matches!`
  consumers clippy cannot flag (clipboard, floating, painting, merge, flatten,
  pixel_transform, undo) — mitigated by the explicit grep sweep in step 1 with
  a recorded per-site verdict, plus the feature tests exercising every gated
  op.
- **Test churn.** ~50 boundary-setting call sites rewritten through one helper;
  a subtle off-by-one in the helper's count→target translation would
  green-wash. The helper gets its own assertion (run contents after placement).
- **Load normalization** is the one place the invariant is established by hand
  (same status the old clamp had); the round-trip and corrupt-manifest tests
  pin it.
- **UX regression:** divider drag loses the live eligibility clamp (refusal
  toast instead). Flagged above; reversible additively.
- **Anchor default change** (all three `resolve_anchor_target` fallbacks —
  no-anchor, hostless-filter, stale-id — collapse to one shared
  `Before(divider)` expression): every add path funnels through it; the
  existing `add_raster_layer_no_anchor_lands_at_root_top` test updates to
  "lands at top of canvas space", a stale-anchor case is added, and
  paste/floating tests cover the rest. The one deliberate behavior change
  (anchorless effect adds no longer grow the run) is stated in the design and
  pinned by the new tests.

## Implementation notes

Implemented as planned, with the review's findings folded in. Material
deltas discovered during implementation:

- **`Document::place_at_slot`** was added alongside `reinsert_entity`:
  `group_layers` *derives* the new group's slot from the topmost source's old
  index rather than restoring a recorded one, so it must go through the
  placement policy (a group of canvas content redirected below the divider, a
  group of effects landing verbatim on the screen side). Undo restores stay
  verbatim through `reinsert_entity`.
- **`Document::node_count` counts deletable nodes only.** Its sole consumers
  are the two "cannot delete the last layer" floors, and the divider must not
  satisfy them.
- **`add_mask` returns `Result<(), String>`** — the loud viewport-space
  refusal; soft cases (locked, unknown, already-masked) stay `Ok` no-ops.
- **The count-era test vocabulary survives as
  `DarklyEngine::test_set_screen_space_boundary(count)`** (testing-gated),
  expressed as the divider move it now is, asserting legality and landing.
- **`engine/load.rs`'s hand-built-manifest unit test used bare small integers
  as ids**, which only ever resolved by slot-index coincidence (manifest ids
  are `to_ffi` outputs with generation bits; `from_ffi` normalizes the version
  to 1, so `from_ffi(2)` aliased slot 2 — previously the first content node,
  now the divider). The test now uses well-formed ffi ids; real save files were
  never affected.
- **The demo recipe** (`freshDocument.ts`) seeds its four effects and then
  moves the divider below the bottom-most one via the ordinary `moveLayer`
  call — the wire method it used died with the count.

**Grep-sweep verdicts (step 1's non-exhaustive `Layer` matches).** Every site
is a *positive* match on a specific variant doing variant-specific work; the
divider falls through to the correct "not that kind" behavior in all of them:
`engine/clipboard.rs:487,653` (non-raster → no copyable pixels),
`engine/merge.rs:104,271` / `engine/flatten.rs:47,203` /
`engine/duplicate.rs` ×5 / `engine/floating.rs:259` (positive matches on
freshly created result ids of known kinds — the divider cannot be one),
`undo/property.rs` and `undo/mod.rs:395-428` (per-property and raster-pixel
paths the divider has no state for), `engine/layers.rs:457,845,892,918,1119`
(void/filter/vector param setters), `gpu/compositor.rs:342` (`Filter` takes
the effect arm; the divider takes the blend path, where `compose_layer_arm`
early-returns on its missing cache entry), `document/pixel_transform.rs`
(unreachable — `transform_capability` answers `None`).

One pre-existing flake observed while running the gate, unrelated to this
change: `format::tests::embedded_font_renders_identically_after_reload` failed
once mid-suite and passes in isolation and on rerun.

## Unresolved questions

All three were put to the independent review, which concurred with each
proposal; they stand as decisions unless the user objects at approval.

1. On load of a hand-edited file with an ineligible node above the divider:
   **degrade** (relink divider above the qualifying suffix), not refuse —
   matches the rationale of today's load clamp (`engine/load.rs:367-372`);
   refusing to open user data would be worse than the count design's failure
   mode. Nested or duplicate dividers remain `CorruptManifest` — structural
   corruption, not a user-expressible state.
2. `move_layers` keeps erroring on `Before(self)`/`After(self)` — a no-op
   contract change is not needed for these bugs (the frontend suppresses
   self-drops, and the divider row makes the legitimate gesture expressible)
   and would be scope creep.
3. The divider row keeps the current divider visuals (rule + "viewport" label)
   with only the drag semantics changed; it stays unselectable (excluded from
   `ids`/`order`) and renders no controls. Pure styling.
