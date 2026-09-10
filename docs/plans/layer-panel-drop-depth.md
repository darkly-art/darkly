# Layer panel: cursor-X picks the drop depth (drag a layer out of its group)

## Revision (step 3) — **this section governs**

The independent review returned `revise`. Its findings are accepted in full; this
section records the dispositions and the two changes that alter what gets built.
Where this section and the body disagree, this section wins. The review is
preserved below.

Re-verified independently before revising, because both the draft and the review
have miscited elsewhere:

- `frontend/src/lib/__tests__/clickOutside.test.ts` **does not exist.** The
  directory holds `backdropDismiss.test.ts` and `dismiss.test.ts`; there is no
  `clickOutside` file anywhere under `frontend/src`. The reference is stale in
  *CLAUDE.md itself*, which is where §8.1 inherited it. Cite
  `frontend/src/lib/__tests__/dismiss.test.ts` for the no-DOM fake-event pattern.
- The drop indicators are **component-scoped** styles keyed on compound
  selectors: `.layer-item.drop-above::before` (`LayerItem.svelte:533-542`) and
  `.group-header.drop-into` (`LayerGroup.svelte:492-495`). Replacing the `class:`
  bindings with `classList.toggle` inside an action makes them statically
  unmatchable, so Svelte prunes them. Finding C-2 is real.
