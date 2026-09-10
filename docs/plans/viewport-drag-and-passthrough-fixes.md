# Viewport-space drag placement and group passthrough fixes

Status: **implemented**, with one material deviation, recorded here per workflow.

**Implementation note — §3 (bug 2) superseded by `layer-panel-drop-depth`.** Between approval and implementation, commits `ac9edf50` and `d70bd620` landed the `layer-panel-drop-depth` design: one pure gap-based resolver (`frontend/src/ui/layers/dropTarget.ts`) consumed by a single shared `use:layerDropTarget` action (`dropTarget.svelte.ts`) across LayerItem, LayerGroup, SpaceDivider and the panel's empty area. That is exactly the outcome §3.4's `dropZones.ts` was scoped toward (anticipated in Q4), and it fixes the reported gesture structurally: the below band of an expanded group header addresses the gap between the header and its first child, whose depth range pins the drop to `into_top` — no sibling-below misread is expressible. §3.4 was therefore not implemented; the delivered change is two pin tests in `__tests__/dropTarget.test.ts` (expanded header below-band → `into_top`; collapsed header below-band → `before group`). §2 (bug 1, plus the paste split and `restore_to_screen_space` grow rule) and §4 (bug 3) were implemented as planned.

## Independent Review

Independently re-investigated the working tree, ran the repro suite, and read every cited file in `gimp/` and `krita/`.

**Verified.**

- Repro run reproduces exactly as claimed: 4 passed, 2 failed (`dragging_an_empty_group_into_the_run_lands_where_dropped` got `[group, e1, e2]` vs wanted `[e1, e2, group]`; `unchecking_passthrough_on_a_run_group_keeps_effects_running` got `[]`).
- Bug 1 mechanism confirmed: `Document::move_layer` (`crates/darkly/src/document/mod.rs:1149-1174`) funnels through `attach_at_target` → `link` (`:1280-1293`) → `enforce_boundary_on_insert` (`:1319-1365`), whose `qualifies && yields_screen_space_effects` gate (`:1359`) floor-redirects an eligible-but-yieldless group; `set_screen_space_members` (`:1183-1191`) then sweeps the redirected position. The doc comment at `:1311-1314` does state the move/add distinction the code fails to implement, and `reinsert_entity`'s count save/restore (`:1219-1221`) is the workaround the plan says it is.
- Bug 2 mechanism confirmed: `LayerGroup.svelte:285-291` (quarters) + `:314-316` (`below → before`); `LayerItem.svelte:370-372` recomputes ratio at drop while `LayerGroup.svelte:304` uses the stored zone — the divergence is real. The `.drop-below` indicator (`LayerGroup.svelte` style block) draws at the header's bottom edge as claimed.
- Bug 3 mechanism confirmed: `screen_space_blocker`'s `!g.passthrough` clause (`crates/darkly/src/layer.rs:777-780`) shrinks `qualifying_screen_space_suffix`, and both compositor consumers are passthrough-agnostic flat-list walks (`compositor.rs` `present_and_screen_run` consumes `doc.screen_space_effects()`; effect-space tagging builds `screen_run` from the same list). Deleting the clause is the simplest general fix; `set_group_passthrough` (`engine/layers.rs:1715-1746`) indeed needs no change.
- All prior-art citations check out at the cited files: GIMP `gimpcontainertreeview-dnd.c` expanded-group branch has no sibling-below zone; `gimpitemtreeview.c` resolves `INTO_OR_AFTER` on a node with children to `parent = dest, index = 0`; `gimpgrouplayer.c` `get_effective_mode` strength-reduces without touching the stored mode; `gimplayer.c` duplicate converts PASS_THROUGH→NORMAL only at the group→layer boundary. Krita `NodeView.cpp:96-98`, `kis_node_model.cpp` `dropMimeData`, `kis_group_layer.cc` `setPassThroughMode`/`extent`/`exactBounds`, and `LayerBox.cpp` `compositeSelectionActive` all match the plan's characterization.

**Material finding — the paste paths break under verbatim moves (must revise).**

The §2.2 caller table classifies "paste" under the anchor-resolved policy row, citing only the document-level `add_*` helpers. But the engine's actual paste flows place a freshly added raster via `Document::move_layer`:

- `crates/darkly/src/engine/clipboard.rs:507-508` (`paste_image`): `resolve_anchor_target(active_layer_id)` → `doc.move_layer(id, target)`.
- `crates/darkly/src/engine/floating.rs:274-275` (`paste_image_floating`): same pattern.

