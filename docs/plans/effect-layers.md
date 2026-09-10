# Effect Layers — one effect, one tree, two spaces

## Independent Review

Step 2 of the CLAUDE.md workflow. Every claim below was checked by opening the
cited file. No production code was changed.

**Verdict: `revise`.** The plan's investigation is the most accurate I have
audited in this repo — all 15 prior-art citations confirm at source, §2.2, §2.4,
§2.5, §2.6 and the frontend line-counts of §2.8 are exact, and the
`crate::catalog::catalogs()` adjudication is correct (`build.rs:258` generates
it, `src/catalog.rs:181` includes it, `engine/veils.rs:130` calls it; a reviewer
grepping the checked-in tree for `fn catalogs` finds nothing, which is how the
earlier false negative happened). The architecture the user fixed is implemented
soundly in outline. But six load-bearing gaps must be closed before
implementation, four of them in the compositor, and the LOC table over-credits
the merge.

### Blocking — the apply pass has no destination

§5.3's table gives `before` and `after` for all four paths but never names a
render target, and there is nowhere for it to write. A ping-pong pair is two
textures. In canvas space, `compose_filter_arm` advances `src → dst`
(`compositor.rs:4391-4401`) and the effect writes `views[src] → views[dst]`
(`:4433-4441`); the apply pass must then read *both* halves, so it needs a third.
Today that third is the mask snapshot, and `lerp_parent_accum_with_mask` writes
back into `1 - after_idx` — the original `src` half (`compositor.rs:4517-4520`).
In screen space there is no snapshot at all: `VeilChain` holds exactly
`textures: Option<[wgpu::Texture; 2]>` (`veil_chain.rs:43-44`) and
`entry.veil.encode(encoder, &entry.cache, current_src, &veil_views[dst])`
(`:385-387`) consumes both. So §5.5's "followed by the in-place apply pass
carrying that layer's opacity and blend mode" cannot be encoded as written.

Worse, Step 4's chosen fix is the expensive one. It says the snapshot is "now
taken **unconditionally** (not only when masked), because opacity/blend need
'before' too". `snapshot_parent_accum` is a full-scissor
`copy_texture_to_texture` (`compositor.rs:4485-4499`) and `mask_snapshot_state`
is currently allocated only for `masked_in_place_hosts()`
(`document/mod.rs:546-570`, which gates on mask *visibility*). Making it
unconditional adds one canvas-sized texture **and one full-canvas copy per
effect layer per frame**.

The simpler general shape is to give the effect its own scratch output instead
of snapshotting its input: effect writes `views[src] → scratch`, apply reads
`(views[src], scratch)` and writes `views[dst]`. Same texture count, one fewer
copy, no per-host snapshot state, and it is identical in both spaces — which is
what §5.3 claims but does not currently deliver. One scratch per space suffices
(the passes are sequential in one encoder), rather than one per host. Rework
§5.3, §5.5 and Step 4 around this; it also removes `snapshot_parent_accum` /
`lerp_parent_accum_with_mask`'s asymmetry rather than widening it.

*Undo-snapshot ordering on the destructive path is fine and is not the risk the
task feared.* `apply_filter_typed` submits `filter-save` and `filter-commit`
before the `apply-filter` encode (`engine/filters/apply.rs:110-133`), reading
the node texture, not any pass output; inserting a third scratch and a second
pass inside `filter_node_region` cannot reorder that. But Step 4 says
`run_filter_region` itself grows the third scratch — and `run_filter_region`
(`compositor.rs:132-217`) is shared with `flip_node_region`, whose closure calls
`ortho_pass.render_mirror_masked` (`compositor.rs:1627-1633`,
`shaders/ortho_transform.wgsl:65`). Either that path silently changes shape, or
the change belongs in `filter_node_region`'s closure and not in
`run_filter_region`. Say which. Note also that `fs_mirror_masked` is a seventh
surviving `*_masked` twin that §1.6's hoist does not reach — hoisting it too
would be the fully general move CLAUDE.md's "place functionality where it
generalizes" asks for, and it is cheap once the apply pass exists.

### Blocking — adding a layer silently converts the whole run to canvas space

Space is derived from tree position, so any insertion above the run ends it.
`add_raster_layer(None)` lands at root top — the document's own test says so
(`document/mod.rs:1439` `add_raster_layer_no_anchor_lands_at_root_top`) — and
with an anchor it lands *above* the anchor (`:1448`
`add_raster_layer_with_layer_anchor_lands_above`). So with three effects in the
run and the top one selected, **New Layer, Paste, Import and Flatten's result
raster all eject the entire run into canvas space**, changing what the user
exports, with no gesture that reads as "move my viewport effects". §1.5 says the
divider "cannot be dragged past a node that is structurally ineligible" but says
nothing about insertion, and §10 does not raise it.

This is the largest consequence of deriving space from position and it needs a
stated policy in the plan — the obvious one being that the divider is an
insertion floor: `add_*_layer` with no anchor lands immediately below the run,
not at root top. That is a change in `document/mod.rs:663`/`:736` and their
siblings, not in the compositor, and it is not in any step or the LOC table.

### Blocking — Flatten and Merge Down consume run members

Step 8 says `compose_children` skips run members and "export, flatten and merge
inherit the exclusion for free", then adds that `bake_subtree_to_layer` "must
not receive run members in its `source_ids`". Neither is sufficient, because the
*document* side of flatten is a separate list. `flatten_image` takes
`top_level = doc.children_of(root_id).to_vec()` (`engine/flatten.rs:17-18`),
passes only the visible subset to the bake (`:56-62`), and then detaches **every
entry of `top_level`** as a source. With the run skipped in the bake, a screen-
space effect would be deleted from the tree without ever being baked — the
user's viewport effects vanish on Flatten Image. Same hazard on Merge Down:
`bake_subtree_to_layer(&[target_id, source_id], result_id)`
(`engine/merge.rs:93`) consumes both, and a run member can be `source_id`.

Move this to `engine/flatten.rs` and `engine/merge.rs` (exclude run members from
the consumed set, or refuse the operation on one), and extend Test 17 to cover
Merge Down as well as Flatten Image — `tests/layer_bake.rs` has merge coverage
at `:133`, `:164`, `:173` to extend.

Two smaller compositor interactions to state while you are there. Isolation:
`compose_children` filters on `is_in_isolation_path` (`compositor.rs:3732`) but
the screen run is driven from `doc.screen_space_run()` and would keep rendering,
so isolating a run member shows the whole canvas with the effect on it. That
matches today's veils and is probably right, but it should be a sentence, not an
accident. Vacuous truth: "passthrough group → true iff every child answers
`true`" makes an *empty* group at the top a run member. Harmless, but state the
choice.

### Blocking — §5.6 is a regression, not only a behaviour change

Q2 frames straight-alpha present as a taste question. It is not. Today
`fs_present` returns `select(vec4f(composed, 1.0), view.bg, oob)`
(`shaders/present.wgsl:72`), so the chain's input is opaque *everywhere*, and
`present_to_veil_pipeline` compiles that same `fs_present` entry point for
`accum_format` (`compositor.rs:1091-1109`). Feed the run straight alpha instead
and three things break that cannot break today:

1. The reduced-resolution path downscales through a 4-tap filter
   (`veil_chain.rs:365-371`, `create_downscale_pipeline`) on **straight-alpha**
   data, so the arbitrary RGB of transparent texels bleeds inward — §2.6's
   fringing, newly introduced into the viewport at the default
   `rendering.veil_scale: 0.7071` (`presets/defaults.yaml:125`).
2. `blit_pass` clears to `wgpu::Color::BLACK` (`veil_chain.rs:658`), so every
   spatial screen-space effect pulls transparent black across the canvas border.
3. `lens_blur` normalizes RGB *by the alpha channel* (`shaders/veils/lens_blur.wgsl:73`,
   `:82-83`) — with varying alpha its colour, not just its alpha, is wrong. The
   plan already knows this (§2.4) but applies the knowledge only to canvas space.

Recommendation on **Q2: do not do §5.6 in this work.** Let the run keep
operating on the presented (checker-composited) image. The two spaces genuinely
are different things — one is document content, one is a viewport treatment —
and Q4's divider labelling already tells the user so. If matched appearance
across the divider is wanted later, it needs a premultiply-before /
unpremultiply-after design that this plan does not have, and it should be its
own plan. Dropping §5.6 also removes `fs_present_raw`, the blit's new view-
uniform binding, and part of Step 8.

### The one-trait collapse holds, with two internal contradictions to fix

The invocation-contract isomorphism of §2.1 is real and I confirm it:
`accum_format = Rgba8Unorm` at `compositor.rs:890` and `:1090` is the same value
handed to `VeilChain::new` at `:1161`, `gs.accum.views` is a `[TextureView; 2]`
(`:262-266`), and `run_filter_region` already allocates its pair at `:167-168`.
Per-apply cache construction is acceptable: `create_cache` takes `&mut self`, so
the destructive path builds a fresh boxed instance per apply exactly as
`VeilSession::build` does (`gpu/veil.rs:317-329`). R8 as a declared target list
is right — `ParamFilter` hardcodes `Rgba8Unorm` in its `ColorTargetState`
(`param_filter.rs:228-235`) purely because a pipeline is compiled against one
format, and `VeilRegistration.create_pipeline` already takes the format
(`gpu/veil.rs:114`). No consumer ends up branching on capability.

Two things in §5.1/§5.2 contradict themselves and §4:

- §4 claims "two preview mechanisms → one". There is already exactly one:
  `PreviewMechanism` (`gpu/preview.rs:681`) and `PreviewSession` (`:706`), which
  both `VeilMechanism` (`gpu/veil.rs:278`) and `FilterMechanism` implement. What
  collapses is two *implementations* of one trait — a free consequence of one
  registry, not an independent DRY win. Restate.
- §5.1 keeps `Effect::preview_at(&mut self, queue, cache, t) -> bool` *and*
  §5.2 keeps `preview_at: Option<fn(f32) -> Vec<ParamValue>>` on the
  registration. Two differently-shaped things with the same name survive the
  merge. Pick one (the registration function plus `set_params` subsumes the
  trait method) or rename. And §5.2's comment "No capability enum, no `Option`
  fields that can both be `None`, no illegal state" is false of the struct
  directly beneath it: `preview` and `preview_at` are both `Option` and can both
  be `None`.

Also unaccounted for: `EffectRegistration` makes `icon` and `hotkey_action`
non-optional, but `VeilRegistration` (`gpu/veil.rs:101-116`) has neither. Ten
effects need new icon strings, new action ids, and bindings in
`presets/{krita,photoshop,gimp}.yaml` — and because the Colors menu is built by
looping the catalog (`frontend/src/actions/index.ts:760-805`), those ten become
ten new destructive-apply menu entries. That is arguably §1.6's headline
consequence working as intended, but it is a visible UI change absent from
§6 and the LOC table.

### Step ordering inside PR 2 does not satisfy its own dependencies

§6 asserts "every step leaves the workspace compiling", but Step 4 says "drive
the instance via `ScaledEffect::encode`" — which Step 5 creates — and driving an
instance at all requires `effect_instances`, which Step 6 creates. Since
`Effect::create_cache` binds against specific views, `compose_effect_arm` cannot
exist before the instance map does. Reorder to 2, 3, 6, 5, 4.

### LOC: −1,400 is roughly the right shape, but two credits are unearned

- **The picker-modal merge is available today, standalone.** §4 credits the plan
  with removing "two picker modals with byte-identical `<style>` blocks". The
  duplication is real and larger than described — `VeilPickerModal.svelte:44-82`
  and `FilterPickerModal.svelte:46-84` diff empty, and the whole component is a
  clone modulo the catalog string, the title and the `pick()` body — but
  `EffectPreview.svelte` already takes `catalog` as a string prop and
  `app.entries(catalogId)` is already generic, so one `EffectPickerModal` with
  `{catalog, title, onpick}` props collapses both **with zero engine change and
  no dependency on any part of this plan**. Roughly 120 lines the merge should
  not claim. (`VoidPickerModal` is a third candidate, per
  `frontend/src/ui/layers/LayerPickers.svelte:21-27`.)
- **The costs above are missing from the table**: the third scratch target in
  both spaces, the `add_*_layer` insertion policy, the flatten/merge exclusions
  in `engine/`, ten icon + hotkey-action declarations and their preset bindings.
  Offsetting that, dropping §5.6 removes work. Net, treat **−900 to −1,600** as
  the honest range rather than −1,000 to −1,700, and re-derive after the §5.3
  rework.
- Everything else in §2.8 checks to the line: `ui/veils/` is 148 + 237 + 82 + 21
  = 488, `ui/filters/` is 1,393 source + 243 test, and `actions/index.ts:37-43`,
  `:594-598`, `:674-677`, `:760-805` are all exact.

### §2.8 is wrong about the veil folder, and §3's motivation depends on it

`VeilFolder` is **not** mounted unconditionally. `frontend/src/ui/layers/LayerPanel.svelte:42`
is `{#if app.veilList.length > 0}`, `:43` mounts the folder, `:44` closes the
guard — the plan cites exactly the guard block it says does not exist. Since the
`app` recipe seeds no veils (`frontend/src/state/freshDocument.ts:61`), **a fresh
document does not contain an empty Veils folder today.** §3's first bullet
("No empty folder in a fresh document") and §1.5's "a fresh document should not
contain an empty folder for a feature the user has not asked for" are therefore
arguing against a problem that does not exist. The architecture is the user's
call and is not in question, but the plan should not justify it with a false
premise; the honest arguments are the remaining four bullets of §3, which stand
on their own. Two smaller cites in the same section: `activeVeilIndex` is
`app.svelte.ts:327` (`:325` is its comment) and `veilList` is `:357` (`:356` is
its comment); the six reset sites and all seven wire methods are exact.

One thing §2.8 misses: deleting the seven veil methods does **not** retire the
veil concept from the wire. The `'veils'` catalog id crosses through the
*generic* `catalogs()` (`protocol_gen.ts:1266`) and `PreviewReq { catalog }`
(`:677`), consumed as `catalog="veils"` in `EffectPreview.svelte`. That is a
rename with frontend consumers, not a delete — fold it into §5.2's `CATALOG_ID`
change and PR 4.

### Tests

Confirmed genuinely net-new and non-vacuous: **7** (blend/opacity — no test in
`tests/` ever sets opacity or blend on a filter layer; `add_filter_layer` appears
only in `tests/filters.rs` and `tests/layer_capabilities.rs:46`, neither varying
either), **8**, **9** (`perf_scale_factor` has zero hits under `crates/darkly/tests/`),
**5**, **12**, **13**, **14**, **15**, **22**.

Confirmed accurate: the 47 → 45 arithmetic (`tests/docs_render.rs:333-334` assert
`("filters", 7)` and `("veils", 10)`, `:340` asserts the sum is 47; 47 − 17 + 15
= 45), and the claim that `present_into_target` (`compositor.rs:3445-3499`) does
not run the chain. Test 12's harness is small — ~20-30 lines, since
`VeilChain::encode` is `&self` (`veil_chain.rs:322`) so there is no borrow
conflict with `&self.present_to_veil_pipeline`; the only real work is ensuring
the chain has been sized for `w × h`, because `encode` unwraps `self.views`
(`:330`). Note two present-level assertions already exist to model it on:
`tests/engine.rs:4038` and `:4086`, via `engine/mod.rs:1107` `test_readback_present`.

Problems:

1. **Tests 2, 3 (isolated half) and 4 largely already exist.**
   `tests/filters.rs:457` `filter_layer_does_not_affect_layers_above_it`, `:483`
   `filter_layer_in_isolated_group_is_scoped`, `:536`
   `masked_filter_layer_confines_inversion` and `:595`
   `masked_filter_layer_in_isolated_group_lerps_against_group_accum`. Only the
   *passthrough* half of Test 3 is new. Retarget rather than re-write, and say so.
2. **Test 20 is in the wrong file.** `tests/schema_contracts.rs` is 119 lines
   about config prefs (`no_duplicate_pref_keys:27`, `every_pref_has_a_resolvable_value:46`,
   `overlay_names_unique:70`, `app_base_settings_options_match_overlays:79`) and
   imports only `darkly::config::*`. Per-catalog counts already live in
   `docs_render.rs:330-340`. Fold Test 20 there.
3. **Test 23 names two files that do not enumerate catalogs.**
   `tests/shader_compile.rs` is filesystem-driven (`std::fs::read_dir` at `:5`)
   and `tests/wgsl_validate.rs` enumerates `builtin_brushes::all()` (`:22`) only.
   Conversely `tests/picker_preview.rs` *is* dynamic over
   `darkly::catalog::catalogs()` (`:256`) and needs **no edit** — it follows the
   merge automatically. Test 23 overstates work on two files and understates the
   coverage already free on the third.
4. **Test 19's premise is overstated.** `tests/chromatic_aberration.rs` is 11
   tests driving the *filter* through `apply_filter_typed` and exactly one veil
   smoke test, `veil_produces_non_identity_output:176`. The collapse's risk is a
   regression on the *veil* side (`views[0]/[1]`, `create_cache`, `encode`), and
   that side has one non-identity assertion. Add a veil-path property test.
5. **Test 21 deletes a guard that survives.** `shared_effects_share_one_preview`
   (`docs_render.rs:184-206`) has two halves: filter-vs-veil preview equality
   (obsolete after the merge) and, at `:195-205`, a filter↔shared-module
   `preview_params` drift guard that is still meaningful. Keep the second half.
   Also `docs_export.rs:363` is a single total, not per-catalog counts, and its
   failure message at `:364-365` spells out "7 filters + 10 veils + …" and must
   be edited too or it will lie.
6. **Test 22 has no home in `engine.rs`.** There is no `requires` coverage under
   `crates/darkly/tests/` at all; the only existing one is the in-file unit test
   `requires_inventory_collects_used_modules` (`engine/save.rs:550-590`, asserting
   `requires.veil` lists `grain` at `:572`). Put the regression there. The bug
   itself is confirmed real — `requires_from_doc` (`:430-470`) collects
   `layer_kind`/`blend_mode` from `Entity::Node` (`:439-440`), `modifier` from
   `Entity::Filter` (`:443`) and `veil` from the chain (`:448-453`), and a filter
   layer's `pipeline` string is recorded nowhere — and the plan correctly treats
   it as a **bug** with a regression test written first, satisfying the Testing
   Principle. The false doc comment it corrects is at
   `document/layer_kinds/filter.rs:10-13`, exact.
7. **Missing tests** for the three blocking findings: a new layer added at root
   top does not silently move the run out of screen space; Merge Down does not
   consume a run member (Test 17 covers Flatten only); and the apply pass's
   third target survives a canvas resize (Test 16 covers the accumulator but not
   the run's own textures, which `ScreenRun::ensure_textures` recreates
   independently of `create_group_state` — `set_canvas_rect:1861-1865` does not
   touch them).

Two notes on PR 1, which is otherwise clean and independently shippable. The
`lens_blur` rewrite genuinely is bit-identical in the viewport: `acc.a` today is
`Σ exp(s.a · inv_t)` with `s.a == 1.0`, so a constant `exp(inv_t)` weight gives
the same sum (`shaders/veils/lens_blur.wgsl:73`, `:82-83`). But the plan does not
say *what alpha* it carries. For a blur the correct alpha is a linear mean
(coverage); reusing the exponential smooth-max would over-weight opaque samples
and inflate coverage at edges. Specify it, and have Test 6's alpha-edge sample
be a value a smooth-max would get wrong, so the test actually discriminates.

### Prior art — clean, and stronger than the plan claims

All 15 citations in §11 confirm verbatim, including the confirmed negatives (5
GIMP display filters, exactly one `KisDisplayFilter` implementation, zero
splitter hits in either layer docker). The strongest claim survives hard
scrutiny and is *understated*: `copyAreaOptimized`'s 5-arg overload
(`krita/libs/image/kis_painter.cc:169`) sets `COMPOSITE_COPY`, whose math is
`opacity = mul(maskAlpha, opacity)` then `lerp` on premultiplied channels
(`krita/libs/pigment/compositeops/KoCompositeOpCopy2.h:40`, `:63`, `:81`) — a
replace-then-lerp, exactly §5.3. Cite it; right now §5.3 rests on the call site's
name. GIMP has the same proof one call past where §11 stops:
`GIMP_LAYER_MODE_REPLACE` is `new_alpha = (layer[alpha] - in[alpha]) * opacity + in[alpha]`
(`gimp/app/operations/layer-modes/gimpoperationreplace.c:257`, `:265`, mask folded
in at `:255`, and a pure-replace short-circuit at `:146-150`). Both editors
independently default an adjustment/filter layer to replace-then-lerp. Nothing is
stretched onto the screen/canvas duality — §4 and §11's calibration paragraphs
are honest. Optional hardening: name and dispose of `KisReferenceImagesLayer`
(`krita/libs/ui/flake/KisReferenceImagesLayer.h:17`, drawn by
`KisReferenceImagesDecoration.cpp:127` through `imageToWidgetTransform()`) as the
one near-miss a Krita-literate reader will raise — it is image-anchored, so it
falls on the right side of the line, but silence invites the objection.

### Recommendations on the three open questions

**Q1 — mask presence, not visibility. Agree with the plan.** The teleport
argument is sound, and presence is also the doc-side structural fact the
Document Authority Principle already names (`has_mask`), whereas visibility is
per-frame session-ish state. The divergence from `masked_in_place_hosts`
(`document/mod.rs:554`, which gates on visibility) is harmless: that predicate
sizes a GPU resource, this one decides a coordinate frame. Worth one sentence in
§1.3 noting that a *technically* mask-capable screen space exists — `fs_present`
already carries the inverse view matrix (`present.wgsl:29-38`), so a future
version could sample a canvas-space mask through it — so the exclusion reads as
a scoping decision rather than an impossibility.

**Q2 — no. Do not present straight alpha into the run.** See the blocking
finding above: at the shipped default scale it is a visible regression at the
canvas border and it breaks `lens_blur` in the viewport. Keep the run on the
presented image, drop §5.6, and lean on Q4's labelling to explain the boundary.

**Q3 — neither of the plan's two options; move the pin up to `NodeCommon`.**
Risk 6 exists only because `pinned` is declared on `EffectLayer` alone. The
plan's own §1.3 already has to special-case groups ("passthrough group → true
iff every child…"), and Q3's compound-undo answer is lossy by the plan's own
admission. `NodeCommon` is `{ name, visible, locked }` (`layer.rs:76-80`) and
already carries exactly this class of cross-kind structural flag; adding
`pinned: bool` makes a group pinnable as a unit, makes the divider drag one
property edit instead of a `CompoundAction` over a subtree, and makes
`supports_screen_space` uniform across variants. Cost is the `pinned` field in
each of the five `layer_kinds/*.rs` serializers (e.g. `filter.rs:60-68`,
`:88-92`) — five small additive edits against removing a whole risk, a whole
open question and a compound-undo path. If that is judged too wide, the
fallback should be "the divider clamps at group boundaries in v1", not the
compound pin.

### PR sequence

Keep four PRs; PR 1 is genuinely independent and should land first. Two changes:

- **Reorder PR 2 internally** to 2, 3, 6, 5, 4 (above), and move the picker-modal
  merge out of the credit column — it can land any time.
- **PR 2 does ship the user's complaint**, with one caveat to state: once
  `CATALOG_ID` becomes `"effects"`, both surviving pickers read the same catalog,
  so "Add Veil" will offer `curves` and "Add Filter Layer" will offer
  `watercolor` until PR 3 Step 10 merges them. The duplicate *entries* are gone,
  which is the reported bug, but the duplicate *surface* is not. Either say so,
  or pull the picker merge forward into PR 2 (it has no dependency on PR 3).

### Not findings — verified and correct

§2.2 (`run_filter_region:132-217`, scratches at `:167-168`), §2.3, §2.4 (all six
lines exact, and `black_and_white:24` / `grain:95` do carry alpha), §2.5, §2.6,
§2.7, `composites_in_place` at `layer.rs:715-721`, `FilterLayer::blend` at
`layer.rs:238` never read by `compose_filter_arm:4359-4454`,
`build_composite_source` at `blend_mode.rs:171-186`, `build.rs:81-101`,
`config/sections/rendering.rs:4`, `animation.rs:7`/`:23`,
`defaults.yaml:121`/`:123`/`:125`, `set_canvas_rect` recreating every
`GroupState` at `:1861-1865` (so `target_generation` bumped in
`create_group_state` really is the single canvas-space choke point — note it
also means creating any group invalidates every effect instance including
screen-space ones, which is safe but wasteful), `compose_children:3711-3743` with
its two existing filters at `:3725` and `:3732`, and `pointerDrag.ts` at 83 lines
being genuinely axis- and container-agnostic (a second consumer already exists at
`frontend/src/ui/Modal.svelte:96`, which the plan does not mention and which
strengthens its case).

revise

---

## Revision Log

Step 3 of the CLAUDE.md workflow. The review above is preserved verbatim. Every
claim below was checked by opening the cited file; where a finding was rejected,
the source that refutes it is cited. No production code was changed.

### A. The model change: a stored boundary, not a derived run

The user replaced the derived trailing-run model after the review. The old model
computed the screen-space run by walking the root group's trailing children and
used a per-layer `pinned` bool to end the walk early. The new model stores the
boundary as one piece of document state.

The governing correction: **a raster layer cannot exist in screen space,
therefore it can never be placed above the boundary.** Nothing can eject the
run, because neither the document model nor the UI permits a non-qualifying node
above the line.

What changed in the plan:

