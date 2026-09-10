# Unified "Add Layer" Modal: the interim, before the registry merge

Status: **implemented** (step 5 of the CLAUDE.md Planning and Independent Review Workflow, drafted, independently reviewed with verdict `revise`, revised, approved, built). All CI gates pass: 1290 Rust tests, 724 frontend tests, `cargo fmt`, workspace + wasm clippy, `tsc`, `svelte-check`, `vite build`, `wasm-pack build`.

### What shipped, against what was planned

| Planned | Shipped |
|---|---|
| `addable` declared on two duplicate registrations only | **All 16.** Rust struct literals have no field defaults, so every registration spells it out the way `preview:` already does. +14 lines over estimate; the upside is a new effect is forced to decide rather than defaulting into the picker. |
| `Group` tab needs no `title` override | **It does.** A catalog-less tab falls back to its *action's* name ("New Group"), not the layer kind's ("Group"), so `addSources/group.ts` declares `title: 'Group'` alongside raster's `'Normal'`. Caught by the F1 tests. |
| R1/R2 in `effect_addability.rs` | Four tests, not two: duplicates declare one add path, unique effects stay addable, the picker offers each once, and `category` does not gate the add path (the `effect-layers.md` Test 29 guard). Verified non-vacuous, flipping the BW veil back to `addable: true` fails two of them. |
| `groupByCategory` extracted, other call sites out of scope | As planned. `frontend/src/lib/groupByCategory.ts` carries `groupByCategory` + `matchesQuery`; `BrushPicker`, `AddNodeMenu` and `NodePalette` are untouched and remain a follow-up. |
| F2 mounts the real `EffectPreview` | Stubbed via `__tests__/EffectPreviewStub.svelte`: the real one opens a GPU preview stream per card, which jsdom has no engine for. |

Files: 6 deleted (418 lines), 11 added, 25 modified.

Read in this order: the **Independent Review** below, then **Revision (step 3)**, then the plan body. Where the body and the revision disagree, the revision wins: §2.2 is superseded by §2.2R, §4 Step 1 and §8 are replaced, and the dedupe is retained rather than cut.

## Independent Review

Step 2 of the CLAUDE.md workflow. Every citation below was checked by opening the file at the commit after `db0d929a`; Krita and GIMP claims were checked against `krita/` and `gimp/` in-tree. The plan reviewed here reorders `docs/plans/effect-layers.md`'s PR 3 ahead of its PR 2 and adds a cross-registry dedupe.

**Verdict up front: the plan splits cleanly into two halves with opposite merit.** The modal (four add surfaces collapsing into one, 418 lines deleted) is well-researched, correctly scoped, and endorsed by the sibling plan's own approved review (`docs/plans/effect-layers.md:379-380`: *"pull the picker merge forward into PR 2 (it has no dependency on PR 3)"*). It should be built. The **dedupe half** (§2.2's Rust `category` field, §2.1's `homeCategory`, the §2.2 suppression pass, tests R1/R2/R3 and the `docs_export.rs` churn) should be cut from this PR and left to PR 2, where the merge makes the duplicate structurally impossible and no capability is lost. That is the plan's own option (d), which it files as a "viable fallback" while understating both its savings and its correctness.

---

### Substantive findings

**S1: The plan's case for suppression contradicts its own rejection of option (c′), and (c′)'s reasoning is the right one.**

§3's option table rejects (c′) because *"Pre-PR-4 the tab visibly predicts destination (`ui/layers/LayerPanel.svelte:42-44` mounts `VeilFolder` above the tree), so this is incoherent."* That citation is exact: `LayerPanel.svelte:42-44` is `{#if app.veilList.length > 0}` / `<VeilFolder onupdate={refresh} />` / `{/if}`, above the `{#each app.layerTree …}` at `:46`. But if the tab predicts destination, then a Filters-tab card and a Veils-tab card for `chromatic_aberration` are **not a duplicate**: they are two genuinely different products (a maskable, positionable, exporting tree node vs. a screen-space post-process over the whole viewport), correctly presented under the two tabs that name their destinations. §1's premise ("in one modal they would surface in two adjacent tabs, which makes the original complaint *more* visible") holds only if the two entries are the same thing, which §3's own analysis of what CA loses (§3 point 3: "the non-destructive, maskable, mid-stack, *exported* CA layer … no substitute for that") says they are not. The plan cannot have both. Post-PR-2 they *are* one thing and the dedupe is structural and free; pre-PR-2 they are two, and suppressing one hides a capability.

**S2: The suppression pass is consumer-side classification, and it is untested against reality.**

§2.2's rule ("for each `type_id` offered by more than one source, keep the entry from the source whose `homeCategory` equals the entries' declared `category`") is a frontend consumer comparing a TypeScript-declared string against a Rust-declared string to decide which of two registries wins. That is the shape CLAUDE.md's Type-owned dispatch rule names ("a consumer-side helper that routes by type"). The mitigation offered (the shared `CATEGORY` const, `gpu/black_and_white.rs:18-20` / `gpu/filters/chromatic_aberration.rs:295`) only makes the two Rust `category:` values agree with each other; it does nothing about the third string, `homeCategory`, which lives in `addSources/filters.ts` and `addSources/veils.ts`.

There is **no test that the real `homeCategory` values match the real Rust categories**. R2 is a Rust test that cannot see `homeCategory`. F1 is fed "fake sources and catalogs" (§6). So renaming `"Filters"` → `"Adjustments"` in Rust (exactly the kind of presentational edit the field's doc comment invites) silently breaks the modal with a green suite. Verified: `crates/darkly/tests/docs_export.rs:128` (`assert_eq!(f("category"), category, …)`) is the only place a category string is asserted end-to-end today, and it asserts against the registration, not against any frontend constant.

**S3: The suppression rule is under-specified in the failure case.**

§2.2 says "keep the entry from the source whose `homeCategory` equals the entries' declared `category`; drop the others." If no source's `homeCategory` matches (a rename, a typo, a third category), the stated rule keeps *none* and the effect vanishes from the add UI entirely: a strictly worse failure than the duplicate it exists to fix. §6's F1 does not cover it. If the pass survives revision it needs a stated fallback ("no match → keep all"), and F1 needs that case.

**S4, Making `category` load-bearing directly contradicts the sibling plan's §1.10 and its risk 10, and this plan defers the guard test.**

`effect-layers.md:1279-1294` (*"The category is presentational and nothing else… Deleting every `category:` declaration would change what the picker looks like and nothing else"*) and its risk 10 at `:3034-3043` (*"The category is one field away from becoming load-bearing… Test 29 is the guard; it must not be weakened"*) are explicit. This plan makes `category` decide the destination subsystem (§3) and defers Test 29 (§0, §3). §3 argues that is "a property of the pre-merge world, not a design flaw introduced by this plan", but nothing forces the pre-merge world to consult `category` at all. Under option (d) it does not, because the *source* decides the destination and the category is never read. The load-bearing-ness is introduced by the dedupe, not inherited from the interim.

**S5: The §2.3 reversal is right about the present and overreaches about the future; do not delete `variant_catalog` from the sibling plan.**

§2.3's "impossible now" argument is correct and well made: veils are not a layer kind (`crates/darkly/src/document/layer_kinds/` has no veil), so there is nothing to hang `variant_catalog: "veils"` on today. Its second argument (that `menuPath` ordering already lives on the frontend (`actions/index.ts:578`, `:590`, `:596`, `:602`, `:608`) and Rust must not become a second authority for it) is also correct and is the strongest point in the section.

But the conclusion ("**Recommendation for `effect-layers.md`: delete PR 3 Step 6a and the `variant_catalog` half of §5.9**") does not follow. The precondition that makes `variant_catalog` unusable (effects are not layer kinds) is precisely what PR 4 removes. At PR 4 the question genuinely reopens, and two of §2.3's four bullets are weak:

- *"The spawn module must exist per source regardless."* True for **voids only**, because of the transient-activation constraint (§2.4, verified). `addSources/filters.ts` and `veils.ts` are one wire call each (`FilterPickerModal.svelte:21-25`, `VeilPickerModal.svelte:23-25`); post-merge they are one module. "It has to exist anyway" is a voids fact generalized to five sources.
- *"'Is this kind user-addable' has never been a Rust fact."* True (`NEW_LAYER_ACTION_IDS`, `actions/index.ts:37-43`) but not an argument. It also sits badly beside §3, which cites Krita's `supportsAdjustmentLayers()` (a **core-side, per-effect capability declaration consulted by the model** (`krita/libs/image/kis_base_processor.cpp:40`, overridden at `plugins/filters/embossfilter/kis_emboss_filter.cpp:48`)) as the model to follow. §3 says "declare addability in the core, per effect"; §2.3 says "addability belongs on the frontend". Both cannot be the lesson drawn from the same repository.

**Recommendation:** keep §2.1's `addSources/` design for this PR (it is the only thing that works today), but change §4 Step 7 and §0 to mark `effect-layers.md` §5.9's `variant_catalog` **deferred to PR 4 and re-decided there**, not deleted. Deleting an approved design element on the strength of a precondition that PR 4 removes is out of this plan's remit.

**S6: `AddSource.action` is singular, so the "final rail rule" does not survive PR 2 for ordering.**

§2.1 claims the derivation "is the final one" and §5 claims PR 2 collapses to "delete one file". But rule 1 orders *sources* by their action's `menuPath`, and after PR 2 the single `effects` source (one `action`, one `menuPath`) yields **two** tabs (Filters, Veils) whose relative order is undefined by the stated rules. Today the order happens to be right because two sources with distinct `menuPath`s produce them. §2.1 rule 4 defines tab *membership* post-merge, not tab *order*. Either the rail rule needs a fifth clause (order within a source's derived tabs), or `AddSource.action` needs to be plural. Concrete and cheap to fix, but the "the interim rail is *the* rail" claim is not currently true.

**S7: DRY: "group catalog entries by category with a fallback bucket, plus a tokenized search" already exists three times in the frontend, and the plan cites none of them.**