Neither goes through `check_screen_space_move` (that guard lives only on the engine's move handlers). Today, when the active layer is a run effect, `enforce_boundary_on_insert` redirects the pasted raster to the canvas floor and the run is preserved. Under the plan's verbatim `move_layer`, the raster lands verbatim inside the run region (or *inside a run group*, when the anchor is an effect nested in one — `resolve_anchor_target` returns `After(effect)` with the group as parent), and `set_screen_space_members`/the read clamp silently degrade the run: every effect at or below the paste position drops to canvas space, a user-visible render change. This is a concrete regression of the exact "silent misplacement" class this plan exists to fix. Required revision: these two call sites are semantically adds and must use the placement-policy path — e.g. a small public `Document::place_layer(id, target)` (= unlink + `resolve_target_slot` + `link_placed`) consumed by both, keeping `move_layer` purely for user moves — plus a guard test (paste with a run-member anchor preserves the run; passes today, would fail under the plan as written). Roughly +10 production LOC over the estimate.

Related, acceptable: `duplicate.rs:84,355,360` targets are derived from the source's own position, so verbatim moves stay legal there; the plan's claim that duplicating an empty run group is a second instance of bug 1 fixed by the relocation is correct. `clone_subtree_into_group`'s `IntoGroupBottom(dest_parent)` (`duplicate.rs:355`) mirrors source legality. On Q1: after the `place_layer` split lands, a `debug_assert!` on move legality in `Document::move_layer` becomes cheap and reasonable — recommended.

**Minor findings.**

- GIMP's expanded-group split is at the row's *half* (`y >= cell_area.y + height/2` → `INTO_OR_AFTER`), not a quarter. The plan keeps Darkly's `< 0.25 → above` while citing GIMP for the rule; the load-bearing part (no below-zone) is GIMP-faithful, but the plan should note this divergence explicitly the way it does for the collapsed thirds-vs-quarters case. Consider matching GIMP's 0.5 — a larger `above` zone makes the surviving sibling-above gesture easier to hit.
- `GroupProperties.svelte` lives at `frontend/src/ui/properties/GroupProperties.svelte`, not under `ui/layers/` as Q2's bare filename suggests.
- Bug 3's doc-comment rewrite should state explicitly that an isolated group's opacity and blend mode are likewise discarded above the divider (the flattened path never reads them) — same rationale already documented for passthrough groups at `document/mod.rs:545-546` — and Q2's "inert control" note extends to the group opacity/blend controls, not just the passthrough checkbox. No code change needed; consistency of the written rule.
- The `dropZones.test.ts` "fails now (resolver doesn't exist)" framing is not a regression test in the CLAUDE.md sense; the jsdom component test asserting `into_top` vs today's `before` is the real one, and the plan does include it (precedent confirmed: `frontend/src/ui/layers/__tests__/maskChain.component.test.ts` exists).
- Cited test line ranges in `tests/effect_space.rs` are accurate (`a_group_is_eligible_exactly_when_its_contents_are` at `:316`, `a_freshly_created_group_is_never_swept_into_the_run` at `:1019`).

**Architecture assessment.** The `link`/`link_placed` split places each placement authority with its owner and deletes a workaround; it matches the already-documented intent and is net-negative in conceptual complexity. The bug 3 fix is a pure deletion backed by strong prior art. The bug 2 resolver is the right DRY consolidation and structurally (not type-) branched. Scope cuts (no `layerDropTarget` action, no empty-area drop) are correctly drawn. The one gap is the paste-path misclassification above, which the plan's own safety argument ("every current direct caller carries legal targets", §2.3) gets wrong.

Verdict: revise

All `crates/darkly` line numbers refer to the current working tree **including the uncommitted in-flight changes** in `crates/darkly/src/document/mod.rs`, `crates/darkly/src/layer.rs`, `crates/darkly/src/engine/layers.rs` and `crates/darkly/tests/effect_space.rs` (empty groups made screen-space eligible; the `yields_screen_space_effects` gate in `enforce_boundary_on_insert`). This plan builds on those changes; they must land in the same pass.

---

## 1. The reports

Three bugs around the layer panel's viewport-space (screen-space) boundary and drag-and-drop:

> 1. "when dragging a group into viewport space, instead of landing where I drag it, it goes to the bottom of viewport space. wtf?" — flagged as a code smell implying viewport-space and canvas-space logic did not stay DRY.
> 2. "the same thing happens with effect layers, when dragging them into a group. they don't properly land where I drop them. instead, they seem to go to the bottom of the group."
> 3. "groups in viewport space have a bug, where when passthrough mode is unchecked, contained effects don't work. In viewport space, passthrough should always be assumed true in groups regardless of their setting, and without clearing it, in case the user wants to move it back and forth."

Repro tests exist in the uncommitted `crates/darkly/tests/repro_drag.rs`. Run:

```
cargo test -p darkly --test repro_drag --features testing -- --test-threads=1
```

Current result (verified): **4 passed, 2 failed.**

- `dragging_an_empty_group_into_the_run_lands_where_dropped` **FAILS** — got `[group, e1, e2]` (group at the run's bottom), wanted `[e1, e2, group]`. Bug 1's mechanism.
- `unchecking_passthrough_on_a_run_group_keeps_effects_running` **FAILS** — run degrades to `[]`. Bug 3.
- `dragging_a_group_into_the_run_lands_where_dropped`, `dragging_an_effect_into_a_run_group_lands_where_dropped`, `dropping_onto_a_run_group_header_lands_on_top`, `before_group_is_a_sibling_below_the_whole_group` **PASS** — they bound the problem: the engine's `Before`/`After`/`IntoGroupTop` handling is correct for nodes that yield effects, so bug 2 is not an engine bug.

---

## 2. Bug 1 — a moved yield-less group is redirected to the run floor

### 2.1 Mechanism (verified against the failing test)

`Document::move_layer` (`crates/darkly/src/document/mod.rs:1149-1174`) already encodes the move's divider intent completely: it snapshots the run membership set, adds the moved node iff the target's reference is a member (`:1161-1163` — "drop this next to that already says it"), attaches, then re-derives the boundary from that set via `set_screen_space_members` (`:1183-1191`).

But the attach runs through `link` (`:1280-1293`) → `enforce_boundary_on_insert` (`:1319-1365`), which applies **new-node placement policy** to the move. For a child that qualifies for screen space but yields no effects — an empty group, made eligible by the in-flight change to `LayerNode::supports_screen_space` (`crates/darkly/src/layer.rs:733-741`) — the `qualifies && self.yields_screen_space_effects(child)` gate (`document/mod.rs:1359`) fails, and the insert is redirected to `Some(floor)` (`:1363`) — the run's bottom slot. `set_screen_space_members` then runs against the *redirected* position: the group sits at the floor, is in the members set, everything above it is too, so the suffix sweeps it in **as the bottom run member**. The user's drop above `e2` lands at the bottom of viewport space.

Groups that do yield effects take the other branch (`:1359-1361`, position honored) — which is why the non-empty repro test passes and the empty one fails.

### 2.2 Root cause — placement policy applied to callers that state exact placement

`enforce_boundary_on_insert`'s own doc comment draws the correct line and then the code crosses it: *"Landing a new layer is not a statement about placement, so it is placed as close as the rules allow; a **move** is such a statement, and the engine refuses those outright rather than quietly putting the node somewhere else"* (`document/mod.rs:1311-1314`). Yet `move_layer` reaches the redirect through `link` (`:1316-1318` claims every path does). The same applies to `reinsert_entity` (`:1206-1225`), which restores a recorded `TreeSlot` — and already *fights* the policy today, saving and restoring `screen_space_count` around `link` (`:1219-1221`) to undo the count guess. It cannot undo the **position** redirect: redoing a move of an empty group into the run replays the same misplacement (latent, same shape as bug 1).

This is the non-DRY the user smelled, made precise: three callers with three different placement authorities all funnel through one function that only implements one of them.

| Caller | Placement authority | What it wants from insertion |
|---|---|---|
| `add_*` / group creation (anchor-resolved, `document/mod.rs:909,923,958,987,1068`) | none — "nearest legal slot" | the boundary policy, exactly as written |
| Engine paste (`engine/clipboard.rs:507-508`, `engine/floating.rs:274-275`) — **an add that currently borrows `move_layer` for placement** | none — "nearest legal slot" | the boundary policy; today it gets it only incidentally, via the gate this plan removes from moves (review finding) |
| `Document::move_layer` (`:1149`) — user moves, refused up front by `check_screen_space_move` | the `MoveTarget` + membership derived from its reference | verbatim attach |
| `reinsert_entity` (undo/redo, `:1206`) | the recorded `TreeSlot` (position + `screen_space` side) | verbatim attach |

### 2.3 Fix — verbatim attach for explicit-placement callers; policy stays where it belongs

Split the two concerns; no new machinery, the policy moves to its owner:

1. Extract the `MoveTarget` → `(parent, Option<position>)` resolution out of `attach_at_target` (`document/mod.rs:1416-1452`) into a private `fn resolve_target_slot(&self, target: MoveTarget) -> (LayerId, Option<usize>)`.
2. Make `link` a **verbatim structural attach** (drop its `enforce_boundary_on_insert` call). Add `fn link_placed(...)` = `enforce_boundary_on_insert` + `link`. `attach_at_target` (used only by the add/paste/group paths after this change) = `resolve_target_slot` + `link_placed`.
3. `Document::move_layer` uses `resolve_target_slot` + verbatim `link`. Its membership derivation (`set_screen_space_members`) is unchanged and now operates on the position the user asked for. Add a `debug_assert!` that the move is legal (`screen_space_move_blocker` is `None`) so a future direct caller that skips the engine's refusal check fails loudly in debug builds instead of silently clamp-degrading (review recommendation on Q1).
4. **Add `Document::place_layer(id, target)`** — `resolve_target_slot` + `link_placed` on an already-linked node (unlink first, like `move_layer`) — and route the two engine paste sites through it: `paste_image` (`crates/darkly/src/engine/clipboard.rs:507-508`) and `paste_image_floating` (`crates/darkly/src/engine/floating.rs:274-275`). These are semantically **adds** (anchor-resolved target, no user placement statement, no `check_screen_space_move`); today they get correct behavior only because the boundary policy inside `link` redirects them. Under verbatim `move_layer` they would land a pasted raster inside the run region and silently degrade the run — the review's material finding.
5. `reinsert_entity` uses verbatim `link`; **delete** the `before`/restore count dance (`:1219-1221`) — it existed solely to undo the policy's count guess. `restore_to_screen_space` (`:1232-1242`) stays.
6. Rewrite the "This is the only place the rule is written" paragraph (`:1311-1318`): the placement policy is the add/paste rule (now spelled `link_placed` / `place_layer`); moves are refused up front by `DarklyEngine::check_screen_space_move` (`crates/darkly/src/engine/layers.rs:1284-1306`) and carry membership via their reference; undo restores carry it via `TreeSlot.screen_space`; and the read clamp in `screen_space_run` (`document/mod.rs:522-537`) remains the invariant's backstop for anything insertion cannot see (its already-documented role).

Safety argument for verbatim moves: a direct `Document::move_layer` that lands a non-qualifying node in the run region cannot corrupt the invariant — `set_screen_space_members` clamps to `qualifying_screen_space_suffix` (`:1190`), and the read path clamps again (`:531-537`), degrading to "fewer things are viewport-only", exactly the philosophy documented at `:492-495` and `:525-530`. Engine-level moves are refused before reaching the document (`engine/layers.rs:1269`, `:1347-1349`), and the new `debug_assert!` makes that contract explicit. Remaining direct `doc.move_layer` callers after the paste split: `duplicate_node` (`crates/darkly/src/engine/duplicate.rs:84,355,360`), whose targets are derived from the source's own (legal) position — a duplicate of a run member derives membership from its source reference and keeps working, and a duplicate of an empty **run** group now lands beside its source instead of being floor-redirected (a second instance of bug 1 fixed by the same relocation).

The in-flight `yields_screen_space_effects` gate (`document/mod.rs:563-575`, `:1354-1361`) is untouched and now applies only to the add path, which is the concern it was written for (`a_freshly_created_group_is_never_swept_into_the_run`, `tests/effect_space.rs:1016-1042`, keeps passing).

---

## 3. Bug 2 — the expanded group header's lower quarter drops *below the whole group*

### 3.1 Mechanism (frontend; engine exonerated by passing repro tests)

`LayerGroup.svelte`'s `onDragOver` (`frontend/src/ui/layers/LayerGroup.svelte:277-292`) splits the header row into quarters: `<25%` → `above`, `>75%` → `below`, middle → `into`. `onDrop` (`:301-329`, mapping at `:314-316`) sends `above → after`, `below → before`, `into → into_top`.

For a **collapsed** group that is right: the row below the header is the next sibling, and `before group` lands there. For an **expanded** group, the row directly below the header is the group's first (panel-top) child — the `.drop-below` indicator drawn at the header's bottom edge (`:481-490`) visually points at the **top slot inside the group** — but the issued `before group` is a sibling slot below the *entire* group block (`Document::attach_at_target`, `document/mod.rs:1418-1429`; pinned by the passing repro test `before_group_is_a_sibling_below_the_whole_group`). The dropped effect renders directly under the group's last child, which reads as "it went to the bottom of the group".

### 3.2 The DRY debt underneath it

`LayerItem.svelte` and `LayerGroup.svelte` carry near-identical `onDragStart`/`onDragOver`/`onDragLeave`/`onDrop` blocks (`LayerItem.svelte:321-385`, `LayerGroup.svelte:263-329`), already diverged in two ways: LayerItem **recomputes** the ratio at drop time (`LayerItem.svelte:370-372`) while LayerGroup uses the stored `dropPos` (`LayerGroup.svelte:304`), and only LayerGroup knows about `into`. Two copies of the zone-to-target mapping is exactly how one of them got the expanded case wrong without the other noticing. (The draft plan `docs/plans/layer-panel-drop-depth.md` §2.5/§4.1 independently identified both divergences.)

### 3.3 Prior art

**GIMP** — `gimp_container_tree_view_drop_status()`, `gimp/app/widgets/gimpcontainertreeview-dnd.c:264-289` (read and verified):

- Leaf rows: Y-midpoint, `BEFORE`/`AFTER` (`:283-289`).
- Group row, **expanded**: **two zones only** — top half `BEFORE`, bottom half `GTK_TREE_VIEW_DROP_INTO_OR_AFTER` (`:266-272`). An expanded group header has **no sibling-below zone at all**.
- Group row, collapsed: thirds — `BEFORE` / `INTO_OR_AFTER` / `AFTER` (`:273-281`).

`INTO_OR_AFTER` on a node with children resolves to `parent = the group, index = 0` — the group's **top** — in `gimp_item_tree_view_get_drop_index()`, `gimp/app/widgets/gimpitemtreeview.c:1075-1083`. That is precisely Darkly's `IntoGroupTop`.

**Krita** — delegates zone semantics wholesale to Qt: `plugins/dockers/layerdocker/NodeView.cpp:96-98` (`setDragDropMode(QAbstractItemView::DragDrop)`, `setDropIndicatorShown(true)`), every handler a pass-through to `QTreeView`; `KisNodeModel::dropMimeData` (`krita/libs/ui/kis_node_model.cpp:822-849`) consumes only Qt's resolved `(row, parent)`. No hand-rolled zone logic exists to copy, which itself is a data point: one resolver, zero duplicates.

### 3.4 Fix — one zone resolver, GIMP's expanded-group rule

New pure module `frontend/src/ui/layers/dropZones.ts` (no DOM, no `app` — Vitest-node testable), consumed by both components:

```ts
export type DropZone = 'above' | 'below' | 'into';
export interface DropRowShape { isGroup: boolean; expanded: boolean; hasChildren: boolean; }

/** Which zone a pointer at `ratio` (0 = row top, 1 = row bottom) addresses. */
export function zoneAt(ratio: number, row: DropRowShape): DropZone;

/** The wire target a zone resolves to, relative to the row's node id. */
export function zoneTarget(zone: DropZone, id: number): MoveTarget; // 'above'→after, 'below'→before, 'into'→into_top
```

Zone rules in `zoneAt`:

- leaf: `< 0.5` → `above`, else `below` (unchanged);
- group, **expanded with children**: `< 0.5` → `above`, else `into` — **no `below` zone**, per GIMP `gimpcontainertreeview-dnd.c:266-272`, and at GIMP's half split (`y >= height/2` → `INTO_OR_AFTER`; review corrected the draft's 0.25, and the larger `above` zone makes the surviving sibling-above gesture easier to hit); the `.drop-into` outline becomes the affordance, honestly showing "this lands inside";
- group, collapsed **or empty**: today's `25 / 50 / 25` → `above` / `into` / `below` (kept; GIMP uses thirds — a cosmetic difference not worth churning, and GIMP's expanded-vs-not distinction is the load-bearing part: an empty group is never "expanded" in GTK terms, so the sibling-below zone survives exactly where it is meaningful).