- §1.3 no longer derives space. It defines `Document::screen_space_count`, a
  stored `usize`, and demotes `supports_screen_space()` to a validation
  predicate. §1.3 also works the representation question (count vs index vs
  marker vs anchor id) and justifies the count.
- §1.5 is rewritten: "Pinning, and the divider" → "The boundary". `pinned` is
  gone; moving the divider *is* the boundary edit.
- A new §1.9 enumerates every mutation path that can place a node among the
  root's children and states where enforcement lives — `Document::link` /
  `Document::unlink` (`document/mod.rs:982`, `:1003`), the single pair every
  structural change funnels through, plus one explicit clamp in `engine/load.rs`
  because load bypasses the pair entirely (`load.rs:202-206`, `:371-390`).
- §5.4, §5.5, §5.8, §6, §7, §8, §9, §10 follow.

Dissolved, and recorded rather than silently dropped:

- **The review's "adding a layer silently converts the whole run to canvas
  space" finding.** Dissolved. Under a stored boundary such a layer cannot land
  above the divider at all: `Document::link` clamps it. The review's diagnosis
  of today's behaviour was exact — `add_raster_layer(None)` resolves to
  `MoveTarget::IntoGroupTop(root)` (`document/mod.rs:1023-1025`) and
  `attach_at_target` links with `position: None`, which `attach_child` turns
  into `list.len()` (`layer.rs:643-644`) — but the consequence it feared is
  unreachable once the boundary is stored.
- **The "insertion-floor policy" the review proposed** for `add_*_layer`.
  Dissolved into the same clamp, and generalized: the clamp lives in `link`, so
  it covers every add path, paste, duplicate, import, group and drag-reorder
  without any of them being edited. This is strictly better than the review's
  proposal, which would have edited `document/mod.rs:663`/`:736` and their three
  siblings individually.
- **Review Q3 (dragging the divider across a group) and its compound-undo
  concern.** Dissolved with `pinned`. There is no per-node flag to fan out over
  a subtree, so a divider drag is one scalar edit and one `UndoAction` — not a
  `CompoundAction` over a subtree, and not lossy. §9's Risk 6 ("Groups have no
  pin") is deleted; the slot is reused for the one genuinely new risk the stored
  boundary introduces — `engine/load.rs` bypasses the enforcement pair. A
  *different* Q3 now occupies that number: undo fidelity across the boundary
  (§1.9).
- **The trailing-run walk in the compositor.** The run is now a slice read
  straight off the stored count (§5.5).

### B. Review findings

1. **The apply pass has no destination — accepted, and the reviewer's fix
   adopted with one correction.** Confirmed at source: `VeilChain` holds
   `textures: Option<[wgpu::Texture; 2]>` (`gpu/veil_chain.rs:43`) and
   `encode` consumes both halves (`:387`), so there is no third target in
   screen space. §5.3, §5.5 and Step 4 are reworked around a scratch *output*:
   the effect writes `views[src] → scratch`, the apply pass reads
   `(views[src], scratch)` and writes `views[dst]`. One scratch per space, no
   per-host snapshot, no full-canvas copy per effect per frame.

   The correction: the reviewer implies `snapshot_parent_accum` /
   `lerp_parent_accum_with_mask` are an asymmetry to be removed outright. Half
   of that is wrong. `compose_passthrough_masked` (`compositor.rs:4693-4715`)
   *cannot* use a scratch output, because its "after" is produced by an
   arbitrary number of child passes writing into the accumulator itself
   (`:4712` `compose_children`). The snapshot survives **for masked passthrough
   groups only**; the effect-layer path stops using it. The apply pass is
   shared by both — `before` and `after` are just two bound views. §5.3 states
   this.

   *Where the third scratch lives on the destructive path — the reviewer asked
   us to say which.* Neither `run_filter_region` nor a widened shared helper:
   it lives inside `filter_node_region`'s closure. `run_filter_region`
   (`compositor.rs:132-217`) hands the closure `src_view` and `out_view` and
   copies `out_scratch` back (`:197-214`); the closure can allocate one
   region-sized intermediate, write the effect into it, then write the apply
   result into `out_view`. `run_filter_region` keeps its two scratches and
   `flip_node_region` (`:1609-1638`) is untouched.

   *Rejected: hoisting `fs_mirror_masked` into the shared apply pass.* The
   reviewer calls it "the fully general move … cheap once the apply pass
   exists". It is neither. `shaders/ortho_transform.wgsl:1-16` states the
   contract — "Flips and 90° rotations relabel texels without resampling, so
   every fetch is a `textureLoad` at an integer source index — no sampler, no
   filtering… Output is bit-identical to the input up to the permutation, which
   is the whole point". `fs_mirror_masked` (`:65-74`) is a hard
   `select(dst, mirror, mask.r > 0.5)` on *source indices*, then one
   `textureLoad`. The shared apply pass is `mix(before, after, mask)` on
   *sampled colours*. Substituting it would (a) blend two different texels at
   every soft-mask texel, inventing colours the permutation contract forbids,
   (b) require a third region texture where zero are needed today, and (c) cost
   a texture read the gather form avoids. It is a seventh `*_masked` twin by
   name only; its body is a different operation. Left alone.

2. **Flatten and Merge Down consume boundary-crossing nodes — accepted,
   confirmed, and the fix placed in the engine.** `flatten_image` snapshots
   `let top_level: Vec<LayerId> = self.doc.children_of(root_id).to_vec();`
   (`engine/flatten.rs:18`), passes only the visible subset to the bake
   (`:27-36`, `:56-62`) and then detaches **every** entry of `top_level`
   (`:67-83`). `merge_down` bakes `&[target_id, source_id]`
   (`engine/merge.rs:93-99`) and `merge_layers` bakes an arbitrary id list
   (`:238`); either can name a run member. Fixed in §6 Step 8 in
   `engine/flatten.rs` and `engine/merge.rs`, not the compositor.

   `flatten.rs:41`'s `add_raster_layer(None)` result placement needs **no
   change**: `:87-88` immediately re-homes it with
   `reinsert_entity(result_id, Some(root_id), 0)` — position 0 is the *bottom*
   of the bottom-to-top child list (`document/layer_kinds/group.rs:26-27`), so
   it is unconditionally below the boundary. Stated in Step 8 so a reader does
   not have to re-derive it.

3. **§5.6 is a regression — accepted after independent verification, and
   dropped.** All three sub-claims confirmed: `fs_present` returns
   `select(vec4f(composed, 1.0), view.bg, oob)` (`shaders/present.wgsl:72`), so
   the chain's input is opaque everywhere; the reduced-resolution path runs a
   multi-tap downscale (`gpu/veil_chain.rs:365-371` calling the pipeline built
   at `gpu/effect.rs:117-135`, whose own doc comment gives the anti-aliasing
   rationale) at the shipped default `rendering.veil_scale: 0.7071`
   (`presets/defaults.yaml:125`, schema `config/sections/rendering.rs:4`);
   `blit_pass` clears to `wgpu::Color::BLACK` (`gpu/veil_chain.rs:658`) and so
   does the final surface blit (`:404`); and `lens_blur` divides RGB by the
   alpha channel it accumulated (`shaders/veils/lens_blur.wgsl:73`, `:82-83`,
   with the shader's own comment at `:78-81` naming the `alpha == 1.0`
   assumption). §5.6 is deleted; §2.7 is rewritten to record the divergence as
   a known, deliberate property of the two spaces; Q2 is closed "no". This
   removes `fs_present_raw`, the blit's new view-uniform binding, and part of
   Step 8.

4. **LOC over-credit — accepted, confirmed, and the merge moved forward
   instead.** `frontend/src/ui/EffectPreview.svelte:21` is
   `let { catalog, entry }: { catalog: string; entry: PreviewEntry } = $props();`
   and `VeilPickerModal.svelte:16` is `app.entries?.('veils') ?? []` — the
   component is already catalog-generic, so one `EffectPickerModal` collapses
   both today with no engine change. The ~120 lines are struck from the credit
   column (§8) and the merge is pulled into PR 2, which also closes finding 6's
   duplicate-surface caveat. §4's DRY bullet no longer claims it.

5. **Mask presence, not visibility — adopted.** §1.3 states the reasoning: an
   eye toggle is a small control and moving a layer between coordinate spaces
   is a large, exported-output-changing consequence; presence is the
   structural, document-authoritative fact (`has_mask`), visibility is
   per-frame. The divergence from `masked_in_place_hosts`
   (`document/mod.rs:554-559`, which does gate on visibility) is harmless and
   is noted inline: that predicate sizes a GPU resource, this one decides a
   coordinate frame. Q1 is closed.

6. **PR sequencing — accepted, with the reviewer's ordering corrected.** The
   dependency finding is right: Step 4 drives `ScaledEffect::encode` (Step 5)
   over `effect_instances` (Step 6). But the reviewer's proposed order
   **2, 3, 6, 5, 4 is itself unbuildable**: §5.4's `EffectInstance` has a
   `scaled: ScaledEffect` field, so Step 6 depends on Step 5, not the reverse.
   Corrected to **2, 3, 5, 6, 4**. The duplicate-*surface* observation is
   recorded in §6 and resolved by pulling the picker merge into PR 2 (finding
   4).

7. **The empty-folder motivation — the review is half wrong, corrected in
   both directions.** `frontend/src/ui/layers/LayerPanel.svelte:42` is
   `{#if app.veilList.length > 0}` — the reviewer is right that `VeilFolder` is
   **not** mounted unconditionally, and §2.8's claim that it is has been fixed.
   But the reviewer's conclusion ("a fresh document does not contain an empty
   Veils folder today") holds only for the **app** flavor. The **demo** flavor's
   recipe seeds four hidden veils into every fresh document —
   `frontend/src/state/freshDocument.ts:47-50` calls `addVeil` four times with
   `visible: false`, under `RECIPES.demo` (`:35`), selected by
   `RECIPES[deployMode]` (`:66`). `veilList.length` is therefore 4 and the
   folder *does* appear in every fresh demo document, holding four things the
   user never asked for. §3's first bullet is rewritten to say exactly that
   rather than the false unconditional claim. The two smaller cites are fixed
   too: `activeVeilIndex` is `app.svelte.ts:327` and `veilList` is `:357`.

8. **Carried-forward items — all confirmed to survive.**
   - The six shaders hard-coding `alpha = 1.0`: re-read, all six exact —
     `vhs.wgsl:118`, `rainy_glass.wgsl:226`, `watercolor.wgsl:75`,
     `frozen.wgsl:79`, `lens_blur.wgsl:83`, `painting.wgsl:127`; and
     `black_and_white.wgsl:24` / `grain.wgsl:95` do carry `color.a`. PR 1 is
     unchanged and still lands first.
   - The `requires_from_doc` gap: confirmed. The node arm records only
     `layer_kinds.insert(node.type_id())` and the blend mode
     (`engine/save.rs:438-441`); `node.type_id()` for a filter layer resolves to
     the generic layer-kind id (`layer.rs:681`), never the per-layer
     `pub pipeline: String` (`layer.rs:243`). Treated as a **pre-existing bug
     with its regression test written first**, and the test is relocated to the
     in-file unit test's home per the review (Test 22).
   - Straight-alpha edge fringing: characterized by Test 6, not solved. Retained.

### C. The adjacent `add_raster_layer(None)` claim — refuted at source

The task flagged `engine/clipboard.rs:485-486` and `engine/floating.rs:258` as a
genuine pre-existing bug: a comment reading "Create a new layer and insert above
the active layer" sitting directly above `add_raster_layer(None)`, which lands
at root top instead. **There is no behavioural bug.** Both functions re-home the
layer a few statements later, before returning and before any undo entry is
pushed:

- `clipboard.rs:506-507` — `let target = self.doc.resolve_anchor_target(active_layer_id);`
  then `self.doc.move_layer(id, target);`, with the undo action recorded from
  the *final* position at `:509-511`.
- `floating.rs:274-275` — the same two lines, verbatim.

The same shape appears at `engine/duplicate.rs:71` + `:83-84`,
`engine/flatten.rs:41` + `:87-88`, `engine/merge.rs:74` + `:134-136` and
`engine/layers.rs:656` + `:692-698`: create with no anchor, then land it
exactly. It is the codebase's established idiom, not an oversight. The only
real defect is that `clipboard.rs:485`'s comment describes the net effect of the
whole block while sitting on the one line that does not produce it. That is a
comment-accuracy fix of one line; **it is not in this plan's scope and warrants
no regression test**, because the Testing Principle's "every bug must have a
regression test" presupposes a bug, and no assertion about final placement fails
today. Under the stored boundary the transient root-top landing is additionally
clamped by `link` (§1.9) and then moved into place as before, so the idiom
survives the model change unchanged.

### D. "Veil" survives in the UI, and one add-layer modal

Two scoped changes made after B and C, both originating with the user rather
than with the review. Everything in A, B and C stands unchanged; nothing in the
compositor, the boundary, the format or the trait is touched by either.

#### D1 — the code-side merge is unchanged; the word "Veil" is not retired

The user corrected the plan's proposal to retire "veil" everywhere: *"we are not
deleting the concept of a veil. If you are DRYifying on the code side that is
fine. But users are not used to seeing animated distortion effects in a paint
program and as such they will continue to be called veils in the UI."*

Unchanged: one trait, one registry, one catalog, one `gpu/effects/` directory,
one layer kind, `black_and_white` and `chromatic_aberration` collapsing to one
each, and the retirement of `manifest.veils`, the seven veil wire methods, the
`veils` catalog id, `rendering.veil_scale` and `animation.veil_divisor`.

Changed:

- **§1.8 is rewritten** from "both words are retired" to "both words are retired
  *as code identifiers*; both survive as UI categories".
- **New §1.10** works the category set from the actual 15 effects and states the
  mechanism. Six `"Filters"` (pointwise colour/tone remaps: `black_and_white`,
  `brightness_contrast`, `curves`, `hsv`, `invert`, `levels`), nine `"Veils"`
  (spatial and/or animated: `chromatic_aberration`, `frozen`, `grain`,
  `lens_blur`, `painting`, `pixelate`, `rainy_glass`, `vhs`, `watercolor`). The
  dividing rule states itself: a Filter is a function of one texel's colour; a
  Veil reads its neighbours, its clock, or both.
- **The mechanism needed no new machinery, which is the finding that shaped it.**
  `CatalogEntry::category: Option<&'static str>` already exists —
  *"Grouping label within the catalog, for variants that group"*
  (`crates/darkly/src/catalog.rs:32-33`) — with `with_category()` at `:86-89`,
  already serialized camelCase (`frontend/src/engine/protocol_gen.ts:468`).
  `BlendModeRegistration` already uses it exactly this way: `category: &'static str`
  (`gpu/blend_mode.rs:35`), declared per variant (`gpu/blend_modes/multiply.rs:10`
  — `category: "Darken"`), projected at `:72`, and grouped on the frontend with
  no hand-written list at `frontend/src/ui/properties/LayerProperties.svelte:20-33`.
  So `EffectRegistration.category` is one field and 15 declarations, not a
  subsystem. An earlier sketch of an `EffectCategory` enum with `display_name()`
  and `order()` methods was discarded on finding this.
- **Single category, not a slice** — decided and justified in §1.10. The
  deciding argument is that the bug this plan exists to fix is "one effect
  appears twice in my UI"; a multi-homed effect appears twice in the *same
  modal*, one tab apart. Prior art agrees: Krita's
  `KisFilter(const KoID& id, const KoID & category, const QString & entry)`
  (`krita/libs/image/filter/kis_filter.h:33`) takes exactly one, and a grep of
  `krita/plugins/` for `FiltersCategory[A-Za-z]*Id` returns 46 occurrences across
  nine categories with **no filter declaring two**. Krita's own assignments also
  corroborate the split entry for entry: raindrops, pixelize and oilpaint →
  Artistic (`kis_raindrops_filter.cpp:47`, `kis_pixelize_filter.cpp:45`,
  `kis_oilpaint_filter.cpp:44`), lens blur → Blur
  (`kis_lens_blur_filter.cpp:33`), levels / desaturate / per-channel curves /
  invert → Adjust (`KisLevelsFilter.cpp:18`, `kis_desaturate_filter.cpp:48`,
  `kis_multichannel_filter_base.cpp:46`, `example.cpp:42`).
- `chromatic_aberration` lands in **Veils** — it displaces channels spatially, an
  optical artifact, and its presence in today's *filters* catalog only ever meant
  "someone wanted it destructively applicable", which §1.6 makes universal.
  Raised as **Q8** because it is the one assignment worth arguing.
- **The category is presentational, and that is now testable.** §1.10 states it
  flatly (not on the layer, not in the manifest, not read by
  `supports_screen_space`, `link`, the trait, the apply pass or the format) and
  Test 29 pins it against exactly the Type-owned-dispatch failure CLAUDE.md
  names. Test 28 pins the partition.
- **Q4 (divider labelling) is CLOSED** with concrete copy: rest label
  `Viewport only — not exported`, a tooltip naming exports/Flatten/Merge, an
  empty-run hint, and a per-row badge. The constraint driving the copy is stated
  in §5.8 and §10: **the divider marks space, the category marks kind, and they
  are orthogonal** — a `Filters` effect may sit above the line and a `Veils`
  effect below it, so labelling the divider "Veils" would be false the first time
  someone drags `curves` across it and would rebuild the pinned-folder model this
  plan removes. The copy also drops "screen space" / "canvas space" as the
  plan's vocabulary rather than the user's.

#### D2 — one "Add Layer" modal

New user request: one `+`, no split-button chevron, a modal with a left tab rail
of layer types (Normal, Filters, Veils, …), Normal preselected, Enter spawns.

- **New §1.11** (semantics) and **new §5.9** (mechanics).
- **Placement was already settled and is not reopened.** Every kind lands
  relative to the most recently selected layer through the existing anchor —
  `addRaster({ anchor: app.activeLayerId })` (`frontend/src/actions/index.ts:582`,
  verified), `addGroup({ anchor: … })` (`:622`),
  `addFilter({ …, anchor: app.activeLayerId })`
  (`ui/filters/FilterPickerModal.svelte:24`). No special rule for effects.
- **The rail is derived, not written.** `LayerKindRegistration`
  (`document/layer_kind.rs:55-104`) gains `new_action` and `variant_catalog`,
  one line each in five files. It already carries this exact class of
  UI-facing type-owned fact — `can_have_mask` (`:63-66`) is documented as
  existing so the panel can gate "Add mask" *"without branching on `type_id`"*,
  and `can_rename` (`:68-70`), `has_thumbnail` (`:72-75`) and `icon` (`:77-80`)
  follow the same pattern. `new_action` projects onto the **existing**
  `CatalogEntry::hotkey_action` (`catalog.rs:34-35`, `:91-94`); only
  `variant_catalog` is a new `CatalogEntry` field, following `capture_kind`'s
  per-kind-optional precedent (`catalog.rs:50-51`). A kind with a variant catalog
  contributes one tab per `category` its entries declare — which is what makes
  Filters and Veils two tabs of one layer kind, exactly as the user described.
- Verified that no void declares a `category`, so `voids` yields one tab titled
  by `Catalog::title`; and that `vector` has no add path at all today
  (`NEW_LAYER_ACTION_IDS`, `actions/index.ts:37-43`, has no entry), so declaring
  both fields empty is a statement of fact rather than an omission.
- **Line counts verified with `wc -l`, and the brief's file list was incomplete.**
  `LayerFooter.svelte` 224, `LayerPickers.svelte` 27,
  `state/layerPicker.svelte.ts` 12, `VeilPickerModal.svelte` 82,
  `FilterPickerModal.svelte` 84 — all exact. **Missing from the brief:**
  `ui/layers/NewLayerMenu.svelte` (93 lines, the dropdown itself, `LayerFooter`'s
  only consumer at `:3` / `:121`, sole user of the `data-keep-open="new-layer"`
  dismiss channel and of `NEW_LAYER_ACTION_IDS`) and
  `ui/voids/VoidPickerModal.svelte` (121, the third clone). Total replaced: 643
  measured lines across six deleted files (§2.9).