- The trailing-divider inversion (finding C-1) is confirmed. `screen_space_run`
  is the *trailing suffix* of `children_of(root)` (`document/mod.rs:531-537`) and
  root children are stored bottom-to-top, so the run is the panel's **top** rows
  — the test helper says as much (`tests/effect_space.rs`: "The response is
  top-first and the run is its prefix"). When `screenSpaceCount >=
  layerTree.length` every root child is in viewport space, the panel's bottom row
  included. §6.3's reasoning is exactly backwards.

### R.1 Ship as two commits, not one

Adopted from finding F. The extraction is where the regression risk lives and it
is independently revertible; the gesture is small once the seam exists.

1. **Extraction, semantics unchanged.** The pure module, the flat `rows`
   projection, the shared Svelte action, both row components converted, and the
   CSS relocated. Depth is *pinned* to today's behaviour, so the panel behaves
   identically and the diff is provably non-functional. ~350 lines.
2. **The depth gesture.** Unpin `k` to the X reading, add the `--drop-indent`
   custom property, wire the divider row and the empty area. The regression test
   goes red→green here. ~200 lines.

### R.2 Tier-1-first is rejected

The §12.1 fallback (empty-area drop alone) is **not** the recommended starting
point and should not be offered as one. It does not answer what the user asked
for — they asked for a *gesture distinction* between "bottom of this group" and
"out of this group", not for one more place that means root — and it is
throwaway work: the empty-area rule falls out of the gap model as
`resolveGapDrop(rows, rows.length, pin: 'min')`, so shipping it standalone means
writing `LayerPanel`'s handler twice.

### R.3 Corrections to the plan body

| Finding | Disposition |
|---|---|
| C-1 §6.3 reasoning inverted | Fixed above. Behaviour is unchanged and correct; the *justification* was backwards, and the case-22 fixture must pin `screenSpaceCount >= rows.length` explicitly rather than relying on a default |
| C-2 Svelte prunes the scoped drop-indicator CSS | **Promoted to a required step in commit 1.** Relocate `.drop-above` / `.drop-below` / `.drop-into` to a shared sheet; this also removes the identical `.drop-above`/`.drop-below` pair duplicated across both components today |
| C-3 band→gap mapping left untestable | Accepted. Export `bandToGap(rowIndex, isGroup, yRatio)` as a pure function (~15 lines) so §3.5's table is tested rather than welded to `DragEvent` |
| D prior-art span drift | Accepted. Krita promote-to-grandparent is `kis_node_juggler_compressed.cpp:422-425`; lower case `:397-400`; auto-enter `:388-392` / `:414-417`; `krita.action:3608-3627`; `kis_node_model.cpp:832-834`. Quoted text was verbatim correct throughout; only spans drift |
| D `dragEnterEvent` is not a pass-through | Accepted, narrow the claim: `NodeView.cpp:500-509` does push mime data into the model. The load-bearing fact survives — `dragMoveEvent` inspects no coordinates |
| E `into` band earns its keep only for **collapsed** groups | Accepted. For an empty *expanded* group the X gesture reaches `IntoGroupTop` unaided. Keep the band, narrow its justification |
| E / §8.1 `clickOutside.test.ts` | Accepted, retarget to `dismiss.test.ts`. Also worth telling the user that CLAUDE.md's own citation is stale |
| E `layers-actions.c` grep | Accepted. The conclusion (two action *entries*) holds; the quoted grep does not reproduce |
| §9 step 10 (`siblingBelowExists` lift) | **Dropped.** Unrelated to this bug and it touches context-menu enablement. Separate pass |
| Not predicting viewport-space refusals | **Upheld, and now positively justified.** `screen_space_eligible` returns `false` for any non-root child (`document/mod.rs:501-507`), so for exactly the nested payloads this feature newly enables the flag is structurally false and carries no information. Mirroring `check_screen_space_move` in TypeScript would be the "keep in sync with X" stop-sign. Let the move fail and toast |

### R.4 Revised LOC estimate

Added / removed, not touched. Step 10 dropped; CSS relocation, `bandToGap`, and
three added test cases budgeted; the test line corrected down from its padded
figure.

| | Production | Tests |
|---|---|---|
| Commit 1 — extraction, semantics unchanged | ~+230 / −150 | ~+120 |
| Commit 2 — the depth gesture | ~+90 / −15 | ~+90 |
| **Combined** | **~+320 / −165** | **~+210** |

Roughly +530 / −165 all in, against the draft's +625 / −148. The user approves
commit 1 (~350 lines) and commit 2 (~200) separately rather than one ~600 block.

---

Status: **draft (step 1)**. No production code has changed.

---

## Independent Review

Reviewed against the repository at `better-veils`. Every file:line citation in the
plan was opened. The diagnosis is correct, the gap model survived a deliberate
attempt to break it, and the prior-art research is real (not recalled) — but the
plan carries one factual error about the viewport divider, one wrong claim about
when the `into` band is load-bearing, an affordance mechanism that Svelte's CSS
pruning will silently break, and several line numbers in its own *corrections*
that are themselves off. Verdict below.

### A. Verified correct

These were re-derived from source, not taken on trust:

- **The root cause.** `LayerItem.svelte:338-347` reduces the pointer to one bit;
  `:370-377` turns it into `{ target_type, target_id: layer.id }`. `layer.id` is
  fixed by DOM hit-testing, and `Document::attach_at_target`
  (`crates/darkly/src/document/mod.rs:1416-1442`) makes `Before`/`After` a
  sibling placement under `parent_of(ref)`. Two degrees of freedom in the
  problem, one in the gesture. Correct and complete.
- **Every Rust citation is exact.** `MoveTarget` `document/mod.rs:62-71`;
  `move_layer` `:1149-1174` (the quoted comment is `:1153-1160`);
  `screen_space_eligible` `:501-507`; `screen_space_run` `:531-537`;
  `canvas_space_children` `:599-602`; `screen_space_move_blocker` `:630-639`;
  `check_screen_space_move` `engine/layers.rs:1284-1306`. `protocol_gen.ts:739`
  is `MoveTarget`. All confirmed to the line.
- **`--depth` is dead.** `grep -rn -- "--depth" frontend/src/` returns exactly
  one hit, the write at `LayerGroup.svelte:333`. Deleting it is correct.
- **Rows are full-width.** `LayerGroup.svelte`'s `<style>` block defines only
  `.group-header`, `.collapse-btn`, `.vis-btn`, `.lock-btn`, `.folder-icon`,
  `.group-name`, `.name-input` — `.layer-group` and `.group-children` carry no
  rules, and `.layer-list` (`LayerPanel.svelte:84-89`) has no padding. So
  `rowRect.left` is the list's left edge at every depth. §3.3's premise holds.
- **Masks are not rows.** `MaskChainControl` renders inline at
  `LayerItem.svelte:444-454` / `LayerGroup.svelte:384-394`. §3.1 correct.
- **The accessibility claim.** `frontend/src/actions/index.ts` declares 60
  action ids (`:228` `undo` through `:948` `addBrushNode`); none reorders a
  layer. `grep` over `frontend/src/actions/` and `crates/darkly/presets/defaults.yaml`
  for `raiseLayer|lowerLayer|moveLayerUp|moveLayerDown|move_layer` finds nothing
  relevant. Drag-and-drop is genuinely the only reorder path. (Nit: the plan
  writes the registry span as `:228-932`; the last id is at `:948` and the file
  is 982 lines.)
- **Vitest is node-env.** `frontend/vite.config.ts` declares no `test` block, so
  the default `environment: 'node'` applies. The per-file
  `// @vitest-environment jsdom` docblock is real —
  `frontend/src/ui/layers/__tests__/maskChain.component.test.ts:1`. The pure
  extraction really is a testability constraint, not a nicety. §4.3's rejection
  of the DOM-measuring alternative is correct on those grounds.
- **§6.1's rejection of `screenSpaceEligible` is right, and for the right
  reason.** `Document::screen_space_eligible` (`document/mod.rs:501-507`) returns
  `false` for anything whose parent is not the root, so for a *nested* node —
  exactly the payload this feature makes newly droppable — the flag is
  structurally `false` and carries no information about renderability. The
  frontend could compute the `in_screen_space_region(target.reference())` half
  (root-ancestor index vs `app.screenSpaceCount`) but not
  `LayerNode::screen_space_blocker` (`crates/darkly/src/layer.rs:760-783`), which
  recurses over descendants. Mirroring it would be the stop-sign. **Confirmed —
  do not predict; toast.**

### B. The gap model: I tried to break it and could not

`maxDepth(g) = prev.depth + (prev.isGroup ? 1 : 0)`, `minDepth(g) = next.depth`
(0 at either end). Counterexamples attempted, all resolving correctly:

| case | result |
|---|---|
| top of list (`g = 0`) | `[0,0]`; `rows[0]` is always a root child at depth 0 — consistent with the special-cased `After(rows[0].id)` |
| bottom of list, last row a depth-2 leaf | `[0,2]`; ancestor scan yields `Before(B)` then `Before(A)` — correct |
| empty **expanded** group as `prev` | `[next.depth, prev.depth+1]` — `IntoGroupTop` reachable by X (see C2) |
| collapsed group as `prev` | `[next.depth, prev.depth+1]` — correct |
| `prev` = expanded non-empty group | `next` is its first child, so `min = max = prev.depth+1`; `Before(prev)` is unreachable, which is what makes §3.4's `k === dPrev` arm safe |
| group header's **top** band above a deeper preceding subtree | range spans, X-resolvable — semantically right, though it silently gives the upper band X-freedom it never had |
| `minDepth <= maxDepth` | holds: `next` is a descendant of `prev` (⇒ `prev` is a group and `next.depth = prev.depth+1 = max`), a sibling, or shallower |
| ancestor lookup (`nearest j < i with rows[j].depth === k`) | exact — every ancestor of a visible row is itself visible, and exists for all `k` in `[0, dPrev)` |
| dragged node still present in `rows` | reported case resolves to `Before(A)` correctly; dragging *a group* and aiming inside itself yields a target the engine already refuses at `layers.rs:1338-1341` |

The `SpaceDivider` does not participate in `rows` (it is not a node row), and its
DOM position — `LayerPanel.svelte:46`, between root child `screenSpaceCount-1`'s
entire subtree and root child `screenSpaceCount` — coincides exactly with the gap
index the plan computes. **The model is sound.** No change needed to §3.1-§3.4.

### C. Factually wrong (must fix before implementing)

**C1 — §6.3 is false when the trailing divider is present.** The plan says:
*"When the trailing divider is present (`LayerPanel.svelte:57`) the empty area is
below the line; the resolved reference is still the bottom root child, which is
in canvas space, so nothing changes."*

The trailing divider renders exactly when `screenSpaceCount >= layerTree.length`
(`LayerPanel.svelte:57`) — i.e. **every** root child is above the line.
`Document::screen_space_run` (`document/mod.rs:531-537`) is the *trailing* suffix
of `children_of(root)`, which is bottom-to-top, so that suffix is the **top** of
the panel; when it covers all children it includes `children_of(root)[0]`, which
is the panel's **bottom** row. So in precisely the case §6.3 names, the bottom
root child is in viewport space, `in_screen_space_region(target.reference())` is
true (`document/mod.rs:635`), and the empty-area drop is subject to refusal. The
behavior is defensible (§6.1 says refuse and toast) but the stated reasoning is
inverted, and the §8.4 case 22 test as written would pass or fail depending on a
fixture detail the plan does not pin. Rewrite §6.3 and pin `screenSpaceCount = 0`
in the test fixture.

**C2 — §3.5 / Q1: the `into` band is redundant for empty *expanded* groups too.**
The plan claims the band "earns its keep only for **collapsed and empty**
groups". For an empty *expanded* group `B` at depth `d`, the below-gap has
`max = d+1` and `min = next.depth <= d`, so the range is not a point and the X
gesture reaches `IntoGroupTop(B)` unaided. Only **collapsed** groups genuinely
need the band. This narrows Q1's justification to one case; the recommendation to
keep the band still stands (a collapsed group's interior has no visible indent
stop to aim at, and the `.drop-into` outline is the universal folder idiom), but
the plan should state the real reason.

**C3 — the affordance mechanism will be silently pruned by Svelte.**
§4.2 specifies `node.classList.toggle('drop-above'|'drop-below'|'drop-into')`
from the action, and step 6/7 delete the `class:drop-above={…}` bindings from the
templates. But `.layer-item.drop-above::before` (`LayerItem.svelte:533-542`),
`.drop-below::after` (`:544-553`), and `.group-header.drop-into`
(`LayerGroup.svelte:492-495`) are **scoped** styles. Svelte 5 prunes selectors it
cannot statically match against the component's own template and emits
`css_unused_selector`; `npm run check` runs `svelte-check` without
`--fail-on-warnings` (`frontend/package.json`), so this passes CI and ships a
drop indicator that never appears. Pick one:
  (a) keep a `$state` `dropPos` per row that the action writes back through a
      callback and keep `class:` bindings — contradicts §4.2's "no `$state`
      round-trip" but is the smallest change;
  (b) wrap the three rules in `:global(...)` inside the component (precedent:
      `LayerGroup.svelte:548`);
  (c) move the indicator rules to `frontend/src/styles/tokens.css` /
      a shared sheet, which also DRYs the two identical copies of
      `.drop-above::before` / `.drop-below::after` that currently exist in
      `LayerItem` and `LayerGroup`.
(c) is the right answer and is a real, unbudgeted line item.

**C4 — `dropTarget.svelte.ts` misuses the repo's file convention.** Every one of
the 45 `*.svelte.ts` files in `frontend/src` is a rune-carrying module (`$state`
/ `$derived`). Svelte *actions* in this repo are plain `.ts`:
`frontend/src/actions/binding_site.ts`, `frontend/src/ui/workspace/pointerDrag.ts`,
`frontend/src/lib/scrubDrag.ts`. The plan's own design says the action holds no
runes. Name it `layerDropTarget.ts` (or fold it into `dropTarget.ts`) — the
`.svelte.ts` suffix would be a lie, and if option (a) above is taken it becomes a
lie in the other direction.

**C5 — `frontend/src/lib/__tests__/clickOutside.test.ts` does not exist.**
§8.1 cites it twice as the pattern for `vi.stubGlobal('window', …)`. The
directory contains `dismiss.test.ts` and `backdropDismiss.test.ts`; there is no
`clickOutside.test.ts`. (CLAUDE.md carries the same stale reference, so this is
inherited rather than invented — but a plan that verifies its citations should
have caught it.) Point at `frontend/src/lib/__tests__/scrubDrag.test.ts` or
`dismiss.test.ts` instead.

### D. Prior art: the corrections were mostly right, but three of them are wrong

Every cited line was opened in `gimp/`, `krita/` and `tldraw/`.

**Upheld (the planner corrected the brief correctly):**

- `gimpcontainertreeview-dnd.c:275-277` **is** the collapsed-group thirds branch,
  not an escape hatch. Verbatim: `:275` `if (y >= (cell_area.y + 2 * (cell_area.height / 3)))`
  → `AFTER` (`:276`), `:277` `else if (y <= (cell_area.y + cell_area.height / 3))`
  → `BEFORE` (`:278`), `else` → `INTO_OR_AFTER` (`:280`). The resulting
  `BEFORE`/`AFTER` resolve to `gimp_viewable_get_parent (dest_viewable)`
  (`:727`) — the group's own parent, sibling placement, no grandparent escape.
  **The planner's correction is right and the brief was wrong.**
- **GIMP never reads `x` after the hit test.** `grep -nE '\bx\b'` over
  `gimpcontainertreeview-dnd.c`: inside `drop_status` the last use is `:245-246`
  (`convert_widget_to_bin_window_coords`, then `get_path_at_pos` with all three
  cell out-params `NULL`); `:248-368` contain zero `x` references, only `y`
  (`:268`, `:275`, `:277`, `:285`, `:296`). Confirmed.
- **Krita has zero `dropIndicatorPosition` hits repo-wide.** `grep -rn` returns
  nothing. Confirmed.
- **tldraw has no tree DnD.** Zero DnD libraries in any `package.json`;
  `ShapeList.tsx` uses `depth` only for `paddingLeft: 10 + depth * 20` (`:58`)
  and has zero `drag|drop` matches; `DefaultPageMenu.tsx` has zero
  `clientX|pageX|offsetX` matches and its slot math is Y-only (`:298-301`).
  Reparenting is geometric containment (`ShapeUtil.onDropShapesOver`,
  `packages/editor/src/lib/editor/shapes/ShapeUtil.ts:748`). Confirmed.
- `gimpcontainertreeview-dnd.c:291-316` blank-area → last top-level row +
  `GTK_TREE_VIEW_DROP_AFTER`; `:318` gates on `dnd_drop_to_empty`, enabled at
  `gimpitemtreeview.c:400`. Confirmed verbatim.
- `gimpimage.c` — the function is `gimp_image_raise_item` (declared `:5265`);
  `:5280-5285` hard-fails at index 0 and `:5287-5289` reorders under
  `gimp_item_get_parent (item)`. Never reparents. Confirmed.

**Miscorrected or overstated (the planner's own numbers are off):**

- **`kis_node_juggler_compressed.cpp:420-425` is wrong — the promote-to-grandparent
  branch is `:422-425`.** The quoted C++ is verbatim correct; the line span is
  off by two. Likewise the symmetric lower case is `:397-400`, not `:396-401`,
  and the auto-enter branches are `:388-392` / `:414-417`, not `:388-393` /
  `:412-417`. This one matters because §5.2 and §7 both present it as a
  *correction* to the brief. Fix all four spans.
- **`krita/krita/krita.action:3618-3628` is wrong.** `move_layer_up` is
  `:3608-3617` (`<shortcut>Ctrl+PgUp</shortcut>` at `:3614`); `move_layer_down`
  is `:3618-3627` (`Ctrl+PgDown` at `:3624`); `:3628` opens `layer_properties`.
  The cited span covers only half the pair. The **shortcuts themselves are
  exactly right.**
- **"Every drag handler is a pass-through to `QTreeView`" is refuted for
  `dragEnterEvent`.** `NodeView.cpp:500-509` pushes the mime data into the model
  on the invalid root index (`model()->setData(QModelIndex(), data, KisNodeModel::DropEnabled)`)
  before chaining. Four of the six also carry a `DRAG_WHILE_DRAG_WORKAROUND_*`
  flag (macros at `:45-50`). The load-bearing claim survives —
  `dragMoveEvent` (`:511-515`) inspects no coordinates at all — but restate it
  as "no handler inspects the pointer's X".
- `kis_node_model.cpp` `rootDummy()` mapping is `:832-834`, not `:830-832`.
- `gimp_item_tree_view_drop_possible` is `:1447-1499`, not `:1447-1497`;
  `gimp_layer_tree_view_drop_possible` is `:705-739`, not `:706-738`. (Both
  coordinate-free, as claimed.)
- **The `layers-actions.c` grep claim is overstated.** `grep -n "group"` yields
  ~50 hits, not two. The *conclusion* is right — the only **action entries**
  containing "group" are `layers-new-group` (`:97`) and `layers-merge-group`
  (`:161`), everything else being `GimpActionGroup *group` API noise and
  `have_groups` sensitivity flags — but the plan states a grep result that does
  not reproduce. Restate as "the only action entries containing 'group' are …".
- `tldraw/packages/editor/src/lib/utils/reparenting.ts:16` is
  `kickoutOccludedShapes`, not `reparentShapes`. The relevant export is
  `getDroppedShapesToNewParents` at `:210`. `Editor.ts:6353` is exact.

None of this changes what the prior art *licenses*. §5.4's conclusion — that
neither reference has an X-depth gesture, that both escape via a row outside the
group, and that the departure needs its own justification — is intact.

### E. Design findings

**E1 — `spaceDivider.ts` is unacknowledged precedent *and* a near-duplicate.**
`frontend/src/ui/layers/spaceDivider.ts` already does exactly what §4.2 proposes:
pure module, split out of the component "because it is the only real logic in it,
and because the bug it exists to prevent is arithmetic" (`:1-12`), with
`gapAt(clientY, rows, maxCount)` (`:28-36`) resolving *which gap the pointer is
in* and a node-env test file (`__tests__/spaceDivider.test.ts`). The plan should
(a) cite it as the in-repo precedent — it is a much stronger argument than
`bindingSite`/`pointerDrag` — and (b) address the fact that after this change the
same directory will hold **two** "which gap is the pointer aiming at" resolvers,
one Y-by-measurement and one Y-by-band. They answer different questions on
different index spaces (root-child count vs `rows` index), so they are probably
not mergeable, but the plan must say so rather than leave a reader to discover
the overlap.

**E2 — the band→gap mapping is left in the untestable layer, which undercuts the
plan's own testability argument.** §4.2 exports `gapDepthRange` and
`resolveGapDrop`, but §3.5's table — *which* band of *which* row kind addresses
gap `i` vs `i+1`, and when `pin` applies — lives inside the action, welded to
`DragEvent` and `getBoundingClientRect`. That is precisely where the
off-by-one-gap bug class lives. Export it too:

```ts
export function bandToGap(rowIndex: number, isGroup: boolean, yRatio: number):
    { gap: number; pin?: 'max' };
```

Then the action is pure DOM plumbing and §3.5 becomes fully node-testable. Cheap
(~15 lines + ~6 cases) and it is the difference between "the math is tested" and
"most of the math is tested".

**E3 — §8.3's cases miss three boundaries I used above.** Add:
(i) empty *expanded* group as `prev` (C2's case);
(ii) the divider's gap-index computation — make "position in `rows` of the root
child at panel index `n`" a pure exported function and test it, rather than only
covering it through jsdom case 23;
(iii) a payload/target ancestry case: dragging a group and X-resolving to a depth
*inside* that group, asserting the drop-time guard or the engine's
`"Cannot move a layer into itself"` (`engine/layers.rs:1338-1341`) fires. The
X gesture makes this materially easier to hit than today's Y-only gesture.

**E4 — `IntoGroupTop` for a collapsed group's below-gap is the wrong end.**
§3.4 maps `k === dPrev + 1` → `IntoGroupTop(prev.id)` unconditionally.
`attach_at_target` (`document/mod.rs:1442-1445`) makes `IntoGroupTop` →
`link(node, group, None)` → appended → panel **top** of the group. For an
expanded group that is exactly right (it coincides with "above `next`", the first
child). For a **collapsed** group the gesture is at the group's bottom edge and
the node lands at its top — invisible at drop time, surprising when the user
expands. `IntoGroupBottom` is the honest reading and is currently **unused by the
entire frontend** (`grep -rn "into_bottom" frontend/src` hits only
`protocol_gen.ts:739`). Either use it for the collapsed case or state why
`IntoGroupTop` is preferred. One line either way; the plan should decide rather
than inherit `LayerGroup.svelte:316`'s existing choice by accident.

**E5 — 16 px is a small horizontal target for a mid-drag gesture.** With
`ROW_INDENT = 16`, picking among three depths spans 48 px total and requires
±8 px precision while the pointer is also being held at a specific Y band. Q2
covers *discoverability* but not *precision*. Neither reference editor has this
gesture, so there is no prior art to lean on for the tolerance. Worth an explicit
note that the indented indicator is the only feedback loop closing this, and
worth considering whether the depth read should use displacement from the drag's
start X rather than absolute X (which would let the stop width be decoupled from
the render indent — at the cost of the two constants no longer being one fact).
Flag, not a blocker.

**E6 — `layerTree.ts`'s "callers never hand-roll another" claim is already
false.** §4.2 quotes `layerTree.ts:56-60` as authority for putting `rows` in the
single walk. That is the right home — but note that `app.svelte.ts:429-440`
(`nodeById`), `:455-472` (`activeMaskId`), `:769` (`findUrl`),
`actions/index.ts:631`/`:644`/`:858`, and `LayerFooter.svelte:41`/`:47` all
hand-roll their own walks today, and both row components hand-roll
`siblingBelowExists`. The plan is moving in the right direction; it should not
cite the doc comment as though the invariant currently holds.

### F. Proportionality — the crux

**The DRY refactor is genuinely forced, but not at the size quoted.**

CLAUDE.md's stop-sign rule fires literally: `LayerGroup.svelte:71-72` already
says *"Same predicate as LayerItem — kept colocated rather than pulled into a
shared helper."* And the arithmetic is unarguable — `LayerItem.svelte:321-385`
and `LayerGroup.svelte:263-329` are ~65 lines apiece differing only in
`layer.id` vs `group.id` and the band thresholds. Adding depth resolution to both
would make it three copies, and the divider and empty area would be four and
five. A Svelte action is the correct shape and is well-precedented
(`binding_site.ts`, `pointerDrag.ts`, `scrubDrag.ts`). **Accept the refactor.**

**Reject Tier-1-first sequencing.** §12.1 offers empty-area-only at ~85/4. It
does not address the user's actual sentence — *"There doesn't seem to be any
distinction in the ui between 'drag to the bottom of this group' and 'drag out of
this group'"* — which is a request for a **gesture distinction**, not for one
more place that happens to mean root. Worse, it is throwaway work: the plan
itself notes the empty-area rule *falls out* of the gap model as
`resolveGapDrop(rows, rows.length, pin: 'min')`, so shipping it standalone means
writing `LayerPanel`'s handler twice. And it only works when the group is
bottom-most in a panel that is not full — the two conditions the plan's own §5.4
argues Darkly's dockable side panel routinely violates.

**Recommended shipping order — Tier 2, split into two commits:**

1. **Commit 1 — the extraction, semantics unchanged.** `dropTarget.ts` (pure:
   `ROW_BASE_PAD`, `ROW_INDENT`, `bandToGap`, `gapDepthRange`, `resolveGapDrop`),
   `layerTree.ts` `rows`, `layerDropTarget` action, both row components
   converted, indicator CSS relocated per C3. Depth is *pinned* to the existing
   behavior (`k = dPrev` for a leaf's below-gap, etc.) so the panel behaves
   identically. Every §8.3 test lands green here except the depth ones. This
   commit is independently reviewable and independently revertible, and it is
   where the risk lives.
2. **Commit 2 — the depth gesture.** Unpin `k` to the X reading, add
   `--drop-indent`, wire the divider and the empty area. §8.2's regression test
   goes red→green here. ~80 lines of the total.

**Drop step 10** (the `siblingBelowExists` / `canMergeDownForThis` lift). It is
unrelated to the reported bug, it touches the context-menu enablement path, and
the plan already offers to drop it. Do it in a separate pass — the stop-sign
comment can wait one PR.

### G. LOC

The plan's ~625/~148 is honest but padded in one place and short in another:

- Production is about right (~300/~150), though `LayerPanel.svelte` +25 is
  generous (the empty area is ~10 lines) and `SpaceDivider` +12 should be split
  ~5 there and ~5 in `LayerPanel` (the component cannot know which of its two
  render sites — `:47` vs `:58` — it is, so the gap must arrive as a prop).
- Tests at ~315 is over-estimated: 21 pure cases at ~4-5 lines plus fixtures is
  ~140, not 180.
- Unbudgeted: C3's CSS relocation (~25 net, and it *removes* a duplicated pair of
  rules), E2's `bandToGap` (~15 + ~30 test), E3's three cases (~25).

**My estimate: ~520-580 added / ~150-165 removed**, with step 10 dropped. Same
order of magnitude, direction of the plan's error conservative. Not a scope
alarm, and the split above means the user approves ~350 for commit 1 and ~200 for
commit 2 rather than one ~600 block.

### H. Required before implementation

1. Fix C1 (§6.3's divider claim) and pin the §8.4 case-22 fixture.
2. Fix C2 (§3.5/Q1 — collapsed only, not "collapsed and empty").
3. Resolve C3 (indicator CSS pruning) and budget it.
4. Rename per C4; drop the stale citation in C5.
5. Correct the four Krita spans and the `krita.action` span in §5.2/§7; soften
   the "every handler is a pass-through" and `layers-actions.c` grep claims;
   fix `reparenting.ts:16`.
6. Adopt E2 (`bandToGap` exported) and E3 (three added cases).
7. Decide E4 explicitly.
8. Restructure §9 into the two commits in F; delete step 10.

The architecture is right, the diagnosis is right, and the model is sound. What
is wrong is fixable in the plan without re-investigation.

revise

---

## 1. The report

> small ui bug. when there's just a single group containing a layer and I want to
> drag the layer down and out of it, I can't. There doesn't seem to be any
> distinction in the ui between "drag to the bottom of this group" and "drag out
> of this group". is there a good solution to that?

Reproduction: a document whose whole tree is

```
Group A          (root child, expanded)
  Layer L        (A's only child)
```

Grab `Layer L` and drag downward. Every pixel of the panel that accepts the drop
resolves to "below `L`, inside `A`". There is no gesture that produces "below
`A`, at the root".

---

## 2. Root cause

### 2.1 The drop gesture can only express *position*, never *parent*

`frontend/src/ui/layers/LayerItem.svelte:338-347` — `onDragOver` reduces the
pointer to a single bit:

```ts
const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
const ratio = (e.clientY - rect.top) / rect.height;
dropPos = ratio < 0.5 ? 'above' : 'below';
```

`onDrop` (`LayerItem.svelte:370-377`) turns that bit into a `MoveTarget` stated
relative to **this row**:

```ts
const where = ratio < 0.5 ? 'after' : 'before';
const skipped = await engine.api.moveLayers({
    ids, target: { target_type: where, target_id: layer.id },
});
```

`layer.id` is fixed by which DOM node the browser hit-tested. Since
`MoveTarget::Before`/`After` place the moved node as a **sibling of the
reference** (`crates/darkly/src/document/mod.rs:1418-1442`), the drop's parent is
always `parent_of(layer.id)`. The gesture has one degree of freedom (Y) and the
problem has two (position **and** depth).

`LayerGroup.svelte:277-292` has the same shape plus an `into` band:

```ts
if (ratio < 0.25)      dropPos = 'above';
else if (ratio > 0.75) dropPos = 'below';
else                   dropPos = 'into';
```

That adds exactly one reachable depth — `depthOf(group) + 1` — and only when a
group header is under the cursor. In the reported tree the only group header is
*above* `L`, so nothing under the drag path offers root level.

### 2.2 Nothing reads depth back from the pointer

The panel already *renders* depth:

- `LayerItem.svelte:406` — `style:padding-left="{8 + depth * 16}px"`
- `LayerGroup.svelte:352` — the same expression on `.group-header`
- `LayerGroup.svelte:431-433` — `depth={depth + 1}` threaded into nested rows

`depth` is write-only. No handler converts `clientX` back into a depth.
(`LayerGroup.svelte:333` also sets `style:--depth={depth}` on the wrapper; grep
shows **no CSS anywhere reads `--depth`** — it is dead and should be deleted in
this pass.)

### 2.3 The empty area below the list swallows drops

`LayerPanel.svelte:18-24`:

```ts
function onDragOver(e: DragEvent) { e.preventDefault(); }
function onDrop(e: DragEvent) { e.preventDefault(); }
```

`preventDefault()` on `dragover` is what makes an element a valid drop target, so
the empty region below the last row *accepts* the drop and then does nothing.
Silent no-op. GIMP treats the same region as "after the last top-level row"
(see §5.1) — the one pointer gesture in GIMP that escapes a group.

The `SpaceDivider` row (`LayerPanel.svelte:47`, `:58`) is an ordinary sibling in
`.layer-list` with no `ondragover`/`ondrop` of its own, so dragging over it also
falls through to the panel handler.

### 2.4 Frontend-only — confirmed

The engine already expresses every move this feature needs.

- `MoveTarget` has exactly the four variants required:
  `crates/darkly/src/document/mod.rs:62-71` — `Before`, `After`, `IntoGroupTop`,
  `IntoGroupBottom`, serialized as `before` / `after` / `into_top` /
  `into_bottom`. The TS mirror is `frontend/src/engine/protocol_gen.ts:739`.
- "Move out one level" is `Before(ancestorGroupId)`. Verified against
  `Document::attach_at_target` (`document/mod.rs:1416-1442`): `Before(ref)` links
  the node into `parent_of(ref)` at `ref`'s index. Combined with `unlink` in
  `Document::move_layer` (`:1149-1174`) that is a full reparent.
- `move_layers` (`crates/darkly/src/engine/layers.rs:1332-1349`) already batches,
  already refuses self-referential targets, and already runs the viewport-space
  check per id before mutating anything.

**No Rust change is required.** Everything below is `frontend/src/`.

### 2.5 A latent inconsistency worth fixing in the same pass

`LayerItem.onDrop` **recomputes** the Y ratio from the drop event instead of
reading the `dropPos` its own `onDragOver` last set (`LayerItem.svelte:370-372`),
while `LayerGroup.onDrop` reads the stored `dropPos` (`LayerGroup.svelte:304`).
Two rows, two rules, and the `LayerItem` path can in principle indicate one thing
and do another. Resolving the drop **once** removes the class of bug.

---

## 3. Feature semantics — the depth-resolution model

### 3.1 The gesture belongs to the *gap*, not to the row

A tree rendered as a flat indented list has, between any two consecutive visible
rows, a range of legal insertion depths. Naming that gap is what makes the
problem tractable — and it is precisely what neither `LayerItem` nor `LayerGroup`
can do today, because each sees only itself.

Let `rows` be the **visible node rows in panel order, top to bottom**, each
carrying `{ id, depth, isGroup }`. Masks are *not* rows: `MaskChainControl` is
rendered inline inside the host's row (`LayerItem.svelte:444-454`), so the row
list is nodes only. Rows inside a collapsed group are absent, which is correct —
they occupy no gap.

Gap `g` (for `g` in `0 ..= rows.length`) sits above `rows[g]` and below
`rows[g-1]`.

```
prev = rows[g-1]   (undefined when g === 0)
next = rows[g]     (undefined when g === rows.length)

maxDepth(g) = prev === undefined ? 0 : prev.depth + (prev.isGroup ? 1 : 0)
minDepth(g) = next === undefined ? 0 : next.depth
```

**Why `minDepth = next.depth`.** Anything shallower would require the inserted
node to sit above `next` while living in an ancestor that `next` is *not* inside
— structurally unrepresentable in a flat rendering. Anything deeper than
`prev.depth + 1` would require a container that does not exist at that point.

**Why `maxDepth` adds 1 only for a group.** `prev.depth + 1` means "first child
of `prev`", which only exists when `prev` can hold children.

`minDepth(g) <= maxDepth(g)` always holds for a well-formed tree: `next` is
either a descendant of `prev` (then `next.depth === prev.depth + 1`, and `prev`
is a group), a sibling of `prev` (equal depths), or shallower.

### 3.2 The reported bug, in the model

```
rows = [ {A, depth 0, group}, {L, depth 1, leaf} ]
```

Hovering `L`'s lower half addresses gap `2` (past the end):
`prev = L` → `maxDepth = 1`; `next = undefined` → `minDepth = 0`.
Two legal depths. Depth 1 = "below L, inside A". Depth 0 = "below A, at root" —
**the gesture the user is missing**. It exists the moment depth is read from X.

More cases:

| Tree (visible rows)                              | Gap                | range   | meaning                          |
|--------------------------------------------------|--------------------|---------|----------------------------------|
| `A(g)`, `L1`, `L2`                                | after `L1`         | `[1,1]` | no choice — correct, no escape   |
| `A(g)`, `L1`, `L2`, `M`                           | after `L2`         | `[0,1]` | inside A, or root above M        |
| `A(g)`, `B(g)`, `L`, `M`                          | after `L`          | `[0,2]` | inside B / inside A / root       |
| `A(g, expanded)`, `L`                             | after `A`'s header | `[1,1]` | forced — this *is* `into_top`    |
| `A(g, collapsed)`, `M`                            | after `A`'s header | `[0,1]` | inside A, or root above M        |
| anything                                          | gap 0 (list top)   | `[0,0]` | root, above everything           |

### 3.3 X → depth

The indent constants are already fixed by the renderer: content for depth `d`
begins at `8 + d*16` px from the row's left edge. Rows are full-width (neither
`.layer-group` nor `.group-children` carries padding or margin — verified in
`LayerGroup.svelte:440-577`; the indent is *entirely* `padding-left`), so a row's
`getBoundingClientRect().left` equals the list's left edge at every depth.

```
xOffset  = e.clientX - rowRect.left
candidate = Math.round((xOffset - ROW_BASE_PAD) / ROW_INDENT)
depth     = clamp(candidate, minDepth(g), maxDepth(g))
```

`ROW_BASE_PAD = 8` and `ROW_INDENT = 16` must become **exported constants** that
`LayerItem` and `LayerGroup` also use for their `padding-left`, or the two
halves of the same fact drift.

Rounding (not flooring) makes the midpoint between two indent stops the switch
point, so the gesture snaps to whichever level the cursor is nearest.

### 3.4 depth → `MoveTarget`

Let `dPrev = prev.depth`.

| condition        | target                                              |
|------------------|-----------------------------------------------------|
| `g === 0`        | `After(rows[0].id)` — above the top row              |
| `k === dPrev + 1`| `IntoGroupTop(prev.id)` (`prev.isGroup` guaranteed) |
| `k === dPrev`    | `Before(prev.id)` — directly below `prev`            |
| `k < dPrev`      | `Before(anc.id)`, `anc` = ancestor of `prev` at depth `k` |

**Panel-down is doc-`Before`.** `children_of(root)` is bottom-to-top
(`document/mod.rs:531-537`, `:599-602`) while the panel renders top-to-bottom, so
"below X in the panel" is `Before(X)` in the document. The existing handlers
already encode this (`LayerItem.svelte:372`, `LayerGroup.svelte:314-316`).

**Finding the ancestor needs no tree walk.** The ancestor of `rows[i]` at depth
`k` is the nearest `j < i` with `rows[j].depth === k`. Ancestors of a visible row
are always themselves visible (a row only renders if every ancestor is expanded),
so a backwards scan of the flat array is exact.

**Correctness of the `k < dPrev` case.** `Before(anc)` places the node as `anc`'s
next panel-sibling. That equals the gap position iff `prev` is the last visible
descendant of `anc` at the gap. It is: the clamp guarantees `k >= next.depth`; if
`next.depth === k` then `next` is a sibling of `anc` (so `anc`'s subtree closed at
the gap), and if `next.depth < k` then `anc`'s subtree closed a fortiori.

### 3.5 Which gap does a row address?

| row kind                  | band       | gap             | note                                    |
|---------------------------|------------|-----------------|-----------------------------------------|
| leaf (`LayerItem`)        | top 50%    | `i`             | X-resolved                              |
| leaf                      | bottom 50% | `i + 1`         | X-resolved                              |
| group header              | top 25%    | `i`             | X-resolved                              |
| group header              | middle 50% | `i + 1`, **depth pinned to `maxDepth`** | = `IntoGroupTop(group)` |
| group header              | bottom 25% | `i + 1`         | X-resolved                              |
| `SpaceDivider`            | whole      | its own gap     | X-resolved (see §6)                     |
| empty area below the list | whole      | `rows.length`, **depth pinned to `minDepth` (0)** | GIMP's rule (§5.1) |

The two bands and the `into` band **cannot fight**, because for an *expanded*
group the bottom band's gap has `minDepth === maxDepth === depth+1` — the pinned
and unpinned resolutions are the same value. The `into` band earns its keep only
for **collapsed and empty** groups, where the child rows the X gesture would aim
between are not on screen. It also preserves the universal "drop onto the folder"
idiom and the existing `.drop-into` outline.

Note the deviation from GIMP this preserves: GIMP uses thirds only for
**collapsed** groups and a plain half-split for expanded ones, where the *bottom
half* means into-the-group
(`gimp/app/widgets/gimpcontainertreeview-dnd.c:264-282`). Darkly's uniform
25/75 is a pre-existing choice; this plan does not change it. See §11 Q1.

### 3.6 Upper half and lower half are the same rule

Both halves address a gap and both run the X clamp. Row `i`'s top band and row
`i-1`'s bottom band address the *same* gap `i` and therefore resolve identically —
the model is consistent by construction, unlike today's two divergent code paths
(§2.5).

---

## 4. Where the logic lives

### 4.1 The duplication this feature would otherwise double

`LayerItem` and `LayerGroup` each carry their own near-identical
`onDragStart` / `onDragOver` / `onDragLeave` / `onDrop`
(`LayerItem.svelte:319-385`, `LayerGroup.svelte:263-329`) — about 65 lines apiece
that differ only in `layer.id` vs `group.id` and the band thresholds. They also
each carry an identical `siblingBelowExists` walk, with a stop-sign comment
already sitting in the file (`LayerGroup.svelte:71-72`: *"Same predicate as
LayerItem — kept colocated rather than pulled into a shared helper"*).

Adding depth resolution to both would triple the copy. Per CLAUDE.md's DRY and
"place functionality where it generalizes" rules, the drop behavior must live in
one place that both row kinds — and the divider, and the empty area — consume.

### 4.2 Recommended shape: one pure module + one Svelte action

**`frontend/src/ui/layers/dropTarget.ts` — pure, no DOM, no `app`.**

```ts
export const ROW_BASE_PAD = 8;
export const ROW_INDENT = 16;

export interface DropRow { id: number; depth: number; isGroup: boolean; }
export interface DropResolution { depth: number; target: MoveTarget; }

/** Legal insertion depths in the gap above `rows[gap]`. */
export function gapDepthRange(rows: DropRow[], gap: number): { min: number; max: number };

/** The drop the pointer is asking for, or `null` when `rows` is empty.
 *  `pin` overrides the X reading for the group `into` band and the empty area. */
export function resolveGapDrop(
    rows: DropRow[],
    gap: number,
    xOffset: number,
    pin?: 'min' | 'max',
): DropResolution | null;
```

This is the whole feature, and it is testable in the node environment with plain
arrays. **Extracting it is a design constraint, not a cleanup** — Vitest has no
DOM, so anything welded to `DragEvent` is untestable (§8).

**`frontend/src/ui/layers/dropTarget.svelte.ts` — `use:layerDropTarget`.**

A Svelte action owning the whole HTML5 DnD lifecycle for one row:

```ts
type LayerDropParams =
    | { row: number }        // a row id — the action finds its index
    | { gap: number }        // an explicit gap (the divider, the empty area)
    ;
export function layerDropTarget(node: HTMLElement, params: LayerDropParams & { pin?: 'min' | 'max' }) { … }
```

Responsibilities:

- `dragstart` — the existing selection rule (grabbed row in selection → drag the
  set; otherwise drag it alone and commit the selection to it), `setData`,
  `effectAllowed`.
- `dragover` — compute band → gap → `resolveGapDrop`, then write the affordance
  **onto the node directly**: `node.classList.toggle('drop-above'|'drop-below'|
  'drop-into')` and `node.style.setProperty('--drop-indent', …px)`. No component
  state, no `$state` round-trip.
- `dragleave` / `dragend` — clear.
- `drop` — re-resolve from the same event (one resolution, one result — §2.5),
  guard `ids.includes(target.target_id)`, call `engine.api.moveLayers`, toast the
  `skipped` count and any `Err`, then `onupdate()`.

Precedent for a singleton-importing action already exists: `bindingSite`
(`frontend/src/actions/binding_site.ts`) and `pointerDrag`
(`frontend/src/ui/workspace/pointerDrag.ts`). This one imports `app` and `toast`
the same way.

**`frontend/src/state/layerTree.ts` — the row list comes from the existing walk.**

The file's own doc comment states the rule: *"The single walk over a layer tree.
Every structural question … is answered from the one traversal, so callers never
hand-roll another"* (`layerTree.ts:56-60`). So `LayerTreeIndex` gains one field:

```ts
/** Visible node rows in panel order, with render depth. Modifiers excluded:
 *  a mask renders inside its host's row, not as a row of its own. */
rows: DropRow[];
```

populated inside the existing `walk` (which already tracks `visible` and can
carry `depth`). ~10 lines. A `dropRows` derived getter on `DarklyInstance`
(`frontend/src/state/app.svelte.ts`, alongside the existing `indexLayerTree`
consumers) exposes it to the action.

### 4.3 Rejected alternative: one panel-level handler measuring the DOM

Hoisting all drop handling to `.layer-list` and hit-testing rows via
`getBoundingClientRect` would also DRY the handlers, but: it discards the free
per-row hit-testing the browser already does, it re-measures on every `dragover`
(~60 Hz), it forces the indicator to become an absolutely-positioned overlay, and
— decisively — it makes the depth math DOM-dependent and therefore **untestable
in Vitest**, since jsdom's layout is stubbed to zeros. Rejected.

---

## 5. Prior art

### 5.1 GIMP — no depth gesture; escape is the blank area below the list

`gimp/app/widgets/gimpcontainertreeview-dnd.c`, function
`gimp_container_tree_view_drop_status()` (`:137-368`).

Leaf rows are a pure Y-midpoint test (`:283-289`):

```c
if (y >= (cell_area.y + cell_area.height / 2))
  drop_pos = GTK_TREE_VIEW_DROP_AFTER;
else
  drop_pos = GTK_TREE_VIEW_DROP_BEFORE;
```

Group rows split by expansion state (`:264-282`) — expanded: top half `BEFORE`,
bottom half `INTO_OR_AFTER`; collapsed: thirds, outer thirds `BEFORE`/`AFTER`,
middle third `INTO_OR_AFTER`.

Depth is then resolved from `drop_pos` alone
(`gimpcontainertreeview-dnd.c:719-728`):

```c
if (drop_pos == GTK_TREE_VIEW_DROP_INTO_OR_AFTER &&
    gimp_viewable_get_children (dest_viewable))
  parent = dest_viewable;
else
  parent = gimp_viewable_get_parent (dest_viewable);
```

**X is never consulted.** After
`gtk_tree_view_get_path_at_pos (tree_view->view, x, y, &drop_path, NULL, NULL, NULL)`
(`:246` — every cell-x out-param is `NULL`), `x` is not read again in the file.
`gimp_container_tree_view_real_drop_possible()` (`:698-817`),
`gimp_layer_tree_view_drop_possible()` (`gimplayertreeview.c:706-738`) and
`gimp_item_tree_view_drop_possible()` (`gimpitemtreeview.c:1447-1497`) take no
coordinates at all.

The blank area below the list **is** handled
(`gimpcontainertreeview-dnd.c:291-316`): it picks the last row with a `NULL`
parent — i.e. the last **top-level** row — and sets `GTK_TREE_VIEW_DROP_AFTER`.
Enabled for layer views via `dnd_drop_to_empty` (`:318`,
`gimpitemtreeview.c:400`). This is the only pointer gesture in GIMP that reaches
the root from inside a group, and it reaches only the *bottom* of it.

GIMP's keyboard raise/lower does **not** escape groups: `gimpimage.c:5280-5290`
hard-fails at index 0 and always reorders under `gimp_item_get_parent (item)`.
`grep -n "group" app/actions/layers-actions.c` yields only `layers-new-group`
(`:97`) and `layers-merge-group` (`:161`) — there is no "move out of group"
action.

*(Correction to the brief this plan was written from: the escape is not "the
group header's top third" at `:275-277`. Those lines are the collapsed-group
branch, and they select `BEFORE`/`AFTER` relative to the group — which resolves
to the group's parent, not the root. The real leaf-row lines are `:283-289`, not
`:283-287`.)*

### 5.2 Krita — no depth gesture; Qt's default indicator

`krita/plugins/dockers/layerdocker/NodeView.cpp:98` is exactly
`setDropIndicatorShown(true);`, with `setDragDropMode(QAbstractItemView::DragDrop)`
at `:96`. Every drag handler is a pass-through to `QTreeView`
(`:371-375` `startDrag`, `:500-509` `dragEnterEvent`, `:511-515` `dragMoveEvent`,
`:517-521` `dragLeaveEvent`, `:472-476` `dropEvent`, `:452-456` `paintEvent`).
`grep -rn "dropIndicatorPosition" krita/` returns **zero hits repo-wide**.
`KisNodeModel::dropMimeData` (`krita/libs/ui/kis_node_model.cpp:823-850`)
receives only `(row, column, parent)` from Qt, whose `position()` is a Y-midpoint
test. `NodeDelegate.cpp` contains no drag/drop code.

Krita's escape hatch is **keyboard**, and it does promote out of a group.
`move_layer_up` / `move_layer_down` (`Ctrl+PgUp` / `Ctrl+PgDown`) are declared at
`krita/krita/krita.action:3618-3628`, wired at
`plugins/dockers/layerdocker/LayerBox.cpp:520-528` → `slotRaiseClicked()`
(`:826-830`) → `KisNodeManager::raiseNode()`
(`libs/ui/kis_node_manager.cpp:1071-1078`) →
`KisNodeJugglerCompressed` `struct LowerRaiseLayer`
(`libs/ui/kis_node_juggler_compressed.cpp:329`). The promote-to-grandparent
branch is `kis_node_juggler_compressed.cpp:420-425`:

```cpp
} else if ((nodesType == AllLayers && grandParent) ||
           (nodesType == AllMasks && grandParent && grandParent->parent())) {
    newAbove = parent;
    newParent = grandParent;
}
```

with the symmetric lower case at `:396-401`. It also auto-*enters* an adjacent
expanded group (`:388-393`, `:412-417`, gated on `!nextNode->collapsed()`).

Krita's blank-area drop also lands at root: `kis_node_model.cpp:662` returns
`Qt::ItemIsDropEnabled` for the invalid (root) index, which `dropMimeData` maps
to `rootDummy()` (`:830-832`).

### 5.3 tldraw — nothing to borrow

Searched `/mega/ARTEXP/darkly/tldraw` in full. No DnD library is declared in any
`package.json` (`react-dnd`, `@dnd-kit/*`, `@atlaskit/*`, `react-arborist`,
`react-complex-tree`, `@hello-pangea/dnd`, `sortablejs` — zero hits), and there
is no `node_modules`. The only layer tree,
`apps/examples/src/examples/ui/layer-panel/ShapeList.tsx`, uses `depth` purely
for render inset (`:58` `paddingLeft: 10 + depth * 20`, `:120` `depth={depth+1}`)
and has no drop target at all. The page list
(`packages/tldraw/src/lib/ui/components/PageMenu/DefaultPageMenu.tsx`) is a flat
1-D sortable with zero references to `clientX`/`pageX`/`offsetX`; its slot math
is `Math.round(dragY / PAGE_MENU_ITEM_HEIGHT)` (`:298-301`). Reparenting in
tldraw is geometric containment (`Editor.reparentShapes`, `Editor.ts:6353`;
`utils/reparenting.ts:16`), never a horizontal drag offset.

### 5.4 What the prior art licenses, and where we depart

Prior art supports, directly:

- **the empty-area drop = "after the last root row"** — GIMP does exactly this
  (`gimpcontainertreeview-dnd.c:291-316`), Krita lands there too
  (`kis_node_model.cpp:662`, `:830-832`);
- **into-the-group via the header's middle band** — both do it.

Prior art does **not** support the X-depth gesture. Neither reference editor has
it; the modern editors that do (Figma, VS Code's explorer, Atlassian's tree
recipe) are not vendored here and this plan cites no source for them, so it makes
no claim about them beyond "this is a known interaction pattern".

The departure is justified on its own merits:

1. Both references escape a group by targeting a row *outside* it — GIMP's blank
   area, or the group header itself. Darkly's panel is a **dockable side panel,
   frequently scrolled and frequently full**, where neither the blank area nor
   the ancestor's header is reliably on screen. GIMP's own escape is one-way
   (bottom of root only).
2. Krita's second escape is a keyboard action Darkly does not have at all (§7).
   Shipping the X gesture is strictly less new machinery than shipping a
   raise/lower action family plus its promote-out-of-group semantics.
3. The gap model *subsumes* the affordances both references have. `into_top`
   falls out of it (§3.5); the empty-area rule falls out of it as
   `resolveGapDrop(rows, rows.length, /* xOffset */ 0)`.

If the reviewer judges the departure unwarranted, §12 offers the strictly
prior-art-backed subset.

---

## 6. Interaction with the viewport-space divider

`docs/plans/viewport-space-effect-groups.md` landed a screen-space boundary whose
rules this feature must not disturb.

### 6.1 The refusal rule, and why we must not predict it

`DarklyEngine::check_screen_space_move`
(`crates/darkly/src/engine/layers.rs:1284-1306`) refuses a move whose
`MoveTarget::reference()` is inside the screen-space region and whose payload
contains a node that cannot render after the view transform, with a
user-readable sentence. `move_layers` runs it for **every** id **before** any
mutation (`layers.rs:1346-1349`), so a refusal leaves the document untouched.
Both existing drop handlers already catch and toast (`LayerItem.svelte:381-383`,
`LayerGroup.svelte:325-327`).

**Recommendation: do not predict refusals; let the drop fail and toast.**

1. The predicate is `Document::screen_space_move_blocker`
   (`document/mod.rs:630-639`) → `LayerNode::screen_space_blocker`, a recursive
   type-owned document query. Mirroring it in TypeScript is exactly CLAUDE.md's
   "keep in sync with X" stop-sign.
2. The viewport plan deliberately chose *refuse loudly with a sentence* over
   silent clamping for moves. Suppressing the affordance would re-hide what that
   work made visible.
3. The frontend's nearest fact, `screenSpaceEligible` on `LayerInfo`
   (`protocol_gen.ts:620` etc.), answers a **different** question — "may *this
   root child* sit above the line", `document/mod.rs:501-507` — not "may this
   dragged payload go there". Using it would be wrong, not merely duplicative.
4. Asking the engine per `dragover` is not viable: the transport is an async
   id→promise FIFO drained on a schedule, so the indicator would lag the cursor
   by frames.

Cost accepted: the indicator can promise a drop that then toasts a refusal. That
is the same contract every other illegal move already has.

### 6.2 The divider row must become a drop target

Today `SpaceDivider` has no drag handlers, so a `dragover` on its 14 px band
bubbles to `LayerPanel`'s catch-all. Once the panel's catch-all *does* something
(the empty-area rule), the divider would wrongly mean "bottom of the root list".

Fix: the divider addresses **the gap it physically occupies**, X-resolved like
any other. Its gap index is the position in `rows` of the root child at panel
index `app.screenSpaceCount` — computable by counting `depth === 0` entries —
or `rows.length` for the trailing divider (`LayerPanel.svelte:57-59`).

This is why `layerDropTarget` takes `{ gap }` as well as `{ row }`.

Semantics that fall out for free: the resolved target's *side* of the boundary is
inherited from the reference node, exactly as `Document::move_layer` documents
(`document/mod.rs:1153-1160`) — *"A move is stated relative to another node, and
that node's side of the viewport divider is the side the moved node lands on."*
Dropping just above the line targets a run member (viewport side); just below,
a canvas-space child. That is the honest reading of the gesture, and illegal
combinations refuse per §6.1.

Note the divider still owns its own `pointerDrag` (`SpaceDivider.svelte:47-51`).
HTML5 DnD and pointer events are separate pipelines, so both can coexist on the
node; the action must not call `preventDefault` on `pointerdown`.

### 6.3 The empty area

`resolveGapDrop(rows, rows.length, 0)` — depth pinned to `minDepth` = 0 by an
`xOffset` of 0 falling below the depth-0 stop, or explicitly via `pin: 'min'`.
Yields `Before(bottomRootRow.id)`: below the bottom root child, at root. GIMP's
exact rule (§5.1).

When the trailing divider is present (`LayerPanel.svelte:57`) the empty area is
*below* the line; the resolved reference is still the bottom root child, which is
in canvas space, so nothing changes.

---

## 7. Accessibility / non-pointer fallback

**There is none, and this plan does not add one.**

Darkly has **no layer-reordering action at all**. The registry
(`frontend/src/actions/index.ts:228-932`) contains `newLayer`, `newGroup`,
`duplicateLayer`, `deleteLayer`, `mergeDown`, `flatten`, `addMask`,
`toggleVisibility`, `toggleLock`, `isolateLayer`, `flipLayerH/V` — and no
`raiseLayer` / `lowerLayer` / `moveLayerOutOfGroup`. `grep -rn
"raiseLayer\|lowerLayer\|moveLayerUp\|moveLayerDown\|move_layer"
frontend/src/actions/ crates/darkly/presets/defaults.yaml` returns nothing.

So drag-and-drop is currently the **only** way to reorder or reparent a layer in
Darkly, and this feature makes the pointer gesture strictly more capable without
closing the keyboard gap.

Recorded as a gap, with the prior art for whoever picks it up: Krita's
`move_layer_up` / `move_layer_down` (`Ctrl+PgUp` / `Ctrl+PgDown`,
`krita/krita/krita.action:3618-3628`) promote out of the enclosing group when the
layer is at the group's boundary
(`kis_node_juggler_compressed.cpp:420-425`, `:396-401`) and auto-enter an
adjacent expanded group (`:388-393`, `:412-417`). GIMP's equivalents deliberately
do not (`gimpimage.c:5280-5290`). Krita's model is the one to copy, and it maps
onto Darkly's existing `MoveTarget` with no engine change — the same four
variants suffice. Out of scope here.

---

## 8. Tests

### 8.1 Environment constraints (binding)

- Vitest runs in **node** by default: no `window`, no `DragEvent`, no
  `DataTransfer`, no `PointerEvent`. Test against plain object fakes; stub
  globals with `vi.stubGlobal` — see
  `frontend/src/lib/__tests__/clickOutside.test.ts`.
- A jsdom environment is available **per file** via a
  `// @vitest-environment jsdom` docblock — used by
  `frontend/src/ui/layers/__tests__/maskChain.component.test.ts:1`,
  `rasterize_menu.component.test.ts`, `addLayerModal.component.test.ts`,
  `src/ui/__tests__/transformModeMenu.component.test.ts`. Those mount real
  components with `mount`/`flushSync`/`unmount` from `svelte`.
- **jsdom does not implement layout**: `getBoundingClientRect()` returns all
  zeros. Any component test that depends on row geometry must stub it.
- **jsdom does not implement `DragEvent` or `DataTransfer`.** Synthesize with a
  plain `Event` plus defined properties:

  ```ts
  function dragEvent(type: string, clientX: number, clientY: number, dt: unknown) {
      const e = new Event(type, { bubbles: true, cancelable: true });
      Object.defineProperties(e, {
          clientX:      { value: clientX },
          clientY:      { value: clientY },
          dataTransfer: { value: dt },
      });
      return e as DragEvent;
  }
  ```

- `svelte-check` (`npm run check`) is the only gate that type-checks `.svelte`
  scripts and templates; `tsc --noEmit` cannot see inside them. Both must pass.

### 8.2 The regression test (write first, must fail before the fix)

`frontend/src/ui/layers/__tests__/dropDepth.component.test.ts`
(`// @vitest-environment jsdom`)

Reproduce the report exactly: mount `LayerItem` for `L` at `depth={1}` with a
tree of `[{ id: A, type: 'group', children: [L] }]`, stub
`getBoundingClientRect` to a 200×28 row at `left: 0, top: 0`, spy on
`engine.api.moveLayers`, then dispatch `dragover` + `drop` at
`clientY = 22` (lower half) and `clientX = 4` (left of the depth-0 indent stop).

Assert: `moveLayers` called with
`{ ids: [L], target: { target_type: 'before', target_id: A } }`.

**Fails today**, because `LayerItem.svelte:375-377` can only ever emit
`target_id: layer.id` — the assertion sees `target_id: L`. That is the bug, and
this is the test that defends against it coming back.

A companion assertion in the same file: the same gesture at `clientX = 30`
(inside the depth-1 stop) still yields `target_id: L` — proving X, not Y, is what
changed the parent.

### 8.3 Pure-function tests

`frontend/src/ui/layers/__tests__/dropTarget.test.ts` (node env, plain arrays).

`gapDepthRange`:

1. gap 0 on any non-empty list → `{ min: 0, max: 0 }`.
2. last gap, last row a depth-1 leaf → `{ min: 0, max: 1 }` — **the reported
   case**.
3. middle child: `[A(g,0), L1(1), L2(1)]`, gap after `L1` → `{ min: 1, max: 1 }`
   (no escape available, correctly).
4. last child with a shallower row following:
   `[A(g,0), L1(1), L2(1), M(0)]`, gap after `L2` → `{ min: 0, max: 1 }`.
5. nested: `[A(g,0), B(g,1), L(2), M(0)]`, gap after `L` → `{ min: 0, max: 2 }`.
6. gap below an expanded group header with children:
   `[A(g,0), L(1)]`, gap 1 → `{ min: 1, max: 1 }` (forced into-top).
7. gap below a collapsed group header: `[A(g,0), M(0)]`, gap 1 →
   `{ min: 0, max: 1 }`.
8. empty `rows` → resolver returns `null`, no throw.

`resolveGapDrop` — depth clamping and target mapping:

9. case 2 with `xOffset` at the depth-0 stop (`8`) → `Before(A)`.
10. case 2 with `xOffset` at the depth-1 stop (`24`) → `Before(L)`.
11. case 2 with `xOffset = -50` → clamps to `min` → `Before(A)`; with
    `xOffset = 500` → clamps to `max` → `Before(L)`. Both ends.
12. case 5 with `xOffset` at each of `8 / 24 / 40` → `Before(A)` / `Before(B)` /
    `Before(L)` — the three-level ladder.
13. case 6 → `IntoGroupTop(A)` regardless of `xOffset` (range is a point).
14. case 7 with `xOffset = 24` → `IntoGroupTop(A)`; with `xOffset = 8` →
    `Before(A)`.
15. gap 0 → `After(rows[0].id)`, `xOffset` irrelevant.
16. `pin: 'max'` on a group's below-gap → `IntoGroupTop`, ignoring `xOffset`
    (the `into` band).
17. `pin: 'min'` on the last gap → `Before(bottomRootRow)` (the empty area).
18. Rounding boundary: `xOffset = 16` (midpoint between the depth-0 and depth-1
    stops) resolves to depth 1; `xOffset = 15` to depth 0.

`indexLayerTree().rows` (extend
`frontend/src/state/__tests__/…` if one exists, else colocate):

19. depths for a two-level tree match `LayerItem`/`LayerGroup`'s
    `depth={depth+1}` threading.
20. a collapsed group contributes its own row but none of its children.
21. modifiers (masks) never appear as rows.

### 8.4 Component tests (jsdom)

22. **Empty-area drop** — mount `LayerPanel` with a two-row tree, dispatch
    `dragover` + `drop` on `.layer-list` below the last row; assert
    `moveLayers` called with `Before(bottomRootRow)`. Fails today (the handler
    is `preventDefault()` and nothing else).
23. **Divider** — with `screenSpaceCount = 1`, drop on the `.divider` row and
    assert the resolved target is the gap's, not the bottom-of-list fallback.
24. **Indicator indent** — after `dragover` at a leftward `clientX`, the hovered
    row carries `drop-below` and `--drop-indent: 8px`; at a rightward `clientX`,
    `--drop-indent: 24px`. This is the "the gesture is visible" half of the
    feature and is otherwise untested.
25. **Refusal toast survives** — make `moveLayers` reject and assert
    `toast.show('error', …)` still fires, so the viewport-space refusal path
    (§6.1) is not regressed by the rewrite.

### 8.5 Not tested

Actual pixel layout of the indicator (CSS), and the engine-side legality rules —
those are covered by `crates/darkly` tests from the viewport-space work.

---

## 9. Implementation steps

1. **Tests first.** Add §8.2's regression test; watch it fail against unmodified
   code. Add §8.3's pure-function tests (red — the module does not exist).
2. `frontend/src/state/layerTree.ts` — add `rows: DropRow[]` to
   `LayerTreeIndex`, populated in the existing `walk`. Export `DropRow`.
3. `frontend/src/state/app.svelte.ts` — a `dropRows` derived getter over
   `layerTree`.
4. `frontend/src/ui/layers/dropTarget.ts` — `ROW_BASE_PAD`, `ROW_INDENT`,
   `gapDepthRange`, `resolveGapDrop`. §8.3 goes green.
5. `frontend/src/ui/layers/dropTarget.svelte.ts` — the `layerDropTarget` action:
   dragstart / dragover / dragleave / dragend / drop, class + `--drop-indent`
   writes, `moveLayers` + toast.
6. `LayerItem.svelte` — delete `dropPos`, `onDragStart`, `onDragOver`,
   `onDragLeave`, `onDrop`; add `use:layerDropTarget={{ row: layer.id }}`; swap
   `padding-left` to the shared constants; `.drop-above/.drop-below` use
   `left: var(--drop-indent, 8px)`.
7. `LayerGroup.svelte` — same, plus the `into` band via `pin: 'max'`; delete the
   dead `style:--depth={depth}` (`:333`).
8. `LayerPanel.svelte` — the empty-area drop target.
9. `SpaceDivider.svelte` — `use:layerDropTarget={{ gap }}` alongside its existing
   `use:pointerDrag`.
10. **Opportunistic, same pass** — lift the duplicated `siblingBelowExists` /
    `canMergeDownForThis` (`LayerItem.svelte:93-109`, `LayerGroup.svelte:73-89`)
    into `layerTree.ts`, retiring the stop-sign comment at `LayerGroup.svelte:71`.
    Small, and it is the same duplication this plan is already paying down. Drop
    it if the reviewer prefers a tighter diff.
11. Full gate: `tsc --noEmit`, `npm run check`, `npm run build`, `npm test`.
    (Rust gates unaffected but should still pass.)

---

## 10. Architectural impact

- **Document authority** — untouched. Every drop still resolves to one
  `moveLayers` request; the document remains the sole authority on legality and
  on which side of the divider a node lands (`document/mod.rs:1153-1173`).
- **Ownership** — depth becomes a fact the row list owns (`layerTree.ts`'s single
  walk) rather than a prop each component re-derives. The pointer→depth reading
  and the padding that renders it share one pair of constants.
- **Modularity / type-owned dispatch** — the resolver branches on
  `row.isGroup`, a structural property of the row, not on `layer.type`. Adding a
  layer kind changes nothing. (`isGroup` is derived once, at the walk, from
  `n.type === 'group'` — the same test `indexLayerTree` already makes at
  `layerTree.ts:91`.)
- **DRY** — net removal: two 65-line handler blocks collapse to one action plus
  one pure module, and the third and fourth call sites (divider, empty area)
  reuse it instead of adding a fifth and sixth copy.
- **Session/compositor** — no new state of any kind. The drag affordance lives in
  DOM classes and a CSS custom property for the duration of the gesture.

---

## 11. Risks and unresolved questions

**Q1 — Keep the group `into` band?** For an *expanded* group the band is exactly
redundant with the below-gap (§3.5); it earns its keep only for collapsed and
empty groups. Keeping it costs one `pin` argument and the existing
`.drop-into` CSS; dropping it removes a state and a band but makes "put this in
that collapsed group" require an X gesture aimed at an indent stop whose child
rows are not on screen. **Plan recommends keeping it.** Related: Darkly's uniform
25/75 split differs from GIMP's expanded-group half-split
(`gimpcontainertreeview-dnd.c:266-272`); this plan does not change it, but the
reviewer may want to.

**Q2 — Discoverability.** The X gesture is invisible until tried. Mitigations
considered: (a) the indented indicator, which this plan ships and which is the
whole reason "render the drop line at the chosen indent" is in scope rather than
optional; (b) outlining the resolved target parent, as Figma does — **not
planned**, flagged; (c) a tooltip or first-run hint — not planned.

**Q3 — Collapsed-group drop is invisible.** `IntoGroupTop(collapsedGroup)` lands
the layer somewhere the user cannot see. GIMP and Krita both expand on
selection-change (`gimp_container_tree_view_selection_changed`,
`app/widgets/gimpcontainertreeview.c` — already cited in
`frontend/src/state/layerTree.ts:19-21`). Auto-expanding the target group after
such a drop is a two-line follow-up (`setGroupCollapsed({ id, collapsed: false })`).
**Not planned; flagged.**

**Q4 — Should `dragover` filter self-referential gaps?** HTML5 DnD hides
`dataTransfer.getData` during `dragover`, so the payload ids are unavailable
there — which is why today's code shows an indicator on the dragged row itself
and only guards at `drop` (`LayerItem.svelte:368`). Since `layerDropTarget` owns
`dragstart` too, it *could* stash the ids in module state and suppress the
affordance. Cheap and strictly better feedback, but it introduces module-level
mutable state that must be cleared on `dragend` including aborted drags.
**Plan preserves today's behavior (guard at drop only); flagged as optional.**

**Q5 — Cross-window drag.** `pointerDrag`'s header
(`frontend/src/ui/workspace/pointerDrag.ts:6-9`) notes that tab dragging is
deliberately window-level because pointer capture traps events in one document.
The layer panel uses HTML5 DnD, which does cross documents — but module-level
drag state (Q4) would not. Another reason to prefer the drop-time guard.

**Risk — jsdom DnD fakery.** §8.2/§8.4 depend on synthesizing `DragEvent` and
`DataTransfer` and on stubbing `getBoundingClientRect`. If that proves brittle,
the fallback is to keep §8.3's pure tests (which carry the real logic) and demote
the component tests to asserting the resolver is *called* with the right
arguments. The regression test must survive in some form: without it there is no
proof the reported bug is fixed.

**Risk — the affordance can promise a refused drop** (§6.1). Accepted
deliberately; consistent with every other illegal move in the panel.

**Risk — scrolled panel.** `xOffset` is measured against the row's own
`getBoundingClientRect().left`, which tracks horizontal scroll for free.
`.layer-list` is `overflow-y: auto` only (`LayerPanel.svelte:84-89`), so there is
no horizontal scroll today; the measurement is correct either way.

---

## 12. LOC estimate

Lines **added** / **removed**, not touched.

| Area | File | Added | Removed |
|---|---|---:|---:|
| Production | `ui/layers/dropTarget.ts` (new) | ~110 | 0 |
| Production | `ui/layers/dropTarget.svelte.ts` (new) | ~120 | 0 |
| Production | `state/layerTree.ts` | ~15 | 0 |
| Production | `state/app.svelte.ts` | ~10 | 0 |
| Production | `ui/layers/LayerItem.svelte` | ~8 | ~68 |
| Production | `ui/layers/LayerGroup.svelte` | ~10 | ~72 |
| Production | `ui/layers/LayerPanel.svelte` | ~25 | ~8 |
| Production | `ui/layers/SpaceDivider.svelte` | ~12 | 0 |
| **Production subtotal** | | **~310** | **~148** |
| Tests | `__tests__/dropTarget.test.ts` (new) | ~180 | 0 |
| Tests | `__tests__/dropDepth.component.test.ts` (new) | ~110 | 0 |
| Tests | existing layer-tree tests | ~25 | 0 |
| **Tests subtotal** | | **~315** | **0** |
| Generated / docs | none (`protocol_gen.ts` untouched — no Rust change) | 0 | 0 |

**Total: ~625 added / ~148 removed. Net ~+475.**

Step 10 (the `siblingBelowExists` lift) is inside the production subtotal at
roughly +12 / −34; dropping it moves the totals to ~613 / ~114.

### 12.1 Smaller-scoped alternative, if that number is too large

The two changes are independent and can ship separately.

**Tier 1 — empty-area drop only.** `LayerPanel.svelte`'s `onDrop` resolves to
`Before(bottomRootRow)`. Strictly prior-art-backed (GIMP
`gimpcontainertreeview-dnd.c:291-316`; Krita `kis_node_model.cpp:662`). Fixes the
reported bug *whenever the group is at the bottom of the list*, which is the
literal tree in the report.

- Production ~+30 / −4, tests ~+55 / 0. **Total ~85 / 4.**
- Does **not** fix: escaping a group that has rows below it, escaping more than
  one level, or the "no visual distinction" half of the complaint.

**Tier 2 — the DRY refactor + the depth gesture**, as planned above.

Shipping Tier 1 first is a defensible sequencing: it is ~15 % of the work, it
closes the reported reproduction, and it is the affordance both reference editors
actually have. The reviewer should decide whether the panel's frequently-full,
frequently-scrolled shape (§5.4) justifies going straight to Tier 2.