Both components: `onDragOver` stores `dropPos = zoneAt(...)`; `onDrop` maps the **stored** zone through `zoneTarget` (also erasing LayerItem's recompute divergence, §3.2). The DnD lifecycle (`dragstart` payload, `dragleave`, toasts) stays in the components — thin and identical in shape.

Losing the expanded header's below-zone is not losing the gesture: "below the whole expanded group" is addressable at the next row's top half (`After(next)`), the same place GIMP sends users. When the group is the last row there is no affordance — the pre-existing gap `docs/plans/layer-panel-drop-depth.md` §2.3/§5.1 documents (empty-area drop), out of scope here and unchanged by this fix.

**Deliberate scope cut:** `layer-panel-drop-depth.md` §4.2 designs a full `use:layerDropTarget` action absorbing the whole DnD lifecycle plus an X-resolved depth gesture. This plan extracts only the pure zone resolver — the piece both bugs 2 and that plan need — shaped so that plan's action can consume it later. Implementing the full action here would fork an in-flight design.

---

## 4. Bug 3 — unchecking passthrough on a run group silently drops the run

### 4.1 Mechanism (verified against the failing test)

`LayerNode::screen_space_blocker` disqualifies any non-passthrough group: `if !g.passthrough { return Some((self.id(), "is an isolated group")) }` (`crates/darkly/src/layer.rs:777-780`). Unchecking passthrough on a run group therefore shrinks `qualifying_screen_space_suffix` (`document/mod.rs:511-520`), the read clamp in `screen_space_run` (`:531-537`) silently degrades the run, and the group's effects vanish from the present chain (`Compositor::present_and_screen_run` consumes `doc.screen_space_effects()`, `crates/darkly/src/gpu/compositor.rs:3766-3770`). The original feature plan chose this clamp knowingly on recoverability grounds (`docs/plans/viewport-space-effect-groups.md` §"loud/silent line", table row "`set_group_passthrough(false)` … Silent clamp"); the user's report supersedes that choice with better semantics.

### 4.2 Requested semantics, and why they are architecturally free

Above the divider, a group composites nothing of its own **by construction**: the run is consumed flattened. `Document::screen_space_effects` (`document/mod.rs:555-561`) → `collect_screen_space_effects` (`:577-587`) recurses through `LayerNode::Group` **without consulting `passthrough`**, and both compositor consumers work off that flat list (`compositor.rs:3766`, `:4762-4776` — effect-space tagging). There is no accumulator, no mask projection, no opacity/blend application anywhere in the screen-space path for a group to be "isolated" *with*. The stored flag is simply **irrelevant above the divider** — so the fix is to stop letting it disqualify, not to clear it.

### 4.3 Fix

Delete the `!g.passthrough` clause from `screen_space_blocker` (`layer.rs:777-780`, −4 lines). Update the doc comments that state the old rule: `screen_space_blocker`/`supports_screen_space` (`layer.rs:721-759`), `screen_space_effects` (`document/mod.rs:539-554` — "A group above the divider **is treated as** passthrough… a group's stored passthrough flag is meaningful only in canvas space and travels with it across the divider"), and the clamp's example list (`document/mod.rs:525-530`). Per review: the rewritten rule must also state that an isolated group's **opacity and blend mode** are likewise discarded above the divider — the flattened path never reads them, the same rationale already documented for passthrough groups at `document/mod.rs:545-546`.

What this yields, all consistent with the request:

- Unchecking passthrough on a run group changes nothing above the divider; the flag round-trips (it is ordinary serialized group state) and re-applies isolation the moment the group moves below the divider. Nothing is cleared.
- An isolated group of effects may now be dragged **into** the run (`check_screen_space_move` no longer refuses it, `engine/layers.rs:1284-1306`) and is rendered flattened there — "regardless of their setting".
- `DarklyEngine::set_group_passthrough` (`engine/layers.rs:1715-1746`) needs no change: it records undo, flips the flag, and pre-warms canvas GPU group state — all still correct for the group's eventual canvas life.
- The compositor needs **no change**: run members never enter the canvas walk (`canvas_space_children`, `document/mod.rs:596-602`), and the screen path was already passthrough-agnostic.

### 4.4 Prior art

**GIMP — stored mode vs. derived effective mode.** `gimp_group_layer_get_effective_mode()` (`gimp/app/core/gimpgrouplayer.c:1270-1309+`) "strength-reduces" a stored `GIMP_LAYER_MODE_PASS_THROUGH` to a normal-group effective mode when context makes the distinction unobservable — the stored mode (`gimp_layer_get_mode`) is never modified; every render-relevant consumer asks for the *effective* mode (`gimp_group_layer_effective_mode_changed`, `:1176-1214`). This is exactly the shape of the fix: the flag is user intent, the context decides what it means, and the two are never conflated in storage. GIMP also treats the flag as contextually invalid rather than clearing it eagerly: `gimp_layer_duplicate` converts `PASS_THROUGH` to `NORMAL` only at the moment a group render becomes a plain layer, deliberately *after* using the pass-through render as-is (`gimp/app/core/gimplayer.c:941-967`).

**Krita — inapplicable settings are ignored/disabled at read sites, retained in the model.** `KisGroupLayer::setPassThroughMode` stores a plain flag (`krita/libs/image/kis_group_layer.cc:325-336`); consumers branch on it at each read site (`extent`/`exactBounds`, `:426-437`) rather than the model enforcing structural validity. And when a setting is meaningless for the current node, Krita's layer docker **disables the control without clearing the value**: the composite-op combo for pass-through groups, `plugins/dockers/layerdocker/LayerBox.cpp:724-727` (`compositeSelectionActive = !(group && group->passThroughMode())`). Precedent for the optional UI note in §7-Q2.

---

## 5. Architectural impact

- **Document authority** — strengthened. The divider's placement authority is stated once per path: adds get the placement policy (`link_placed`), moves get their reference-derived membership, undo gets its recorded `TreeSlot`. The read clamp remains the single invariant backstop. No new state anywhere.
- **DRY** — net removal on both sides. Rust: `reinsert_entity`'s count save/restore workaround is deleted; the policy stops being applied-then-overridden. Frontend: one zone resolver replaces two diverged copies of the mapping.
- **Type-owned dispatch** — `screen_space_blocker` stays the one home of eligibility (`layer.rs:743-786`); it loses a clause rather than growing a context branch. No consumer starts asking groups what they are.
- **Modularity** — `dropZones.ts` branches on structural row shape (`isGroup`/`expanded`/`hasChildren`), not on layer `type`; new layer kinds change nothing.
- **Wire/protocol** — untouched. `MoveTarget` and `LayerTree` are unchanged; no `protocol_gen.ts` regeneration.
- **Compositor** — untouched by all three fixes.

## 6. Implementation steps

1. **Regression tests first** (see §8): fold `repro_drag.rs` into `tests/effect_space.rs`, add the new cases, confirm the failures reproduce; write the frontend zone tests, confirm the expanded-group case fails.
2. Bug 3 (smallest): delete the passthrough clause in `layer.rs:777-780`; update doc comments (`layer.rs`, `document/mod.rs:525-554`); rewrite the isolated-group section of `a_group_is_eligible_exactly_when_its_contents_are` (`tests/effect_space.rs:314-358`) to the new semantics.
3. Bug 1: extract `resolve_target_slot`; make `link` verbatim; add `link_placed` and `Document::place_layer`; route `attach_at_target` (adds) through `link_placed`, the two engine paste sites (`clipboard.rs:507-508`, `floating.rs:274-275`) through `place_layer`, and `move_layer` + `reinsert_entity` through verbatim `link`; add the move-legality `debug_assert!` in `move_layer`; delete the count save/restore in `reinsert_entity`; rewrite the affected comments (`document/mod.rs:1295-1318`, `:1206-1225`).
4. Bug 2: add `frontend/src/ui/layers/dropZones.ts`; rewire `LayerItem.svelte` and `LayerGroup.svelte` `onDragOver`/`onDrop` through it (stored-zone drop in both).
5. Full gates per CLAUDE.md (fmt, clippy ×2, `cargo test … -- --test-threads=1`, wasm-pack, `tsc --noEmit`, `npm run check`, `npm run build`, `npm test`).

## 7. Risks and unresolved questions

- **Q1 — resolved per review.** Verbatim moves shift enforcement to refusal + clamp; the paste paths that relied on the old in-link policy are rerouted through `place_layer` (§2.3 item 4), and `Document::move_layer` gains a `debug_assert!` on move legality so any future policy-skipping caller fails loudly in debug builds. Release behavior remains the documented degrade-don't-corrupt clamp (`document/mod.rs:492-495`).
- **Q2 — Passthrough (and opacity/blend) controls are inert above the divider.** `frontend/src/ui/properties/GroupProperties.svelte:18-27` keeps offering the toggle; it now visibly does nothing while the group is in the run (by design — the user wants to pre-set it for canvas use). The same inertness applies to a run group's opacity and blend-mode controls (the flattened path reads none of them). Option: a hint ("ignored in viewport space") or Krita-style disable (`LayerBox.cpp:724-727`). Default in this plan: no UI change (0 LOC); flagged for user approval.
- **Q3 — Losing the expanded header's below-zone** removes a (broken) gesture some user may have adapted to. GIMP-consistent; "below the group" remains reachable via the next row, except when the group is last (pre-existing empty-area gap, owned by `layer-panel-drop-depth.md`).
- **Q4 — Overlap with `layer-panel-drop-depth.md`.** That draft rewrites these same handlers. This plan's `dropZones.ts` is designed to be absorbed by its `layerDropTarget` action; if that plan ships first, bug 2 reduces to the zone-rule change inside its resolver.
- **Risk — in-flight coupling.** The empty-group eligibility change (uncommitted) is a prerequisite of the bug-1 story; landing this plan without it changes the failing tests' meaning. Land together.
- **Risk — jsdom DnD synthesis** for the component regression test (no native `DragEvent`/`DataTransfer`); mitigation and precedent in `layer-panel-drop-depth.md` §8.1 and `maskChain.component.test.ts`.

## 8. Tests

Rust (all in `tests/effect_space.rs`; delete `tests/repro_drag.rs` after folding; port its `group_children`/`root_rows` helpers):

- **Regression, bug 1** (fails now): `dragging_an_empty_group_into_the_run_lands_where_dropped`.
- **Regression, bug 3** (fails now): `unchecking_passthrough_on_a_run_group_keeps_effects_running` (structural + `test_screen_space_effects`, `engine/mod.rs:1228`).
- New: undo/redo round-trip of the empty-group-into-run move (pins the `reinsert_entity` position fix — redo must land at the recorded slot, and would fail floor-redirected today); an isolated group of effects moves into the run and flattens (new-semantics feature test); a run group with passthrough unchecked, moved below the divider, is isolated again with its flag intact.
- **Guard (review finding): paste with a run-member anchor preserves the run.** Set the active layer to a run effect, `paste_image`, assert the pasted raster lands at the canvas floor and the run is unchanged. Passes today; would fail under verbatim `move_layer` without the `place_layer` reroute — this test is what pins the paste paths to add semantics.
- Kept from `repro_drag.rs` as bounds: the four passing tests (fold; keep `before_group_is_a_sibling_below_the_whole_group` — still reachable via collapsed headers).
- Revised: `a_group_is_eligible_exactly_when_its_contents_are` (`effect_space.rs:314-358`) — the "isolated group cannot be above the line" section inverts.

Frontend:

- `__tests__/dropZones.test.ts` (node env, pure): zone table for leaf / collapsed / empty / expanded rows; `zoneTarget` mapping. **Regression** (fails now, resolver doesn't exist / logic inline says `before`): expanded group, ratio 0.9 → `into` → `into_top`.
- `__tests__/layerDrop.component.test.ts` (jsdom): mount `LayerGroup` expanded with a child; synthesize `dragover` at the lower quarter + `drop`; assert `moveLayers` called with `{target_type: 'into_top'}` (fails today with `'before'`); companion collapsed-group case asserting `'before'` still.

## 9. LOC estimate (added / removed, not touched)

| Area | File | Added | Removed |
|---|---|---:|---:|
| Production | `crates/darkly/src/document/mod.rs` (incl. `place_layer` + debug_assert, per review) | ~38 | ~22 |
| Production | `crates/darkly/src/layer.rs` | ~6 | ~8 |
| Production | `crates/darkly/src/engine/clipboard.rs` + `engine/floating.rs` (paste reroute) | ~2 | ~2 |
| Production | `frontend/src/ui/layers/dropZones.ts` (new) | ~55 | 0 |
| Production | `frontend/src/ui/layers/LayerItem.svelte` | ~5 | ~12 |
| Production | `frontend/src/ui/layers/LayerGroup.svelte` | ~7 | ~16 |
| **Production subtotal** | | **~113** | **~60** |
| Tests | `crates/darkly/tests/effect_space.rs` (fold + new + revised + paste guard) | ~360 | ~20 |
| Tests | `crates/darkly/tests/repro_drag.rs` (deleted) | 0 | ~236 |
| Tests | `frontend …/__tests__/dropZones.test.ts` (new) | ~90 | 0 |
| Tests | `frontend …/__tests__/layerDrop.component.test.ts` (new) | ~110 | 0 |
| **Tests subtotal** | | **~560** | **~256** |
| Generated / docs | this plan; no protocol regen | ~470 | 0 |

**Production net: roughly +53 lines.** The bug-3 fix alone is net-negative in production code.