CLAUDE.md's DRY Principle ("Search before writing… grep for similar functionality"). `addLayerTabs.ts` rules 4/5 and the cross-tab search would be a fourth and fifth copy of:

- `frontend/src/ui/brush_builder/AddNodeMenu.svelte:62-83`: 410-line **categorized, searchable add-menu** with a hand-written `CATEGORY_ORDER` / `CATEGORY_LABELS` pair (`:41-55`) and a flat `searchResults` fallback (`:85-`). This is the closest in-repo analogue to what this plan is building and is absent from the plan entirely.
- `frontend/src/ui/brush_picker/BrushPicker.svelte:58-89`: `matches()` (whitespace-tokenized substring over name + category + tags) and `groups` (first-seen-order category grouping with an "Uncategorised" bucket).
- `frontend/src/ui/brush_builder/NodePalette.svelte:14-20`: the same bucketing again.
- `frontend/src/ui/properties/LayerProperties.svelte:20-33`: the run-length variant the plan *does* cite (§2.2, correctly).

Note that `LayerProperties`'s grouping is **adjacency-based** (`current.label !== label` at `:24`) and only works because the blend-mode catalog is emitted in category order. The veils/filters catalogs are sorted by `type_id` (`gpu/veil.rs:132-141`, `gpu/filter.rs:121-…`), so a merged effects catalog would interleave Filters and Veils entries and adjacency grouping would produce four tabs, not two. `addLayerTabs.ts` must use a map, and the plan should say so rather than pointing at a precedent that would be wrong here. The DRYify opportunity is a shared `groupByCategory(entries)` helper that all five call sites use; `AddNodeMenu`'s hand-written label/order lists are the anti-pattern it would retire.

**S8: The keyboard model collides with the search input, unaddressed.**

§4 Step 4 specifies "Up/Down move the rail, Left/Right and Tab move within the grid" **and** a header search input (§1, §4 Step 4). With focus in a `<input type="search">`, Left/Right must move the caret, not the grid selection, and Enter must not preventDefault the way `NewDocumentModal.svelte:130-135` does unconditionally. `Modal.svelte`'s `stopPropagation` (`:68-76`, see C3) protects against *window* handlers only; it does nothing about the modal's own body handler. The three modals being replaced have no search box, so this is new surface with no in-repo precedent to lift. It needs a stated rule (e.g. only Enter and Up/Down are intercepted while the search input has focus) and an F2 assertion.

**S9: §1's "their `pick()` bodies differ only in which engine call they make" is false, and understates the void lift.**

The `<style>` blocks *are* byte-identical: verified by diffing `VeilPickerModal.svelte:44-82`, `FilterPickerModal.svelte:46-84` and `VoidPickerModal.svelte:83-121`: zero differences across all three pairs. But `VoidPickerModal.svelte:18-66` is a 48-line body with `MediaStream` acquisition, an ordering constraint, a session allow-list opt-in and a failure-path track release. §2.4 gets this right; §1's summary contradicts it and should be corrected, because the summary is what an implementer skims.

**S10: The `chromatic_aberration` mitigations in §3 are real, and the recommendation is nonetheless wrong.**

Verified, all four:

1. **Colors-menu applicability: TRUE.** `crates/darkly/src/gpu/filters/chromatic_aberration.rs:304` declares `hotkey_action: "filterChromatic_aberration"`; `frontend/src/actions/index.ts:760-762` loops `app.entries?.('filters')` and registers a `menuPath: ['Colors:10']` action for every entry carrying one; `PARAMS` is non-empty (`:63`) so `parametric` is true and it opens `filterModal`. The plan's cited line numbers are exact.
2. **Still addable as a veil: TRUE.**
3. **What is lost: TRUE and complete.** `ui/filters/FilterProperties.svelte` has no pipeline switcher (`node.pipeline` is read-only at `:7`, `:10`), and `FilterPickerModal.svelte:21` is the *only* frontend producer of `addFilter`; the other `add_filter_layer` callers are `engine/duplicate.rs:259` and load, both of which require an existing CA filter layer. So the plan is right that there is no other route.
4. **Documents keep loading: TRUE.** `restore_veils` is at `engine/load.rs:647-664` (the plan's `:642-660` starts inside the doc comment and ends early).

The option table is fair and (b), (c), (c′), (e) are correctly rejected. But (a) is the wrong pick for the reason in S1: the interim world genuinely has two products, and (d) is not a fallback, it is the honest model of that world. The plan's §7 risk 2 already half-concedes this ("If PR 2 is genuinely next, that trade is poor and PR 2 should go first"), and nothing in `handoff-effect-layers.md` suggests PR 2 is *not* next: §5 scopes it, §7 sequences it, and §7 says explicitly *"PR 2 ships the original complaint."*

**S11: Option (d)'s saving is understated by an order of magnitude.**

§8 says option (d) is "**−10 production lines** … it does not change any other row." That is false. Under (d) nothing in the interim reads `category`, so the whole of §4 Step 1 goes with it:

| Row that disappears under (d) | + |
|---|---:|
| `gpu/veil.rs` + `gpu/filter.rs` field + doc + projections | 14 |
| 16 `category:` declarations + 2 shared consts | 22 |
| suppression pass + `homeCategory` | ~10 |
| R1 + R2 (`crates/darkly/tests/effect_categories.rs`) | 90 |
| R3 + `docs_export.rs` category columns | 6 |
| **Total** | **~142** |

That is ~46 production lines and ~96 test lines, all of which §5 says PR 2 deletes or replaces anyway, plus the CA regression and the user decision in §3. Presented that way, (d) is not a fallback: it is the smaller change that delivers the same modal.

`addLayerTabs.ts` should keep rule 4 ("one tab per distinct category") under (d): it is a no-op today (verified: zero `category` occurrences under `crates/darkly/src/gpu/voids/`, `gpu/veils/`, `gpu/filters/`) and is what makes the rail PR-2-shaped, at a cost of ~6 lines. F1's "two categories yields two tabs" fixture stays as the forward-property assertion.

**S12: §8's comparison sentence has its sign backwards.**

"Removed is 23 lines lighter than `effect-layers.md`'s PR 3" (§8, after the production table). The reviewed plan removes **487**; the sibling's PR 3 row removes **419 + 45 = 464**. The reviewed plan removes 23 lines *more*, not fewer. The stated cause is also wrong: the delete-files rows differ by exactly 1 (406 + 12 = 418 vs 419, the `NewLayerMenu` 93→92 correction), and the +23 comes from itemizing `LayerFooter` (52) + `actions/index.ts` (15) + `App.svelte` (2) = 69 against the sibling's lumped 45. Everything else in §8 checks out: added `537`, removed `487`, net `+50`, and the `+147` delta against the sibling's `390` are all arithmetically correct, and the file line counts the removal column rests on are exact (`wc -l`: 92 / 27 / 82 / 84 / 121 / 12). The estimate is otherwise honest, including its declared 0-to-+110 spread.

**S13: The "asset counts do not move" claim (§6) is correct.**

Verified. `crates/darkly/src/gpu/filters/` holds 7 modules and `gpu/veils/` holds 9; `docs_render.rs:333-337` asserts exactly `{filters: 7, veils: 9, voids: 1, blendModes: 16, brushes: 13}` and `:340` / `:412` assert the 46 total; `docs_export.rs:363` asserts `previewable, 46`. None is a function of `category`, and the plan changes no registry membership. The claim holds. (Note `effect-layers.md:3026-3027` still says "7 filters and 10 veils": stale post-watercolor; this plan's 7/9 is the correct current figure and the plan is right to have re-derived it.)

**S14 (No regression test is owed, and that claim is correct) but F4 should be reclassified.**

§6's framing is right: this is a feature. However §2.4 says F4 "stops being true" of the sibling plan's "losing any of them breaks camera and screenshare in ways no test catches". F4 tests a constraint on *new* code (`addSources/voids.ts`), not on the code that carries the constraint today. It is good coverage and should be written, but it does not retroactively pin `VoidPickerModal.svelte`, which is deleted in the same PR. Minor framing, not a defect.

---

### Citation audit

Load-bearing citations that are **wrong**, in descending importance:

| Plan says | Actual | Impact |
|---|---|---|
| §4 Step 1.5, §6: "`docs_export.rs:141`: swap the `None` category column"; filters rows `:135-150`, veils rows `:153-169` | The `None` category column is at **`:147`** (filters) and **`:166`** (veils). `:141` is `.map(\|r\| {`. The blocks are **`:135-152`** and **`:154-171`**. | An implementer editing `:141` edits the wrong line. Three errors in one instruction. |
| §2.5, §4 Step 4: `Modal.svelte:66-76` stopPropagation; Escape/backdrop `:78-80`; × `:99-101`; `size-lg` at `:167` | `onKeydown` is **`:68-76`**; `use:backdropDismiss` **`:83`** and `onkeydown` **`:84`**; the × button **`:104`**; `size-lg` **`:172`**. | Four wrong. The sibling plan had `:68-76` right; this plan introduced the error. |
| §4 Step 6, §2.7: `data-keep-open="new-layer"` at `NewLayerMenu.svelte:29` | `:29` is `$effect(() => watchDismiss('new-layer', onclose));`; the attribute is at **`:34`**. | Cosmetic, but it is the "confirm no stragglers" list. |
| §3: `engine/load.rs:642-660` restores `manifest.veils` | `restore_veils` is **`:647-664`**; `:642-646` is the doc comment. | Cosmetic. |
| §2.2: veil imports the shared CA core at `gpu/veils/chromatic_aberration.rs:10-13` | The import is **`:11-13`**; `:10` is the unrelated `gpu::effect` import. | Cosmetic. |
| §8: `CatalogEntry.category` at `protocol_gen.ts:468` | `:468` is the `CatalogEntry` type declaration; the `category` field is at **`:477`**. | Cosmetic (the sibling plan makes the same cite). |
| §4 Step 4: "the card grid lifted from `VeilPickerModal.svelte:44-81`" | `:44-82` is the `<style>` block. The card **grid markup** is **`:30-42`**. | An implementer following it lifts CSS and no markup. |
| §2.1 rule 1: `newGroup` `Layer:20` at `actions/index.ts:606` | `id: 'newGroup'` **`:607`**, `menuPath` **`:608`**. (The other four (`:577`, `:589`, `:595`, `:601`) are the `id:` lines, consistently one before the `menuPath:` line; internally consistent, so fine.) | Cosmetic. |
| §7 risk 5: `veil_list` documented highest-index-first at `app.svelte.ts:295-297` | The comment is at **`:297-299`**; `addVeil` spans `:291-305`. | Cosmetic. |

Citations verified **exact**: `NewLayerMenu.svelte` = 92 lines (the plan's §0 correction of the sibling's 93 is right, and `wc -l` confirms all six file counts); `LayerFooter.svelte:104-123`, `:195-223`, `:10`, `:29`, `:3`, `:12-21`, `:28-32`, `:34-37`, `:43-47`, `:49-55`, `:57-61`, `:63-66`, `:68-71`, `:73-87`, `:89-95`, `:97-100`, `:125-150`, `:175-179`, `.split-main` 18px at `:208`, `.split-btn` margin at `:199`, every one correct, and the derived "52 lines removed" is exact; `VoidPickerModal.svelte:18-66`, `:31-37`, `:46`, `:59-63`; `FilterPickerModal.svelte:19-29`, `:24`; `VeilPickerModal.svelte:17-27`, `:23-26`; `LayerPickers.svelte:8-11`, `:12-18`; `App.svelte:14`, `:71`; `actions/index.ts:10`, `:37-43`, `:582`, `:591`, `:597`, `:603`, `:622`, `:760`; `menu_actions.test.ts:2`, `:141-149`, `:152-156`, `:154` (the Layer-order list is `:122-138`, close enough to the cited `:121-137`); `action_metadata_join.test.ts:31-33`; `registry.ts` `parseMenuSegment` at `:80-86` (cited `:76-85`, includes its doc comment); `gpu/veil.rs:103-117`, `:123-129`; `gpu/filter.rs:73-…` (the struct actually closes at `:104`, one past the cited `:103`), `:112-119`; `gpu/blend_mode.rs:35`, `:72`; `gpu/blend_modes/multiply.rs:9` (cited `:10`, off by one); `gpu/void.rs:344` = `"Voids"`; zero `category` under `gpu/voids/`; `gpu/black_and_white.rs:18`, `:19`, `:20`, `:27` (a `static`, not a `const`), `:73`, `:258-279`; `gpu/filters/chromatic_aberration.rs:63`, `:91`, `:295`, `:304`; `catalog.rs:33` / `:86`; `LayerProperties.svelte:20-33`; `LayerPanel.svelte:42-44`; `EffectPreview.svelte:21`, `:149-161`, and the preview→icon→name chain at `:172-197`; `preview_frames.ts:1-8`; `SettingsModal.svelte:98-105`, `:213-218`, `:220-228`, `:229-250`; `NewDocumentModal.svelte:130-135`, `:139`; `protocol_gen.ts:677` (`PreviewReq`); `iconBundle.test.ts:27`.

§2.5's claim that `EffectPreview` needs no change for a synthetic entry is **verified**: `showsPreview(entry)` gates the canvas branch and `{:else if entry.icon}` at `:189` renders the icon, so `{ supportsPreview: false, icon: 'fa6-solid:square-plus' }` renders as an icon card with no edit. `crates/darkly/src/actions/layers.rs:8` (`fa6-solid:square-plus`) and `:32` (`fa6-solid:folder-plus`) are exact.

§7 risk 3's `import.meta.glob` concern resolves cleanly: `frontend/tsconfig.json` lists `"vite/client"` in `types`, so `import.meta.glob` typechecks under both `tsc --noEmit` and `svelte-check`. Worth noting the sole precedent (`iconBundle.test.ts:27`) is a `query: '?raw'` **text** glob, not an eager module glob: a thinner precedent than §2.1 implies, though the mechanism is the same.

---

### Prior-art audit

Checked against `krita/` and `gimp/` in-tree. **No claim is substantively refuted**; three have wrong line numbers and two are one indirection removed from what the cited line does.

- `kis_filters_model.cc`: the registry-driven category build **is** at `:62-84`; the skip is at **`:67-69`**, not `:66-69` (`:66` is the `KisFilterRegistry::instance()->get(…)` call). Condition text is verbatim modulo brace style. ✓
- `kis_dlg_filter.cpp:87`: the line is exact and the `true` is the `showAll` value, but it is `setPaintDevice(true, …)`; `KisFiltersModel` is constructed at `krita/libs/ui/widgets/kis_filter_selector_widget.cc:120`. The claim "passes `showAll = true` when constructing the filters model" is true only through that indirection. Cite `:120` alongside.
- `kis_dlg_adjustment_layer.cc:66`: **wrong line**. The `setPaintDevice(false, paintDevice)` call is at **`:65`**; `:66` is `layerName->setText(…)`.
- `kis_base_processor.cpp:40`: `, supportsAdjustmentLayers(true)` in `Private::Private()`. ✓ (member is `supportsAdjustmentLayers`, not `m_…`; getter `:138-140`, setter `:160`.)
- `kis_small_tiles_filter.cpp:47` and `kis_emboss_filter.cpp:48`: both exact `setSupportsAdjustmentLayers(false);`. ✓
- `kis_node_manager.cpp`: the `XXX: make factories for this kind of stuff, / with a registry` comment is at **`:654-655`** exactly as claimed; `createNode` spans **`:637-689`** (not `:693`; `:691-693` is `createFromVisible()`), the if/else chain proper is `:657-687` (13 branches), and the macro list is `:365-390` as claimed. The claim is if anything *stronger* than stated: the string list at `:365-390` and the string chain at `:657-687` must be hand-synced, and `:383-384` carries `// NOTE: FastColorOverlayFilterMask is just an identifier, not an actual class name`. ✓
- `gimp/menus/image-menu.ui.in.in`: the `_Blur` submenu is **`:694-708`** (the cited `:694-706` truncates two entries). Members are hand-listed `:697-707`, order is not alphabetical (`filters-pixelize` at `:702` sits between `median-blur` and `gaussian-blur-selective`), and `:696` carries `<!-- TODO: missing subsections? -->`: direct evidence the hand-maintained grouping is known-incomplete. Stronger than claimed. ✓

**The one place prior art is used to support something it does not support:** §3 reads Krita's `supportsAdjustmentLayers()` as *"'applicable but not addable as a layer' is a shipped state"* and therefore as cover for the CA regression. What Krita actually ships is a **declared, per-effect capability the effect itself owns**, consulted by one shared model. This plan's equivalent is not a declaration: it is a frontend suppression pass that removes an effect's add path as a *side effect* of a presentational grouping string, with the effect saying nothing about it. If the CA regression is genuinely wanted, the Krita-faithful shape is an explicit capability on the registration (§4 of the sibling plan's PR 2 already contemplates `targets`), not a category comparison. As written, §3 cites Krita for the outcome while adopting the opposite mechanism.

The `KisFiltersModel` citation is nonetheless the right piece of prior art to have found, and §7 risk 3's citation of `kis_node_manager.cpp` and `image-menu.ui.in.in` as anti-patterns is accurate and well used.

---

### Nits

- **N1**: §2.2 calls `PARAMS` a "const"; `gpu/black_and_white.rs:27` is a `pub static` (deliberately, per `:23-26`, so `ptr::eq` holds). The proposed `pub const CATEGORY` would be fine as a `const` (R2's `ptr::eq` assertion would then be testing a `&'static str` fat-pointer equality that a `const` does not guarantee across two use sites), if R2 really wants pointer identity, `CATEGORY` must be a `static`, matching `PARAMS`/`PREVIEW`. As written R2 could fail or pass by accident.
- **N2**: §7 risk 4 ("Normal" vs "Raster Layer") is a real open question and the plan is right to raise it. Note the same override problem does not exist for "Group", which matches the kind's display name.
- **N3**: §7 risk 5 is right to flag `selectVeil(app.veilList.length - 1)` and right to lift it verbatim. For the record, it looks wrong: `app.svelte.ts:297-299` documents `veil_list` as highest-index-first, so `length - 1` is the *oldest* veil. Worth a follow-up issue rather than a silent carry-forward.
- **N4**: §7 risk 6 (lazy pane mounting) is correct and worth keeping; `EffectPreview.svelte:149-161` fires a `startPreview` per mounted card, and the current pickers only ever mount one catalog.
- **N5**: §2.8's instruction not to "clean up" the lifted `addVeil`/`selectVeil` sequence is the right call and well justified.
- **N6**: §6 F2 asserts "exactly one card named 'Chromatic Aberration' exists across the whole modal". Under option (d) this becomes "exactly two, one per tab, each spawning into its own destination": an equally good assertion of the user-visible property, and it is the assertion that actually pins the tabs' meaning.

---

### Recommended revision

1. **Cut §4 Step 1 entirely** (Rust `category` field, 16 declarations, 2 shared consts, `docs_export.rs` columns), **cut `homeCategory` from `AddSource`, cut the suppression pass, cut tests R1/R2/R3.** Adopt §3 option (d) as the design. ~142 lines lighter, no CA regression, no user decision needed at approval, and PR 2 delivers the dedupe structurally and losslessly.
2. **Keep** §2.1's addSources model, `addLayerTabs.ts` rules 1/2/4/5, the uniform card panes (§2.5), the actions work (§2.7), the deletions (§4 Step 6), and tests F1/F2/F3/F4: all of which are sound and none of which depend on the dedupe.
3. **Fix S6**: add an ordering clause for multiple tabs derived from one source, or make `AddSource.action` plural.
4. **Fix S8**: state the search-input keyboard rule and assert it in F2.
5. **Fix S5**: mark `effect-layers.md` §5.9's `variant_catalog` *deferred to PR 4*, not deleted; drop that instruction from §4 Step 7.
6. **Address S7**: extract a shared `groupByCategory` helper rather than writing a fourth copy, and cite `AddNodeMenu.svelte` as the in-repo prior art for a categorized searchable add-menu.
7. **Correct** §1's "`pick()` bodies differ only in which engine call" (S9), §8's inverted 23-line sentence (S12), and the nine citations in the audit table above: particularly `docs_export.rs:141`, which is an instruction pointing at the wrong line.

With those changes the plan is a ~30-line-net frontend PR that deletes 418 lines, ships the unified modal the user asked for, regresses nothing, and leaves PR 2 exactly as scoped. As written it also ships a capability regression and ~142 lines of machinery that PR 2 deletes, to make a duplicate disappear one PR early.

revise

---

## Revision (step 3)

Step 3 of the CLAUDE.md workflow: every substantive finding is addressed in the plan
or rejected with a reason. The review is preserved above verbatim.

### The one rejection

**S1 / S10 / S11's central recommendation ("cut the dedupe") is rejected.** The
duplicated catalog entry is the originating complaint this effort exists to fix;
a modal that ships each effect twice does not deliver the feature. Recorded as a
user decision at step 3. S1's observation (pre-PR-4 the tab predicts destination,
so the two entries are arguably two products) is accepted as *true* and rejected as
*decisive*: the split is an artifact of the unmerged registries, and surfacing it to
the user is the bug, not the feature. §3's Resolution states this.

The review's mechanical objections to the draft's dedupe *mechanism* are accepted
in full and are what drove §2.2R.

### Accepted, with the change made

| Finding | Disposition |
|---|---|
| **S2** untested TS-vs-Rust string comparison | Fixed by **§2.2R**. No frontend string comparison exists; the effect declares `addable` and the frontend honours it. |
| **S3** suppression rule undefined when nothing matches | Dissolved by §2.2R: there is no matching step. `addable` defaults `true`, so the failure mode is "shown", never "vanishes". |
| **S4** `category` becomes load-bearing, Test 29 deferred | Dissolved by §2.2R. `category` decides tabs only; `addable` decides the add path. `effect-layers.md` §1.10 holds and **Test 29 is not deferred**. |
| **S5** don't delete `variant_catalog` from the sibling plan | Accepted. §4 Step 7 now marks it **deferred to PR 4 and re-decided there**, not deleted. §0's table row updated to match. |
| **S6** `AddSource.action` singular → undefined tab order post-PR-2 | Accepted. `addLayerTabs.ts` gains rule 6: tabs derived from one source order by the source's declared `categoryOrder`, falling back to catalog order. ~4 lines. |
| **S7** DRY: categorized searchable menu exists 3× already | Accepted. Extract `frontend/src/lib/groupByCategory.ts` and use it in `addLayerTabs.ts`; cite `AddNodeMenu.svelte:62-83` as the in-repo analogue. Retrofitting the other three call sites is explicitly **out of scope** here (it touches the brush builder and brush picker) and is noted as a follow-up. Adjacency grouping is *not* used: §2.1 rule 4 uses a map, per S7's warning about `type_id`-sorted catalogs. |
| **S8** keyboard collides with the search input | Accepted. Rule: while the search input has focus only Enter, Escape and Up/Down are intercepted; Left/Right and Home/End reach the caret. Asserted in F2. |
| **S9** §1's "`pick()` bodies differ only in the engine call" is false | Accepted; §1 corrected to say the *style blocks* are byte-identical and the void body carries a 48-line activation-ordering constraint. |
| **S12** §8's 23-line comparison has its sign inverted | Accepted; superseded by §8R below. |
| **S13 / S14** asset counts don't move; no regression test owed | Confirmed correct, no change. F4 reclassified as new coverage of a constraint carried into new code, not a retroactive pin. |
| **N1** `PARAMS` is a `static`, and `ptr::eq` on a `const` is unsound | Moot: §2.2R's R2 asserts a set property (no `type_id` addable from two registries), not pointer identity. No shared `CATEGORY` const is introduced. |
| **N2-N6** | N2 open question carried to §7. N3 logged as a follow-up, still lifted verbatim. N4, N5 accepted as written. N6 moot under §2.2R (exactly one CA card, which F2 asserts). |
| **Citation audit** | All nine corrected: `docs_export.rs` category column `:147`/`:166` (blocks `:135-152`/`:154-171`); `Modal.svelte` `:68-76`, backdrop `:83-84`, × `:104`, `size-lg` `:172`; card grid is `VeilPickerModal.svelte:30-42` (not `:44-81`, which is the style block); `data-keep-open` at `NewLayerMenu.svelte:34`; `restore_veils` `engine/load.rs:647-664`; CA core import `:11-13`; `protocol_gen.ts:477`; `newGroup` `:607-608`; `veil_list` comment `app.svelte.ts:297-299`. Prior-art: `kis_filters_model.cc:67-69`, `kis_dlg_adjustment_layer.cc:65`, add `kis_filter_selector_widget.cc:120` for the `showAll=true` indirection, `createNode` `:637-689`, `_Blur` `:694-708`. |

### Consequential simplification: `category` is no longer declared in this PR

With `addable` resolving the duplicate, nothing in the interim needs `category` at
all: the `filters` and `veils` catalogs each yield one tab titled by their own
`Catalog::title`, which §2.1 rule 4 already handles. So the 16 `category:`
declarations and the two shared consts are **cut** and land in PR 2, where a single
merged catalog genuinely needs them to split into two tabs. This takes most of what
S11 asked for without giving up the dedupe.

`addable` does need to cross the wire, so unlike the draft this PR **does** add one
`CatalogEntry` field and regenerate `protocol_gen.ts`. That is the one cost of
moving the decision into Rust, and it is the right trade.

### §4 Step 1, replaced

1. `pub addable: bool` on `VeilRegistration` (`gpu/veil.rs:103-117`) and
   `FilterPipelineRegistration` (`gpu/filter.rs:73-104`), with §2.2R's doc comment.
2. `pub addable: bool` on `CatalogEntry` (`catalog.rs`, beside `category` at `:32-33`)
   defaulting `true` in `CatalogEntry::new`, with a `with_addable` setter following
   `with_category`'s shape (`:86-89`); projected in both `catalog_entry()` bodies
   (`gpu/veil.rs:123-129`, `gpu/filter.rs:112-119`).
3. `addable: false` in `gpu/veils/black_and_white.rs` and
   `gpu/filters/chromatic_aberration.rs`. Every other registration omits it.
4. Regenerate `protocol_gen.ts`.
5. Tests R1, R2 (below). **No `docs_export.rs` category-column edit**, that file is
   untouched by this PR now.

### §6 tests, amended

- **R1: `crates/darkly/tests/effect_addability.rs`, `duplicate_effects_declare_one_add_path`.**
  For every `type_id` registered in both `gpu::veils::registrations()` and
  `gpu::filters::registrations()`, exactly one of the two declares `addable: true`.
  Fails today for both pairs; this is the assertion that pins the user's complaint at
  its source, in Rust, with no frontend fixture.
- **R2 (same file, `addable_is_the_only_add_gate`.** Every `type_id` appearing in
  exactly one registry is addable, and `category` is not read by any add path)
  the `effect-layers.md` Test 29 guard, written here rather than deferred.
- **R3 deleted** (it asserted a shared `CATEGORY` const that no longer exists).
- **F1** drops the `homeCategory` fixtures and asserts instead that a non-addable
  entry is absent from every tab while remaining present in the raw catalog.
- **F2** gains S8's keyboard assertion.

### §8R: corrected LOC

| | + | − |
|---|---:|---:|
| Production | **~531** | **~487** |
| Tests | **~395** | **~10** |
| Generated (`protocol_gen.ts`) | **~4** |, |

**Production net ≈ +44**, range 0 to +105. Against the draft: −22 (category
declarations cut), −10 (suppression pass cut), +26 (`addable` on two registrations,
on `CatalogEntry`, its setter and two projections). Tests are ~45 lighter than the
draft because one focused Rust test replaces R1+R2+R3.

The removal column is unchanged at 487 and, per S12, that is **23 lines more** than
`effect-layers.md`'s PR 3 row (464), not fewer: the draft's sentence had its sign
inverted and the cause misattributed; the +23 comes from itemizing `LayerFooter`,
`actions/index.ts` and `App.svelte` against the sibling's lumped 45.

---

This plan implements what `docs/plans/effect-layers.md` calls **PR 3**, *before* its PR 2. Everything here was verified against source at the commit after `db0d929a`.

---

## 0. Relationship to `docs/plans/effect-layers.md`

| Section of that plan | Fate here |
|---|---|
| §1.11 (semantics: one `+`, one modal, tab rail, Enter spawns) | **Adopted unchanged.** |
| §2.9 (measured add-layer surface, what survives in `LayerFooter`) | **Adopted unchanged**, re-verified; one correction: `NewLayerMenu.svelte` is 92 lines, not 93. |
| §5.9 "The rail is derived, not written": `LayerKindRegistration.new_action` + `variant_catalog`, `CatalogEntry.variant_catalog` | **Deferred, not deleted.** Unusable now (veils are not a layer kind), so this PR uses §2.1's `addSources/` instead. Whether it returns is **re-decided at PR 4**, when effects become layer kinds and the precondition that blocks it disappears: per the step-2 review's S5, which rejected the draft's move to delete it outright. |
| §5.9 "The panes" (description + primary button for a catalog-less tab) | **Superseded** by a uniform entry model (§2.5). Strictly less code, one keyboard model, no branch. |
| §5.9 "Spawning stays per-catalog" (`ui/layers/addSources/*.ts`) | **Adopted and generalized**: the add-source module becomes the unit the rail is derived from, not only the spawn. |
| §5.9 "Actions" (`addLayer` new; `newVeil`/`newFilterLayer`/`newVoid` become deep links; `newLayer`/`newGroup` spawn directly; `NEW_LAYER_ACTION_IDS` dies) | **Adopted unchanged.** |
| §1.10 (one registry, two categories; single category per effect; `chromatic_aberration` → Veils) | **Pulled forward.** The 16 `category` declarations land here instead of in PR 2, on the two registration structs that exist today. They survive the merge verbatim. |
| §7 Test 24 (rail unit test), Test 25 (menu actions), Test 28 (category partition) | **Pulled forward**, adapted to two catalogs. Test 28's "six Filters, nine Veils" split assertion becomes "no `type_id` appears under two tabs", which is the property the user actually asked for. |
| §7 Test 29 (`category_is_presentation_only`) | **Written here, not deferred.** Under §2.2R the category never decides a destination, so its claim holds throughout the interim; it lands as R2. (The draft deferred it; the step-3 revision reverses that.) |
| Everything about the trait, the registry merge, the boundary, the divider, the format, `VeilChain`, `manifest.veils` | **Untouched.** This plan changes no rendering, no serialization, no registry membership. |

After PR 2 lands, the deltas listed in §5 collapse to: delete one file, delete two struct fields, delete a ten-line function. Nothing else in this plan is thrown away.

---

## 1. Feature semantics

One `+` in the layer footer opens one modal: a vertical tab rail on the left (**Normal · Filters · Veils · Voids · Group**), a card grid on the right, one cross-tab search input in the header, Enter spawns the selected card and closes.

Four surfaces collapse into it: the split button (`ui/layers/LayerFooter.svelte:104-123`), the chevron dropdown (`ui/layers/NewLayerMenu.svelte`, 92 lines), the mount switch (`ui/layers/LayerPickers.svelte`, 27) and three byte-identical picker modals (`ui/veils/VeilPickerModal.svelte` 82, `ui/filters/FilterPickerModal.svelte` 84, `ui/voids/VoidPickerModal.svelte` 121; their `<style>` blocks are identical and their `pick()` bodies differ only in which engine call they make).

**Each effect appears exactly once.** `black_and_white` and `chromatic_aberration` are registered in both `gpu/veils/` and `gpu/filters/` and today surface in two separate pickers; in one modal they would surface in two adjacent tabs, which makes the original complaint *more* visible, not less. They are deduped by `type_id`.

**Placement is unchanged.** Every kind lands by the most recently selected layer through the existing anchor (`actions/index.ts:582` `addRaster({ anchor })`, `:622` `addGroup({ anchor })`, `ui/filters/FilterPickerModal.svelte:24` `addFilter({ …, anchor })`, `ui/voids/VoidPickerModal.svelte:46`). Settled; not reopened.

---

## 2. Architecture

### 2.1 The rail is derived from *add sources*, and the derivation rule is the final one

The unit is an **add source**: one way of putting something new into the document. Each is a file under `frontend/src/ui/layers/addSources/`, discovered by `import.meta.glob('./*.ts', { eager: true })`: the frontend's equivalent of `build.rs` scanning a module directory, and already used in-repo at `frontend/src/__tests__/iconBundle.test.ts:27`.

```ts
export interface AddSource {
    /** The action that deep-links here. Supplies the tab's icon, description
     *  and rail position (its `menuPath` order), and is what a catalog-less
     *  source spawns through. */
    action: string;
    /** Registry catalog this source picks from, or '' when choosing the kind
     *  is the whole choice. */
    catalog: string;
    /** Tab title for a catalog-less source; a catalog titles its own tab. */
    title?: string;
    /** The one category this source is the home of. Interim only: it exists
     *  solely to resolve an effect registered in two catalogs. */
    homeCategory?: string;
    /** Create one from a chosen entry. Absent → dispatch `action`. */
    spawn?: (entry: CatalogEntry) => Promise<void>;
}
```

Five files, each ~14-60 lines:

| file | `action` | `catalog` | `title` | `homeCategory` | `spawn` | tab |
|---|---|---|---|---|---|---|
| `raster.ts` | `newLayer` |: | `Normal` |: |: (dispatch) | Normal |
| `filters.ts` | `newFilterLayer` | `filters` |: | `Filters` | `addFilter` | Filters |
| `veils.ts` | `newVeil` | `veils` |: | `Veils` | `addVeil` | Veils |
| `voids.ts` | `newVoid` | `voids` |: |: | `addVoid` | Voids |
| `group.ts` | `newGroup` |: | `Group` |: |: (dispatch) | Group |

`ui/layers/addLayerTabs.ts`: a plain `.ts` so the arithmetic is node-testable without mounting Svelte, exactly the reason `ui/preview_frames.ts:3-4` was split out of `EffectPreview.svelte`:

1. **Order** sources by their action's `menuPath` order: `newLayer` `Layer:10` (`actions/index.ts:577`), `newFilterLayer` `:12` (`:589`), `newVeil` `:14` (`:595`), `newVoid` `:16` (`:601`), `newGroup` `:20` (`:606`), parsed by the existing `parseMenuSegment` (`actions/registry.ts:76-85`). The rail order and the Layer-menu order cannot drift because they are the same numbers. `NEW_LAYER_ACTION_IDS` (`actions/index.ts:37-43`) is deleted; it was the hand-written copy of this.
2. **Entries** = the source's catalog entries, or (for a catalog-less source) one synthetic entry built from the action's own doc (`{ type: '', displayName, description, icon, supportsPreview: false }`).
3. **Suppress** duplicates (§2.2).
4. **Group** the kept entries into tabs: **one tab per distinct `category` the source's entries declare; if none declares one, a single tab titled by `Catalog.title`.**
5. **Title** a tab by its category, else `source.title ?? catalog.title`.

Rule 4 is verbatim the rule `effect-layers.md` §5.9 states for the merged world. It is correct today with no interim special-casing: the `filters` and `veils` catalogs each yield one tab (every entry declares the same category), `voids` declares no category on any entry (verified: zero `category` occurrences under `crates/darkly/src/gpu/voids/`) and yields one tab titled `"Voids"` (`gpu/void.rs:344`). When PR 2 merges the catalogs, the same function produces two tabs from one source with no edit, which is what makes the interim rail *the* rail.

### 2.2 Deduplication

**Superseded by §2.2R below.** The mechanism described in this section (a frontend
`homeCategory` string compared against the Rust `category`) was replaced during
step 3 revision. Read §2.2R instead; this text is kept only because §2.1, §3 and
§8 refer back to it.

Two Rust registration structs each gain one field, projected through the setter that already exists:

```rust
/// Which group of the picker this effect appears under: the word the user
/// reads for the kind of thing it is. Presentational: it does not affect the
/// pipeline, the layer, serialization or rendering.
pub category: &'static str,
```

on `VeilRegistration` (`gpu/veil.rs:103-117`) and `FilterPipelineRegistration` (`gpu/filter.rs:73-103`), each projecting `.with_category(self.category)` in `catalog_entry()` (`gpu/veil.rs:123-129`, `gpu/filter.rs:112-120`). This is the shape `BlendModeRegistration` already uses: `pub category: &'static str` (`gpu/blend_mode.rs:35`), declared per variant (`gpu/blend_modes/multiply.rs:10`), projected at `:72`, grouped on the frontend with no hand-written list (`ui/properties/LayerProperties.svelte:20-33`).

Sixteen declarations, from `effect-layers.md` §1.10's table (`watercolor` is gone): six `"Filters"`, `black_and_white`, `brightness_contrast`, `curves`, `hsv`, `invert`, `levels`; eight `"Veils"`, `chromatic_aberration`, `frozen`, `grain`, `lens_blur`, `painting`, `pixelate`, `rainy_glass`, `vhs`. **The two duplicated effects declare theirs once**, as a `pub const CATEGORY` in the module that already owns their shared identity: `gpu/black_and_white.rs` (which owns `TYPE_ID` `:18`, `DISPLAY_NAME` `:19`, `DESCRIPTION` `:20`, `PARAMS` `:27`, `PREVIEW` `:73`) and `gpu/filters/chromatic_aberration.rs` (which owns `PARAMS` `:63`, `PREVIEW` `:91`, `DESCRIPTION` `:295`, imported by the veil at `gpu/veils/chromatic_aberration.rs:10-13`). Both registrations reference the one const, so the two copies cannot disagree: structurally, the way `PARAMS` already is, asserted by `veil_and_filter_share_one_identity` (`gpu/black_and_white.rs:258-279`).

The suppression pass in `addLayerTabs.ts`, ~10 lines: for each `type_id` offered by more than one source, keep the entry from the source whose `homeCategory` equals the entries' declared `category`; drop the others. `black_and_white` declares `"Filters"` → the `filters` source wins. `chromatic_aberration` declares `"Veils"` → the `veils` source wins. Entries that declare no category cannot collide (only the `voids` catalog has none, and no id is shared with it).

**A kept entry carries its winning source.** That is what decides both the preview catalog (`PreviewReq { catalog, type, variant }` (`engine/protocol_gen.ts:677`), consumed by `EffectPreview` which already takes `catalog` as a string prop (`ui/EffectPreview.svelte:21`)) and the spawn path. There is no place where the modal asks what kind of entry it is holding.

### 2.2R Deduplication: the effect declares its own add path

Deduplication is the originating requirement for this whole effort, so it stays.
What changes is where the duplicate is resolved: **in the registration, not in the
frontend.**

Both registration structs gain one field:

```rust
/// May the user add this effect from the add-layer modal? `false` where a
/// second registration of the same `type_id` in another registry owns the add
/// path: the effect is still applicable, loadable and renderable, it simply
/// is not offered twice. Defaults to `true`; a duplicate opts out in its own
/// file. Collapses when the registries merge and the duplicate ceases to exist.
pub addable: bool,
```

Two declarations, in the two modules that are the redundant half:
`gpu/veils/black_and_white.rs` declares `addable: false` (the filter form is
strictly more capable, exports, maskable, positionable), and
`gpu/filters/chromatic_aberration.rs` declares `addable: false` (settled Q8 puts
CA under Veils). The other fourteen say nothing and get the `true` default.

The frontend then does no classification at all: `addLayerTabs.ts` keeps entries
whose `addable` is not `false`, and every remaining `type_id` is unique by
construction. No `homeCategory`, no suppression pass, no cross-language string
comparison, no failure case where an effect matches nothing and vanishes.

This is the shape CLAUDE.md's Type-owned dispatch rule asks for and the shape the
prior art actually uses: Krita's `supportsAdjustmentLayers` defaults to `true`
(`krita/libs/image/kis_base_processor.cpp:40`), is overridden in the filter's own
file (`plugins/filters/embossfilter/kis_emboss_filter.cpp:48`,
`plugins/filters/smalltilesfilter/kis_small_tiles_filter.cpp:47`), and is consulted
by one shared model that branches on nothing else
(`krita/libs/ui/kis_filters_model.cc:67-69`). The step-2 review's prior-art audit
found the draft citing this passage for the outcome while adopting the opposite
mechanism; §2.2R adopts the mechanism.

**`category` stays presentational.** It groups tabs and does nothing else: it does
not decide a destination, so `effect-layers.md` §1.10 holds unchanged and its
Test 29 guard is **no longer deferred** (§6, R2).

**Consequence, unchanged and accepted:** the CA filter layer and the BW veil are
not addable until PR 2 merges the modules. §3 documents what that costs.

`effect-layers.md` §5.9 derives the rail from two new fields on `LayerKindRegistration` plus a new `CatalogEntry.variant_catalog` wire field. That is impossible now: veils are not a layer kind, so there is no registration to hang `variant_catalog: "veils"` on, and inventing one would be a lie about the document model that PR 2/PR 4 would then have to unwind.

It is also, on inspection, unnecessary later:

- The spawn module must exist per source regardless (§5.9's own conclusion, and the constraint in §2.4 below). Letting it name its catalog costs one line in a file that has to exist.
- Ordering already lives on the frontend: `menuPath: ['Layer:10']` is declared in `actions/index.ts:577`, not in Rust. Deriving order from a Rust field would create a second authority for the same fact.
- "Is this kind user-addable" has never been a Rust fact; it is `NEW_LAYER_ACTION_IDS` today.
- It avoids widening the wire `CatalogEntry` (and regenerating `protocol_gen.ts`) for a purely presentational fact.

**Recommendation for `effect-layers.md`, as revised at step 3: mark PR 3 Step 6a and the `variant_catalog` half of §5.9 *deferred to PR 4*, not deleted.** The step-2 review's S5 accepted the "impossible now" argument and the `menuPath`-second-authority argument, but rejected deleting an approved design element on the strength of a precondition (effects are not layer kinds) that PR 4 itself removes. The question genuinely reopens there and should be decided there. The honest cost meanwhile: a reader of `document/layer_kinds/*.rs` cannot see which kinds are addable. Mitigated by the addSources directory being five one-screen files with a doc comment naming the kind each serves.

### 2.4 The void spawn's transient-user-activation constraint: verified and preserved

`ui/voids/VoidPickerModal.svelte:18-66` acquires the `MediaStream` inside the click gesture **before any other `await`** (`:31-37`), because `getDisplayMedia` requires transient user activation that an `addVoid` round-trip would expire; its comment at `:20-27` spells this out. The same body opts the new layer into the session allow-list (`:48-58` `markStreamVoidStarted` + `startStreamSource`) and stops the tracks when layer creation failed (`:59-63`).

`addSources/voids.ts` carries that body **verbatim**, ordering included. This is why spawning is per-source rather than a branch in the modal: no shared spawn path can be refactored into acquiring the stream after the add. Enter is a user gesture and still grants activation, but only if nothing is awaited first, which is a property of the function body, not of the key.

Unlike the plan, this is **testable and will be tested** (§6, Test F4): mock `app.acquireMediaStream` and `engine.api.addVoid`, assert acquisition resolves before `addVoid` is called. The plan says "losing any of them breaks camera and screenshare in ways no test catches"; that stops being true here.

### 2.5 The panes are uniform

`effect-layers.md` §5.9 gives a catalog-less tab a description plus a primary button, and a catalog tab a card grid. That is two pane shapes and two keyboard models. Instead, **every tab is a grid of cards**, and a catalog-less source contributes exactly one synthetic card from its action's doc. `EffectPreview` already handles it with no change: its fallback chain is preview → icon → name (`ui/EffectPreview.svelte:172-195`), and a synthetic entry with `supportsPreview: false` and the action's icon (`fa6-solid:square-plus` for `newLayer`, `fa6-solid:folder-plus` for `newGroup`, `crates/darkly/src/actions/layers.rs:8`/`:32`) renders as an icon card. One selection model, one Enter handler, no `{#if}` on kind.

Single-card panes are a CSS concern (the card can span the grid), not a branch.

### 2.6 What survives in `LayerFooter.svelte`: verified against the current file

Removed: the split button markup (`:104-123`, 20 lines), the `.split-btn` / `.split-main` / `.split-chevron` rules (`:195-223`, 29 lines), `menuOpen` (`:10`), the `menuOpen = false` line in `pick` (`:29`) and the `NewLayerMenu` import (`:3`). Replaced by one `footer-btn` dispatching `addLayer`.

Kept, all confirmed present and unrelated to adding layers: `findNode` (`:12-21`), `pick` (`:28-32`), `hostHasMask` (`:34-37`), `activeEditable` (`:43-47`, mirroring the engine's `is_node_editable`), `canAddMask` (`:49-55`), `addMask` (`:57-61`), `canDelete` (`:63-66`), `canDuplicate` (`:68-71`), the multi-selection tooltips (`:73-87`), `remove` (`:89-95`), `duplicate` (`:97-100`), and the Add-mask / Duplicate / Delete buttons (`:125-150`).

One visual detail: `.split-main` declared `font-size: 18px` and `.split-btn` `margin-right: 4px`; the generic `.footer > .footer-btn` (`:175-179`) is 14px with no margin. Add a two-line `.add-layer { font-size: 18px; margin-right: 4px; }` to keep the `+` looking as it does.

### 2.7 Actions

- **New `addLayer`** ("Add Layer…"), opening the modal on its first tab. Needs an `ActionDef` in `crates/darkly/src/actions/layers.rs`: `frontend/src/actions/__tests__/action_metadata_join.test.ts:31-33` asserts the documented set equals the handler set exactly, so a frontend-only action fails CI. `menuPath: ['Layer:8']`, ahead of `newLayer`'s `:10`.
- **`newFilterLayer` / `newVeil` / `newVoid` survive** as category deep links, retargeted from `layerPicker.kind = …` (`actions/index.ts:591`, `:597`, `:603`) to `addLayerModal.open('Filters' | 'Veils' | 'Voids')`. This keeps "New Veil" in the Layer menu and the palette: the property `actions/__tests__/menu_actions.test.ts:141-149` asserts and the settled "Veil survives as a user-facing word" decision requires. Deep-linking by tab title (not by source) is what keeps these two lines correct after PR 2 splits the effects source into two tabs.
- **`newLayer` and `newGroup` keep spawning directly** (settled Q9). `addSources/raster.ts` and `group.ts` therefore declare no `spawn` and dispatch the action: the wrap-or-empty-group logic (`actions/index.ts:606-628`) stays in one place rather than being copied into a spawn module.
- `NEW_LAYER_ACTION_IDS` (`:37-43`) deleted along with its only consumer (`NewLayerMenu.svelte:6`, `:17`) and its guard test (`menu_actions.test.ts:152-156`).
- `state/layerPicker.svelte.ts` (12 lines) → `state/addLayerModal.svelte.ts` (`{ open: boolean; tab: string | null }`).

### 2.8 Mount point

`<AddLayerModal />` replaces `<LayerPickers />` at `frontend/src/App.svelte:71`, for the reason `LayerPickers.svelte:8-11` gives: reachable from the palette and menu bar, not only from the panel. Its close handler inherits `LayerPickers.svelte:12-18` verbatim: `refreshLayerTree()`, `refreshVeilList()`, `requestFrame()`. Do not "clean up" `VeilPickerModal.svelte:23-26`'s `addVeil` → `selectVeil(app.veilList.length - 1)` sequence while lifting it; `app.addVeil` already awaits `refreshVeilList` (`state/app.svelte.ts:290-305`), and whether that index is right is a separate question this PR must not silently change.

---

## 3. The spawn-path consequence: the decision that needs the user

**The problem.** Category decides the tab; the tab's source decides the spawn. A deduped entry has one home, so:

- the **`black_and_white` veil** becomes unreachable from the UI;
- the **`chromatic_aberration` filter layer** becomes unreachable from the UI.

Nothing is deleted. Both registrations remain; documents containing either keep loading and rendering (`engine/load.rs:642-660` restores `manifest.veils`; the filter registry is untouched). It is an *add-path* regression only.

**How bad, precisely.** For `black_and_white`: harmless. It is a pointwise tonal remap; the filter-layer form is strictly more capable (exports, maskable, positionable) and the veil form exists only because the veil surface was built first.

For `chromatic_aberration` the loss is real but narrower than it first looks:

1. **CA is still applicable destructively.** `actions/index.ts:760-804` registers a Colors-menu action per filter carrying a `hotkeyAction`; CA declares `"filterChromatic_aberration"` (`gpu/filters/chromatic_aberration.rs:304`) and has params, so it opens the param dialog and bakes into the active layer or mask. Untouched by this plan.
2. **CA is still addable as a veil**: the viewport form, which is what a user reaching for "make this look like a bad lens" most often wants.
3. **What is lost** is the non-destructive, maskable, mid-stack, *exported* CA layer. There is no substitute for that and no other route to it: a filter layer's `pipeline` is fixed at creation (`ui/filters/FilterProperties.svelte` has no pipeline switcher; `addFilter` is the only producer).
4. **Duration**: until PR 4 puts effects in the tree and the divider decides space, at which point it comes back strictly better (it can also be screen-space).

**Prior art says "applicable but not addable as a layer" is a shipped state, declared per effect.** Krita's `KisFiltersModel` builds the category tree straight off the registry and skips filters that decline adjustment layers: `krita/libs/ui/kis_filters_model.cc:62-84`, with the skip at `:66-69` (`if (!showAll && !filter->supportsAdjustmentLayers()) continue;`). The destructive dialog passes `showAll = true` (`libs/ui/dialogs/kis_dlg_filter.cpp:87`); the adjustment-layer dialog passes `false` (`libs/ui/dialogs/kis_dlg_adjustment_layer.cc:66`). The capability defaults to `true` (`libs/image/kis_base_processor.cpp:40`) and is overridden in the filter's own file: `plugins/filters/smalltilesfilter/kis_small_tiles_filter.cpp:47` and `plugins/filters/embossfilter/kis_emboss_filter.cpp:48` both declare `setSupportsAdjustmentLayers(false)`. So Krita ships effects that can be applied but not layered, one shared model, per-effect declaration, no consumer-side branching: CLAUDE.md's Type-owned dispatch rule, in an editor.

### Options considered

| | Option | Verdict |
|---|---|---|
| **(a)** | Accept the regression; CA under Veils per settled Q8 | **Recommended.** |
| **(b)** | Categorize CA as Filters, losing the CA veil instead | Rejected: reverses a settled decision, and pre-PR-4 the Veils tab is the *only* route to a screen-space effect, so this loses the more distinctive form. |
| **(c)** | One card, either destination (secondary affordance / destination toggle) | Rejected: new UI for a two-effect edge case, deleted by PR 4, and it asks the user a question ("tree or viewport?") that the divider is designed to stop asking. |
| **(c′)** | One card under its category tab, always spawning into the tree | Rejected: the Veils tab would create a tree node for CA and a veil-folder entry for the other eight. Pre-PR-4 the tab visibly predicts destination (`ui/layers/LayerPanel.svelte:42-44` mounts `VeilFolder` above the tree), so this is incoherent. |
| **(d)** | Do not dedupe; tabs are add sources; CA and BW appear in two tabs each | **Viable fallback**, see below. |
| **(e)** | Delete one module of each pair | Rejected: `gpu/filters/chromatic_aberration.rs` owns the shared core the veil imports (`gpu/veils/chromatic_aberration.rs:10-13`), and the *veil* modules are the ones closest to PR 2's target trait ("today's `Veil` plus `set_params`"). Deleting either half destroys work PR 2 will merge, and moves the asset counts twice. |

### Resolution: (a), decided; dedupe is the requirement, not an option

**Settled by the user at step 3.** Eliminating the duplicated catalog entry is the
originating complaint this whole effort exists to fix, so option (d) (ship the
modal and let each effect appear in two tabs) does not deliver the feature. The
step-2 review's recommendation to cut the dedupe is **rejected on that ground**,
and the review's supporting argument (S1: pre-PR-4 the tab predicts destination, so
the two entries are two products rather than one duplicate) is noted as true but
not decisive: it describes an implementation artifact of the unmerged registries,
which is exactly the thing being hidden from the user rather than exposed to them.

The review's *mechanical* findings against the draft's dedupe are accepted in full
and fixed by §2.2R: the untested cross-language string comparison (S2), the
undefined no-match failure case (S3), and `category` becoming load-bearing (S4) all
disappear when the effect declares `addable` itself. §2.2R is strictly less
machinery than the pass it replaces, so the review's line-count objection (S11)
also mostly dissolves: see §8R.

**What it costs, accepted:** the CA filter layer is not addable until PR 2, and the
BW veil is not addable at all (it will not survive the merge). CA remains
destructively applicable via the Colors menu and addable as a veil; the review
verified all four mitigations in §3 as true.

**`effect-layers.md` §1.10 is unaffected.** Under §2.2R the category never decides a
destination (`addable` does) so "the category is presentational and nothing else"
stays true throughout the interim and Test 29 is **not** deferred.

---

## 4. Ordered implementation steps

Every step leaves the workspace compiling and the suite green.

**Step 1: categories (Rust).**
1. `pub category: &'static str` on `VeilRegistration` (`gpu/veil.rs:103-117`) and `FilterPipelineRegistration` (`gpu/filter.rs:73-103`), with the doc comment from §2.2.
2. `.with_category(self.category)` in both `catalog_entry()` bodies (`gpu/veil.rs:123-129`, `gpu/filter.rs:112-120`).
3. `pub const CATEGORY: &str = "Filters";` in `gpu/black_and_white.rs` beside `DISPLAY_NAME` (`:19`); `pub const CATEGORY: &str = "Veils";` in `gpu/filters/chromatic_aberration.rs` beside `DESCRIPTION` (`:295`). Both pairs of registrations reference the const.
4. The other twelve declare a literal in their own file.
5. `crates/darkly/tests/docs_export.rs:141`: swap the `None` category column to `some(r.category)` for the `filters` (`:135-150`) and `veils` (`:153-169`) rows.
6. Tests R1, R2 (§6).

**Step 2: the `addLayer` action.** `ActionDef` in `crates/darkly/src/actions/layers.rs` (after `newLayer`, `:4-9`). Frontend registration in `actions/index.ts` with `menuPath: ['Layer:8']`, handler opening the modal on its first tab. Update `menu_actions.test.ts:121-137`.

**Step 3: add sources + rail arithmetic (frontend, no UI yet).** `ui/layers/addSources/{raster,filters,veils,voids,group}.ts` + `index.ts` (glob). `ui/layers/addLayerTabs.ts`. `state/addLayerModal.svelte.ts`. The three catalog-bearing spawn bodies are lifted **verbatim** from `FilterPickerModal.svelte:19-29`, `VeilPickerModal.svelte:17-27` and `VoidPickerModal.svelte:18-66`. Tests F1, F4.

**Step 4: the modal.** `ui/layers/AddLayerModal.svelte`. `<Modal bind:open title="Add Layer" size="lg">` (`ui/Modal.svelte`, `size-lg` is `min(92vw,960px) × min(82vh,720px)` at `:167`). Inside: a search row in the shape of `ui/settings/SettingsModal.svelte:98-105`, then `.main { display: flex; flex-direction: row }` (`SettingsModal.svelte:213-218`) containing `.tab-strip` (`flex-direction: column`, `border-right`, `min-width: 140px` (`:220-228`) with the `.tab` / `.tab.active::after` marker rail (`:229-250`)) and the card grid lifted from `VeilPickerModal.svelte:44-81`. Enter-to-spawn in the shape of `NewDocumentModal.svelte:130-135` bound on the body div (`:139`); safe unqualified because `Modal.svelte:66-76` `stopPropagation()`s every keydown. Escape, backdrop and × are already `Modal.svelte`'s (`:78-80`, `:99-101`). Up/Down move the rail, Left/Right and Tab move within the grid; the `.tab`s are real `<button>`s so tab order needs no `tabindex`. Test F2.

**Step 5: swap the surfaces in.** `App.svelte:14`/`:71`. `LayerFooter.svelte` per §2.6. Retarget `newFilterLayer` / `newVeil` / `newVoid`; delete `NEW_LAYER_ACTION_IDS`.

**Step 6: delete.** `ui/layers/NewLayerMenu.svelte` (92), `ui/layers/LayerPickers.svelte` (27), `state/layerPicker.svelte.ts` (12), `ui/veils/VeilPickerModal.svelte` (82), `ui/filters/FilterPickerModal.svelte` (84), `ui/voids/VoidPickerModal.svelte` (121) = **418**. Update `menu_actions.test.ts` (F3). Confirm no stragglers: the full current reference set is `App.svelte:14,71`; `actions/index.ts:10,37-43,591,597,603`; `LayerFooter.svelte:3,121`; `NewLayerMenu.svelte:6,17`; `menu_actions.test.ts:2,154`. `lib/dismiss.ts` stays (generic); `data-keep-open="new-layer"` was its only add-layer user (`NewLayerMenu.svelte:29`, `LayerFooter.svelte:113-114`) and goes with it.

**Step 7 (docs.** Update `docs/plans/effect-layers.md`: mark PR 3 done, mark Step 6a and the `variant_catalog` half of §5.9 **deferred to PR 4 for re-decision** (not deleted) step-2 review S5), record the CA add-path consequence under Q8, and note that PR 2 still owes the 16 `category:` declarations this PR no longer makes. §1.10's presentational claim and Test 29 need no amendment; Test 29 lands here as R2. Update `handoff-effect-layers.md` §7's sequence table.

---

## 5. What PR 2 deletes from this work

The complete interim seam, so it can be audited:

1. `ui/layers/addSources/veils.ts`: merged into `effects.ts` (`catalog: 'effects'`). One file.
2. `homeCategory` on the two effect sources, and the suppression pass in `addLayerTabs.ts` (~10 lines + its field). One registry means no cross-source duplicates.
3. One of each pair's two `category:` lines, when the modules merge (the shared `CATEGORY` const survives on the merged registration).
4. Test R2 (cross-catalog duplicate resolution) is replaced by `effect-layers.md` Test 28's partition assertion over one catalog.

Everything else (`addLayerTabs.ts`'s four other rules, the modal, the panes, the keyboard, the actions, the deletions, tests R1/F1/F2/F3/F4) carries forward unchanged.

---

## 6. Tests

Per CLAUDE.md: every feature has a test. There is no bug here, so no regression test is owed; F4 is nonetheless new coverage of an existing untested constraint.

**Rust**

- **R1: `crates/darkly/tests/effect_categories.rs` (new), `every_effect_declares_a_category`.** Every `VeilRegistration` and `FilterPipelineRegistration` declares a non-empty `category`, and the projected `CatalogEntry.category` is `Some` for every entry of the `veils` and `filters` catalogs. Pins that a new effect must choose rather than landing nowhere.
- **R2 (same file, `shared_effects_declare_one_category`.** Every `type_id` present in both `gpu::veils::registrations()` and `gpu::filters::registrations()` declares the same category in both) asserted by value *and* by pointer (`std::ptr::eq`) so the shared-const structure is what makes it true, in the shape of `gpu/black_and_white.rs:258-279`. Also: grouping the union of both catalogs by category and applying the resolution rule yields every `type_id` exactly once. This is the "no effect appears under two tabs" guarantee at its source.
- **R3: extend `veil_and_filter_share_one_identity` (`gpu/black_and_white.rs:258`)** with `assert_eq!(veil.category, filter.category)`.
- `crates/darkly/tests/docs_export.rs:141`'s category column now asserts real values for `filters` and `veils`: `export_is_a_faithful_projection` becomes the wire-level proof that categories cross the boundary.

**Asset counts do not move.** `docs_export.rs:363` (`previewable, 46`) and `docs_render.rs:323` / `:340` / `:412` (46, with the per-catalog `{filters: 7, veils: 9, voids: 1, blendModes: 16, brushes: 13}` map) are driven by registry membership, which this plan does not change: the dedupe is a frontend projection. They move 46 → 44 in **PR 2**, when the modules actually merge, along with the `all_forty_six_assets_land` name. Nothing in this PR touches them. This is a deliberate property of doing the dedupe in the UI rather than by deleting a module (§3 option (e)).

**Frontend** (`vitest`; node environment by default (see `src/lib/__tests__/clickOutside.test.ts` for the `vi.stubGlobal('window', …)` pattern) with `// @vitest-environment jsdom` per file where a mount is needed, as `ui/__tests__/transformModeMenu.component.test.ts:1` and `ui/layers/__tests__/maskChain.component.test.ts` already do)

- **F1: `ui/layers/__tests__/addLayerTabs.test.ts`** (node) over `buildTabs`, fed fake sources and catalogs:
  - tabs are ordered by their action's menu order, and the first tab is the one preselected on open;
  - a catalog-less source yields one tab with exactly one synthetic entry carrying the action's display name and icon;
  - a catalog whose entries declare no category yields **one** tab titled by the catalog (the `voids` case);
  - a catalog whose entries declare **two** categories yields two tabs with no change to the modal, the assertion that the rail is already PR-2-shaped;
  - **no `type` appears under two tabs**, with a fixture reproducing the real `black_and_white` / `chromatic_aberration` pair, asserting which source wins each and that the surviving entry keeps that source (which is what makes its preview and its spawn correct);
  - the search filter spans tabs and never resurrects a suppressed entry.
- **F2: `ui/layers/__tests__/addLayerModal.component.test.ts`** (jsdom, mocking `state/app.svelte` with fake catalogs in the shape `maskChain.component.test.ts` uses): mounting yields five tabs titled Normal · Filters · Veils · Voids · Group; **exactly one card named "Chromatic Aberration" exists across the whole modal**; Enter on the default tab dispatches `newLayer` and closes; switching to Voids and pressing Enter calls the void spawn. This is the test that pins the user's actual request end to end.
- **F3: `actions/__tests__/menu_actions.test.ts`** (updated): the Layer-menu order assertion (`:121-137`) gains `addLayer` at the head; the palette-reachability test (`:141-149`) keeps `hit('veil') → newVeil`, `hit('void')`, `hit('filter layer')`; the `NEW_LAYER_ACTION_IDS` test (`:152-156`) is deleted with the constant.
- **F4: `ui/layers/__tests__/addSourceVoids.test.ts`** (node): with `app.acquireMediaStream` and `engine.api.addVoid` both mocked, spawning a `display`-capture void resolves acquisition **before** `addVoid` is called; `markStreamVoidStarted` + `startStreamSource` run on success; the tracks are stopped when `addVoid` returns null. Pins `VoidPickerModal.svelte:20-27`'s constraint, which nothing tests today.

---

## 7. Risks and unresolved questions

1. **`chromatic_aberration` (§3): the one decision that needs the user.** Recommend (a): accept, document, restore at PR 4. Reversible for ten lines; if it is unacceptable, the right answer is to run PR 2 first rather than to complicate this modal.
2. **This PR is ~55 lines of Rust and ~15 lines of frontend that PR 2 deletes** (§5), against a CA add-path regression, in exchange for the dedupe arriving exactly one PR earlier. If PR 2 is genuinely next, that trade is poor and PR 2 should go first. It is a good trade only if PR 2 is not next (it is a session's work by the handoff's own estimate (~1,700 lines across ~40 files)) or if the duplicate is visibly annoying enough now to be worth it. The modal itself is worth doing first either way: it is independent of the registry merge, deletes 418 lines, and its rail rule is the final one.
3. **`import.meta.glob` in production code** has no in-repo precedent outside a test (`__tests__/iconBundle.test.ts:27`). It is standard Vite and resolves under vitest, but if the reviewer objects, the fallback is a five-line `addSources/index.ts` with explicit imports: a hand-written list of exactly the kind CLAUDE.md's Modularity Principle names, which is why it is the fallback and not the default. Note that Krita's own equivalent is a hand-written `if/else` chain over node-type strings (`krita/libs/ui/kis_node_manager.cpp:637-693`) preceded by its authors' apology at `:654-655` (*"XXX: make factories for this kind of stuff, with a registry"*) fed by a hand-written macro list at `:365-390`. GIMP is the same shape for filter categorization (`gimp/menus/image-menu.ui.in.in:694-706` lists `_Blur`'s members by hand). Both are the anti-pattern; neither is copied.
4. **Tab title "Normal": CLOSED.** The rail reads `Normal`, matching what the add-layer UI has always said; there is no reason to change a working label as a side effect of this refactor. `addSources/raster.ts` declares `title: 'Normal'`, overriding the kind's `display_name: "Raster Layer"` (`document/layer_kinds/raster.rs:52`). `Group` needs no override: it matches (`group.rs:35`).
5. **Lifted `pick()` bodies.** `VeilPickerModal.svelte:23-26` calls `selectVeil(app.veilList.length - 1)` after an `addVeil` that already refreshed the list (`app.svelte.ts:290-305`); `veil_list` is documented as highest-index-first (`:295-297`), so whether that index is right is unclear. Lift verbatim; do not fix or "clean up" here.
6. **Preview load.** All three pickers currently show one catalog's cards; the modal shows one tab's cards at a time, and `EffectPreview` requests a still on mount (`ui/EffectPreview.svelte:150-161`). Mount panes lazily (render only the active tab's grid) so opening the modal costs one tab's previews, not five. Cheap; state it so it is not discovered as a stutter.
7. **`svelte-check` is the only gate that type-checks `.svelte` scripts and templates** (CLAUDE.md). A modal this size will not be covered by `tsc --noEmit`; run `npm run check`.
8. **The frontend tree needs `npm install`**: `jsdom`, `mediabunny` and `gifenc` are in `package.json` but missing from `node_modules` (handoff §9). F2 needs `jsdom`. Unrelated to this work but blocking for its tests.

---

## 8. LOC estimate

Lines **added / removed**, not lines touched.

### Production

| Area | + | − |
|---|---:|---:|
| `gpu/veil.rs` + `gpu/filter.rs`, one field each, doc comments, two projections | 14 |, |
| 16 `category:` declarations, 2 shared `CATEGORY` consts with doc comments | 22 |, |
| `crates/darkly/src/actions/layers.rs`, `addLayer` `ActionDef` | 6 |, |
| `ui/layers/AddLayerModal.svelte` | 215 |, |
| `ui/layers/addLayerTabs.ts` | 90 |, |
| `ui/layers/addSources/*.ts` (5 sources + glob index) | 152 | (|
| `state/addLayerModal.svelte.ts` (replaces `layerPicker.svelte.ts`) | 16 | 12 |
| `LayerFooter.svelte`) split button + styles out, plain `+` in | 8 | 52 |
| `actions/index.ts` (`NEW_LAYER_ACTION_IDS` out, `addLayer` in, three retargets | 12 | 15 |
| `App.svelte`) mount swap | 2 | 2 |
| Delete `NewLayerMenu` (92), `LayerPickers` (27), `VeilPickerModal` (82), `FilterPickerModal` (84), `VoidPickerModal` (121) |, | 406 |
| **Production subtotal** | **~537** | **~487** |

**Production net ≈ +50.** Honest range **0 to +110**: the modal component is the only line item with real spread (a leaner rail could reach ~170; a richer one ~260).

Compare `effect-layers.md` §8's PR 3 row (+390 / −464). The delta is +147 added: the 36 lines of category declarations pulled forward from PR 2, ~40 lines of `addLayerTabs.ts` and `addSources/index.ts` that the plan folded into its 340-line lump, and a more conservative modal estimate. Removed is 23 lines lighter because `NewLayerMenu` is 92 not 93 and I have not counted `LayerPickers`' import lines twice.

Option (d) (no dedupe) is **−10 production lines** (the suppression pass and `homeCategory`); it does not change any other row.

### Tests

| Area | + | − |
|---|---:|---:|
| R1 + R2, `crates/darkly/tests/effect_categories.rs` (new) | 90 | (|
| R3) extend `veil_and_filter_share_one_identity`; `docs_export.rs` category columns | 6 | 2 |
| F1, `addLayerTabs.test.ts` | 150 |, |
| F2, `addLayerModal.component.test.ts` (jsdom) | 130 |, |
| F3, `menu_actions.test.ts` updates | 4 | 8 |
| F4, `addSourceVoids.test.ts` | 60 |, |
| **Tests subtotal** | **~440** | **~10** |

### Generated / docs

| Area | + | − |
|---|---:|---:|
| `protocol_gen.ts` | **0** | **0** |
| `docs/plans/unified-layer-picker.md` (this file) | 480 |, |
| `docs/plans/effect-layers.md` + `handoff-effect-layers.md` amendments (§4 Step 7) | 30 | 45 |
| **Generated/docs subtotal** | **~510** | **~45** |

**No protocol regeneration.** `CatalogEntry.category` already exists on the wire (`frontend/src/engine/protocol_gen.ts:468`), and this plan adds no new `CatalogEntry` field: the single biggest saving over `effect-layers.md`'s PR 3, which added `variant_catalog` and regenerated.

**Headline: production ≈ +50 net (537 added, 487 removed); tests ≈ +430; no generated churn.** Four surfaces become one and 406 lines of duplicated modal disappear; the near-zero net is the honest number because a real modal replaces them.