- **`LayerFooter` is not deleted, contrary to the brief's framing of it as "224
  lines, the split button".** The split button is `:104-123` plus its styles at
  `:195-223`, about 40 lines. Add-mask (`:125-132`, `canAddMask` `:49-55`,
  `hostHasMask` `:34-37`), duplicate (`:134-141`), delete (`:143-150`,
  `activeEditable` `:43-47` mirroring the engine's `is_node_editable`),
  `findNode` (`:12-21`) and the multi-selection tooltips (`:76-87`) all survive.
  §2.9 enumerates them so implementation does not delete the file wholesale.
- **Modal conventions matched, not invented.** The vertical tab rail is
  `ui/settings/SettingsModal.svelte:114-128` verbatim — `.main` is
  `flex-direction: row` (`:213-218`), `.tab-strip` is `flex-direction: column`
  with a `border-right` and `min-width: 140px` (`:220-228`) — including its
  cross-tab search input (`:100-104`), which is what answers the discoverability
  objection to single categories without duplicating entries. Enter-to-confirm is
  `NewDocumentModal.svelte:130-135` / `ResizeCanvasModal.svelte:255`. Escape,
  backdrop and × are already `Modal.svelte`'s (`:79-88`, `:103`), and `:68-76`
  already `stopPropagation()`s every keydown, which is what makes an unqualified
  Enter binding safe.
- **One behaviour identified as easy to lose and explicitly protected.**
  `VoidPickerModal.svelte:19-36` acquires the `MediaStream` inside the click
  gesture *before any `await`*, because `getDisplayMedia` needs transient user
  activation that an `addVoid` round-trip would expire. §5.9 keeps it by giving
  each variant catalog its own spawn module (`ui/layers/addSources/*.ts`) rather
  than a branch in the modal, and Risk 9 names it as the specific regression a
  refactor would introduce silently.
- **`newVeil` and `newFilterLayer` survive** as category deep links (§5.9), which
  is what keeps "New Veil" in the Layer menu and command palette — the property
  `frontend/src/actions/__tests__/menu_actions.test.ts:141-149` asserts and D1
  requires. PR 5's rename list drops them. `NEW_LAYER_ACTION_IDS` is deleted; the
  rail replaces it with catalog order.
- **`newLayer` and `newGroup` keep spawning directly** rather than opening the
  modal — routing a bound key through a modal turns one keystroke into two.
  Raised as **Q9**.

#### D3 — PR sequencing changed

The modal is **its own PR, third of five**: pure frontend plus two declarative
Rust fields, depending only on PR 2's `category` declarations and on nothing in
the compositor. Old PR 3 (effects in the tree) becomes PR 4 and old PR 4
(vocabulary) becomes PR 5.

This also **supersedes Step 4b**, which the review's finding 4 had pulled forward
as a shared `EffectPickerModal.svelte`. That component would now be built in PR 2
and deleted in PR 3. Step 4b instead becomes a two-line category filter on each
existing picker (`VeilPickerModal.svelte:15`,
`FilterPickerModal.svelte:18` — both already `app.entries?.(…)` calls), which
ships the same correctness, throws nothing away, and makes PR 2 the first
consumer of D1's declarations. The review's underlying point — that PR 2 must not
ship two pickers each offering all 15 entries — is honoured; only the mechanism
changed. The ~120 "no credit" lines are still not claimed (§4, §8).

#### D4 — numbers

Production **≈ −1,200** (about 3,335 added, 4,535 removed), honest range
**−750 to −1,500**, revised from −1,350 / −900 to −1,600. The movement is:
Change 1 ≈ +80 with no removals; Change 2 ≈ +390 / −464; and the deleted
picker-merge row (+40 / −160) folding into the larger deletion. Tests grow from
30 items / ~700 lines to 32 / ~790. The `ui/filters/` relocation drops from 9
files to 8 because `FilterPickerModal` is deleted rather than moved.

---

Status: **revised** (step 3 of the CLAUDE.md Planning and Independent Review
Workflow — draft revised against the independent review above, the user's
stored-boundary model change, and the user's two subsequent corrections: veils
survive as a user-facing category, and the add-layer UI consolidates into one
modal). No production code has been changed. Awaiting step 4 approval.

This is a **rewrite**, not an edit, of `docs/plans/veils-as-layers.md`. That
plan's investigation of the existing contracts largely survives and is re-cited
here (re-verified against source, not copied on trust). Its central
architectural conclusion — "space cannot be inferred from tree position, so the
viewport chain stays a separate stack with a pinned UI folder" — was rejected in
discussion. §3 states what replaces it.

---

## 1. Feature semantics

### 1.1 There is one kind of thing: an *effect*

An **effect** is a registered, parameterized image transform: a `type_id`, a
`ParamDef` schema, a WGSL pipeline, a preview. One registry, one catalog, one
picker, one preview mechanism, one directory.

Today the same concept exists twice — as a `Veil` (`crates/darkly/src/gpu/veil.rs:20-100`,
10 modules in `gpu/veils/`) and as a `FilterEffect` (`crates/darkly/src/gpu/filter.rs:38-68`,
7 modules in `gpu/filters/`) — with `black_and_white` and `chromatic_aberration`
implemented in **both**, surfacing twice in the UI. That duplication is the
user's original complaint. After this work the catalog holds **15** entries
(10 + 7 − 2), not two catalogs of 10 and 7.

One registry does **not** mean one undifferentiated list in front of the user.
Each effect declares a **category** — `"Veils"` or `"Filters"` — in its own
registration file, and every user-facing surface groups by it. "Veil" survives
as a word the user reads; it stops being a subsystem. §1.10 works the set.

### 1.2 Effects are ordinary layers

An effect is a layer in the document tree. There is no second stack, no pinned
UI folder, no `manifest.veils`, no `activeVeilIndex`, no `moveVeil`. It is
selected, renamed, reordered, grouped, masked, hidden, duplicated, saved and
undone by the machinery every other layer already uses.

An effect transforms **whatever accumulator it composites into** — which is what
today's filter layers already do (`compositor.rs:4343-4351`): lower siblings
plus everything beneath the group, because a passthrough group (the default,
`layer.rs:512`) inlines into its nearest non-passthrough ancestor's accumulator.
An isolated group confines it. This rule is unchanged and needs no new
machinery.

### 1.3 Space is a stored boundary

The root group's children carry a **boundary**. Everything **above** it renders
in **screen space**, post-present, at viewport resolution — exactly where
today's veil chain runs. Everything **below** it renders in **canvas space**, in
the tree walk, at canvas resolution. Only canvas space is exported (§1.4).

The boundary is **document state**: it survives save/load, it is undoable, and
it is reasonable-about without a GPU, which is precisely the Document Authority
Principle's test. It is not derived, not session, not compositor.

#### The representation

```rust
pub struct Document {
    // …
    /// How many of the root group's trailing children the user has placed
    /// above the screen-space boundary. `0` in a fresh document.
    pub screen_space_count: usize,
}
```

Root children are stored **bottom-to-top** (`document/layer_kinds/group.rs:26-27`;
the panel renders them reversed, `engine/veils.rs:82-92`), so the screen-space
region is the *suffix* of the list and the boundary is naturally expressed as a
count from the top. Four candidates were weighed:

| representation | why not |
|---|---|
| **count of trailing children** (chosen) | — |
| index of the first screen-space child | Bottom-anchored. Every insert or removal *below* the boundary shifts it, so every structural path needs a fix-up. The count needs one only when the change is inside the run. |
| `Option<LayerId>` anchor — "the run starts at this node" | Adds a dangling-reference state: `detach_for_undo` orphans a node without deleting it (`document/mod.rs:927` → `unlink`, `:1003`), so deleting the anchor leaves the boundary pointing at a node that is no longer a child. Re-anchoring on unlink loses the same information a count loses, plus the extra state. |
| a marker node among the root's children | Every `children_of(root)` consumer, `compose_children`, the bake source lists, save/load and the picker would have to know a phantom child exists, and it would need its own invariants (exactly one, never inside a group, never deletable, never duplicable) — more machinery than the count, not less. |

The count also has the right *meaning*: it is the user's **intent** — "I put the
divider here" — and it stays put when the thing above it is temporarily
disqualified (§1.9).

#### `supports_screen_space()` is a validation predicate

It no longer computes where the boundary is. It answers one question: *may this
node sit above the boundary?* Same type-owned shape as the existing
`LayerNode::composites_in_place()` (`crates/darkly/src/layer.rs:714-721`):

```rust
/// Whether this node may sit above the screen-space boundary — i.e. whether it
/// can be realized after the view transform, on the presented image, rather
/// than inside the canvas-space tree walk.
pub fn supports_screen_space(&self, doc: &Document) -> bool
```

- effect layer → `true`
- **passthrough** group → `true` iff every child answers `true`, recursively
- **isolated** (non-passthrough) group → `false` — it owns a canvas-space
  accumulator whose contents have no screen-space counterpart
- raster / vector / void → `false`
- **any node carrying a mask** → `false`. Mask textures are canvas-space R8 at
  full canvas rect — `ensure_node_texture(…, R8Unorm, bounds)`
  (`crates/darkly/src/engine/filters/mask.rs:82-88`) with `bounds` from
  `host_default_bounds` → `canvas_rect()` (`document/mod.rs:1120-1124`) — and
  are sampled through `sample_mask_window` in plane coordinates
  (`crates/darkly/shaders/mask_lerp.wgsl`). Applying one to a post-present,
  view-transformed image is a coordinate-frame mismatch of exactly the class
  `docs/coordinate-systems.md` exists to prevent.

An **empty** passthrough group answers `true` vacuously. That is the intended
reading and it is harmless: an empty group composites nothing in either space.
Stated so it is a choice rather than an accident.

Two deliberate refinements, both so that **eligibility is a function of
structure alone**:

- **Visibility is not consulted.** A hidden raster is still ineligible. If
  visibility fed the predicate, toggling an eye could change what gets exported
  — a large, surprising consequence from a small control.
- **Mask *presence*, not mask visibility.** The user's brief said "visible
  mask"; hiding a mask would then make its host eligible on a toggle, the same
  teleport in the other direction. Presence is the structural, document-side
  fact the Document Authority Principle already names (`has_mask`); visibility
  is per-frame. Presence is stable, strictly safer, and costs the user nothing
  they cannot get by deleting the mask. **Settled: presence** (the review agreed;
  Q1 is closed).

  The divergence from `masked_in_place_hosts` (`document/mod.rs:554-559`, which
  *does* gate on mask visibility) is deliberate and harmless: that predicate
  sizes a GPU resource for the current frame, this one decides a coordinate
  frame. Worth noting also that a mask-capable screen space is *technically*
  reachable — `fs_present` already carries the inverse view matrix
  (`shaders/present.wgsl:29-38`), so a future version could sample a
  canvas-space mask through it — so the exclusion is a scoping decision, not an
  impossibility.

#### The effective run

```rust
/// The root children realized in screen space, bottom-to-top.
pub fn screen_space_run(&self) -> &[LayerId]
```

is `&children_of(root)[n - k_eff ..]`, where `k_eff = min(screen_space_count, s)`
and `s` is the length of the longest trailing suffix all of whose members answer
`supports_screen_space()`. The `min` is a **safety clamp, not the definition** —
under §1.9's invariant `k_eff == screen_space_count` always; the clamp exists so
that an in-place property change that disqualifies a run member (attaching a
mask, un-passthrough-ing a group), a corrupt save, or a path we missed degrades
into "fewer things are screen space" rather than "a raster is being rendered in
screen space". The stored intent is left alone, so removing the mask restores
the run.

### 1.4 Only canvas space is exported

The tree walk skips the run, so `render_offscreen`
(`compositor.rs:3403-3439`) — which is what export, flatten and merge all
composite through — never sees it. This is today's veil behaviour, achieved by
not writing any code.

### 1.5 The divider

The **UI affordance is a draggable divider drawn across the layer panel** at the
boundary. It is a first-class structural element of the panel — explicitly *not*
a checkbox in a properties window. It is always visible: in a fresh document
`screen_space_count == 0`, the run is empty, and the divider sits at the very
top of the list. Dragging it down includes more effects; dragging it up excludes
them. It cannot be dragged past a node that answers `supports_screen_space()`
false.

**Moving the divider is the only way to change the boundary**, and it is one
scalar edit: `screen_space_count = k`. There is no per-layer flag. There is no
`Property::Pinned`, no fan-out over a subtree, no `CompoundAction`, and no
lossiness when the divider crosses a group — the group is either wholly above
the line or wholly below it, which is what the recursive clause of
`supports_screen_space()` already means.

This is what replaces the pinned "Veils" folder the old plan preserved, and it
is why the user rejected that plan: a fresh document should not contain a folder
pre-populated with four hidden things the user never asked for (§3), and the
screen-space capability is the thing that makes Darkly distinctive — it deserves
a structural element, not a sidecar.

### 1.6 Masking is hoisted out of the effect

Today `FilterEffect::render` takes `mask: Option<&TextureView>`
(`gpu/filter.rs:58-67`) and every filter shader ships a `*_masked` twin
(`shaders/filters/invert.wgsl:41`, `black_and_white.wgsl:33`, `hsv.wgsl:124`,
`curves.wgsl:90`, `brightness_contrast.wgsl:68`, `chromatic_aberration.wgsl:34`)
whose entire body is `select(orig, filtered, selected)`. Meanwhile the
filter-*layer* path already does the same thing **outside** the effect: snapshot
the accumulator, run the effect unmasked, then `mix(before, after, mask)`
(`compositor.rs:4379-4390`, `:4444-4453` via `snapshot_parent_accum` /
`lerp_parent_accum_with_mask`). Two implementations of one idea; `param_filter.rs`
even names the duplication in its own doc comment (`:5-6`: "exactly like
invert's `fs_invert_masked`") — a CLAUDE.md stop-sign phrase.

The mask parameter, the `*_masked` entry points, and the masked pipeline halves
in `ParamFilter` (`gpu/param_filter.rs:104-105`, `:200-203`, `:250-251`,
`:304-307`) and `MaskedFilterPipeline` (`gpu/effect.rs:219-224`, `:264-267`,
`:321-327`, `:347-369`) all go. One generic pass serves every path.

**Consequence: every effect becomes maskable and destructively applicable,
including `watercolor`, `painting` and `rainy_glass`.** Masking stops being a
per-effect capability and becomes a property of the compositor's apply step.

### 1.7 Opacity and blend mode work

`FilterLayer::blend` is stored (`layer.rs:238`), serialized
(`document/layer_kinds/filter.rs:67-68`, `:100-103`), shown in the UI
(`engine/types.rs:567-568`) and **never read** — `compose_filter_arm`
(`compositor.rs:4359-4454`) ignores it entirely. That is a live Document
Authority violation: a document field with no realization.

Both are implemented, in **both** spaces, over the existing `gpu/blend_modes/`
registry — so "rain in overlay mode" and "grain at 30%" work. Prior art supports
this reading: Krita sets `COMPOSITE_COPY` on an adjustment layer as a
constructor *default* only, with the comment literally reading "by default
Adjustment Layers have a copy composition" (`krita/libs/image/kis_adjustment_layer.cc:32-41`);
opacity and blend stay user-settable (`kis_dlg_layer_properties.cc:98-110` has
zero occurrences of "adjustment"), are honoured for every layer type
(`kis_layer_projection_plane.cpp:71-73`), and round-trip through `.kra`
(`kis_kra_savexml_visitor.cpp:382-395` writes `OPACITY` and `COMPOSITE_OP` for
adjustment layers like any other node). GIMP's `GimpDrawableFilter` likewise
carries `opacity` and `paint_mode` fields with public setters
(`gimp/app/core/gimpdrawablefilter.c:102-106`, `:622-636`, `:646-670`).

### 1.8 Vocabulary retirement is a *code-side* change

Both "veil" and "filter" (in the GPU-effect sense) are retired **as code
identifiers**: the directories, the traits, the registries, the catalog id, the
wire methods, the manifest slot, the config keys. One code identifier means one
thing. `gpu::filter` vs `document::filter` is an existing collision the code
apologises for in a comment (`gpu/filter.rs:106-108`); this removes it.

**Both words survive in the UI**, as the two categories of §1.10. An earlier
draft proposed retiring "veil" everywhere including user-facing copy; that is
overreach. Animated, distorting, viewport-style effects are not a thing paint
program users expect to find, and the word "veil" is how Darkly names them. The
category is the mechanism that lets the code be one thing and the UI be two.

Concretely: no user-facing string that says "Veil" today stops saying it because
of this plan. `newVeil` survives as an action id (§5.9 — it is a deep link to
the Veils tab, not a legacy name), and the Veils tab, the Veils group header in
the picker, and any effect row's category label all keep the word.

### 1.9 The invariant, and the one place it is enforced

**Invariant.** Every root child at index `>= n - screen_space_count` answers
`supports_screen_space()` true.

#### There is exactly one insertion chokepoint

Every structural insert in the crate funnels through one private method.
`LayerNode::attach_child` (`layer.rs:630-645`) is the only code that touches a
group's `children` vec — its index computation is
`let at = position.map_or(list.len(), |p| p.min(list.len())); list.insert(at, child);`
(`:643-644`) — and its **only** caller is `Document::link`
(`document/mod.rs:982-994`). Removal is the mirror: `Document::unlink`
(`document/mod.rs:1003-1019`) → `detach_child`. `link` has three internal
callers and two public doors:

- `Document::attach_at_target` (`document/mod.rs:1040-1078`), reached by every
  `add_*_layer` and by `Document::move_layer` (`:912-920`).
- `Document::reinsert_entity` (`document/mod.rs:935-947`), the raw-index door
  used by undo/redo and by the engine's exact-slot landing code.

**Enforcement therefore lives in `link` / `unlink` and nowhere else.** Every
path below inherits it without being edited — which is the point (CLAUDE.md,
"place functionality where it generalizes"). The alternative the review proposed
(an insertion-floor policy written into `document/mod.rs:663`/`:736` and their
siblings) would have been four edits that a fifth `add_*` would silently miss.

Rules, root children only:

- **`link`**: a node may be placed at an index inside the run only if it answers
  `supports_screen_space()`. Otherwise its position is clamped to at most
  `n - screen_space_count`. A qualifying node placed inside the run increments
  the count; nothing else changes it.
- **`unlink`**: removing a child whose index was inside the run decrements the
  count, so the divider does not drift down over the survivors.

#### The paths this covers, exhaustively

| path | file:line | reaches `link` via |
|---|---|---|
| `add_raster_layer` / `add_vector_layer` / `add_void_layer` / `add_filter_layer` / `add_group` | `document/mod.rs:663`, `:680`, `:706`, `:736`, `:825` | `resolve_anchor_target` → `attach_at_target` |
| New-layer handlers | `engine/layers.rs:101`, `:129`, `:275`, `:594`, `:745`, `:797` | the `add_*` above |
| Paste | `engine/clipboard.rs:486` + `:506-507` | `add_raster_layer` then `move_layer` |
| Floating paste | `engine/floating.rs:258` + `:274-275` | same |
| Duplicate (root and nested) | `engine/duplicate.rs:121`, `:163`, `:223`, `:259`, `:289`, then `:83-84`, `:333-339` | `add_*` then `move_layer` |
| Flatten Image / Flatten Node result | `engine/flatten.rs:41` + `:87-88`, `:179` + `:233-237` | `add_raster_layer` then `reinsert_entity` |
| Merge Down / Merge Layers result | `engine/merge.rs:74` + `:134-136`, `:238` + `:280-282` | same |
| Group | `engine/layers.rs:656` + `:692-698` | `add_group(None)` then `reinsert_entity` |
| Drag-and-drop reorder (single and multi) | `engine/layers.rs:1170`, `:1209-1266` via `move_layer_inner` `:1184` | `Document::move_layer` |
| Undo / redo of every structural op | `undo/layer.rs:35`, `:40`, `:90`, `:96`, `:138-139`, `:145-146`, `:193`, `:199`, `:290-298`, `:306-309` | `detach_for_undo` / `reinsert_entity` |

There is **no import or place-file path** in the crate to cover — the only
`import_*` is `engine/brush_library.rs:188` for brush bundles. There is **no
ungroup** either; `grep -rn ungroup` over `crates/` returns nothing.

#### The two paths that do *not* go through the pair

1. **Load.** `build_staging_document` (`engine/load.rs:202-206`) constructs the
   document from the manifest and `rebuild_parent_map` (`:371-390`) derives the
   parent map straight from each group's deserialized `children` vec — `link` is
   never called. Load therefore clamps explicitly:
   `doc.screen_space_count = min(manifest.screen_space_count, longest qualifying suffix)`.
   One line, beside the existing `selection_id` remap at `:361-365`.
2. **In-place property changes that disqualify a node already above the line** —
   attaching a mask to a run member (`document/mod.rs:843`), setting a
   run-member group's `passthrough` to false (`Property::Passthrough`,
   `undo/property.rs:14`), or linking a raster *into* a passthrough group that
   is itself above the line. These do not insert among the root's children, so
   `link`'s clamp cannot see them. They are handled by §1.3's **read clamp**:
   `screen_space_run()` returns `min(count, qualifying suffix)`, so the affected
   member and everything below it in the run silently fall back to canvas space
   until the disqualifying change is reverted. Stored intent is untouched, so
   deleting the mask restores the run. This is deliberately *not* enforced at
   each of those call sites — that would be the per-variant branching the
   Modularity Principle forbids, and it would have to be rediscovered by every
   future property that can disqualify a node.

#### Undo fidelity

Undo actions restore *absolute indices* (`undo/layer.rs:40`, `:90`, `:139`,
`:199`, `:297`), and an index plus a count cannot recover which side of the
boundary a node was on when it sat immediately adjacent to it. Concretely, with
`[R, E_a, E_b, E_c]` and `count == 3`, deleting `E_a` decrements the count to 2;
reinserting at index 1 is indistinguishable from inserting a brand-new effect
just below the divider, so `E_a` comes back in canvas space. That is a
one-node, user-visible, export-changing discrepancy.

The fix is to carry the side, and the DRY-positive way to carry it is to notice
that `(parent: Option<LayerId>, position: usize)` is already duplicated across
five undo structs — `EntityAddAction` (`undo/layer.rs:18-20`),
`EntityRemoveAction` (`:59-61`), `LayerMoveAction` (`:111-115`, twice),
`DuplicateAction` (`:162-164`) and `BakeSourceSlot` (`:212-216`). Replace all
six occurrences with one

```rust
pub struct TreeSlot {
    pub parent: Option<LayerId>,
    pub position: usize,
    /// Whether this node sat above the screen-space boundary. Restored
    /// verbatim, because position alone cannot distinguish "lowest member of
    /// the run" from "topmost canvas-space child".
    pub screen_space: bool,
}
```

produced by a new `Document::slot_of(id) -> Option<TreeSlot>` and consumed by
`reinsert_entity(id, slot)`, which sets the count so the node lands on the
recorded side. Net: one new field, one struct, five sites that get shorter.

**Fallback if that is judged too wide:** skip `TreeSlot`, accept the positional
rule, and accept the single lossy case above. It is narrow — it requires an
*effect layer* immediately adjacent to the divider to be deleted and undone —
but it is a real correctness gap and it should be a deliberate choice, not an
omission. Raised as **Q3**.

### 1.10 One registry, two user-facing categories

#### The mechanism already exists

No new machinery. `CatalogEntry` already carries
`pub category: Option<&'static str>` — *"Grouping label within the catalog, for
variants that group"* (`crates/darkly/src/catalog.rs:32-33`) — with a
`with_category()` setter (`:86-89`), serialized camelCase across the wire and
already present in the generated TS (`frontend/src/engine/protocol_gen.ts:468`).

**Blend modes are the worked example and the exact shape to copy.**
`BlendModeRegistration` has `pub category: &'static str`
(`crates/darkly/src/gpu/blend_mode.rs:35`); each mode declares its own in its own
file (`gpu/blend_modes/multiply.rs:10` — `category: "Darken"`); the registration
projects it (`gpu/blend_mode.rs:72` — `.with_category(self.category)`); and the
frontend groups on it with no hand-written list at all —
`frontend/src/ui/properties/LayerProperties.svelte:20-33` builds the dropdown's
`<optgroup>`s by run-length grouping `bm.category ?? ''` over the catalog in
emitted order. Six categories over sixteen modes, zero central list.

So `EffectRegistration` gains one field:

```rust
/// Which group of the picker this effect appears under — the word the user
/// reads for the kind of thing it is. Presentational only: it does not
/// affect the trait, the layer kind, rendering, serialization, or which
/// side of the screen-space boundary a layer may occupy.
pub category: &'static str,
```

declared beside `display_name` / `icon` / `description` in the effect's own file,
projected through the existing `.with_category()`. Adding an effect stays "drop
one file"; adding a *category* is one new string in one effect file, and every
surface that groups by category picks it up with no edit.

#### The set, from the actual 15

| effect | category | why |
|---|---|---|
| `black_and_white` | Filters | per-pixel tonal remap |
| `brightness_contrast` | Filters | per-pixel tonal remap |
| `curves` | Filters | per-pixel tonal remap |
| `hsv` (Hue/Saturation) | Filters | per-pixel colour remap |
| `invert` | Filters | per-pixel colour remap |
| `levels` | Filters | per-pixel tonal remap |
| `chromatic_aberration` | **Veils** | displaces colour channels *spatially*; an optical artifact, not a remap (**Q8**) |
| `frozen` | Veils | spatial refraction |
| `grain` | Veils | animated textural overlay |
| `lens_blur` | Veils | spatial gather |
| `painting` | Veils | spatial, painterly |
| `pixelate` | Veils | spatial resample |
| `rainy_glass` | Veils | animated spatial refraction |
| `vhs` | Veils | animated analog artifacts |
| `watercolor` | Veils | multi-pass spatial bleed |

Six Filters, nine Veils. The dividing line is mechanical and states itself: a
**Filter** is a function of one texel's colour; a **Veil** reads its neighbours,
its clock, or both. Every one of the six Filters is a pointwise remap and every
one of the nine Veils is spatial and/or animated, so the taxonomy is not a matter
of taste except at the single contested entry.

`chromatic_aberration` is that entry, and it is the one the user flagged. It is a
Filter today only because someone needed it destructively appliable, and §1.6
makes *every* effect destructively appliable, so that signal is now worth
nothing. What it actually does is offset the R and B samples spatially — the same
family as `lens_blur`. Filed under Veils. Raised as **Q8** because it is the one
assignment a reasonable person would argue.

#### Exactly one category per effect, not a slice

`category: &'static str`, not `categories: &'static [&'static str]`. Four
reasons, in order of weight:

1. **A slice re-admits the bug this plan exists to fix.** The reported problem is
   "Chromatic Aberration appears twice in my UI". A multi-homed effect appears
   twice in the *same modal*, one tab apart. Fixing a duplicate at the registry
   level and reintroducing it at the tab level is not a fix.
2. **Prior art is single-valued and type-owned.** Krita's filter base class takes
   exactly one: `KisFilter(const KoID& id, const KoID & category, const QString & entry)`
   (`krita/libs/image/filter/kis_filter.h:33`), with the nine category constants
   in one file (`krita/libs/image/filter/kis_filter_category_ids.cpp:11-19`:
   Adjust, Artistic, Blur, Colors, Edge Detection, Emboss, Enhance, Map, Other)
   and each filter naming one in its own constructor —
   `kis_raindrops_filter.cpp:47` Artistic, `kis_pixelize_filter.cpp:45` Artistic,
   `kis_oilpaint_filter.cpp:44` Artistic, `kis_lens_blur_filter.cpp:33` Blur,
   `KisLevelsFilter.cpp:18` Adjust, `kis_desaturate_filter.cpp:48` Adjust,
   `kis_multichannel_filter_base.cpp:46` Adjust. 39 registrations across nine
   categories; **none multi-homed.** Note also that Krita independently files our
   three closest analogues (`rainy_glass`, `pixelate`, `painting`) as *Artistic*
   and our `lens_blur` as *Blur* — spatial families, not adjustments — and files
   our six Filters as *Adjust*. The split above matches its judgement entry for
   entry.
3. **A partition makes the correctness test total.** "Group the catalog by
   category; the concatenation is the catalog, with no id repeated" is a
   one-line property (§7 Test 28). Under a slice there is no such invariant, and
   the picker needs a tie-break rule for "which tab do I open on" anyway.
4. **Single is the reversible choice.** Widening a `&'static str` to a slice
   later is additive at 15 declaration sites. Narrowing is not.

The counter-argument — discoverability, someone hunting Chromatic Aberration
under Filters — is real and is answered by search rather than by duplication:
the picker's search box spans every tab (§5.9), the same shape as
`SettingsModal.svelte:100-104`.

**GIMP is the anti-pattern here and is cited as one.** Its filter categorization
is a hand-written central menu file: `menus/image-menu.ui.in.in:695` opens a
`_Blur` submenu and `:697-700` lists `app.filters-focus-blur`,
`filters-gaussian-blur`, `filters-lens-blur`, … by hand, with twelve such
submenus (`_Blur`, `_Distorts`, `_Light and Shadow`, `_Noise`, `_Generic`,
`_Artistic`, `_Decor`, `_Map`, `_Render`, `_Fractals`, `_Pattern`, `_Web`). A new
filter that forgets to edit that file is invisible. That is precisely what
CLAUDE.md's Modularity Principle forbids ("Add entries to a handwritten list"),
and it is why the declaration goes on the effect and not in a menu.

#### The category is presentational and nothing else

Stated flatly because it is the property everything downstream depends on:

- It is **not** on `EffectLayer`, not in the manifest body, not undoable. It is a
  property of the effect *type*, read off the registry, exactly like
  `display_name`.
- It does **not** decide space. §1.3's `supports_screen_space()` never consults
  it. A `Filters`-category effect may sit above the boundary and a
  `Veils`-category effect below it, and both render correctly there.
- It does **not** decide placement. §1.9's `link` clamp never consults it.
- It does **not** reach the trait, the pipeline, the apply pass or the format.
  Deleting every `category:` declaration would change what the picker looks like
  and nothing else.

Test 29 pins this so the property cannot rot into a load-bearing one.

### 1.11 One "Add Layer" modal

Today the layer panel's add affordance is a split button: a `+` that dispatches
`newLayer`, and a 16px-wide chevron beside it that opens a dropdown
(`frontend/src/ui/layers/LayerFooter.svelte:104-123`, the chevron at `:112-119`).
The dropdown is `NewLayerMenu.svelte` (93 lines), and three of its five entries
just set `layerPicker.kind` (`actions/index.ts:591`, `:597`, `:603`) to raise one
of three near-identical picker modals (`ui/layers/LayerPickers.svelte:21-27`).
Four surfaces, three of them clones, behind a chevron 16 pixels wide.

That collapses to **one modal**:

- **One `+` button.** No chevron, no split button, no dropdown. Clicking it opens
  the modal.
- **A left tab rail** listing the addable layer kinds, with the typed kinds
  expanded into their categories: **Normal · Filters · Veils · Voids · Group**.
  Filters and Veils are two tabs of one underlying layer kind, which is exactly
  what §1.10's categories mean.
- **Normal is preselected** and its pane is a single "Create" affordance, focused
  on open. **Enter spawns and closes** — so `+`, Enter is the fast path for a
  plain raster layer.
- The typed tabs show the grid of live-preview cards the existing pickers already
  show (`VeilPickerModal.svelte:31-41`), with one card selected; Enter spawns
  that one.

**Placement is unchanged and is not reopened.** Every kind lands relative to the
most recently selected layer through the existing anchor mechanism —
`engine.api.addRaster({ anchor: app.activeLayerId })`
(`frontend/src/actions/index.ts:582`), `addGroup({ anchor: … })` (`:622`),
`addFilter({ …, anchor: app.activeLayerId })`
(`ui/filters/FilterPickerModal.svelte:24`) — resolving to `Document::add_*`'s
anchor (`document/mod.rs:1023-1025`). Effects get no special rule; §1.9's `link`
clamp is what keeps a raster out of the run, and it applies here as it does
everywhere.

---

## 2. What exists today, verified

Every claim below was checked by opening the file.

### 2.1 The two invocation contracts are isomorphic — CONFIRMED

`Veil::create_cache(&mut self, device, queue, ping_pong_views: &[TextureView; 2],
sampler, render_width, render_height) -> EffectCache` +
`Veil::encode(&self, encoder, cache, src_idx, dst_view)` (`gpu/veil.rs:37-45`, `:93-99`).

`compose_filter_arm` has exactly those values in hand: `gs.accum` is
`AccumPair { textures: [Texture; 2], views: [TextureView; 2] }`
(`compositor.rs:262-266`), `src = gs.current_accum`, `dst = 1 - src`
(`:4392-4402`), rendering `views[src] → views[dst]` (`:4433-4441`). Formats
match: `accum_format = Rgba8Unorm` (`compositor.rs:890`, `:1090`) is the same
value handed to `VeilChain::new` (`compositor.rs:1161`, `veil_chain.rs:77`).

### 2.2 The destructive path already allocates a ping-pong pair — CONFIRMED

`run_filter_region` (`compositor.rs:132-217`) allocates `src_scratch` and
`out_scratch` at region size and format (`:167-168`), copies the node region
into `src_scratch` (`:171-189`), hands both views to `run_pass` (`:191-195`),
and copies `out_scratch` back (`:197-215`).

That pair **is** the `[TextureView; 2]` the accumulator contract wants. So the
destructive path needs no separate trait — it can drive a `dyn Effect` directly.
Per-apply cache construction is not a cost worth designing around: the apply is
one-shot and user-initiated, and the caches involved (a 256×2 curves LUT, a
784-byte CA uniform) do not scale with the target.

### 2.3 R8 is a pipeline-variant question — CONFIRMED

A `wgpu::RenderPipeline` is compiled against one target format. `MaskedFilterPipeline`
builds four (`plain_rgba`, `plain_r8`, `masked_rgba`, `masked_r8`,
`gpu/effect.rs:313-330`) precisely because of this; `ParamFilter` builds RGBA8
only and early-returns on anything else (`param_filter.rs:228-235`, `:286-288`).
`Veil`'s registration already takes the format as a pipeline-construction
parameter (`gpu/veil.rs:115`, `VeilRegistry::pipeline(type_id, device, format)`
at `:228-242`).

The only R8 consumer is destructive apply on a mask node, where the format comes
from the target texture at runtime (`engine/filters/apply.rs:75-78`). Today only
`invert` serves it. So R8 is a **declared, per-effect list of target formats**,
and the registry caches pipelines per `(type_id, format)`.

### 2.4 Six shaders hard-code `alpha = 1.0` — CONFIRMED at every line

```
shaders/veils/vhs.wgsl:118          return vec4f(col, 1.0);
shaders/veils/rainy_glass.wgsl:226  return vec4f(col, 1.0);
shaders/veils/watercolor.wgsl:75    return vec4f(cmyk_to_rgb(cmyk), 1.0);
shaders/veils/frozen.wgsl:79        return vec4f(r, g, b, 1.0);
shaders/veils/lens_blur.wgsl:83     return vec4f(result.rgb / result.a, 1.0);
shaders/veils/painting.wgsl:127     return vec4f(out.rgb / out.w, 1.0);
```

`black_and_white` (`shaders/veils/black_and_white.wgsl:24` — `vec4f(bw_transform(color.rgb, params), color.a)`),
`grain` (`:95`) and `chromatic_aberration` carry alpha correctly.

Harmless in a viewport today, because `fs_present` returns `vec4f(composed, 1.0)`
(`shaders/present.wgsl:71-72`) — the chain's input alpha is already 1.0. **Fatal
in canvas space**: such an effect would force the whole canvas rect opaque.

Two of the six resist the "declare a capability and restore alpha in the caller"
remedy an earlier draft proposed, which is why the shaders get fixed directly:

- **`watercolor`.** All three passes go through one pipeline
  (`gpu/veils/watercolor.rs:322`, bound at `:340`, `:363`, `:384`), and passes
  0/1 legitimately store CMYK-in-RGBA with **K in the alpha slot**
  (`shaders/veils/watercolor.wgsl:65-70`, `:72-76`). Only the final pass (`:75`)
  may be touched.
- **`lens_blur`.** Its RGB normalizer *is* the alpha channel: it accumulates
  `exp(s * inv_t)` over the full `vec4` (`:73`) then divides `result.rgb / result.a`
  (`:82-83`), with the shader's own comment at `:79-81` stating "Alpha input is
  1.0, so each sample contributes `exp(1/threshold)` to `acc.a`". With varying
  alpha the *colour* is wrong, not just the alpha, so restoring source alpha
  yields a correct alpha over a broken image. ~4 lines: accumulate the
  normalizer from a constant weight and carry alpha separately.

All six stay **bit-identical in the viewport**, which is what makes this
shippable and verifiable on its own, before anything else lands.

### 2.5 `requires` never records effect pipeline ids — CONFIRMED, and unification would regress it

`requires_from_doc` (`engine/save.rs:430-470`) collects `layer_kind` and
`blend_mode` from nodes (`:438-441`), `modifier` from `Entity::Filter` — the
*mask/selection* registry, not the GPU one (`:442-444`) — and `veil` from the
chain (`:448-453`). A filter layer's `pipeline` id is **never recorded**.

`document/layer_kinds/filter.rs:79-92` validates `blend_mode` on deserialize but
not `pipeline`, and `compose_filter_arm` early-returns on an unknown id
(`compositor.rs:4371-4377`) — a silent no-op. The module doc comment asserting
the opposite (`document/layer_kinds/filter.rs:10-13`: "surfaces as a
`LoadError::CorruptManifest` rather than a silent fallback") is **wrong today**.

After unification, `painting`/`watercolor` named in a layer's `pipeline` would go
from *covered* (as `requires.veil`) to *silently dropped*. Fixing this is in
scope, and the stale comment is corrected with it.

### 2.6 The accumulator is straight alpha — CONFIRMED

`shaders/source_over.wgsl:1-2` ("premultiplied foreground onto straight-alpha
background. Returns straight-alpha result"); `composite.wgsl:132-137` divides by
`out_a`. So spatial effects (`lens_blur`, `frozen`, `painting`, `watercolor`,
`rainy_glass`) will pull the arbitrary RGB of fully-transparent texels across
alpha edges in canvas space. Expect fringing. **Characterized by a test, not
solved** (§7 Test 6, §9 Risk 4).

### 2.7 `present.wgsl` composites over the checkerboard *before* the chain

`shaders/present.wgsl:62-73` composites the canvas over a screen-space checker
and fills out-of-bounds with `view.bg`, then returns `vec4f(composed, 1.0)`.
`present_to_veil_pipeline` is the same `fs_present` entry point compiled for
`accum_format` (`compositor.rs:1091-1109`). So today's veils operate on the
checkerboard and the app background, not on the artwork's alpha.

An earlier draft proposed changing this (a deleted §5.6) so that an effect looked
the same on both sides of the divider. **That is a regression and it is not
done.** Presenting straight alpha into the run breaks three things that cannot
break today:

1. The reduced-resolution path runs a multi-tap soft downscale
   (`gpu/veil_chain.rs:365-371`, pipeline from `gpu/effect.rs:117-135`) on the
   presented image. On straight-alpha data the arbitrary RGB of fully
   transparent texels bleeds inward — §2.6's fringing, newly introduced into the
   viewport at the shipped default `rendering.veil_scale: 0.7071`
   (`presets/defaults.yaml:125`).
2. `blit_pass` clears to `wgpu::Color::BLACK` (`gpu/veil_chain.rs:658`), as does
   the final surface blit (`:404`), so every spatial screen-space effect would
   pull transparent black across the canvas border.
3. `lens_blur` normalizes RGB *by the alpha channel it accumulated*
   (`shaders/veils/lens_blur.wgsl:73`, `:82-83`; the shader's own comment at
   `:78-81` names the `alpha == 1.0` assumption). With varying alpha its
   *colour* is wrong, not merely its alpha.

So the two spaces genuinely present differently, and that is recorded rather
than papered over: canvas space is document content with real alpha; screen
space is a viewport treatment applied to the composited-and-checkered image.
Q4's divider labelling is what tells the user. Matching appearance across the
divider would need a premultiply-before / unpremultiply-after design that this
plan does not have, and it should be its own plan.

### 2.8 Frontend surface (measured)

`frontend/src/ui/veils/` — 4 files, **488 lines**: `VeilFolder.svelte` (148, the
hard-coded pinned folder, mounted at `ui/layers/LayerPanel.svelte:43` behind
`{#if app.veilList.length > 0}` at `:42` — *conditionally*, which matters for §3),
`VeilItem.svelte` (237, its own
`application/x-veil` drag-reorder), `VeilPickerModal.svelte` (82),
`VeilProperties.svelte` (21, a shim that delegates to `FilterParamsEditor`).

`frontend/src/ui/filters/` — 8 files + 1 test, **1,393 lines of source** (+243 of test), of which
`FilterParamsEditor.svelte` (220), `filterParams.ts` (147), `ParamRow.svelte`
(140), `ListParamEditor.svelte` (333), `LevelsEditor.svelte` (218) and
`FilterModal.svelte` (147) are shared substrate already used by veils.

`app.svelte.ts` carries a whole parallel selection model: `activeVeilIndex`
(`:327`, documented at `:325` as mutually exclusive with `activeLayerId`),
`veilList` (`:357`), `addVeil` (`:291`), `selectVeil` (`:657`), `removeVeil` (`:966`),
`moveVeil` (`:979`), `refreshVeilList` (`:1055`), plus six reset sites
(`:449, 518, 545, 562, 635, 666`). `actions/index.ts:674-677` gives veils
priority in `deleteLayer`.

`protocol_gen.ts` exposes 7 veil wire methods (`addVeil`, `clearVeils`,
`moveVeil`, `removeVeil`, `setVeilVisible`, `updateVeil`, `veilList`) and the
`VeilInfo` type. **All of it is deleted**, not renamed — effects are layers, so
`moveLayers` / `setLayerVisible` / `selectLayer` already do the work.

Deleting those seven methods does **not** retire "veil" from the wire, though.
The `'veils'` catalog id also crosses the boundary through the *generic*
`catalogs()` (`protocol_gen.ts:1266`) and through `PreviewReq { catalog }`
(`:677`), consumed as `catalog="veils"` in `EffectPreview.svelte`. That half is a
**rename with frontend consumers**, not a delete, and it rides along with §5.2's
`CATALOG_ID` change in PR 2 Step 2 — Step 4b's category filter is what updates
the two picker consumers, and PR 3's modal then replaces them outright.

`frontend/wasm/src/` contains **zero** occurrences of "veil" or "filter": the
bridge is a generic deferred FIFO (`api.rs:1-30`). Nothing to change there.

**There is no divider or separator element anywhere in `ui/layers/`** (all of
`LayerPanel`, `LayerItem`, `LayerGroup`, `LayerFooter` read). The reusable
prior art in-repo is `ui/workspace/pointerDrag.ts` (83 lines, a `use:pointerDrag`
action owning pointer capture and Escape-abort) driving
`ui/workspace/Subdivision.svelte:61-67`'s `.gutter`.

### 2.9 The add-layer surface (measured)

Every line count below was taken with `wc -l`.

| file | lines | fate |
|---|---:|---|
| `ui/layers/LayerFooter.svelte` | 224 | **kept, shrunk** — see below |
| `ui/layers/NewLayerMenu.svelte` | 93 | deleted |
| `ui/layers/LayerPickers.svelte` | 27 | deleted |
| `state/layerPicker.svelte.ts` | 12 | deleted |
| `ui/veils/VeilPickerModal.svelte` | 82 | deleted |
| `ui/filters/FilterPickerModal.svelte` | 84 | deleted |
| `ui/voids/VoidPickerModal.svelte` | 121 | deleted |
| | **643** | replaced by one modal |

**`LayerFooter` is not deleted, and the earlier framing that it is (224 lines of
split button) is wrong.** The split button is `:104-123` plus its
`.split-btn` / `.split-main` / `.split-chevron` rules at `:195-223` — about 40
lines. Everything else in the file survives and is unrelated to adding layers:

- the **Add mask** button (`:125-132`) with `canAddMask` (`:49-55`),
  `hostHasMask` (`:34-37`) and `addMask` (`:57-61`);
- the **Duplicate** button (`:134-141`) with `canDuplicate` (`:68-71`);
- the **Delete** button (`:143-150`) with `canDelete` (`:63-66`), `remove`
  (`:89-95`) and `activeEditable` (`:43-47`, which mirrors the engine's
  `is_node_editable` so locked nodes grey out);
- the multi-selection tooltip derivation (`:76-87`), which reports
  "Delete (3)" / "Duplicate (3)" off `app.selectedLayerIds.size`;
- `findNode` (`:12-21`), used by four of the above.

`NewLayerMenu.svelte` was **absent from the change brief** and must be in it: it
is `LayerFooter`'s only consumer of the chevron (`:3`, `:121`), it is the only
user of the `data-keep-open="new-layer"` dismiss channel (`NewLayerMenu.svelte:29`
`watchDismiss('new-layer', …)`, trigger tagged at `LayerFooter.svelte:113-114`),
and it is the sole consumer of `NEW_LAYER_ACTION_IDS` (`actions/index.ts:37-43`).
All four go together.

`LayerPickers` is mounted at the app root (`frontend/src/App.svelte:71`), not in
the panel — deliberately, per its own comment (`LayerPickers.svelte:9-11`),
because the pickers are reachable from the palette and menu bar. The new modal
inherits that mount point and that reason.

**Modal conventions already in the repo**, both of which the new modal matches
rather than reinvents:

- **A vertical tab rail inside a modal already exists.**
  `ui/settings/SettingsModal.svelte:114-128` is `.main { display: flex;
  flex-direction: row; }` (`:213-218`) containing a `.tab-strip` that is
  `flex-direction: column` with a `border-right` and `min-width: 140px`
  (`:220-228`), whose `.tab` buttons are text-only with an `.active` marker rail
  (`:229-250`). It also carries the cross-tab search input (`:100-104`) that
  §1.10 leans on. This is the layout, verbatim.
- **Enter-to-confirm inside a modal already exists**, twice:
  `NewDocumentModal.svelte:130-135` (`if (e.key === 'Enter') { e.preventDefault();
  create(); }`) bound at `:139` on a `<div class="body" onkeydown={…}
  role="presentation">`, and `ResizeCanvasModal.svelte:255` / `:265` in the same
  shape.
- **Escape, the backdrop and the × are already free.** `Modal.svelte` owns all
  three (`:79-88`, `:103`), and `:68-76` `stopPropagation()`s every keydown so
  the window-level hotkey handlers cannot fire while the modal is open — which is
  what makes an unqualified Enter binding safe.

**One behaviour that must survive the merge, and is easy to lose.**
`VoidPickerModal.svelte:20-36` acquires the `MediaStream` for camera and
screenshare voids **inside the click gesture, before any `await`**, because
`getDisplayMedia` requires transient user activation and the `addVoid` round-trip
would expire it. Its comment (`:20-27`) spells this out. The same `pick` body
also opts the new layer into the session's stream allow-list (`:55-58`, which is
what distinguishes "the user just added this" from "this came back from a save")
and stops the tracks if layer creation failed (`:59-63`). Folding the void picker
into a modal whose spawn path is shared with four other kinds is exactly how all
three get refactored away. §5.9 keeps them by giving each variant catalog its own
spawn module rather than a branch in the modal.

---

## 3. Why the old plan's conclusion is superseded

The old plan (§2, `docs/plans/veils-as-layers.md:340-363`) argued: group
accumulators are canvas-sized (`compositor.rs:1829-1866`), the view transform is
applied only in `fs_present` (`present.wgsl:29-38`), and the veil chain runs on
that pass's output (`veil_chain.rs:322-416`); therefore *no tree position is
screen space*, therefore space must be determined by which stack an instance
lives in.

Every premise is still true. The conclusion does not follow. It assumed the tree
walk must composite every child. It need not. (The old plan's *other* claim —
that space must be stored rather than inferred — turns out to be right, and this
plan now agrees with it: §1.3 stores the boundary. What it gets wrong is *where*
the storage lives. A separate stack is not the only way to store which space
something is in; one integer on the document is.)

The change is: **the canvas-space tree walk stops early.** `compose_children`
(`compositor.rs:3711-3745`) already filters children on two orthogonal grounds
(`node.visible()` at `:3725`, isolation at `:3732`). A third — "this node is
realized in screen space" — is one line, and the excluded nodes are then driven
by the present-side run instead. A tree row that the tree walk skips is not a
lie in the data model; it is the same shape as a hidden layer, which the tree
also skips.

What this buys, and why the user asked for it:

- **No pre-populated sidecar folder in a fresh document.** Stated precisely,
  because the earlier draft overstated it and the review then understated it.
  `VeilFolder` is mounted behind `{#if app.veilList.length > 0}`
  (`frontend/src/ui/layers/LayerPanel.svelte:42-44`), so the **app** flavor —
  whose recipe is `seedVeils: () => {}` (`frontend/src/state/freshDocument.ts:61`)
  — shows nothing, and the review is right that there is no *empty* folder. But
  the **demo** flavor's recipe seeds four hidden veils into every fresh document
  (`freshDocument.ts:47-50`: `rainy_glass`, `grain`, `lens_blur`, `vhs`, all
  `visible: false`), so `veilList.length` is 4 and the folder appears every time,
  pre-loaded with four things the user never asked for. That is the real defect,
  and it is worse than an empty folder, not better.
- One selection model, one drag-and-drop, one delete, one undo, one save slot.
- The screen-space capability becomes a visible, structural, discoverable part of
  the layer panel rather than a sidecar most users will never open.
- Moving an effect between spaces is a drag, not a delete-and-re-add.

The cost is one new piece of document state (`screen_space_count`, §1.3) and one
invariant to hold (§1.9). Both are cheap because the document already funnels
every structural mutation through a single `link` / `unlink` pair, so the
invariant has exactly one home and every caller inherits it.

---

## 4. Architectural impact, per CLAUDE.md principle

**DRY.** Removes: two registries → one; **two implementations of the one
existing `PreviewMechanism` trait** → one (there is already exactly one trait —
`gpu/preview.rs:681` and `PreviewSession` at `:706` — which both `VeilMechanism`
(`gpu/veil.rs:278`) and `FilterMechanism` implement; collapsing them is a free
consequence of one registry, not an independent win, and the earlier "two
preview mechanisms → one" phrasing overclaimed); two duplicated effect
implementations (4 `.rs` + 4 `.wgsl` → 2 + 2); six `*_masked` WGSL entry points
and their pipeline halves; `param_filter.rs`'s bespoke bind-group assembly
alongside the veils' own; the entire parallel veil selection model on the
frontend (`activeVeilIndex`, `veilList`, `moveVeil`, `removeVeil`, `selectVeil`,
`refreshVeilList`, plus 7 wire methods); and the five-fold duplication of
`(parent, position)` across the structural undo actions, folded into `TreeSlot`
(§1.9). It also removes two existing stop-sign comments: `gpu/filter.rs:15-20`
("deliberately distinct from `Veil` … not the invocation contract" — §2.1 shows
the invocation contract *is* shared) and `param_filter.rs:5-6` ("exactly like
invert's `fs_invert_masked`").

It also removes the four-surface add-layer affordance — split button, dropdown,
and three near-identical picker modals — for one modal (§1.11, §5.9): 643
measured lines of frontend replaced (§2.9).

*Partially claimed:* the picker-modal collapse. Merging `VeilPickerModal` and
`FilterPickerModal` alone was already available **today**, standalone —
`EffectPreview.svelte:21` already takes `catalog` as a `string` prop and
`app.entries(catalogId)` is already generic — so roughly 120 of those 643 lines
are not a saving this work earned. The rest (the split button, `NewLayerMenu`,
`LayerPickers`, `layerPicker.svelte.ts`, and folding the *void* picker in
alongside the two effect pickers) is, because it depends on the tab rail being
derivable from the layer-kind and effect catalogs, which is this work.

**Modularity.** One `gpu/effects/` directory, one `build.rs` scan, one
`EffectRegistration`. Adding an effect stays "drop one file". A new layer kind
declares its own eligibility by implementing one method — and its own presence
in the add-layer modal by declaring two strings (§5.9), so the modal never
enumerates kinds. An effect's UI grouping is likewise one string in the effect's
own file (§1.10), so a third category is a third tab with no consumer edited.
The one hand-written list this removes on the frontend is `NEW_LAYER_ACTION_IDS`
(`actions/index.ts:37-43`), whose own doc comment concedes it exists to order a
dropdown; the rail replaces it with catalog order.

**Type-owned dispatch.** The diagnostic question — would adding a variant force
me to edit this code? — is answered *no*:
- `supports_screen_space()` is a `LayerNode` method, not a `matches!` at a call
  site. A future layer kind that can live in screen space slots in by
  overriding it; the boundary machinery in `link` / `unlink` never learns its
  name.
- The `if let Layer::Filter(f) = layer` branch in `CompositionContext::compose_layer`
  (`compositor.rs:330-345`) is **removed**, routed through
  `composites_in_place()` instead, so the compose walk stops asking layers what
  kind they are.
- R8 support is a declared field on the registration, not a `matches!(type_id, "invert")`.
- Masking and blending are the compositor's apply step, so no effect declares or
  implements them.

**Ownership.** The per-layer effect instance + GPU cache lives in one map keyed
by `LayerId` on the compositor — the same shape as `layer_cache`
(`compositor.rs:293-310`) — because it describes a layer. The screen-space run
owns only its render targets and pipelines, not the instances. The boundary
belongs to the **document**, not to any layer and not to the panel that draws
it, because it describes a partition of the root group's children and nothing
smaller owns that fact. This is the concrete reason `pinned` was wrong: a
per-layer bool distributed one document-level fact across N layers, which is why
it needed a `CompoundAction` to edit and why it could go internally
inconsistent.

**Document Authority.** `pipeline`, `params`, `blend`, tree position and
`screen_space_count` are document state: persistent, undoable, serializable,
reasonable-about without a GPU. Which space a node renders in is read off the
document by a pure function (§1.3). The compositor's instance map is derived and
rebuildable. `FilterLayer::blend` stops being an unrealized document field (a
violation this closes rather than introduces). Nothing flows uphill.

**Prior Art.** Genuinely useful for the document-side questions — adjustment
layers keeping user-settable opacity and blend (§1.7), applying the selection
*outside* the filter after it runs (§10 D, which is exactly §1.6's move), and
**one category per filter declared by the filter itself** (§1.10: Krita's
`KisFilter` constructor takes exactly one, `kis_filter.h:33`; GIMP's hand-written
`image-menu.ui.in.in` is the anti-pattern). Genuinely **absent** for the
screen-space/canvas-space duality: neither
Krita nor GIMP has viewport-space artistic effects at all — all five GIMP
display filters are colour transforms, proofing or accessibility devices, and
Krita's single `KisDisplayFilter` implementation is OCIO (§10 E). Neither has a
draggable divider in a layer panel (§10 F). Those parts are Darkly-specific and
are argued from Darkly's own model, not stretched onto a reference.

**Testing.** This is a feature, so feature tests (§7). One pre-existing bug is
fixed inside it (§2.5, `requires` dropping pipeline ids) and gets a genuine
regression test written first.

**No Blocking GPU Readbacks.** Nothing here reads back from the GPU.

**No Migrations.** `.darkly` bodies, the manifest `requires` shape, the manifest
`veils` slot, config keys and hotkey action ids all change without an upgrade
path, per the pre-release rule. Existing saves are invalidated.

**Engineering.** The bug being answered — "the same effect appears twice in my
UI" — is a signal that two subsystems model one concept. The fix removes the
second subsystem rather than adding a third surface to reconcile them.

---

## 5. The shape, concretely

### 5.1 One trait

In `crates/darkly/src/gpu/effect.rs`, beside `EffectCache` and `EffectPipeline`
(which already serve both sides today):

```rust
/// A parameterized image transform, prepared against a ping-pong pair at a
/// known resolution. One instance per place the effect is used; the shared
/// pipeline behind it is Arc'd by the registry.
pub trait Effect: std::fmt::Debug {
    fn type_id(&self) -> &'static str;
    fn clone_boxed(&self) -> Box<dyn Effect>;
    fn param_values(&self) -> Vec<ParamValue>;

    fn create_cache(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue,
        ping_pong_views: &[wgpu::TextureView; 2], sampler: &wgpu::Sampler,
        render_width: u32, render_height: u32,
    ) -> EffectCache;

    fn encode(
        &self, encoder: &mut wgpu::CommandEncoder, cache: &EffectCache,
        src_idx: usize, dst_view: &wgpu::TextureView,
    );

    fn perf_scale_factor(&self) -> f32 { 1.0 }
    fn needs_animation(&self) -> bool { false }
    fn update_time(&mut self, _queue: &wgpu::Queue, _cache: &EffectCache, _dt: f32) {}

    /// Adopt a new parameter vector, answering whether `cache` still describes
    /// this instance. The default rebuilds — which is what the viewport chain
    /// does for every slider drag today (`VeilChain::update_veil`). An effect
    /// whose cache *shape* is parameter-independent overrides this to rewrite
    /// its uniform in place and answer `true`. Same idiom as `preview_at`.
    fn set_params(&mut self, _queue: &wgpu::Queue, _cache: &EffectCache,
                  _params: &[ParamValue]) -> bool { false }
}
```

This is today's `Veil` with `set_params` added — the generalization of
`FilterEffect::ensure`. Note `encode` needs **no** `&wgpu::Device`: with one
trait there is no adapter binding views per call, so the old plan's signature
widening is not needed.

`Veil::preview_at` is deliberately **not** carried onto the trait. An earlier
draft kept both it and `EffectRegistration::preview_at`, two differently-shaped
things sharing one name. The registration's
`preview_at: fn(f32) -> Vec<ParamValue>` plus `Effect::set_params` subsumes the
trait method exactly — "what do my parameters look like at time `t`" is a
registration-level question about the effect *type*, and applying the answer is
what `set_params` already does. One name, one shape.

`FilterEffect` disappears. `ParamFilter` (`gpu/param_filter.rs`) becomes
`ParamEffect` (`gpu/param_effect.rs`) — the same substrate, but instance-side:
it holds this instance's `params`, an `Arc<EffectPipeline>`, a packing function
and an optional aux builder, and implements `create_cache` / `encode` /
`set_params`. `gpu/veils/black_and_white.rs`'s 160 lines are almost exactly this
shape already, so the substrate absorbs both families.

`MaskedFilterPipeline` keeps its plain RGBA8 + R8 pipelines and loses its masked
halves, becoming the parameter-free instance substrate `invert` rides.

### 5.2 One registration

```rust
pub struct EffectRegistration {
    pub type_id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    /// Iconify name shown in the picker, the tree row and the Colors-menu
    /// action. Effects render live previews in the picker; the icon is for the
    /// row and the menu.
    pub icon: &'static str,
    /// Which group of the picker this effect appears under — `"Veils"` or
    /// `"Filters"` today (§1.10). Presentational only: nothing in the trait,
    /// the layer kind, the compositor, the boundary or the format reads it.
    /// Same field, same projection and same frontend grouping as
    /// `BlendModeRegistration::category` (`gpu/blend_mode.rs:35`, `:72`).
    pub category: &'static str,
    /// Action id that applies this destructively to the active node. Bindings
    /// in `presets/*.yaml` name this string.
    pub hotkey_action: &'static str,
    pub params: &'static [ParamDef],
    pub preview: Option<PreviewAnim>,
    pub preview_at: Option<fn(f32) -> Vec<ParamValue>>,
    /// Target formats this effect's pipeline may be compiled against.
    /// Declaring `R8Unorm` is what lets an effect run over a mask node.
    pub targets: &'static [wgpu::TextureFormat],
    pub create_pipeline: fn(&wgpu::Device, wgpu::TextureFormat) -> EffectPipeline,
    pub from_params: fn(&[ParamValue], Arc<EffectPipeline>) -> Box<dyn Effect>,
}
```

No capability enum and no capability branching: every effect is constructible,
maskable and destructively applicable, and the only per-effect variation is
which target formats it offers. The two `Option`s are genuine optionality —
`preview` is `None` for an effect with no animated preview, `preview_at` is
`None` for one with no parameter sweep — and an effect with both `None` simply
shows a still card, which the picker already handles
(`EffectPreview.svelte:16-19`'s `supportsPreview`). An earlier draft claimed "no
`Option` fields that can both be `None`, no illegal state" directly above a
struct with two such fields; that claim is withdrawn rather than the fields
being contorted to justify it.

**Two visible consequences to plan for, not to discover.** `EffectRegistration`
makes `icon` and `hotkey_action` non-optional, and `VeilRegistration`
(`gpu/veil.rs:101-116`) has neither. So the ten former veils each need an
Iconify name and an action id, plus bindings in
`crates/darkly/presets/{krita,photoshop,gimp}.yaml`. And because the Colors menu
is built by looping the catalog (`frontend/src/actions/index.ts:760-805`), those
ten become **ten new destructive-apply menu entries** — `watercolor`,
`rainy_glass` and friends become things you can bake into a layer from the menu.
That is §1.6's headline consequence working as intended, but it is a user-facing
change, it belongs in §6 Step 3, and it is now in the LOC table.

`EffectRegistry` caches `HashMap<(&'static str, wgpu::TextureFormat), Arc<EffectPipeline>>`
and exposes `instance(type_id, params, device, format) -> Option<Box<dyn Effect>>`
plus the metadata accessors both current registries already have. `CATALOG_ID`
becomes `"effects"` and `Catalog::title` becomes `"Effects"` — the umbrella term,
with Veils and Filters as its two categories.

`catalog()` emits entries sorted by `(category, display_name)`, so the frontend
can run-length group them exactly as the blend-mode dropdown does
(`LayerProperties.svelte:20-33`) rather than bucketing. `BlendModeRegistry`
already keeps an explicit `ordered: Vec<usize>` for the same reason
(`gpu/blend_mode.rs:101-105`). Category order is alphabetical, which today puts
Filters before Veils; if a third category ever wants a position other than
alphabetical, the fix is the one blend modes already use — a declared order
integer on the registration — and it is additive.

`build.rs` (`crates/darkly/build.rs:81-101`) replaces its two
`generate_catalog_registry` calls (`gpu/veils`, `gpu/filters`) with one for
`gpu/effects` over `crate::gpu::effect::EffectRegistration`.

### 5.3 One apply pass

`shaders/mask_lerp.wgsl` becomes `shaders/in_place_apply.wgsl`. It already
computes `mix(before, after, mask_alpha)` with the mask sampled in its own plane
space. It gains two uniform fields and the blend switch:

```wgsl
let Cs = blend_rgb(after, before, uniforms.blend_mode);   // @blend-switch
let f   = uniforms.opacity * mask_alpha;
return mix(before, vec4f(Cs, after.a), f);
```

At `opacity = 1`, `mask = 1`, `blend_mode = normal` this reduces to `after` —
byte-for-byte today's behaviour. Deliberately **not** Porter-Duff source-over:
source-over of an effect result over its own input inflates alpha at partially
transparent texels (`a + a(1−a) ≠ a`), which is precisely why Krita defaults
adjustment layers to `COMPOSITE_COPY`. Replace-then-lerp is the adjustment-layer
semantic, and both reference editors implement exactly this arithmetic — see
§11's "Replace-then-lerp is what both editors actually compute", which reads
`KoCompositeOpCopy2.h` and `gimpoperationreplace.c` rather than resting on a
call site's name.

The `@blend-switch` splice already exists —
`blend_mode::build_composite_source()` (`gpu/blend_mode.rs:171-186`) is
`TEMPLATE.replacen(MARKER, arms, 1)` over `composite.wgsl`. It generalizes to
`build_blend_source(template: &str)` with two callers, ~10 lines.

Two pipelines are built from it, RGBA8 and R8, because a pipeline is compiled
against one format (§2.3). The R8 one serves destructive apply on a mask node.

#### The apply pass needs a destination, so the effect gets a scratch *output*

A ping-pong pair is two textures, and the apply pass must read both halves and
write a third. `VeilChain` holds exactly `textures: Option<[wgpu::Texture; 2]>`
(`gpu/veil_chain.rs:43`) and `encode` consumes both
(`entry.veil.encode(encoder, &entry.cache, current_src, &veil_views[dst])`,
`:385-387`), so in screen space there is no third target at all. In canvas space
today's third target is the per-host mask snapshot, taken by a full-scissor
`copy_texture_to_texture` (`compositor.rs:4461-4499`) and only for
`masked_in_place_hosts()` (`document/mod.rs:540-566`).

The shape this plan adopts is **not** "snapshot unconditionally". It is to give
the effect a scratch output instead of snapshotting its input:

```
effect:  views[src] ─────────────▶ scratch
apply:  (views[src], scratch) ───▶ views[dst]
```

Same texture count as a snapshot, **one fewer full-canvas copy per effect layer
per frame**, no per-host state, and — unlike the snapshot — literally the same
encoding in both spaces, which is what this section claims and must therefore
deliver. One scratch per space suffices, not one per host: the passes are
sequential within a single encoder, so no two effects hold the scratch at once.

**`snapshot_parent_accum` does not go away.** It survives for masked passthrough
groups, whose "after" cannot be redirected: `compose_passthrough_masked`
(`compositor.rs:4693-4715`) snapshots, then calls `compose_children` (`:4712`),
which writes an arbitrary number of child passes straight into the accumulator,
then lerps (`:4714`). Only the *effect-layer* path stops snapshotting.
`lerp_parent_accum_with_mask` generalizes into the shared apply pass, taking
`before` and `after` as two bound views rather than assuming
`(snapshot, accum[after])`. `masked_in_place_hosts` then narrows to groups —
expressed as a new defaulted `LayerNode` method (passthrough group → `true`,
everything else → `false`) so the compositor still never asks a node what kind
it is.

**This single pass serves every path:**

| path | before | after | destination | mask | opacity / mode |
|---|---|---|---|---|---|
| canvas-space effect layer | accum `views[src]` | canvas scratch | accum `views[dst]` | layer mask | from `EffectLayer::blend` |
| screen-space effect | run `views[src]` | run scratch | run `views[dst]` | none (§1.3 excludes masks) | from `EffectLayer::blend` |
| destructive apply | `src_scratch` | closure-local scratch | `out_scratch` | selection crop | 1.0 / normal |
| masked passthrough group | accumulator snapshot | accum `views[after]` | accum `views[dst]` | group mask | 1.0 / normal (today's `mask_lerp`) |

**Where the destructive path's third texture lives — explicitly.** Not in
`run_filter_region`. That helper (`compositor.rs:132-217`) is shared with
`flip_node_region` (`:1609-1638`), whose closure calls
`ortho_pass.render_mirror_masked`; growing a third scratch there would change
that path's shape for no reason. Instead `filter_node_region`'s closure
(`:1657-1686`) allocates one region-sized intermediate when a mask is present,
writes the effect into it, and writes the apply result into the `out_view` that
`run_filter_region` already copies back (`:197-214`). `run_filter_region` keeps
its two scratches (`:167-168`) and `flip_node_region` is untouched.

**`fs_mirror_masked` is deliberately left alone.** It looks like a seventh
`*_masked` twin that §1.6's hoist misses, but it is a different operation.
`shaders/ortho_transform.wgsl:1-16` states the contract — "no sampler, no
filtering, no premultiply … bit-identical to the input up to the permutation,
which is the whole point" — and `fs_mirror_masked` (`:65-74`) implements it as a
hard `select` between two *source indices* followed by one `textureLoad`. The
shared apply pass mixes two *sampled colours*. Folding one into the other would
invent colours at every soft-mask texel, need a third region texture where zero
are needed today, and cost an extra texture read. The name collides; the
semantics do not.

**Undo-snapshot ordering on the destructive path is unaffected.**
`apply_filter_typed` submits `filter-save` and `filter-commit` before the
`apply-filter` encode (`engine/filters/apply.rs:110-133`), reading the node
texture rather than any pass output, so adding an intermediate and a second pass
inside `filter_node_region` cannot reorder it.

### 5.4 Compositor state

```rust
/// One realized effect: the instance, its scaled render scaffolding, and the
/// facts it was built against. Any drift rebuilds.
struct EffectInstance {
    effect: Box<dyn Effect>,
    scaled: ScaledEffect,
    params: Vec<ParamValue>,
    pipeline_id: String,
    space: EffectSpace,        // Canvas { parent: LayerId } | Screen
    render_size: (u32, u32),
    /// Bumped whenever any accumulator or run texture is recreated, so a stale
    /// bind group can never point at a freed texture.
    target_generation: u64,
}
effect_instances: HashMap<LayerId, EffectInstance>,
```

This replaces `filter_caches: HashMap<LayerId, (Vec<ParamValue>, EffectCache)>`
(`compositor.rs:687`). `sync_effect_instances(device, queue, doc)` prunes to live
effect layers and rebuilds any entry whose recorded facts differ from the
document's — the same prune-and-fingerprint shape the existing sync already uses
(`compositor.rs:3993-4017`), widened from one fact to five.

`target_generation` is the complete answer to stale bind groups: bumped in
`create_group_state` and in `ScreenRun`'s texture (re)creation. `set_canvas_rect`
already recreates every `GroupState` (`compositor.rs:1860-1865`), so the counter
covers canvas resize, crop, group creation and viewport resize with one
comparison and no enumeration of trigger sites to get wrong.

`sync_effect_instances` is called from `render_offscreen` (before the compose
walk) and from `render` (before present), so both spaces are current whichever
dirty flag woke the frame. It is idempotent and cheap when nothing changed.

**The apply scratch (§5.3), one per space.** The canvas-space one is
canvas-padded and `accum_format`, allocated beside the group accumulators and
covered by the same `target_generation` bump — `set_canvas_rect` already
recreates every `GroupState` (`compositor.rs:1860-1865`). The screen-space one
lives on `ScreenRun` beside its ping-pong pair and is recreated by the same
`ensure_textures` that sizes them, which `set_canvas_rect` does **not** touch —
noted here because it is the one invalidation trigger that is not the canvas
rect, and Test 16 covers it.

One consequence of bumping `target_generation` in `create_group_state` is that
creating *any* group invalidates *every* effect instance, including screen-space
ones that never pointed at that group's accumulator. Safe but wasteful; a
per-space counter would be the refinement if profiling ever asks for it.

### 5.5 The screen-space run

`gpu/veil_chain.rs` (667 lines) becomes `gpu/screen_run.rs`. `ScreenRun` keeps
the ping-pong textures at viewport resolution, the blit/downscale/upscale
pipelines and `resize`; it loses `entries`, `add_veil`, `remove_veil`,
`move_veil`, `update_veil`, `set_veil_visible`, `clear_veils`, `count`, `info`,
`type_id`, `param_values` and its own registry — all of which the document tree
now provides.

Its encode loop is driven from the document — a slice read straight off the
stored boundary, with no walk and no predicate evaluation per frame:

```rust
// compositor.rs, replacing present_and_veils (:3242-3277)
let run = doc.screen_space_run();                    // &[LayerId], bottom-to-top
if run.iter().all(|id| !doc.effective_visible(*id)) { /* present direct */ }
```

then, per visible member: `ScaledEffect::encode` writes `views[src] → scratch`,
and the in-place apply pass writes `(views[src], scratch) → views[dst]` carrying
that layer's opacity and blend mode (§5.3). No mask binding is ever needed here,
because §1.3 makes a masked node ineligible to be above the boundary.

**Isolation and the run.** `compose_children` filters on `is_in_isolation_path`
(`compositor.rs:3732`) but the screen-space run is driven from
`doc.screen_space_run()` and is not subject to that filter, so isolating a
canvas-space layer still shows the whole viewport with the run's effects on it.
That matches today's veil behaviour exactly, and it is the right answer — the
run is a viewport treatment, and isolation is a statement about document
content. Stated so it is a decision rather than an accident.

The scaling machinery moves out of `veil_chain.rs:15-27, 536-667` into
`gpu/effect_scaling.rs` as a space-agnostic `ScaledEffect` (prepare + encode,
owning downscale → effect → upscale), because both spaces need it: `painting`
declares `perf_scale_factor() == 0.7` for 169 taps/pixel
(`gpu/veils/painting.rs:110-114`) and a 4096² canvas is ~8× a 1440p viewport.

Config: `rendering.veil_scale` (`config/sections/rendering.rs:4`) becomes
`rendering.screen_effect_scale`; a sibling `rendering.canvas_effect_scale`
(default `1.0`) is added — canvas-space output is document content, so full
resolution is the right default there while `0.7071` remains right for the
viewport.

### 5.6 The present path is unchanged

An earlier draft proposed splitting `fs_present` into a checker-compositing
entry point and a straight-alpha `fs_present_raw` feeding the run, so that an
effect looked identical on both sides of the divider. **Dropped** — §2.7 shows
it is a visible regression at the shipped default resolution scale, not a taste
question. The run keeps operating on the presented, checker-composited image
exactly as today's veil chain does, `fs_present` is untouched, and the
appearance difference across the divider is explained by Q4's labelling rather
than engineered away. The §2.4 shader alpha fixes remain load-bearing for canvas
space, which is the space that has real alpha.

### 5.7 Animation

`animation.veil_divisor` and `animation.void_divisor`
(`config/sections/animation.rs:7`, `:23`; `presets/defaults.yaml:121`, `:123`)
are renamed by what they now govern:

- `animation.screen_divisor` — the screen-space run; drives `needs_present`.
- `animation.canvas_divisor` — animated voids **and** canvas-space effect
  layers; drives `needs_composite`.

`void_divisor` becomes an outright misnomer once it governs effect layers, and
the two keys are now distinguished by space rather than by subsystem, which is
the honest axis. `any_animated_layer` (`compositor.rs:2832-2837`) and
`tick_animated_layers` (`:2844-2857`) are extended to walk `effect_instances`
alongside `layer_cache`, gated on `doc.effective_visible(id)` exactly as the
void path already is. A layer that moves between spaces simply starts being
picked up by the other loop on the next frame — no migration, no state.

### 5.8 The divider, on the frontend

`SpaceDivider.svelte`, rendered inside `.layer-list`
(`ui/layers/LayerPanel.svelte:41-57`) at the boundary the tree data implies. It
uses `use:pointerDrag` (`ui/workspace/pointerDrag.ts`) in the `.gutter` style of
`ui/workspace/Subdivision.svelte:61-67` — a *resize*-style drag with pointer
capture and Escape-abort, **not** HTML5 DnD. HTML5 DnD is the wrong tool here:
the divider is not a droppable item, and the panel already runs two independent
DnD MIME channels whose interaction is imperfect (`ui/layers/LayerItem.svelte:325`
does not guard on MIME type).

The panel needs two facts, and neither requires it to reimplement anything:

- **The boundary itself.** One number on the layer-tree response —
  `screenSpaceCount` — read straight from `doc.screen_space_count`. The divider
  is drawn that many rows down from the top (the panel renders root children
  reversed, `engine/veils.rs:82-92`).
- **`screenSpaceEligible: bool` on each root-level `LayerInfo`** — the drag
  clamp. The divider's range is the leading rows that answer `true`; it stops at
  the first that answers `false`. This is UI responsiveness only; the engine
  clamps authoritatively when the drag lands.

On release the panel calls one wire method,
`setScreenSpaceBoundary({ count })`, which validates `count` against the
qualifying suffix, assigns `doc.screen_space_count`, and pushes one
`ScreenSpaceBoundaryAction { old, new }` — a document-level undo action in the
shape of the existing `CanvasGeometryAction` (`undo/canvas_geometry.rs:35-100`)
and `SelectionMetadataAction`, both of which already restore plain document
fields. One drag, one scalar, one undo step, no compound.

`ui/veils/` is deleted outright (488 lines). `ui/filters/` becomes `ui/effects/`.
`actions/index.ts` loses `deleteLayer`'s veil-priority branch (`:674-677`).
`newVeil` and `newFilterLayer` **survive** as the two category deep-links of
§5.9. The three picker modals and `state/layerPicker.svelte.ts` are deleted by
§5.9's modal, not by this section.

#### The labelling (Q4, closed)

**The divider marks *space*; the category (§1.10) marks *what kind of effect it
is*. These are orthogonal, and the copy must not conflate them.** A
`Filters`-category effect may sit above the divider and a `Veils`-category effect
below it, and both are correct there. So:

- **The divider must not be labelled "Veils"**, and no row's category badge may
  be read as a statement about which side of the line it is on. Labelling the
  divider "Veils" would recreate the pinned-folder mental model this whole plan
  removes, and it would be a lie the first time a user drags `curves` above it.
- **What the divider actually means is "not part of the image".** That is the one
  consequence a user must never be surprised by (Risk 5), and it is what the copy
  says.

Concrete copy:

| surface | text |
|---|---|
| divider rest label (dimmed, on the rule) | `Viewport only — not exported` |
| divider tooltip | `Anything above this line is applied to your view of the canvas. It is not part of the image: exports, Flatten and Merge ignore it.` |
| divider at `count == 0` (top of list) | same label, further dimmed, plus hover hint `Drag down to make an effect viewport-only` |
| badge on each row above the line | monitor glyph, `title="Viewport only — not exported"` |
| add-layer modal, Veils tab blurb | `Animated and distorting effects. Add one anywhere in the stack; drag it above the viewport line to apply it to your view instead of the image.` |

Deliberately absent from all of it: the words "screen space" and "canvas space".
They are the plan's vocabulary, not the user's; the user's question is "will this
end up in my PNG", and the label answers exactly that.

### 5.9 The add-layer modal, concretely

`ui/layers/AddLayerModal.svelte`, mounted where `LayerPickers` is today
(`frontend/src/App.svelte:71`) and for the same reason — it is reachable from the
palette and menu bar, not only from the panel.

#### The rail is derived, not written

Two declarations on `LayerKindRegistration`
(`crates/darkly/src/document/layer_kind.rs:55-104`), which already carries
exactly this class of UI-facing, type-owned fact — `can_have_mask` (`:63-66`),
`can_rename` (`:68-70`), `has_thumbnail` (`:72-75`) and `icon` (`:77-80`), each
documented as existing so the panel need not branch on `type_id`:

```rust
/// Action that adds this kind, or `""` when the kind is not user-addable.
/// Its presence is what puts the kind in the add-layer modal at all, and its
/// menu order is what positions the kind's tab(s) — the same declared order
/// the Layer menu already lists them in, so the two cannot drift.
pub new_action: &'static str,

/// Catalog of this kind's selectable variants, or `""` when choosing the
/// kind is the whole choice. A kind with variants contributes one tab per
/// `category` its catalog's entries declare, rather than one tab.
pub variant_catalog: &'static str,
```

| kind | `new_action` | `variant_catalog` | tabs |
|---|---|---|---|
| raster (`layer_kinds/raster.rs`) | `newLayer` | — | **Normal** |
| effect (`layer_kinds/filter.rs` → `effect.rs`) | `newEffectLayer` | `effects` | **Filters**, **Veils** |
| void (`layer_kinds/void.rs`) | `newVoid` | `voids` | **Voids** |
| group (`layer_kinds/group.rs`) | `newGroup` | — | **Group** |
| vector (`layer_kinds/vector.rs`) | `""` | `""` | none |

`vector` declaring neither is not an omission — there is no way to add a vector
layer today (`NEW_LAYER_ACTION_IDS`, `actions/index.ts:37-43`, has no entry for
it), and the empty string is how the registration says so.

`new_action` projects onto the **existing** `CatalogEntry::hotkey_action`
(`catalog.rs:34-35`, *"Action id this variant is bound to"*) via the existing
`with_hotkey_action` setter (`:91-94`). Only `variant_catalog` is a new
`CatalogEntry` field, and it follows the precedent of `capture_kind`
(`catalog.rs:50-51`, *"voids only"*) — a per-kind optional on the shared shape.

No void declares a `category` (verified: zero `category` occurrences under
`crates/darkly/src/gpu/voids/`), so `voids` yields one tab, titled by
`Catalog::title`. If a void ever declares one, Voids splits into category tabs
with no modal edit — the same rule, applied uniformly.

#### The panes

- **A tab with no variant catalog** shows the kind's `description` (already on
  the registration, `layer_kind.rs:59-61`) and one primary button labelled from
  the action's `display_name` ("New Layer", "New Group" —
  `crates/darkly/src/actions/layers.rs:6`, `:30`). Focused on open.
- **A tab with a variant catalog** shows the card grid the pickers already show,
  lifted from `VeilPickerModal.svelte:31-41` — `EffectPreview` plus a name, one
  card marked selected. `EffectPreview.svelte:21` already takes `catalog` as a
  string prop, so it needs no change.
- **One search input** in the modal header, in the shape of
  `SettingsModal.svelte:100-104`, filtering across tabs — the answer to §1.10's
  discoverability objection, and the reason a single category per effect is
  sufficient.

#### Keyboard

- **Enter** spawns the selected item and closes. Implemented as
  `NewDocumentModal.svelte:130-135` does it — `onkeydown` on the modal's body
  div with `e.preventDefault()` — which is safe without qualification because
  `Modal.svelte:68-76` already stops every keydown from reaching the window
  hotkey handlers.
- **Escape / backdrop / ×** close, all three already owned by `Modal.svelte`
  (`:79-88`, `:103`).
- **Up/Down** move the tab rail; **Left/Right** and Tab move within a card grid.
  The rail's `.tab` buttons are real `<button>`s (as in
  `SettingsModal.svelte:116-127`), so Tab order is correct with no `tabindex`
  bookkeeping.
- The **first tab is preselected**, and "first" is a declared fact rather than a
  `type_id` check in the modal: raster's `newLayer` is `menuPath: ['Layer:10']`
  (`actions/index.ts:577-578`), the lowest of the five. Deliberately reordering
  the Layer menu would deliberately change the default, which is right.

#### Spawning stays per-catalog

The modal never branches on which kind it is showing. Each variant catalog owns
its spawn function in its own module — `ui/layers/addSources/effects.ts`,
`addSources/voids.ts` — keyed by catalog id, so a future variant catalog adds a
file rather than an arm. This is what preserves `VoidPickerModal.svelte:19-36`'s
load-bearing constraint (§2.9): the void spawn function acquires the
`MediaStream` **before its first `await`**, inside the activating gesture,
because `getDisplayMedia` requires transient user activation. Enter is a user
gesture and still grants activation, but only if nothing is awaited first.

#### Actions

`newLayer` and `newGroup` keep spawning directly — they are the hotkey path, and
routing a bound key through a modal would be a regression (**Q9**). The `+`
button dispatches a new `addLayer` action ("Add Layer…") that opens the modal on
its first tab.

`newVeil` and `newFilterLayer` **survive**, retargeted from
`layerPicker.kind = 'veil' | 'filter'` (`actions/index.ts:591`, `:597`) to
opening the modal on the named category tab. That is what keeps "New Veil" in the
Layer menu and reachable from the command palette — the property
`frontend/src/actions/__tests__/menu_actions.test.ts:141-149` asserts and
Change 1 requires. They are *category deep links*, so `newVeil` is a current
name, not a legacy one, and PR 5 does not rename it.

These deep links are optional: a category's tab exists because an effect declares
the category, not because an action points at it. A third category appears in the
modal with zero edits; giving it a palette shortcut is a separate, additive line.

`NEW_LAYER_ACTION_IDS` (`actions/index.ts:37-43`) is **deleted** — the rail it
ordered is now derived from the layer-kind catalog.

---

## 6. Implementation steps

Grouped into **five** sequenced PRs (§8). Every step leaves the workspace
compiling — and PR 2's steps are ordered **2, 3, 5, 6, 4** to make that true.
Step 4 drives `ScaledEffect::encode` over an `effect_instances` entry, so it
needs both Step 5 (which creates `ScaledEffect`) and Step 6 (which creates the
map); and §5.4's `EffectInstance` has a `scaled: ScaledEffect` field, so Step 6
needs Step 5 in turn. The review proposed `2, 3, 6, 5, 4`, which inverts that
last dependency.

Step numbers are identifiers, not an execution order — PR 2's internal ordering
above already establishes that.

**The add-layer modal is its own PR, third of five.** It is pure frontend, it
depends only on PR 2's merged catalog and its `category` declarations, and it has
no dependency whatever on the boundary, the divider or the compositor. Landing it
inside PR 2 would put ~350 lines of Svelte on top of the largest GPU change in
the plan; landing it inside PR 4 would couple a UI consolidation to a rendering
change. It reviews cleanly on its own, so it ships on its own.

### PR 1 — Alpha correctness

**Step 1.** Fix the six shaders of §2.4. Five are one line — return the sampled
`.a` instead of `1.0`. `lens_blur` is ~4: accumulate the normalizer from a
constant `exp(inv_t)` weight and carry alpha separately. Do **not** touch
`watercolor`'s passes 0/1.

**Specify `lens_blur`'s alpha explicitly: a linear mean.** Alpha is coverage,
and the correct blur of coverage is an unweighted average of the samples'
alpha. Reusing the exponential smooth-max the *colour* path uses would
over-weight opaque samples and inflate coverage at every alpha edge. The rewrite
is bit-identical in the viewport regardless — `acc.a` today is
`Σ exp(s.a · inv_t)` with `s.a == 1.0` (`shaders/veils/lens_blur.wgsl:73`,
`:82-83`), so a constant `exp(inv_t)` weight gives the same sum — which is why
the choice must be *stated* rather than inferred from a passing viewport test.
Test 6's alpha-edge sample is therefore chosen to be a value a smooth-max would
get wrong, so the test actually discriminates between the two.

Verifiable today against the existing veil path, viewport-bit-identical, and it
de-risks everything after it.

### PR 2 — One effect

**Step 2 — the trait and the registry.**
- New `Effect` trait in `gpu/effect.rs` (§5.1); `EffectRegistration`,
  `EffectRegistry` with per-`(type, format)` pipeline caching (§5.2); one
  `EffectMechanism`/`EffectSession` folded from `VeilMechanism`/`VeilSession`
  (`gpu/veil.rs:272-373`) and `FilterMechanism`/`FilterSession`
  (`gpu/filter.rs:267-352`). `PreviewRegistries` (`gpu/preview.rs`) carries one
  `effects` field instead of `veils` + `filters`.
- `gpu/param_filter.rs` → `gpu/param_effect.rs`, instance-side, masked halves
  deleted.
- `MaskedFilterPipeline` loses `masked_rgba`, `masked_r8`, `masked_bgl` and the
  `match mask_view` branch (`gpu/effect.rs:216-408`).
- Delete `gpu/veil.rs` and `gpu/filter.rs`.
- `build.rs:81-101`: two catalog registries → one over `gpu/effects`.

**Step 3 — the modules (`git mv` commit, then edits).**
- `git mv` all 17 files from `gpu/veils/` and `gpu/filters/` into `gpu/effects/`,
  and all 15 shaders from `shaders/veils/` + `shaders/filters/` into
  `shaders/effects/`. **Its own commit**, so rename detection keeps the diff
  reviewable.
- Collapse the two duplicate pairs: `black_and_white` (160 + 70 → one file over
  the existing shared core `gpu/black_and_white.rs`, 281 lines, untouched) and
  `chromatic_aberration` (149 + 443 → one; the veil already imports `PARAMS`,
  `pack_uniform` and `GpuAberrationParams` from the filter module per its own
  doc header).
- Collapse the two shader pairs; delete the six `*_masked` entry points.
- Adapt the 7 former filter modules to instance form over `ParamEffect` /
  `MaskedFilterPipeline`; `gpu/lut_filter.rs` follows.
- `invert` declares `targets: &[Rgba8Unorm, R8Unorm]`; everything else declares
  RGBA8 only.
- Give the ten former veils an `icon` and a `hotkey_action`, and bind the
  actions in `crates/darkly/presets/{krita,photoshop,gimp}.yaml`. Ten new
  destructive-apply entries appear in the Colors menu, which is built by
  looping the catalog (`frontend/src/actions/index.ts:760-805`) — a deliberate,
  user-facing consequence of §1.6, not a side effect.
- **Give all 15 effects a `category`** per §1.10's table — six `"Filters"`, nine
  `"Veils"` — projected through the existing `CatalogEntry::with_category`
  (`catalog.rs:86-89`) exactly as `BlendModeRegistration` does
  (`gpu/blend_mode.rs:72`). Sort `catalog()` by `(category, display_name)` so the
  frontend can run-length group it the way `LayerProperties.svelte:20-33`
  already groups blend modes.

**Step 5 — resolution scaling.**
- Extract `VeilScaling`, `create_veil_resources`, `create_veil_scaling`,
  `blit_pass` (`veil_chain.rs:15-27, 536-667`) into `gpu/effect_scaling.rs` as
  `ScaledEffect` (§5.5).
- `rendering.veil_scale` → `rendering.screen_effect_scale`; add
  `rendering.canvas_effect_scale`. Update `config/sections/rendering.rs:4` and
  `presets/defaults.yaml:125`.

**Step 6 — instance state.**
- Replace `filter_caches` with `effect_instances` + `sync_effect_instances`
  (§5.4), including the `target_generation` counter bumped in
  `create_group_state` and in `ScreenRun`'s texture (re)creation.
- Allocate the canvas-space apply scratch beside the group accumulators (§5.4).
- Remove `CompositionContext::compose_layer`'s `if let Layer::Filter(f)` branch
  (`compositor.rs:330-345`), routing through `LayerNode::composites_in_place()`.

**Step 4 — the apply pass.**
- `shaders/mask_lerp.wgsl` → `shaders/in_place_apply.wgsl` with opacity,
  blend_mode and the `@blend-switch` splice (§5.3).
- `blend_mode::build_composite_source()` → `build_blend_source(template)`
  (`gpu/blend_mode.rs:171-186`), two callers.
- Build RGBA8 + R8 pipelines from it.
- `compose_filter_arm` → `compose_effect_arm`: drop the `mask` argument to
  `render`, drive the instance via `ScaledEffect::encode` into the **canvas
  scratch**, then route `(views[src], scratch)` through the apply pass carrying
  `EffectLayer::blend` into `views[dst]` — which closes §1.7's Document
  Authority violation. **No snapshot on this path at all.** The histogram tap
  (`compositor.rs:4409-4417`) and the ping-pong advance are unchanged.
- Generalize `lerp_parent_accum_with_mask` into the shared apply call, taking
  `before` / `after` as bound views; keep `snapshot_parent_accum` for
  `compose_passthrough_masked` (`compositor.rs:4693-4715`), which cannot use a
  scratch output. Narrow `masked_in_place_hosts` (`document/mod.rs:540-566`) to
  groups via a defaulted `LayerNode` method (§5.3).
- `filter_node_region`'s closure (`compositor.rs:1657-1686`) allocates one
  region-sized intermediate when a mask is present: effect writes
  `src → intermediate`, apply writes `(src, intermediate, mask) → out_view`.
  `run_filter_region` (`:132-217`) and `flip_node_region` (`:1609-1638`) are
  **not** changed, and `fs_mirror_masked` is left alone (§5.3).
- `engine/filters/apply.rs` resolves through the unified registry; the R8 path
  keeps working because `invert` declares R8.

**Step 4b — the two pickers filter by category.** Both existing pickers now read
the merged `"effects"` catalog, so without this "Add Veil" would offer `curves`
and "Add Filter Layer" would offer `watercolor` — the duplicate *entries* (the
reported bug) fixed, but a worse duplicate *surface* left standing.

The fix is **two lines each**, not a new component: `VeilPickerModal.svelte:15`
becomes `app.entries?.('effects').filter(e => e.category === 'Veils') ?? []` and
`FilterPickerModal.svelte:18` the same with `'Filters'`. Both then also call
`addFilter` rather than `addVeil`, which is where "veils are layers now" first
becomes visible to a user.

An earlier draft had this step build a shared `EffectPickerModal.svelte`. That is
now wasted work: PR 3 deletes both pickers outright (§5.9), so a merged component
would live for exactly one PR. The category filter is smaller, ships the same
correctness, and throws nothing away — and it is the first consumer of §1.10's
declarations, which is what earns them their place in PR 2.

At the end of PR 2 the duplicate catalog entries are gone, each picker shows its
own category, every effect is maskable and destructively applicable, and
opacity/blend work on effect layers — **with no new rendering space and no
boundary semantics**. The screen-space chain still exists, unchanged in
behaviour, resolving from the merged registry.

#### PR 2 — ✅ SHIPPED. Where the implementation diverged

Everything above landed. Four things differ from what this section describes,
each because implementation found something the plan had not:

- **`Effect::seek(t)` exists, alongside `set_params`.** §5.1 claims the
  registration's `preview_at` plus `set_params` "subsumes the trait method
  exactly". It does not, for `grain`, `rainy_glass` and `vhs`, whose previews
  position a **clock** rather than a parameter — `self.time = PREVIEW_SECONDS *
  t * self.speed` is not expressible as a parameter vector, because time is not
  in the schema and does not serialize. `seek` is the absolute counterpart to
  the `update_time` the trait already had: it never invalidates a cache (a clock
  cannot change what a cache *is*), so unlike `set_params` it answers nothing.
  Two questions, two methods.
- **`MaskedFilterPipeline` is deleted rather than narrowed.** §6 Step 2 has it
  survive as "the parameter-free instance substrate `invert` rides". Once the
  masked halves went, it was a single-pass pipeline over `[src]` — which is
  `ParamEffect` with `Resources::None`. Keeping both would have been two
  substrates for one shape. `invert` rides `ParamEffect` like everything else.
- **`ParamEffect` models its resources rather than taking one `prepare`
  closure.** `ParamFilter`'s `prepare(device, queue, params, cache)` needed a
  device on every parameter change, which forced a rebuild for a slider drag.
  Splitting it into `Resources::{None, Packed, Baked{alloc, write}}` makes
  allocation once-per-instance and writing device-free, so `set_params` can
  always answer `true`. That is what turns a Curves drag from "reallocate a LUT
  texture" into "write 1 KB".
- **`ParamSlots` (`gpu/params.rs`) replaces ~25 copies of the same `match
  params.get(i)` block** across the effect modules. Not in the plan; it fell out
  of every module needing the same positional decode once construction and
  `set_params` both had to do it.

Two costs the plan's LOC table did not carry, both real: the compositor grew
`sync_effect_instances`, the canvas apply scratch and `target_generation`
(+686/−309), and the blend math had to be extracted into `shaders/lib/blend.wgsl`
so `in_place_apply.wgsl` could reach it. Production is **≈ +360**, not the
**≈ −550** §8 projected for this PR — the deletions the estimate counted on
(`manifest.veils`, the veil wire methods, `ui/veils/`, `EffectChain` itself)
all live in PR 4, while PR 2 paid for the machinery they will be deleted
*through*. Tests are +5 net files.

### PR 3 — One add-layer modal — ✅ SHIPPED, ahead of PR 2

Implemented by `docs/plans/unified-layer-picker.md`, which ran this PR before
PR 2 at the user's direction. Read that plan for what was actually built; the
notes below record where it diverged from this one.

**Step 6a — the add-source declarations (Rust) — NOT DONE, deferred to PR 4.**

The rail was to be derived from `LayerKindRegistration.new_action` +
`variant_catalog` plus a new `CatalogEntry.variant_catalog` wire field. That was
impossible ahead of PR 2 — veils are not a layer kind, so there is no
registration to hang `variant_catalog: "veils"` on — so the rail is derived
instead from `frontend/src/ui/layers/addSources/*.ts`, one file per way of
adding something, globbed.

Whether `variant_catalog` returns is **re-decided at PR 4**, when effects become
layer kinds and the blocking precondition disappears. It is deferred, not
rejected: the interim plan argued for deleting it outright and its independent
review (finding S5) overruled that, on the grounds that the argument against it
rests on a precondition PR 4 removes.

**PR 2 still owes the `category` declarations.** The interim shipped without
them — with one merged registry each catalog titles its own tab, so nothing
needed `category` until the registries merge and one catalog must split into
two tabs. The 16 declarations from §1.10 land in PR 2.

**PR 2 also inherits `addable`.** The duplicate `black_and_white` /
`chromatic_aberration` pairs are resolved by an `addable: bool` on
`VeilRegistration` / `FilterPipelineRegistration` / `CatalogEntry`, declared
`false` by the redundant half of each pair. When PR 2 merges the modules the
duplicates cease to exist; delete the field, its two `false` declarations and
`crates/darkly/tests/effect_addability.rs` with them.

**Step 6b — the modal (frontend).**
- New `ui/layers/AddLayerModal.svelte` (§5.9): rail derived from the `layerKinds`
  catalog, expanded per `category` for kinds with a `variantCatalog`; panes;
  cross-tab search; Enter-to-spawn in the shape of
  `NewDocumentModal.svelte:130-135`.
- New `ui/layers/addSources/{effects,voids}.ts` — one spawn function per variant
  catalog, keyed by catalog id. `voids.ts` carries `VoidPickerModal.svelte`'s
  whole `pick` body (`:18-66`) verbatim, **including the acquire-before-await
  ordering** its comment explains (`:20-27`), the session allow-list opt-in
  (`:55-58`) and the release-on-failure path (`:59-63`); losing any of them
  breaks camera and screenshare in ways no test catches.
- Mount it at `App.svelte:71` in place of `<LayerPickers />`.
- Delete `ui/layers/NewLayerMenu.svelte` (93), `ui/layers/LayerPickers.svelte`
  (27), `state/layerPicker.svelte.ts` (12), `ui/veils/VeilPickerModal.svelte`
  (82), `ui/filters/FilterPickerModal.svelte` (84),
  `ui/voids/VoidPickerModal.svelte` (121).
- `LayerFooter.svelte`: the split button (`:104-123`) and its
  `.split-btn` / `.split-main` / `.split-chevron` rules (`:195-223`) become one
  plain `+` dispatching `addLayer`. **Everything else in the file stays** — mask,
  duplicate, delete, `findNode`, `activeEditable`, `canAddMask`, `canDelete`,
  `canDuplicate` and the multi-selection tooltips (§2.9).
- `actions/index.ts`: delete `NEW_LAYER_ACTION_IDS` (`:37-43`); add `addLayer`
  (opens the modal on its first tab) with an `ActionDef` in
  `crates/darkly/src/actions/layers.rs`; retarget `newFilterLayer` (`:589-592`)
  and `newVeil` (`:595-598`) to open the modal on the Filters / Veils tab, and
  `newVoid` (`:601-604`) on the Voids tab. `newLayer` (`:576-587`) and `newGroup`
  (`:606-…`) keep spawning directly (**Q9**).

At the end of PR 3 there is one add-layer surface, the word "Veils" is a tab in
it, and nothing about rendering has changed.

### PR 4 — Effects in the tree

**Step 7 — the boundary, the predicate, and the invariant.**
- `LayerNode::supports_screen_space(&self, doc) -> bool` in `layer.rs`, beside
  `composites_in_place()` (`:714-721`) — a *validation* predicate (§1.3).
- `Document::screen_space_count: usize` (`document/mod.rs:91-170`), defaulted to
  `0` in `Document::new` (`:181-193`).
- `Document::screen_space_run(&self) -> &[LayerId]` with the read clamp,
  `Document::renders_in_screen_space(id) -> bool`,
  `Document::screen_space_eligible(id) -> bool`.
- **The invariant, in `Document::link` (`:982-994`) and `Document::unlink`
  (`:1003-1019`) only** (§1.9). Every add path, paste, duplicate, group,
  drag-reorder and undo/redo inherits it; none of them is edited.
- `TreeSlot { parent, position, screen_space }` + `Document::slot_of`;
  `reinsert_entity` takes it; the five structural undo actions
  (`undo/layer.rs:18-20`, `:59-61`, `:111-115`, `:162-164`, `:212-216`) replace
  their `(parent, position)` pairs with it (§1.9 — **Q3** if this is descoped).
- `ScreenSpaceBoundaryAction { old, new }` in `undo/`, shaped like
  `CanvasGeometryAction` (`undo/canvas_geometry.rs:35-100`).

**Step 8 — the compositor, flatten and merge.**
- `compose_children` (`compositor.rs:3711-3743`) skips nodes above the boundary —
  one filter beside the existing `visible()` (`:3725`) and isolation (`:3732`)
  filters. Export inherits the exclusion for free, since `render_offscreen`
  (`:3403-3439`) composites through the same walk.
- **Flatten and merge do not inherit it and must be fixed in the engine, not the
  compositor.** `flatten_image` snapshots
  `let top_level: Vec<LayerId> = self.doc.children_of(root_id).to_vec();`
  (`engine/flatten.rs:18`) and later detaches **every** entry of it (`:67-83`),
  so a screen-space effect would be deleted from the tree without ever having
  been baked. Restrict `top_level` to the below-boundary slice; both the bake
  input (`:27-36`, `:56-62`) and the detach loop then follow. The result layer
  needs no change — `:87-88` re-homes it with
  `reinsert_entity(result_id, Some(root_id), 0)`, and position 0 is the *bottom*
  of the bottom-to-top list (`document/layer_kinds/group.rs:26-27`), always
  below the boundary.
- `merge_down` (`engine/merge.rs:24`, baking `&[target_id, source_id]` at
  `:93-99`) and `merge_layers` (`:174`, baking an arbitrary id list at `:238`)
  **reject** ids above the boundary. Both already return
  `Result<LayerId, String>`, so the refusal surfaces to the user rather than
  silently eating a viewport effect. Refusal rather than exclusion here because
  "merge these two things" with one of them absent from the merge is not a
  meaningful outcome, whereas Flatten Image legitimately means "flatten the
  document content".
- `gpu/veil_chain.rs` → `gpu/screen_run.rs`, driven from `doc.screen_space_run()`
  (§5.5). `present_and_veils` (`compositor.rs:3242-3277`) →
  `present_and_screen_run`. `fs_present` is **unchanged** (§5.6).
- Animation: divisor renames, `effect_instances` walked in `any_animated_layer` /
  `tick_animated_layers`, screen members driving `needs_present` and canvas
  members `needs_composite` (§5.7). Add `Compositor::mark_effect_dirty(doc, id)`
  so a screen-space param drag does not force a full canvas recomposite.

**Step 9 — engine, protocol, save/load.**
- `engine/veils.rs`: delete the veil half (the whole `add_veil` … `veil_param_defs`
  block, `:13-77` and `:98-118`); keep `layer_tree` (`:82-95`) and `catalogs`.
- `engine/layers.rs`: `add_filter_layer` → `add_effect_layer` over the unified
  registry (`:782-805`); new `set_screen_space_boundary(count)` — validate,
  assign, push one `ScreenSpaceBoundaryAction`.
- `engine/save.rs`: delete `build_manifest_veils` (`:407-424`); write
  `screen_space_count` in the `Manifest` literal (`:334-352`, beside
  `selection_id` at `:350`); `requires_from_doc` (`:430-470`) records
  effect-layer `pipeline` ids and emits them as `requires.effect`, replacing
  `requires.veil` — **fixing §2.5's silent drop**, where the node arm
  (`:438-441`) records only the generic layer-kind id (`layer.rs:681`) and never
  the per-layer `pipeline` string (`layer.rs:243`). Validate `pipeline` on
  deserialize and correct the false doc comment at
  `document/layer_kinds/filter.rs:10-13`.
- `engine/load.rs`: read `screen_space_count` and clamp it to the qualifying
  suffix (`:202-206`, beside the `selection_id` remap at `:361-365`) — load
  bypasses `link` entirely (`:371-390`), so this is the one place the invariant
  is re-established by hand (§1.9).
- `format/manifest.rs`: add `screen_space_count` with `#[serde(default)]`
  alongside `selection_id` (`:68-70`); drop `ManifestVeil` and the `veils` field
  (`:71-72`, `:162`); `requires.veil` → `requires.effect` (`:153`).
  `format/error.rs:5,23,154` messages retarget from `"veil/…"` to `"effect/…"`.
- `LayerInfo` gains `screenSpaceEligible`, and the layer-tree response gains
  `screenSpaceCount` (`engine/types.rs:552-576`, §5.8).

**Step 10 — frontend.**
- Delete the rest of `ui/veils/` — `VeilFolder` (148), `VeilItem` (237),
  `VeilProperties` (21); `VeilPickerModal` (82) already went in PR 3.
- `ui/filters/` → `ui/effects/` (`FilterPickerModal` already deleted in PR 3, so
  8 files move, not 9).
- `SpaceDivider.svelte` + its mount in `LayerPanel.svelte:41-57`, carrying §5.8's
  labelling copy; the per-row "viewport only" badge in `LayerItem.svelte`.
- `app.svelte.ts`: delete `activeVeilIndex` (`:327`), `veilList` (`:357`),
  `addVeil`, `selectVeil`, `removeVeil`, `moveVeil`, `refreshVeilList` and the
  six reset sites. `LayerFooter.svelte:64` and `:146` lose their
  `activeVeilIndex` reads with them.
- `LayerPanel.svelte:42-44`: drop the `{#if app.veilList.length > 0}` guard and
  the `<VeilFolder>` mount; `:54` loses its `&& app.veilList.length === 0`.
- `actions/index.ts`: drop `deleteLayer`'s veil branch (`:674-677`); the dynamic
  Colors-menu loop (`:760-805`) reads the effects catalog. **`newVeil` stays** —
  it is now the Veils-tab deep link (§5.9), retargeted in PR 3 and not removed
  here. `newFilterLayer` likewise stays as the Filters deep link; the *layer
  kind's* action id becomes `newEffectLayer`, which is a distinct, third
  registration added in PR 3.
- `state/freshDocument.ts`: the demo recipe's four `addVeil` calls (`:47-50`)
  become four seeded effect layers followed by one
  `setScreenSpaceBoundary({ count: 4 })`, so the demo's viewport effects land
  above the divider (asserted by `state/__tests__/freshDocument.test.ts:31-38`).
  The `resize` preamble at `:41-46` — which existed only because `add_veil`
  unwrapped the veil chain's lazily-allocated views — goes with it.

#### PR 4 — ✅ SHIPPED. Where the implementation diverged

Everything above landed. What differs, each because implementation found
something the plan had not:

- **The step list was written against pre-PR-2 names and was re-synced first.**
  `compose_filter_arm` is `compose_effect_arm`, `lerp_parent_accum_with_mask` is
  `apply_in_place`, `gpu/veil_chain.rs` was already `gpu/effect_chain.rs`, and
  `masked_in_place_hosts` no longer exists (`snapshot_in_place_hosts` does). PR 2
  had also already shipped `target_generation` and the canvas apply scratch, so
  Step 8's invalidation work was narrower than written.
- **`sync_effect_instances` had to run on the present path too, and this was a
  real bug.** §5.4 says it is called "from `render_offscreen` … and from
  `render`"; only the first was true. `render_offscreen` returns early on
  `!needs_composite` (`compositor.rs:3879-3881`), and a viewport resize does not
  dirty the composite — correctly, the canvas did not change — so a screen-space
  instance was never rebuilt against the run's replaced textures and the run
  silently stopped rendering. `present_and_screen_run` now syncs first.
  `screen_space_effect_survives_viewport_resize` is the regression test; it
  failed against the first implementation.
- **The apply uniform moved into `sync_effect_instances`.** It was written by a
  loop in `sync_projection_states` *after* the sync — fine while the sync had one
  caller, wrong the moment it had two: an instance rebuilt from the present path
  got a fresh, never-written buffer and composited at opacity zero. The uniform
  belongs to the instance, so it is now written beside it, and
  `sync_projection_states` keeps only the masked-passthrough half.
- **`reinsert_entity` pins the recorded side rather than letting position
  decide.** §1.9 is right that an index cannot distinguish the two cases, but the
  first implementation only used `TreeSlot::screen_space` to *grow* the run —
  which left the other direction broken: reinserting the topmost canvas-space
  node at the boundary index made `link`'s "inside the run" rule adopt it.
  `undo_restores_run_membership_exactly` caught this; the fix is to discard the
  insertion rule's guess and apply the recorded side both ways.
- **`layer_tree` returns an envelope.** §5.8 wanted `screenSpaceCount` "on the
  layer-tree response", which was a bare `Vec<LayerInfo>`. It is now
  `LayerTree { layers, screen_space_count }` — the panel cannot draw either fact
  without the other. `screenSpaceEligible` went onto `LayerInfo` per variant as
  planned.
- **`engine/veils.rs` is deleted rather than trimmed.** With the veil half gone
  it held `layer_tree` and `catalogs`, neither of which is about veils.
  `layer_tree` moved to `engine/layers.rs`, `catalogs` to a new
  `engine/catalogs.rs`.
- **The two add-sources were DRYed.** `addSources/veils.ts` and `filters.ts`
  became identical once the Veils tab started spawning a layer, so both now
  declare through one `effectLayerSource(action, category)`.
- **`add_filter_layer` was NOT renamed to `add_effect_layer`.** §6 Step 9 lists
  it, but it is the same `filter` → `effect` vocabulary change PR 5 makes
  wholesale, and renaming the wire method here would churn the frontend twice.
  Deferred to PR 5 with the rest.
- **Test 12 landed as a visibility assertion, not a pan assertion.**
  `screen_space_effect_is_visible_only_after_the_present_pass` proves the two
  spaces differ by the property that matters — the same effect changes the
  surface and not the composite — through a new
  `Compositor::test_present_through_screen_run` harness, which also normalizes a
  BGRA surface to RGBA so no test has to know which surface it got. The
  view-pan variant is not written.

Two costs the LOC table did not carry: the `TreeSlot` migration touched ~25 call
sites rather than the five undo structs (every one of them got shorter), and the
`layer_tree` envelope rippled through eight integration tests plus two frontend
fakes.

### PR 5 — Vocabulary

**Step 11.** `document/layer_kinds/filter.rs` → `effect.rs`; `Layer::Filter(FilterLayer)`
→ `Layer::Effect(EffectLayer)`; manifest type id `"filter"` → `"effect"`;
`filter_pipeline_registry` → `effect_registry`; hotkey action ids
`filterInvert`/`filterBlack_and_white` → `effect*` in
`crates/darkly/presets/{krita,photoshop,gimp}.yaml`; wire methods
`addFilter`/`setFilterParams`/`applyFilter`/`previewFilter`/`commitFilterPreview`/
`cancelFilterPreview` → effect equivalents. Pure churn; separable; regenerates
`protocol_gen.ts`.

**Explicitly out of scope for this step**, per §1.8: `newVeil` and
`newFilterLayer` are **not** renamed. They are category deep links (§5.9) naming
categories that still exist, not survivors of the retired subsystem. Nor is any
user-facing string touched — the retirement is of code identifiers only.

---

## 7. Tests

Feature tests (CLAUDE.md Testing Principle), plus **one genuine regression test**
for the pre-existing `requires` bug of §2.5, written first and confirmed failing.

Three of the tests below (2, 4, and the isolated half of 3) largely **exist
already** and are *retargeted* rather than written: `tests/filters.rs:457`
`filter_layer_does_not_affect_layers_above_it`, `:483`
`filter_layer_in_isolated_group_is_scoped`, `:536`
`masked_filter_layer_confines_inversion` and `:595`
`masked_filter_layer_in_isolated_group_lerps_against_group_accum`. Only the
*passthrough* half of Test 3 is genuinely new. They are listed anyway because
the assertions must keep holding across §1.6's mask hoist and §5.3's rework —
that is their whole value here.

New `crates/darkly/tests/effect_layers.rs`:

1. **`effect_layer_transforms_composite_by_property`** — a solid raster with a
   `black_and_white` effect layer above it: every texel satisfies `r == g == b`.
   A second case with `pixelate` asserts block uniformity. *Verifies that an
   effect that exists only as a veil today renders correctly from the layer
   tree.* Asserts a **property**, not "the composites differ" — the latter passes
   even if the effect writes garbage.
2. **`effect_layer_only_affects_layers_beneath`** — red raster, `invert` effect,
   opaque blue raster covering the left half on top. Left half still blue; right
   half inverted red. *Verifies stacking position.*
3. **`effect_layer_scope_follows_passthrough`** — a raster below a group
   containing an `invert` effect: with the group **isolated**, the outside raster
   is untouched; with the group **passthrough** (the default), it *is* inverted.
   *Pins both halves of §1.2. Covering only the isolated case would pin the rarer
   configuration and leave the default untested.*
4. **`masked_effect_layer_confines_to_mask`** — a half-covering mask on a
   `black_and_white` effect layer; masked half grey, rest untouched. *Verifies
   the hoisted apply pass (§1.6) still confines correctly.*
5. **`watercolor_effect_layer_is_maskable_and_appliable`** — `watercolor` as a
   masked effect layer, and destructively applied to a raster. *Verifies §1.6's
   headline consequence: an accumulator-only effect gained both capabilities.
   Fails today — `watercolor` has no `FilterEffect` at all.*
6. **`spatial_effect_preserves_transparency`** — an opaque blob on a transparent
   canvas under a `frozen` effect layer: texels well outside the blob keep
   `a == 0`, **and** an alpha *edge* texel is sampled and its value recorded, so
   §2.6's straight-alpha fringing is characterized rather than discovered later.
   *Verifies Step 1.*
7. **`effect_layer_opacity_and_blend_are_honoured`** — a `black_and_white` effect
   layer at 50% opacity yields a half-desaturated composite; the same layer in
   `multiply` differs from `normal`. *Verifies §1.7. Fails today — `blend` is
   never read.*
8. **`effect_layer_alpha_is_not_inflated`** — an effect layer at opacity 1.0 over
   a 50%-alpha raster leaves alpha at 50%. *Pins §5.3's choice of replace-lerp
   over source-over; a Porter-Duff implementation would report 75%.*
9. **`perf_scale_factor_output_is_canvas_sized`** — a `painting` effect layer
   (`perf_scale_factor == 0.7`) still produces a full-canvas composite.
   *Verifies `ScaledEffect` on the tree arm.*

New `crates/darkly/tests/effect_space.rs`:

10. **`boundary_partitions_the_root_into_two_spaces`** — raster, effect A,
    effect B with `screen_space_count == 2`: A and B are in the run, the raster
    is not. Set the count to 1: only B is. Set it to 0: none is. *Verifies the
    stored boundary is what decides space.*
11. **`screen_space_effect_is_absent_from_the_composite_and_the_export`** — the
    canvas-space composite readback is identical with and without a screen-space
    `invert` above everything; a canvas-space `invert` changes it. *Verifies
    §1.4 — the one property that decides what a user gets in a PNG.*
12. **`screen_space_effect_moves_with_the_view_and_a_canvas_one_does_not`** —
    the screen-level assertion, through a new
    `test_present_with_screen_run(device, queue, doc, w, h)` harness (the
    existing `present_into_target`, `compositor.rs:3445-3499`, deliberately does
    not run the chain). Pan the view: a screen-space `grain` lands on the same
    screen texels; the same effect below the boundary pans with the artwork.
    *Verifies the two spaces are genuinely different, which is this plan's whole
    premise. A composite-level "view independence" assertion would be vacuous —
    the composite precedes `fs_present` for every layer kind.* The harness is
    ~20-30 lines: `VeilChain::encode` is `&self` (`gpu/veil_chain.rs:322`) so
    there is no borrow conflict with `&self.present_to_veil_pipeline`, and the
    only real work is sizing the run's textures, because `encode` unwraps
    `self.views` (`:330`). Two present-level assertions already exist to model
    it on: `tests/engine.rs:4038` and `:4086`, via `engine/mod.rs:1107`
    `test_readback_present`.
13. **`a_raster_can_never_be_placed_above_the_boundary`** — **the invariant, and
    the single most important test in this plan.** With `screen_space_count == 3`
    over a run of effects, exercise *every* door: `add_raster_layer(None)` (which
    resolves to root top today, `document/mod.rs:1023-1025`),
    `add_raster_layer(Some(top_run_member))`, `paste_image`,
    `paste_image_floating`, `duplicate_node` of a raster, `group_layers`,
    `move_layers` with the target above the run, and `flatten_image`'s result.
    In every case the raster lands at or below the boundary and
    `screen_space_run()` still returns the same three effects. *Verifies §1.9's
    claim that one chokepoint covers every path — a test that would have to be
    edited to add a new insertion path is exactly the coverage this needs.*
14. **`masked_or_isolated_nodes_cannot_be_above_the_boundary`** — setting the
    boundary above an isolated group, a raster, or a masked effect is refused.
    A passthrough group of effects is accepted. Attaching a mask to a node
    already in the run drops it (and everything below it in the run) to canvas
    space via the read clamp; removing the mask restores it. *Verifies §1.3's
    structural clauses and the read clamp of §1.9.*
15. **`visibility_never_changes_which_space_a_node_is_in`** — hide the raster
    below the run, hide a run member, hide a mask on a canvas-space effect: the
    boundary and `screen_space_run()` are unchanged in all three. *Verifies
    §1.3's two deliberate refinements; all three would fail on a
    visibility-aware predicate.*
15a. **`boundary_survives_save_load_round_trip`** — save a document with
    `screen_space_count == 2`, reload, assert the count and the run membership
    are identical. A second case: a manifest whose `screen_space_count` exceeds
    the qualifying suffix loads clamped rather than rendering a raster in screen
    space. *Verifies the manifest field and `engine/load.rs`'s hand-written
    clamp — the one path that does not go through `link` (§1.9).*
15b. **`boundary_move_is_one_undo_step`** — drag the divider across two effects,
    undo once: the boundary is back where it started, and the tree is
    untouched. Redo restores it. *Verifies §5.8's single
    `ScreenSpaceBoundaryAction` — a `CompoundAction` over per-layer flags would
    need two undos, which is exactly what deleting `pinned` bought.*
15c. **`undo_restores_run_membership_exactly`** — with `[raster, E_x, E_a, E_b]`
    and a two-member run `{E_a, E_b}`, delete `E_a` and undo: `E_a` is a run
    member again and `E_x` is not. Then delete `E_x` and undo: `E_x` is still
    *not* a run member. *This is the pair of cases §1.9 shows a bare
    index-plus-count cannot both satisfy; it is the test that justifies
    `TreeSlot`, and the one that will fail if Q3 is descoped to the fallback.*

Extend existing files:

16. **`crates/darkly/tests/canvas_resize.rs`** — add an effect layer, resize the
    canvas, composite, assert correct output; then move the effect across the
    divider and repeat. *Covers the highest-severity failure mode (stale
    accumulator bind groups) and the `target_generation` counter of §5.4.* A
    third case resizes the **viewport** with a non-empty run, because
    `ScreenRun`'s textures *and its apply scratch* are recreated by
    `ensure_textures`, which `set_canvas_rect` (`compositor.rs:1860-1865`)
    does not touch — the one invalidation trigger the canvas-rect path misses.
17. **`crates/darkly/tests/layer_bake.rs`** — three cases, since flatten and
    merge fail differently (Step 8):
    - Flatten Image over a document with a screen-space run leaves the run in
      the tree and its effects out of the baked pixels. *Without the
      `engine/flatten.rs:18` fix the run members are detached at `:67-83`
      without ever reaching the bake, so this fails by the effects
      **disappearing**, not by their pixels being wrong.*
    - Merge Down with a run member as `source_id` is refused with an error
      rather than consuming it. Extend beside the existing merge coverage at
      `:133` `merge_down_baked_result_combines_two_layers`, `:164`
      `merge_down_fails_on_bottom_layer` (the existing refusal test to model the
      assertion on) and `:173` `merge_down_undo_restores_both_sources`.
    - `merge_layers` with a run member in `ids` is likewise refused (beside
      `:559` `merge_layers_rejects_locked`, the same shape).
18. **`crates/darkly/tests/filters.rs`** — retargeted to the unified registry;
    every existing assertion (RGBA8 invert, **R8 mask invert**, invert-twice
    round-trip, undo/redo, selection clipping) must keep passing. *The sharpest
    guard that hoisting the mask out of the effect (§1.6) and moving R8 onto a
    declared target list (§2.3) changed nothing.*
19. **`crates/darkly/tests/chromatic_aberration.rs`** — must keep passing against
    the single merged CA implementation. But its premise needs correcting: the
    file is 11 tests driving the *filter* through `apply_filter_typed` and
    exactly **one** veil smoke test, `veil_produces_non_identity_output:176`.
    The collapse's risk is on the *veil* side (`views[0]/[1]`, `create_cache`,
    `encode`), and one non-identity assertion is thin cover for it. **Add a
    veil-path property test** rather than relying on the existing 11.
20. **`crates/darkly/tests/docs_render.rs`** (*not* `schema_contracts.rs`) — the
    effect catalog has exactly one entry per `type_id`, exactly **15** entries,
    exactly one `black_and_white` and one `chromatic_aberration`. *Directly
    asserts the user's reported problem is gone.* `schema_contracts.rs` is 119
    lines about config prefs (`no_duplicate_pref_keys:27`,
    `every_pref_has_a_resolvable_value:46`, `overlay_names_unique:70`,
    `app_base_settings_options_match_overlays:79`) and imports only
    `darkly::config::*`; per-catalog counts already live in
    `docs_render.rs:330-340`, which is where this belongs. Enumerate through
    `EffectRegistry::types()` or `crate::catalog::catalogs()` (which **does**
    exist — `build.rs:258` generates it, `catalog.rs:181` includes it,
    `engine/veils.rs:130` calls it; grepping the checked-in tree for
    `fn catalogs` finds nothing, which is how an earlier false negative
    happened. `catalog.rs:194`'s `settings_catalogs()` is a different function).
21. **`crates/darkly/tests/docs_render.rs`** — `all_forty_seven_assets_land`
    (`:320-341`) becomes 45 over `("effects", 15)`.
    `shared_effects_share_one_preview` (`:184-206`) is **not** deleted wholesale:
    its first half (filter-vs-veil preview equality) is obsolete after the
    merge, but `:195-205` is a filter↔shared-module `preview_params` drift guard
    that stays meaningful. **Keep the second half.**
    `crates/darkly/tests/docs_export.rs:363` is a single total rather than
    per-catalog counts, and its failure message at `:364-365` spells out
    "7 filters + 10 veils + …" — it must be edited too or it will lie.
22. **`crates/darkly/src/engine/save.rs`** — **REGRESSION for §2.5, written
    first and confirmed failing.** Not `tests/engine.rs`: there is no `requires`
    coverage anywhere under `crates/darkly/tests/`, and the only existing
    coverage is the in-file unit test `requires_inventory_collects_used_modules`
    (`engine/save.rs:549-592`, asserting `requires.veil` lists `grain` at
    `:572`, `layer_kind` lists `raster`/`group`, and `blend_mode` lists
    `normal`). Extend it: add an effect layer whose `pipeline` names an effect,
    assert `requires.effect` lists it. It fails today because the node arm
    (`:438-441`) records only `node.type_id()` — the generic layer-kind id
    (`layer.rs:681`) — and never the per-layer `pipeline` string
    (`layer.rs:243`). A second case: a document naming an unknown pipeline id
    fails to load with `CorruptManifest` rather than compositing as a silent
    no-op (`compositor.rs:4371-4377` early-returns today). *While there, fix the
    doc comment drift at `:544-548`, which says "the `noise` veil" over a test
    that adds `grain`.*
23. **`crates/darkly/tests/shader_compile.rs`**, **`wgsl_validate.rs`** — these
    do **not** enumerate catalogs and need only their path constants updated:
    `shader_compile.rs` is filesystem-driven (`std::fs::read_dir` at `:5`) and
    `wgsl_validate.rs` enumerates `builtin_brushes::all()` (`:22`) only.
    Conversely `crates/darkly/tests/picker_preview.rs` **is** already dynamic
    over `darkly::catalog::catalogs()` (`:256`) and needs **no edit at all** —
    it follows the merge automatically and is free coverage that the merge is
    complete.

Categories (§1.10), in `crates/darkly/tests/docs_render.rs` beside the existing
per-catalog assertions at `:330-340`:

28. **`effect_categories_partition_the_catalog`** — every effect declares a
    non-empty `category`; grouping the 15 entries by category and concatenating
    the groups yields all 15 with **no `type_id` repeated** and none dropped.
    *This is the "no duplicate effect ids in the tab list" guarantee at its
    source: the modal's rail is these groups, so a partition here is a partition
    there. It is also what a `categories` slice (§1.10) could not assert, and the
    reason a slice was rejected.* A second assertion pins the split at the number
    the plan promises — six `"Filters"`, nine `"Veils"` — so a new effect must
    make a deliberate choice rather than silently landing wherever.

29. **`category_is_presentation_only`**, in
    `crates/darkly/tests/effect_space.rs` — three assertions, each of which fails
    if any consumer starts branching on `category`:
    - **Space.** A `"Veils"`-category effect (`grain`) placed *below* the
      boundary composites into the canvas and appears in the export; a
      `"Filters"`-category effect (`invert`) placed *above* it does not. Both are
      the opposite of what a category-aware implementation would do.
    - **Placement.** Adding a `"Veils"` effect and a `"Filters"` effect with the
      same anchor lands them at the same index; §1.9's clamp never consults
      category.
    - **Rendering.** The composite produced by an effect layer is unchanged when
      its registration's category is swapped — asserted by driving two effects
      whose categories differ but whose declared params and pipeline behaviour
      are pinned, so the readback depends on `pipeline` alone.

    *Pins §1.10's central claim. Without it, `category` is one PR away from
    becoming load-bearing, and CLAUDE.md's Type-owned dispatch rule names exactly
    that failure — a consumer branching on what a variant "is".*

Frontend (`vitest`, node environment — no DOM globals; see
`src/lib/__tests__/clickOutside.test.ts` for the `vi.stubGlobal('window', …)`
pattern):

24. `frontend/src/ui/layers/__tests__/addLayerModal.test.ts` — over the modal's
    pure rail-building function, fed fake catalogs (there is no DOM, so the
    component itself is not mounted; the arithmetic lives outside it for exactly
    this reason):
    - the rail is built **from the catalogs**: a fake `layerKinds` entry with a
      `hotkeyAction` produces a tab; one without produces none; adding a third
      `category` value to the fake `effects` catalog produces a third tab with
      no change to the modal;
    - **no effect `type` appears under two tabs**, and every effect appears under
      exactly one — the frontend half of Test 28;
    - a kind whose `variantCatalog` names a catalog where no entry declares a
      category yields **one** tab titled by the catalog (the `voids` case);
    - the first tab is the one whose action has the lowest menu order, and it is
      the tab preselected on open.
    *Supersedes the earlier `effect_picker.test.ts`, which asserted only the
    duplicate-free entry list — a strictly weaker property.*
25. `frontend/src/actions/__tests__/menu_actions.test.ts` — updated. The
    Layer-menu order assertion (`:121-137`) gains `addLayer` and keeps `newVeil`;
    the palette-reachability test (`:141-149`) keeps `hit('veil')` →
    `newVeil` (Change 1: the word stays reachable) and gains `newEffectLayer`;
    the "backs every new-layer dropdown entry" test (`:152-156`) is **deleted**
    along with `NEW_LAYER_ACTION_IDS` and the dropdown it guarded.
26. `frontend/src/state/__tests__/freshDocument.test.ts` — the demo recipe seeds
    effect layers, not veils (`:31-38`); the app recipe seeds none (`:65-70`).
27. A new divider unit test over the pure index math: given `screenSpaceCount`
    and a tree's `screenSpaceEligible` flags, the divider's resting row and drag
    clamp are correct, and a drag release maps to the right `count`. *Keeps the
    arithmetic out of the untestable Svelte component — note there is currently
    **no** test mounting `LayerPanel`, `LayerItem` or `LayerGroup`.*

---

## 8. LOC estimate

Lines **added / removed**, excluding pure relocation, which is called out
separately because it would otherwise swamp the signal.

| Area | + | − |
|---|---:|---:|
| **PR 1** — six shader alpha fixes | 20 | 10 |
| **PR 2** — `gpu/effect.rs`: trait, registration, registry, one preview mechanism | 430 | 60 |
| **PR 2** — `gpu/param_effect.rs` replacing `gpu/param_filter.rs` | 250 | 372 |
| **PR 2** — delete `gpu/veil.rs` (373) + `gpu/filter.rs` (356) | — | 729 |
| **PR 2** — `in_place_apply` (opacity + blend switch + R8 variant) and `build_blend_source` | 100 | 30 |
| **PR 2** — 7 former filter modules + `lut_filter.rs` to instance form | 120 | 180 |
| **PR 2** — collapse the two duplicate effect pairs (`.rs`) | 170 | 430 |
| **PR 2** — collapse the two shader pairs, delete 6 `*_masked` entry points | 65 | 190 |
| **PR 2** — 10 icon + hotkey-action declarations and their 3 preset bindings | 60 | — |
| **PR 2** — `gpu/effect_scaling.rs` extraction | 180 | 130 |
| **PR 2** — compositor: `effect_instances`, `compose_effect_arm`, kind-branch removal, two apply scratches, `filter_node_region`'s intermediate | 300 | 200 |
| **PR 2** — engine/frontend follow-through for the renamed registry | 60 | 60 |
| **PR 2** — 15 `category` declarations, catalog sort, two picker filters (§1.10, Step 4b) | 50 | — |
| **PR 3** — `LayerKindRegistration.new_action` / `variant_catalog` (5 files) + `CatalogEntry.variant_catalog` | 25 | — |
| **PR 3** — `AddLayerModal.svelte` + `addSources/{effects,voids}.ts` | 340 | — |
| **PR 3** — delete `NewLayerMenu` (93), `LayerPickers` (27), `layerPicker.svelte.ts` (12), `VeilPickerModal` (82), `FilterPickerModal` (84), `VoidPickerModal` (121) | — | 419 |
| **PR 3** — `LayerFooter` split button + styles, `NEW_LAYER_ACTION_IDS`, action retargets, `App.svelte` mount | 25 | 45 |
| **PR 4** — `supports_screen_space` + document queries + read clamp | 60 | — |
| **PR 4** — `screen_space_count` + `link`/`unlink` invariant + `ScreenSpaceBoundaryAction` | 75 | — |
| **PR 4** — `TreeSlot` folded into the five undo actions (**Q3**) | 45 | 30 |
| **PR 4** — `gpu/screen_run.rs` replacing `gpu/veil_chain.rs` | 330 | 667 |
| **PR 4** — tree-walk skip, flatten/merge boundary handling | 70 | 20 |
| **PR 4** — animation: space-driven ticks, divisor renames, `mark_effect_dirty` | 50 | 25 |
| **PR 4** — invalidation: `target_generation` + hooks | 35 | — |
| **PR 4** — engine: `engine/veils.rs` collapse, add + boundary handlers | 110 | 240 |
| **PR 4** — save/load: `manifest.veils` removal, `screen_space_count` + load clamp, `requires.effect` incl. pipeline ids, `pipeline` validated | 55 | 100 |
| **PR 4** — `LayerInfo.screenSpaceEligible` + `screenSpaceCount` | 20 | — |
| **PR 4** — frontend: delete the rest of `ui/veils/`, `SpaceDivider` + its labelling copy, the row badge, app-state purge, actions | 290 | 598 |
| **Production subtotal** | **~3,335** | **~4,535** |

**Production net ≈ −1,200** (about 3,300 added, 4,500 removed). Treat
**−750 to −1,500** as the honest range.

**What moved since the draft**, so the number can be audited rather than trusted:

- **The picker-modal merge row is gone**, superseded. An earlier draft carried it
  at +40 / −160 with an explicit "no credit" note, because merging
  `VeilPickerModal` and `FilterPickerModal` was available standalone. PR 3 now
  deletes both outright along with four more files, so the merge would have been
  built and thrown away one PR later. Step 4b is a two-line category filter
  instead. Of PR 3's 419 deleted lines, roughly 120 are still not a saving this
  plan earned (§4); the other ~300 are, because they depend on the rail being
  derivable from the layer-kind and effect catalogs.
- **Added by Change 1 (categories):** 15 declarations, the catalog sort, the two
  interim picker filters, and the divider labelling copy + row badge — ~80 lines,
  no removals. Cheap because `CatalogEntry.category` and its wire projection
  already exist (`catalog.rs:32-33`, `:86-89`).
- **Added by Change 2 (the modal):** ~390 lines of new frontend and Rust
  declarations against 464 removed, net ≈ −75 — but the real return is four
  add-layer surfaces becoming one, which the line count understates.
- **Added:** the two apply scratches and `filter_node_region`'s intermediate
  (§5.3); the flatten/merge boundary handling in `engine/` (§6 Step 8); ten icon
  and hotkey-action declarations plus their preset bindings (§5.2); the
  `screen_space_count` field, its `link`/`unlink` invariant, its manifest slot
  and its load clamp; `TreeSlot`.
- **Removed by dropping §5.6:** `fs_present_raw`, the blit's view-uniform
  binding, and part of Step 8 — roughly 35 production lines that are no longer
  written.
- **Removed by the stored boundary**, relative to the derived-run draft: the
  `pinned` field and its five per-layer-kind serializers, `Property::Pinned`,
  the `CompoundAction` divider handler, `LayerInfo.screenSpace`, and the
  trailing-run walk in `screen_space_run()` (now a slice). Net ≈ −45 production
  lines and one whole open question. The boundary field, its invariant and its
  undo action cost ≈ +75, so the stored model is roughly LOC-neutral in the
  table while removing a risk, a question and a compound-undo path — the saving
  is in complexity, not in lines.

| | + | − |
|---|---:|---:|
| Tests (32 items: ~20 new Rust integration tests, 6 extended, 4 vitest) | ~790 | ~100 |
| Generated + docs (`mod.rs` regen, `protocol_gen.ts`, `presets/*.yaml` action ids + config keys, `CLAUDE.md` repo-layout block, `README.md`, a new `docs/effects.md`, `docs/gpu-passes.md`) | ~290 | ~210 |

**Plus ~7,300 lines of mechanical relocation**, each in its own `git mv` commit:

| Relocation | ~lines |
|---|---:|
| `gpu/veils/` + `gpu/filters/` (17 files) → `gpu/effects/` | 4,140 |
| `shaders/veils/` + `shaders/filters/` (15 files) → `shaders/effects/` | 1,320 |
| `frontend/src/ui/filters/` → `frontend/src/ui/effects/` (8 files — `FilterPickerModal` is deleted in PR 3, not moved) | 1,556 |
| `document/layer_kinds/filter.rs` → `effect.rs` (PR 5) | 272 |

Done as dedicated `git mv` commits these are rename-detected and reviewable;
done inline they read as +7,400/−7,400 and destroy the diff.

**Honest scope note.** The capability the user asked for — effects at any tree
position, with a divider — is roughly **500 production lines** (Steps 7, 8 and
the divider). The add-layer modal is a further **~390** and is separable: it is
its own PR and could be dropped without touching anything else in the plan.
Categories are **~80** and are not separable — without them the merged catalog is
one undifferentiated list of 15 and the word "Veils" has nowhere to live.
Everything else is de-duplication that the request forces: without one registry,
the layer arm would have to ask "veil registry or filter registry?", and the user
would get a *third* place to find "Chromatic Aberration" rather than one. The net
figure is negative because the second subsystem, the four-surface add-layer
affordance, and the parallel frontend selection model are all deleted.

---

## 9. Risks

1. **Cache invalidation across two spaces.** `effect_instances` entries hold bind
   groups pointing at accumulator, apply-scratch or run textures. A missed
   invalidation site is a stale bind group over freed memory. Mitigated
   structurally by `target_generation` (§5.4) rather than by enumerating trigger
   sites, and covered by Test 16. Residual: the counter must actually be bumped
   everywhere a target is created — one grep-able call site each in
   `create_group_state` and `ScreenRun::ensure_textures`. Note that
   `set_canvas_rect` (`compositor.rs:1860-1865`) recreates every `GroupState` but
   does **not** touch the run's textures, so the two bump sites are genuinely
   independent and Test 16 exercises both.
2. **Canvas-resolution cost.** A full-canvas pass per effect layer per frame, on
   a large canvas, with `painting`-class effects, may be too slow. Mitigated by
   `perf_scale_factor` + `rendering.canvas_effect_scale`, but the ceiling is real
   and there is no incremental-region escape hatch. **Measure `painting` and
   `watercolor` at 4096² before shipping PR 2.**
3. **Parameter-drag churn on aux-heavy effects.** `set_params` defaults to a full
   rebuild. For uniform-only effects the cache is tiny (a 256×2 LUT, a 784-byte
   uniform) and rebuild cost does not scale with the canvas. For `watercolor` and
   `pixelate`, whose aux textures *are* render-sized, a canvas-space slider drag
   would reallocate canvas-sized textures per frame. Override `set_params` on
   those two if measurement demands it; the hook exists precisely so the fix is
   additive.
4. **Straight-alpha edge fringing** (§2.6). Not solved; characterized by Test 6
   so it is known rather than discovered.
5. **Divider discoverability and mis-drag.** A structural element in the layer
   panel that silently changes whether work is exported is powerful and
   dangerous. Mitigated by §5.8's resting label
   (`Viewport only — not exported`), the per-row badge, and one undo step per
   drag from the single `ScreenSpaceBoundaryAction`. Q4 is now closed with that
   copy. **Residual risk: the copy must not say "Veils".** Space and category are
   orthogonal (§1.10), so a divider labelled with a category name would be wrong
   the first time a user drags `curves` above it — and it would rebuild in the
   user's head exactly the pinned-folder model this plan removes.
6. **The invariant has one home, and one path that bypasses it.** Everything
   structural funnels through `Document::link` / `unlink` (§1.9), which is the
   mitigation; but `engine/load.rs` builds the tree without them
   (`:202-206`, `:371-390`), so the boundary clamp there is hand-written and is
   the single line whose omission would let a save put a raster in screen space.
   Covered by Test 15a's second case, and the §1.3 read clamp is the backstop
   that keeps even that failure benign.
7. **PR size.** PR 2 is the largest and carries ~5,500 lines of relocation.
   Non-negotiable that the `git mv`s are separate commits. Splitting the
   add-layer modal into its own PR 3 (§6) is part of the same mitigation.
8. **`docs_export` / `docs_render` count assertions** hard-code 7 filters and 10
   veils (`docs_render.rs:334-341`, `docs_export.rs:363-366`). They will fail
   loudly, which is the point — they are coverage that the merge is complete.
9. **The modal is one surface for five add paths.** A regression in it breaks
   every way of adding a layer at once, where today a broken void picker leaves
   raster, group and filter adds intact. Mitigated by the per-catalog spawn
   modules (§5.9) — the shared code is the rail and the keyboard, not the
   spawning — and by Test 24 covering the rail arithmetic outside the component.
   The specific thing most likely to break is **`getDisplayMedia`'s transient
   user activation**: `VoidPickerModal.svelte:19-36` acquires the `MediaStream`
   before its first `await` for exactly this reason, and a spawn path
   restructured to `await` a wire call first would break camera and screenshare
   with no compile error and no test failure. Verify by hand on both.
10. **The category is one field away from becoming load-bearing.** It is
    presentational by intent (§1.10) and by test (Test 29), but it is a string on
    a registration that every consumer can see, and the cheapest way to implement
    almost any future per-effect behaviour is to branch on it — the exact shape
    CLAUDE.md's Type-owned dispatch rule forbids. Test 29 is the guard; it must
    not be weakened into "the categories are what the table says".

---

## 10. Unresolved questions

**Q1 — mask presence or mask visibility in the predicate? CLOSED: presence.**
§1.3 states the reasoning; the independent review agreed. Hiding a mask must not
move its host between coordinate spaces, because that changes what gets
exported. Presence is the structural, document-authoritative fact; visibility is
per-frame. No longer open.

**Q2 — should the screen-space run see the transparency checkerboard? CLOSED:
yes, unchanged from today.** The alternative (present straight alpha into the
run) is a visible regression at the shipped default resolution scale and breaks
`lens_blur`'s colour, not merely its alpha — §2.7 has the three confirmed
mechanisms. §5.6 is dropped. The cost is that an effect looks different on
either side of the divider; Q4's labelling carries that. No longer open.

**Q3 — does undo need to restore run membership exactly?** §1.9 shows that an
absolute index plus a count cannot distinguish "lowest member of the run" from
"topmost canvas-space child", so deleting an effect layer adjacent to the
divider and undoing can bring it back on the wrong side — which changes what is
exported. The recommended fix is `TreeSlot { parent, position, screen_space }`,
which also collapses an existing five-fold duplication of `(parent, position)`
across the structural undo actions (~45 added, ~30 removed). The alternative is
to accept the one lossy case. **Recommend `TreeSlot`; surface the cost.**

**Q4 — how is the divider labelled? CLOSED: "Viewport only — not exported".**
§5.8 has the full copy — rest label, tooltip, empty-run hint, and a per-row
badge. The governing constraint, and the reason the obvious answer is wrong:
**the divider marks *space*, the category (§1.10) marks *what kind of effect it
is*, and the two are orthogonal.** A `Filters` effect may sit above the line and
a `Veils` effect below it. So the divider is **not** labelled "Veils" — that
would be false the first time someone drags `curves` across it, and it would
rebuild the pinned-folder mental model the plan removes. The copy also avoids
"screen space" and "canvas space" entirely: those are this document's words, and
the user's actual question is whether the effect ends up in their PNG. No longer
open.

**Q4b — should adding an effect layer land it above or below the divider?**
§1.9's `link` rule places new nodes below unless the caller explicitly targets a
position inside the run, which means "New Effect Layer" with the divider
non-empty creates a *canvas-space* effect and the user drags the divider to
promote it. That is the conservative, invariant-preserving default. The
alternative — have the effect-layer add path target the top of the run when the
new layer qualifies — is one line and arguably the nicer gesture for someone who
thinks of the run as "where veils go". **Surface to the user; recommend the
conservative default for v1.**

**Q5 — one resolution-scale key or two?** §5.5 proposes
`rendering.screen_effect_scale` (default `0.7071`, today's value) and
`rendering.canvas_effect_scale` (default `1.0`). One key with a single default
cannot serve both, since one output is transient and the other is document
content.

**Q6 — should `set_params` be implemented for `watercolor` and `pixelate` in this
work, or deferred behind measurement?** Risk 3. Deferring is recommended; the
hook is additive.

**Q7 — what happens when the root contains *only* effects?** The canvas
composite is empty, the present is the checkered empty canvas, and the run
operates on that. Degenerate but coherent; recommend leaving it rather than
inventing a floor. Note this is now reachable only by the user dragging the
divider to the bottom, since insertion never grows the run (Q4b).

**Q8 — is `chromatic_aberration` a Veil or a Filter?** The one contested entry in
§1.10's table, and the one the correction that produced this revision named. It
is a Filter today only because someone wanted it destructively applicable, and
§1.6 makes every effect destructively applicable, so that signal is now worth
nothing. What it does is displace colour channels *spatially* — an optical
artifact in the same family as `lens_blur`, which Krita files under Blur
(`krita/plugins/filters/blur/kis_lens_blur_filter.cpp:33`) rather than Adjust.
**Recommend Veils.** It is one string in one file either way, and §1.10's
single-category rule means it cannot be both. Pure taxonomy; no technical
dependency; the only cost of getting it wrong is that a user looks in the other
tab first, which the cross-tab search (§5.9) already covers.

**Q9 — should the `newLayer` hotkey open the modal, or keep spawning directly?**
The `+` button opens the modal, per the user's request. The *action* is a
different affordance: it is bound to a key, and routing a bound key through a
modal that then needs Enter turns one keystroke into two. §5.9 keeps `newLayer`
and `newGroup` spawning directly and gives the `+` its own `addLayer` action.
**Recommend direct**, but it is a one-line choice and the user may prefer one
behaviour for both.

---

## 11. Prior art

All line numbers from the checkouts under the project root. Krita:
`/mega/ARTEXP/darkly/krita`. GIMP: `/mega/ARTEXP/darkly/gimp`. Every claim below
was read at source for this plan; anything that could not be confirmed is marked.

**Calibration.** Prior art is genuinely useful here for the *document-side*
questions — what an adjustment layer is, how it composites, how masking relates
to filtering, what persists — and for **how effects are categorized for the
user**, where Krita's answer is directly transferable and GIMP's is a documented
anti-pattern. It is genuinely **absent** for the screen-space / canvas-space
duality and for the divider, because neither editor has viewport-space artistic
effects at all. Claims of the second kind are recorded as confirmed negatives,
not stretched into support.

### Adjustment layers keep user-settable opacity and blend mode

- `krita/libs/image/kis_adjustment_layer.cc:32-41` — the constructor comment
  reads "by default Adjustment Layers have a copy composition, which is more
  natural for users", citing bugs 324505 and 294122, then
  `setCompositeOpId(COMPOSITE_COPY); setUseSelectionInProjection(false);`. It is
  a **default**, not an invariant.
- No enforcement found: `krita/libs/ui/kis_multinode_property.h:52-53`'s generic
  setter calls `node->setCompositeOpId(value)` with no type check;
  `krita/libs/ui/kis_node_model.cpp:588-590` uses `COMPOSITE_COPY` only as the
  docker's *display* default for adjustment layers.
- `krita/libs/image/kis_layer_projection_plane.cpp:71-73` —
  `painter->setCompositeOpId(m_d->layer->compositeOpId()); painter->setOpacityU8(m_d->layer->projectionLeaf()->opacity());`
  with no branch on node type; adjustment layers do not override the projection
  plane.
- `krita/libs/ui/dialogs/kis_dlg_layer_properties.cc:98`, `:105` — opacity and
  composite-op properties wired for every node type; a case-insensitive grep for
  "adjustment" over the whole file returns **zero** hits.
- `.kra` round-trips both: `krita/plugins/impex/libkra/kis_kra_savexml_visitor.cpp:382-395`
  writes `OPACITY` and `COMPOSITE_OP` for every node, including adjustment layers
  (`:195-207`, which stores only the filter *name* and *version* inline —
  parameters and selection go to side files).
- GIMP: `gimp/app/core/gimpdrawablefilter.c:102-106` — `opacity`,
  `paint_mode`, `blend_space`, `composite_space`, `composite_mode` on the
  instance; defaults `GIMP_OPACITY_OPAQUE` / `GIMP_LAYER_MODE_REPLACE` at
  `:253-257`; public setters `gimp_drawable_filter_set_opacity` (`:622-636`) and
  `gimp_drawable_filter_set_mode` (`:646-670`), reaching the applicator at
  `:1649-1650`.

*Supports §1.7 (implement `blend`) and §5.3 (replace-lerp, not source-over — a
"copy composition" default is exactly a replace).*

**Not confirmed:** the comment at `kis_adjustment_layer.cc:38` cross-references
`KisLayerUtils::mergeMultipleLayersImpl()`, but `COMPOSITE_COPY` does not appear
in `libs/image/kis_layer_utils.cpp` in this checkout. The cross-reference is
stale; nothing in this plan depends on it.

### The mask is applied outside the filter, after it runs

`krita/libs/image/kis_async_merger.cpp`, `KisUpdateOriginalVisitor::visit(KisAdjustmentLayer*)`
from `:62`:

1. `:75-76` clear the destination.
2. `:95-96` fetch the selection and use it **only to shrink the work rect** —
   `const QRect filterRect = selection ? applyRect & selection->selectedRect() : applyRect;`
3. `:101-105` with a selection present, the filter writes to a **scratch device**:
   `KisPaintDeviceSP dstDevice = originalDevice; if (selection) { dstDevice = new KisPaintDevice(...); }`
4. `:112` run the filter, **selection-unaware** —
   `filter->process(m_projection, dstDevice, 0, filterRect, filterConfig.data(), 0);`
   (the third argument is the selection slot, passed `0`).
5. `:115-118` **then** composite: copy the unfiltered source, then blend the
   filtered scratch through the selection —
   `KisPainter::copyAreaOptimized(filterRect.topLeft(), dstDevice, originalDevice, filterRect, selection);`

`kis_adjustment_layer.cc:41`'s `setUseSelectionInProjection(false)` disables the
other selection path (`kis_selection_based_layer.cpp:178-179`) so the mask is
applied exactly once, in the merger, after the filter.

*This is literally `mix(before, after, mask)` outside the filter — direct support
for §1.6's hoist, and the strongest prior-art claim in this plan.*

### Replace-then-lerp is what both editors actually compute

The merger's final step (above) calls `KisPainter::copyAreaOptimized`'s 5-arg
selection overload (`krita/libs/image/kis_painter.cc:169`), whose body sets
`gc.setCompositeOpId(COMPOSITE_COPY)` (`:196`) before the blit. That composite
op's arithmetic is §5.3's formula, term for term
(`krita/libs/pigment/compositeops/KoCompositeOpCopy2.h`):

- `:40` — `opacity = mul(maskAlpha, opacity);` — the mask folds into opacity,
  exactly `f = opacity * mask_alpha`.
- `:63` — `newAlpha = lerp(dstAlpha, srcAlpha, opacity);` — alpha is a lerp
  between before and after, **not** a source-over accumulation.
- `:81` — `blendedValue = lerp(dstMult, srcMult, opacity);` on premultiplied
  channels, unpremultiplied by `newAlpha` at `:83`.

GIMP computes the same thing independently.
`GIMP_LAYER_MODE_REPLACE`'s kernel
(`gimp/app/operations/layer-modes/gimpoperationreplace.c`) folds the mask at
`:254-255` (`if (has_mask) opacity_value *= *mask;`), then
`:257` `new_alpha = (layer[alpha] - in[alpha]) * opacity_value + in[alpha];` and
`:265` `out[b] = (layer[b] - in[b]) * ratio + in[b];` — both algebraically
`lerp(in, layer, f)`. There is even a pure-replace short-circuit at `:146-150`
that passes `aux` straight through when opacity is 1 and there is no mask, which
is §5.3's "reduces to `after`".

*Two editors, independently, default an adjustment/filter layer to
replace-then-lerp with the mask folded into opacity. This is the strongest
prior-art support in the plan and it is now cited at the arithmetic rather than
at a function name.*

### The one Krita near-miss, named and disposed of

A Krita-literate reader will raise `KisReferenceImagesLayer`
(`krita/libs/ui/flake/KisReferenceImagesLayer.h:17`) as an apparent
viewport-space layer. It is not one. It is drawn by
`KisReferenceImagesDecoration::drawDecoration`
(`krita/libs/ui/KisReferenceImagesDecoration.cpp:127`) through
`converter->imageToWidgetTransform()` (`:137`) — i.e. it is *image*-anchored and
merely rendered as a canvas decoration, so it pans and zooms with the artwork.
It falls on the canvas-space side of Darkly's line, and it is named here because
silence invites the objection.

### Adjustment layers are the one node type that reads what is below them

`krita/libs/image/kis_projection_leaf.cpp:276-279`:

```cpp
bool KisProjectionLeaf::dependsOnLowerNodes() const
{
    return (bool)qobject_cast<const KisAdjustmentLayer*>(m_d->node.data());
}
```

Darkly's `LayerNode::composites_in_place()` (`layer.rs:715-721`) is the same
predicate, already expressed as a method rather than a consumer-side kind
enumeration. Convergent; nothing to change. `supports_screen_space()` is
deliberately built to the same shape.

### `.kra` persistence of adjustment layers

Node type string `ADJUSTMENT_LAYER = "adjustmentlayer"`
(`krita/plugins/impex/libkra/kis_kra_tags.h:37`); XML carries name + filter name
+ filter version (`kis_kra_savexml_visitor.cpp:195-207`, refusing a filterless
layer at `:197-199`); parameters and the internal selection go to side files;
load dispatches on the string (`kis_kra_loader.cpp:967-968`) and builds a default
config (`:1184-1233`), with deprecated filter-id remapping. Darkly's equivalent
already exists and is simpler (`pipeline` + `params` in the manifest body,
`document/layer_kinds/filter.rs:58-108`) — noted as confirmation that a
string-id-plus-params body is the right shape, not as a change driver.

### Confirmed negative: neither editor has viewport-space *artistic* effects

GIMP's complete display-filter set (`gimp/modules/display-filter-*.c`, 5 files):
`-gamma.c:86` ("Gamma color display filter"), `-color-blind.c:194` ("Color
deficit simulation filter (Brettel-Vienot-Mollon algorithm)"),
`-clip-warning.c:125` ("Clip warning color display filter"), `-aces-rrt.c:84`
("An HDR to SDR proof color display filter…"), `-high-contrast.c:86,128`
(accessibility contrast cycling). All colour-transform, proofing or
accessibility devices.

Krita: `KisDisplayFilter` (`krita/libs/ui/canvas/kis_display_filter.h:31`, doc
comment "the base class for filters that are applied by the canvas to the
projection before displaying") has **exactly one** implementation repo-wide —
`OcioDisplayFilter` (`krita/plugins/dockers/lut/ocio_display_filter_vfx2021.h:56`).
Everything else referencing the type is canvas plumbing. Soft-proofing
(`KisProofingConfiguration`) is likewise colour management.

*So there is no prior art either way on merging a viewport catalog with a layer
catalog, on inferring space from tree position, or on which of the two an effect
should default to. §3 and §4 argue those from Darkly's own model. An earlier
draft's claims that GIMP's `GimpFilterStack` unification and Krita's ASC-CDL
duplication supported the registry merge were refuted at source in that plan's
review and are not reintroduced.*

### One category per filter, declared by the filter itself

Krita's filter base class takes the category as a constructor argument, exactly
one of them: `KisFilter(const KoID& id, const KoID & category, const QString & entry)`
(`krita/libs/image/filter/kis_filter.h:33`). The nine categories are `KoID`
constants in one file — `krita/libs/image/filter/kis_filter_category_ids.cpp:11-19`:
Adjust, Artistic, Blur, Colors, Edge Detection, Emboss, Enhance, Map, Other —
and every filter names one in its own constructor, in its own plugin directory:

- `plugins/filters/raindropsfilter/kis_raindrops_filter.cpp:47` — Artistic
- `plugins/filters/pixelizefilter/kis_pixelize_filter.cpp:45` — Artistic
- `plugins/filters/oilpaintfilter/kis_oilpaint_filter.cpp:44` — Artistic
- `plugins/filters/halftone/KisHalftoneFilter.cpp:43` — Artistic
- `plugins/filters/blur/kis_lens_blur_filter.cpp:33` — Blur (also
  `kis_gaussian_blur_filter.cpp:32`, `kis_blur_filter.cpp:27`,
  `kis_motion_blur_filter.cpp:33`)
- `plugins/filters/levelfilter/KisLevelsFilter.cpp:18` — Adjust
- `plugins/filters/colorsfilters/kis_desaturate_filter.cpp:48` — Adjust
- `plugins/filters/colorsfilters/kis_multichannel_filter_base.cpp:46` — Adjust
  (the per-channel curve filters)
- `plugins/filters/example/example.cpp:42` (invert) — Adjust
- `plugins/filters/gradientmap/KisGradientMapFilter.cpp:32` — Map

A grep over `krita/plugins/` for `FiltersCategory[A-Za-z]*Id` returns 46
occurrences across the nine ids; **not one filter declares two.**

Two things follow, both load-bearing for §1.10:

1. **Single-valued and type-owned is the shape.** A category is a positional
   argument on the registration, in the variant's own file — structurally
   identical to `EffectRegistration.category` and to
   `BlendModeRegistration.category` (`gpu/blend_mode.rs:35`), which Darkly
   already ships.
2. **Krita's own assignments corroborate the split.** Its analogues of our nine
   Veils land in Artistic and Blur; its analogues of our six Filters land in
   Adjust. The line between "reads one texel" and "reads its neighbours or its
   clock" is where an established editor independently drew it.

### Confirmed anti-pattern: GIMP categorizes filters in a hand-written menu

GIMP's filter categories are not declared by the filters. They are a static menu
file: `gimp/menus/image-menu.ui.in.in:695` opens the `_Blur` submenu and
`:697-700` lists `app.filters-focus-blur`, `app.filters-gaussian-blur`,
`app.filters-lens-blur`, `app.filters-mean-curvature-blur` by hand. Twelve such
submenus exist (`_Blur`, `_Distorts`, `_Light and Shadow`, `_Noise`, `_Generic`,
`_Artistic`, `_Decor`, `_Map`, `_Render`, `_Fractals`, `_Pattern`, `_Web`). GEGL
operations do carry a multi-valued `"categories"` key
(`gimp/app/gegl/gimp-gegl-utils.c:99`, `:672`), but GIMP reads it only to
*blacklist* operations (`:101` `gimp_gegl_op_blacklisted`), not to place them —
placement stays in the `.ui` file.

*Cited as the thing not to do.* A filter that forgets to edit that menu file is
invisible, which is precisely CLAUDE.md's "Add entries to a handwritten list".
Darkly's category goes on the effect for the same reason its `register()` does.

### Confirmed negative: no draggable divider prior art

Krita `plugins/dockers/layerdocker/` (`LayerBox`, `LayerDocker`, `NodeDelegate`,
`NodeView`, `NodeViewVisibilityDelegate`, `NodeToolTip`, `WdgLayerBox.ui`) — a
case-insensitive grep for `splitter|divider|QSplitter|setHandleWidth` returns
**zero** hits. GIMP `app/widgets/gimplayertreeview.c` and `gimpitemtreeview.c` —
zero hits for `splitter|GtkPaned|divider`; the only `gtk_paned_*` uses in
`app/widgets/` are dock-column splitting (`gimppanedbox.c:786`), the
display-filter configuration dialog (`gimpcolordisplayeditor.c:104`) and the
device editor (`gimpdeviceeditor.c:144`) — none inside a layer list.

*The divider is a Darkly invention. Its in-repo prior art is
`frontend/src/ui/workspace/pointerDrag.ts` + `Subdivision.svelte:61-67`, which
§5.8 reuses.*
