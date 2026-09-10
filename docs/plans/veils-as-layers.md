# Veils as Layers: one effect catalog, two rendering spaces

Status: **revised** (step 3 of the CLAUDE.md Planning and Independent Review Workflow),
awaiting user approval. No production code has been changed.

---

## Revision Log

How each review finding was resolved. Findings are accepted unless a reason is
recorded; the review itself is preserved verbatim below.

| # | Resolution |
|---|---|
| F1 | **Accepted.** The GIMP citation is re-scoped in §2, §3.7(e) and §4.2: GIMP's filter/layer unification is document-side only and is *not* authority for merging a viewport catalog with a layer catalog. The registry merge now rests on DRY and the Modularity Principle, which is sufficient on its own. |
| F2 | **Accepted.** The "Krita pays with duplicated ASC-CDL math" claim is deleted from §2 and §3.6. Verified independently: the only CDL implementation is `krita/plugins/filters/asccdl/`. |
| F3 | **Accepted.** `FilterLayer::blend` moves from "not done" to in-scope (§5 Step 6a, ~40 lines) and is removed from §8's unresolved questions. Flagged to the user as a product call. |
| F4 | **Accepted.** `authors_alpha()` is deleted entirely. Six shaders are fixed directly (§5 Step 0). This removes the capability, the alpha-restore pass, and the caller-side space branch. |
| F5 | **Accepted.** Recording filter-layer `pipeline` ids in `requires` is added to §5 Step 7 (~15 lines), and the stale doc comment at `document/layer_kinds/filter.rs:11-13` is corrected. |
| F6 | **Accepted.** `EffectRegistration` capabilities become an enum so "neither capability" is unrepresentable (§4.3). |
| F7 | **Accepted.** Split into three sequenced PRs (§7). Step 8 deferred to a fourth. |
| F8 | **Accepted.** Test 6 replaced with a screen-level assertion, Test 1 given a property assertion, Test 9's function name corrected, Test 5 retargeted to an alpha edge; two cache-invalidation tests and a group-scoping test added (§6). |
| F9 | **Accepted.** `animation.void_divisor` renamed (§5 Step 6). |
| F10 | **Accepted.** §3.6 and the LOC table no longer credit the registry merge with the shared-style saving; noted as independently available. |
| F11 | **Accepted.** §3.2's description of `composites_in_place()` corrected; the `compose_layer` kind branch is now called out and removed in §5 Step 6. |
| F12 | **Accepted.** §8 Q2 resolved: **keep** `manifest.veils`. The confusion is fixed at the label, not by discarding user work. |
| F13 | **Accepted.** The menu move ships in PR 3. |

Two items are surfaced to the user as product calls rather than settled here:
**F3** (implementing `FilterLayer::blend`) and **F13/§8 Q3** (moving "Add Viewport
Effect" to the View menu).

### Post-review corrections (user feedback)

- **Group scoping was stated incorrectly and over-attributed to prior art.** §4.1 said an effect "affects only its parent group, matching Krita". That asserts the opposite of the default behaviour: `passthrough: true` is the group default (`layer.rs:512`), and a passthrough group inlines into its non-passthrough ancestor's accumulator, so the effect reaches *past* the group boundary. Corrected in §4.1 from `compositor.rs:4343-4351`; Test 13 now pins both the isolated and passthrough cases; §8 Q4 resolved.
- **Prior-art calibration.** The Krita citation attached to that rule (`kis_filter_mask.h:16-21`) was about `KisFilterMask` vs `KisAdjustmentLayer` scoping and says nothing about passthrough. More broadly: Krita and GIMP have **no viewport-space artistic effects at all**; their view-side filters are colour-management and proofing devices (§2). Prior art is genuinely useful in this plan for the document-side questions (adjustment-layer semantics, save/load shape, `neededRect`/`changedRect`), and genuinely absent for the viewport/canvas duality, which is Darkly-specific. Claims of the second kind have been removed rather than stretched. Behaviour that falls out of Darkly's own accumulator model needs no external precedent.

---

## Independent Review

Step 2 of the CLAUDE.md workflow. Every claim below was checked by opening the
cited file; the Krita and GIMP checkouts were read directly, not recalled.

### What holds up

- **§2's central claim: CONFIRMED, and it is the plan's best contribution.** Group
  accumulators are canvas-sized (`crates/darkly/src/gpu/compositor.rs:1841-1866`
  sets `padded_width/height = width/height` and recreates every `GroupState`);
  the view transform is applied only in `fs_present`
  (`crates/darkly/shaders/present.wgsl:29-38`); the veil chain runs on that
  pass's output, at viewport resolution
  (`crates/darkly/src/gpu/veil_chain.rs:322-350` presents into `veil_views[0]`,
  `:352-392` ping-pongs, `:394-414` blits to surface), driven from
  `Compositor::present_and_veils` (`compositor.rs:3242-3277`). No tree position
  is screen space. Both the user's "infer mode from tree position" design and
  the explicit-`space`-property alternative are correctly rejected.
- **§3.1 isomorphism: CONFIRMED.** `AccumPair` is `[Texture;2]`/`[TextureView;2]`
  (`compositor.rs:262-266`); `compose_filter_arm` advances `src`/`dst`
  (`compositor.rs:4393-4402`) and renders `views[src] → views[dst]`
  (`:4429-4441`); `accum_format = Rgba8Unorm` (`compositor.rs:890`, `:1090`) is
  the same value handed to `VeilChain::new` (`compositor.rs:1161`,
  `veil_chain.rs:62`, `:94`).
- **§3.4's largest correction: CONFIRMED.** The layer kind, serialization and
  undo genuinely already exist: `crates/darkly/src/document/layer_kinds/filter.rs:43-56`
  (registration), `:59-107` (serialize/deserialize), `crates/darkly/src/engine/layers.rs:782-806`
  (`add_filter_layer`, `EntityAddAction` at `:805`).
- **§3.7(a)'s per-veil alpha listing: CONFIRMED** at every cited line, and
  `black_and_white` (`shaders/veils/black_and_white.wgsl:24`), `grain`
  (`shaders/veils/grain.wgsl:95`) and `chromatic_aberration`
  (via `shaders/lib/aberration.wgsl:102-140`, which handles coverage explicitly)
  do carry alpha correctly.
- **Frontend duplication: CONFIRMED.** The `<style>` blocks of
  `frontend/src/ui/veils/VeilPickerModal.svelte` and
  `frontend/src/ui/filters/FilterPickerModal.svelte` are byte-identical, 917
  bytes each.
- **The two-trait split (§4.3): JUSTIFIED**, though for a narrower reason than
  stated. `AccumEffect` cannot subsume `NodeEffect`: the destructive path picks
  its format from the target node's texture at runtime
  (`crates/darkly/src/engine/filters/apply.rs:75-77`), which pre-baked bind
  groups cannot serve. `NodeEffect` cannot subsume `AccumEffect`: multi-pass aux
  state (`crates/darkly/src/gpu/veils/watercolor.rs:315-390`) has nowhere to
  live. The containment direction, the single `NodeAsAccum` adapter, and
  widening `encode` with `&wgpu::Device` are all correct. **No consumer branches
  on capability**: presence is answered by the registry, which is the right
  owner. The one exception is Step 5's alpha handling; see F4.

### Findings

**F1: MAJOR (prior art). The GIMP citation does not support the plan's central
move; remove or re-scope it.** §3.7(e) and §4.2 call `GimpFilterStack`/`GimpItemStack`
"direct prior-art support for the central move in §4". The hierarchy claims are
all exactly right (`gimp/app/core/gimpfilter.h:26,40`; `gimpitem.c:182`;
`gimpdrawable.c:227`; `gimplayer.c:279`; `gimpdrawablefilter.c:183`;
`gimpitemstack.c:47`; `gimpdrawablestack.c:63`; `gimplayerstack.c:59`;
`gimpfilterstack.c:187-227`), but that unification is **document-side only** and
has nothing to do with display effects. `GimpColorDisplay` is
`G_DECLARE_DERIVABLE_TYPE(..., GObject)` (`gimp/libgimpwidgets/gimpcolordisplay.h:35`,
`G_DEFINE_ABSTRACT_TYPE_WITH_CODE(..., G_TYPE_OBJECT, ...)` at
`gimp/libgimpwidgets/gimpcolordisplay.c:93`); `GimpColorDisplayStack` is a
`G_DECLARE_FINAL_TYPE` over a plain `GList *filters`
(`gimp/libgimpwidgets/gimpcolordisplaystack.c:77`) applied by a sequential
in-place buffer loop (`:447-463`), against `GimpFilterStack`'s GEGL graph.
`grep -rn "GimpFilterStack\|GIMP_TYPE_FILTER_STACK" app/display/` → zero hits.
No display-filter module is a GEGL op (`grep -rln GeglOperation modules/` → zero;
all five are `G_DEFINE_DYNAMIC_TYPE(..., GIMP_TYPE_COLOR_DISPLAY)`, e.g.
`gimp/modules/display-filter-gamma.c:94`), so they physically cannot be spliced
into a filter stack. GIMP is neutral-to-counter-evidence for unifying a viewport
catalog with a layer catalog. Separately, `gimp/app/core/gimpgrouplayer.c:1248-1255`
is labelled "Older explanation for #4634"; the live rationale is `:1230-1246`,
which the maintainers annotate `XXX It feels like the parent's
get_bounding_box() implementation ... is bugged`: weaker authority than cited.

**F2: MAJOR (prior art). "Krita pays for separation with duplicated ASC-CDL
math" is false; delete the claim.** §2 ("And both pay for it") and §3.6 lean on
it. The file/line is right (`krita/plugins/filters/asccdl/kis_asccdl_filter.h:21`)
but the analysis is not: the only ASC-CDL implementation in the tree is
`krita/plugins/filters/asccdl/kis_asccdl_filter.cpp`
(`KisASCCDLTransformation::transform`, `qPow((normalised[c]*m_slope[c])+m_offset[c], m_power[c])`).
`krita/plugins/dockers/lut/ocio_display_filter_vfx2021.cpp:70-85` is pure
delegation (`m_processorCPU->apply(img)`) and contains no CDL code at all;
repo-wide `grep -rn "CDLTransform\|asccdl"` outside `plugins/filters/asccdl/`
returns zero. Nothing structural in the plan depends on this, but CLAUDE.md's
Prior Art Principle forbids leaving it in. (Krita citations 1-4 and 6-10 all
verified exact.)

**F3: MAJOR (product call #1). `FilterLayer::blend` should be IMPLEMENTED, not
removed; the prior art cited for removal says the opposite.**
`krita/libs/image/kis_adjustment_layer.cc:40` sets `COMPOSITE_COPY` as a
*default* only: the preceding comment cites bugs 324505/294122 and calls it
"more natural for users". Opacity and blend mode remain fully user-settable and
honoured: `krita/libs/image/kis_layer_projection_plane.cpp:73` runs
`painter->setOpacityU8(m_d->layer->projectionLeaf()->opacity())`, and
`krita/libs/ui/dialogs/kis_dlg_layer_properties.cc:98-110` wires both properties
for all node types with no adjustment-layer special case. The field is genuinely
unread today (`compose_filter_arm`, `compositor.rs:4359-4454`) while being
stored (`layer.rs:238`), serialized (`document/layer_kinds/filter.rs:67-68`) and
shown in the UI: a real Document Authority violation. Wiring it is cheap: the
arm already has "before" (`views[src]`) and "after" (`views[dst]`) in hand, and
opacity is the constant-factor case of the mask lerp the arm already runs
(`compositor.rs:4444-4453`). ~40 lines. This becomes visibly wrong the moment
veils are stackable ("30% grain layer" is the obvious first request), so it
belongs **in** this work, not in §8.

**F4: MAJOR (alpha). `authors_alpha()` is the wrong shape; the preferred remedy
is unimplementable for one effect and insufficient for another. Fix the shaders
instead and delete the capability.**
- *Write-mask remedy refuted for `watercolor`.* All three passes go through the
  **same** pipeline (`crates/darkly/src/gpu/veils/watercolor.rs:322`
  (`let pipeline = &self.shared.pipeline;`), bound at `:340`, `:363`, `:384`)
  and passes 0/1 store **CMYK in RGBA, with K in the alpha slot**
  (`crates/darkly/shaders/veils/watercolor.wgsl:67-69`, `:72-76`). Masking alpha
  writes would corrupt K. `EffectPipeline` holds exactly one
  `wgpu::RenderPipeline` (`crates/darkly/src/gpu/effect.rs:3-6`), so there is no
  per-pass write mask to set.
- *`authors_alpha() -> false` does not fix `lens_blur` at all.* Its RGB
  normalizer **is** the alpha channel: `acc += exp(s * inv_t)` over the full
  `vec4` (`shaders/veils/lens_blur.wgsl:73`) then `result.rgb / result.a`
  (`:82-83`), with the shader's own comment at `:79-81` stating "Alpha input is
  1.0, so each sample contributes exp(1/threshold) to acc.a". With varying alpha
  the *colour* is wrong. Restoring source alpha yields a correct alpha over a
  broken image.
- *Simplest general solution.* Fix the six shaders. For `vhs` (`:118`),
  `rainy_glass` (`:226`), `frozen` (`:79`), `painting` (`:127`) and
  `watercolor`'s final pass (`:75`) it is **one line each**: return the sampled
  `.a` instead of `1.0`; `vhs` and `rainy_glass` contain no `.a`/`alpha`
  reference anywhere today, so nothing else is affected. For `lens_blur` it is
  ~4 lines: accumulate the normalizer from a constant `exp(inv_t)` weight and
  carry alpha separately. **All six stay bit-identical in the viewport**, because
  `fs_present` returns `vec4f(composed, 1.0)`
  (`crates/darkly/shaders/present.wgsl:71-72`): the chain's input alpha is
  already 1.0. This deletes `authors_alpha()`, the alpha-restore pass, the
  write-mask question, and (importantly) the caller-side branch §5's last
  bullet otherwise needs ("the viewport chain always sets `authors_alpha`
  handling off"), which would make `ScaledEffect::encode` depend on both the
  effect's answer *and* which space its caller is in. That is precisely the
  centralized branching the Modularity Principle wants gone, introduced to work
  around six shaders being wrong. Fix the shaders. ~20 lines, replacing ~70.
- *Residual issue the plan should name.* The accumulator is straight alpha
  (`crates/darkly/shaders/source_over.wgsl:1-2`), so spatial effects
  (`lens_blur`, `frozen`, `painting`, `watercolor`) will pull the arbitrary RGB
  of fully-transparent texels across alpha edges: expect dark fringing in canvas
  space regardless of alpha authorship. Not a blocker; it makes Test 5's
  assertion site matter (see F8).

**F5: MAJOR (unscoped gap). The `requires` story is not what §5 Step 7 assumes,
and unification regresses it.** `requires_from_doc`
(`crates/darkly/src/engine/save.rs:430-465`) collects `layer_kind`, `blend_mode`,
`modifier` (from `Entity::Filter`: the *mask/selection* registry, `:445-447`)
and `veil` (`:450-455`). A filter layer's `pipeline` id is **never recorded**.
`document/layer_kinds/filter.rs:82-91` validates `blend_mode` on deserialize but
not `pipeline`, and `compose_filter_arm` early-returns on an unknown id
(`compositor.rs:4370-4377`): a silent no-op. The module doc comment asserting
the opposite (`document/layer_kinds/filter.rs:11-13`: "surfaces as a
`LoadError::CorruptManifest` rather than a silent fallback") is wrong.
Consequences: (a) Step 7 has no "filter-layer contribution" to collapse; (b)
Test 8's `requires.effect` assertion needs work the plan does not budget; (c)
after unification, `painting`/`watercolor` named in a layer's `pipeline` goes
from *covered* (as `requires.veil`) to *silently dropped*, a genuine
regression. Recording filter-layer pipeline ids in `requires` (~15 lines) must
be in scope, and the stale doc comment corrected.

**F6: MEDIUM (registry shape). `accum: Option<_>` + `node: Option<_>` makes an
illegal state representable.** §4.3's `EffectRegistration` permits both `None`,
which no consumer can serve: `accum_instance` synthesizes from `node`, and with
neither it returns `None` and the effect silently does nothing, the same
silent-no-op failure mode as F5. Encode "at least one" in the type (an enum), or
assert at registry construction plus a `crates/darkly/tests/schema_contracts.rs`
case.

**F7: MEDIUM (scope). Split this into three sequenced PRs; the plan's own
reduced-scope lever is the wrong one.** §7 is honest that ask (A) is ~300
production lines and the rest is de-duplication. It then argues the merge is a
*precondition* for (A) because otherwise the layer arm branches between two
registries. That is only true if (A) ships first. Reverse the order and the
coupling disappears:
  1. **Shader alpha fixes** (F4): ~20 lines, viewport-bit-identical, testable
     against the existing veil path today. Lands first; de-risks everything else.
  2. **Registry + module unification** (Steps 1-3, plus F6): the ~4,600-line
     relocation and the two duplicate collapses. Pure de-duplication, no new
     user-visible behaviour. **This is exactly ask (B) and it stands alone.**
  3. **Effect layers** (Steps 4-7, plus F3 and F5): the plan's own ~300
     production lines on top, plus F13's menu/labelling change.
  §7's "Defer Step 3" option is the wrong lever: it defers churn while *keeping*
  the coupling. Sequencing removes both. **Deferring Step 8 (layer-kind rename):
  agreed, and it should follow PR 3.**

**F8: MEDIUM (tests). Several proposed tests cannot fail; the highest-risk
behaviour is untested.**
- **Test 6 (`effect_layer_is_independent_of_view_transform`) is vacuous.** §2
  establishes the composite is produced before `fs_present` applies the view
  transform, so a composite readback is view-independent by construction for
  *every* layer kind. It cannot fail. Drop it, or replace it with a screen-level
  assertion (viewport effect moves with the view; effect layer does not).
- **Test 1 asserts only "the composites differ"**: it passes if the effect
  writes garbage. Assert a property instead: `black_and_white` ⇒ `r == g == b`;
  `invert` ⇒ `255 - original`.
- **Risk 3 (stale accumulator bind groups) is the highest-severity failure mode
  and has no test.** `crates/darkly/tests/canvas_resize.rs` already exists:
  extend it: add an effect layer, resize the canvas, composite, assert correct
  output. Add a second for the reparent trigger Step 6 introduces
  (`parent: LayerId` on `EffectLayerState`).
- **Test 5 should sample an alpha *edge* texel**, not only a fully-transparent
  one, so the straight-alpha fringing in F4's last bullet is at least
  characterized.
- **Test 9 names a function that does not exist.** `crate::catalog::catalogs()`
  is not defined; `crates/darkly/src/catalog.rs:194` exposes `settings_catalogs()`.
- **Missing:** a test that an effect layer inside a non-passthrough group does
  not reach outside it. §4.1 asserts this rule and Risk 6 flags it as
  unconfirmed; it is the one semantic in §4.1 with no coverage.

**F9: MINOR (ownership). `animation.void_divisor` becomes a misnomer.** Step 6
reuses the `void_fires` branch (`compositor.rs:2997-3015`). Reusing the mechanism
is right, but the key name would then govern effect-layer animation. Rename it
(pre-release, no migration per CLAUDE.md) or add a sibling; don't leave a
void-named key driving effect layers.

**F10: MINOR. The frontend duplication saving is smaller than §3.6 implies.**
The `<style>` blocks are byte-identical (917 bytes, ~40 of each file's ~82
lines), but the `<script>` bodies genuinely differ: the veil picker calls
`app.addVeil` then `app.selectVeil(app.veilList.length - 1)`
(`frontend/src/ui/veils/VeilPickerModal.svelte:18-27`), against the filter-layer
path. Extracting a shared picker-grid component is a ~60-line change available
**today**, independent of any registry merge. Worth stating so the merge is not
credited with savings it does not produce.

**F11: MINOR. Two small overstatements about existing dispatch.** §3.2 calls
`composites_in_place()` "a trait-style predicate, not a kind enumeration"; it is
a `match` on `LayerNode::Layer(Layer::Filter(_))` (`crates/darkly/src/layer.rs:715-721`).
The *placement* is right (the type answers for itself) and nothing needs to
change, but describe it accurately. Likewise `CompositionContext::compose_layer`
still opens `if let Layer::Filter(f) = layer` (`compositor.rs:330-345`): a
centralized kind branch the plan renames but does not remove; say so, or remove
it.

**F12: Product call #2: KEEP the viewport stack in `.darkly`; the cited prior
art is not analogous.** §8 Q2 leans on Krita storing OCIO settings in `kritarc`
(`krita/libs/ui/kis_config.cc:1975-1992`). But `KisDisplayFilter` is a
*colour-management* device profile (`krita/libs/ui/canvas/kis_display_filter.h:31-49`:
`program()`, `setupTextures()`, `filter(quint8*, quint32)`, and nothing else),
which would be actively wrong to carry between machines. GIMP's display filters
are likewise proofing modules: gamma, colour-blindness, clip-warning
(`gimp/modules/display-filter-gamma.c`, `-color-blind.c`, `-clip-warning.c`).
Darkly's viewport effects are authored artistic choices with parameters; neither
reference is authority for discarding them. Keep `manifest.veils`
(`crates/darkly/src/format/manifest.rs:71`, written at
`crates/darkly/src/engine/save.rs:407-424`). The "why didn't my grain export?"
confusion is a labelling problem: fix it at the label and in the export dialog,
not by throwing away the user's work.

**F13: ENDORSE Step 7's menu move.** "Add Viewport Effect" under View, "Add
Effect Layer" under Layer, and retitling the pinned folder in
`frontend/src/ui/veils/VeilFolder.svelte` to "Viewport Effects" with a
not-exported tooltip, addresses the user's literal complaint ("confusing from a
UI perspective") at near-zero cost. It should ship in PR 3 regardless of what
else is cut, and it is the change most likely to resolve the request on its own.

### LOC assessment

The table is plausible for what it covers. Adjustments: **−70/+20** from F4
(shader fixes replace the alpha machinery), **+40** from F3 (`blend`), **+15**
from F5 (`requires`), **+60** from F8's added tests. Net production still lands
near zero. The ~4,600-line relocation is correctly called out and correctly
flagged as a review hazard: F7 resolves that by making it its own PR rather
than its own commit inside a behavioural one.

### Verdict

The diagnosis is sound and the architecture is broadly right: §2 is a genuine
correction that saves the project from a coordinate-frame bug, and §3.4's
discovery that the layer kind, serialization and undo already exist is the
right kind of scope reduction. But two of the three prior-art arguments that
motivate the shape are refuted at the source (F1, F2), one of the two product
calls is recommended backwards (F3), the alpha remedy does not work for two of
the six effects it targets (F4), a save/load gap is unscoped and would regress
(F5), and the scope should be split rather than shipped as one diff (F7).
None of this requires re-investigation of the core approach.

revise

## 1. The request, and what it actually resolves to

The user's words:

> I'm coming to realize that veils are super useful, not just as viewport overlays, but as real layers. Already I've duplicated a few like black and white and chromatic aberration to become filter layers, and I don't like having it this way because it's confusing from a UI perspective.
>
> My question is, how hard would it be to make veils more flexible, where they could be stacked at any point in the layer tree? … this would require them to effectively have two modes: 1) a viewport mode (if they're at the very top of the tree), and 2) a canvas mode, if they have any other layer type stacked above them. Essentially, the first would be in viewport space, the second in canvas space, while the underlying shader would be modular and reused between the two.

Two separable asks are bundled here:

- **(A) Stacking.** A veil should be placeable anywhere in the layer tree and transform the composite beneath it.
- **(B) De-duplication.** `black_and_white` and `chromatic_aberration` currently exist twice (once as a veil, once as a filter) and surface twice in the UI.

**(A) is nearly free.** The two invocation contracts are already isomorphic (§3.1). **(B) is the real work**, and it is what forces the architecture: without one registry, the code that resolves a layer's effect id has to ask "veil registry or filter registry?", which is precisely the centralized type-branching CLAUDE.md's Modularity Principle forbids.

The user's proposed **two-modes-by-tree-position mechanism is not implementable as described**, for a physical reason spelled out in §2. A different mechanism reaches the same user-visible outcome and is simpler. That is the core recommendation of this plan.

---

## 2. Why "mode inferred from tree position" cannot work

Darkly composites the whole layer tree into **canvas-space** accumulators, and applies the view transform **afterwards**, in the present pass:

- Group accumulators are canvas-sized: `set_canvas_rect` sets `padded_width/height = canvas width/height` and recreates every `GroupState` at those dimensions, `crates/darkly/src/gpu/compositor.rs:1829-1866`.
- The view transform is applied in `fs_present`, which maps a *screen* pixel back to a canvas pixel and samples the root composite cache: `crates/darkly/shaders/present.wgsl:29-38`.
- The veil chain runs on the **output** of that present pass: `present_to_veil_pipeline` writes into `veil_views[0]` at viewport resolution, then veils ping-pong at viewport resolution, then a blit goes to the surface, `crates/darkly/src/gpu/veil_chain.rs:322-416`, driven from `Compositor::present_and_veils`, `compositor.rs:3242-3276`.

Therefore **every position in the layer tree is canvas space, pre-view-transform. There is no tree position that is screen space**, not even the very top of the root group. A veil placed at the top of the root group would still run on the canvas-space composite and then be view-transformed, which is *not* what today's viewport veil does.

The difference is user-visible and both behaviours are wanted:

| | viewport veil (today) | veil as top-of-tree layer |
|---|---|---|
| `grain` | fixed-size film grain, independent of zoom | grain that zooms with the canvas |
| `chromatic_aberration` | fringe radiating from screen centre, sized in screen px | fringe radiating from canvas centre, sized in canvas px |
| `vhs` scanlines | locked to the display | locked to the artwork |
| exports? | never (it is a view effect) | yes (it is document content) |

So the two are **not the same effect in two modes**; they are the same *shader* invoked from two different compositing stages. The honest model is therefore:

> **Space is not a property of the layer and is not inferred from tree position. Space is determined by which stack the instance lives in:** the layer tree (canvas space, exports, undoable, saved) or the viewport stack (screen space, view-only).

This also refutes the alternative that was floated internally: an explicit `space: Canvas | Screen` property on the layer, settable only while the layer sits in the top run of the root group. It fails for the same reason: a `Screen` layer would have to be yanked out of the tree walk and handed to the veil chain, so it would be a tree row that is not rendered by the tree. That is a lie in the data model and exactly the class of coordinate-frame bug `docs/coordinate-systems.md` exists to prevent. **Rejected.**

**What survives from the user's intuition** is the UI shape, and it survives intact: the viewport stack is *already* rendered as a folder pinned above everything in the layer panel (`frontend/src/ui/layers/LayerPanel.svelte:42-44` mounts `VeilFolder` above the tree). "At the very top of the panel = viewport" is already true and stays true. What changes is that both stacks draw from **one catalog**, so the duplicate entries disappear.

### Prior art agrees

Both reference editors keep the two worlds strictly separate and neither offers a conversion:

- **Krita.** Document-side effects are `KisFilter` subclasses (`krita/libs/image/filter/kis_filter.h:26`) reached through `KisFilterRegistry`. View-side is `KisDisplayFilter` (`krita/libs/ui/canvas/kis_display_filter.h:31-49`): a bare `QObject` exposing GLSL source via `program()` and a CPU `filter(quint8 *pixels, quint32 numPixels)`, with **no node, no selection and no rect API at all**. The separation is enforced structurally: `grep -rn displayFilter libs/image/` returns zero hits, and OCIO settings live in `kritarc` (`krita/libs/ui/kis_config.cc:1975-1992`), never in a `.kra`. There is no code path converting either into the other.
- **GIMP.** Display filters are `GimpColorDisplay` objects living in `libgimpwidgets` (`gimp/libgimpwidgets/gimpcolordisplay.c:93`), applied to an already-rendered buffer in the shell (`gimp/app/display/gimpdisplayshell-render.c:372`). Document filters are `GimpDrawableFilter`/GEGL nodes inside the projection. Zero shared code paths.

**But neither reference is authority for merging the two catalogs, and an earlier draft of this plan wrongly claimed otherwise.** Two corrections, both from the review:

- There is no duplicated ASC-CDL math in Krita. The only implementation in the tree is `krita/plugins/filters/asccdl/kis_asccdl_filter.cpp`; `krita/plugins/dockers/lut/ocio_display_filter_vfx2021.cpp:70-85` is pure delegation to OpenColorIO (`m_processorCPU->apply(img)`) and contains no CDL code. Krita's separation is not visibly costing it duplication.
- GIMP's filter/layer unification (§3.7e) is **document-side only**. `GimpColorDisplay` is a plain `GObject` in `libgimpwidgets` (`gimp/libgimpwidgets/gimpcolordisplay.c:93`), no display-filter module is a GEGL op (all five are `G_DEFINE_DYNAMIC_TYPE(..., GIMP_TYPE_COLOR_DISPLAY)`), and `grep -rn GimpFilterStack app/display/` returns zero. They physically cannot enter a filter stack.

So **prior art neither authorizes nor forbids the registry merge**: it simply does not address the case, because in both editors the view-side effects are *colour-management and proofing devices* (gamma, colour-blindness simulation, clip warning, OCIO display transforms), not authored artistic effects. Darkly's veils are the latter, which is exactly why the same shader is wanted in both places and why the duplication the user reported arose at all. The justification for one registry rests on CLAUDE.md's DRY and Modularity Principles, which is sufficient on its own; it does not need a prior-art crutch.

What prior art *does* support is keeping the two rendering stages distinct (both editors do, structurally) and scoping an effect layer to its parent group (§4.1).

---

## 3. Verification of the earlier scan's hypotheses

Every claim from the prior discussion was checked against source. Results:

### 3.1 "The two invocation contracts are nearly isomorphic": **CONFIRMED, strongly**

```rust
// crates/darkly/src/gpu/veil.rs:37-45, 93-99
fn create_cache(&mut self, device, queue,
                ping_pong_views: &[wgpu::TextureView; 2], sampler,
                render_width: u32, render_height: u32) -> EffectCache;
fn encode(&self, encoder, cache: &EffectCache, src_idx: usize, dst_view: &wgpu::TextureView);
```

`compose_filter_arm` has exactly these values in hand: `gs.accum` is an `AccumPair { textures: [Texture;2], views: [TextureView;2] }` (`compositor.rs:263-266`), `src = gs.current_accum`, `dst = 1 - src` (`compositor.rs:4393-4402`), and it renders `views[src] → views[dst]` (`compositor.rs:4433-4441`). Formats match: `accum_format = Rgba8Unorm` (`compositor.rs:890`, `:1090`) and `VeilChain::new` is constructed with that same value (`compositor.rs:1161`).

`src_idx` varying frame-to-frame is safe: caches hold `bind_groups[pass][src_idx]` for both directions, and even the multi-pass veils honour it correctly (`gpu/veils/pixelate.rs:284, 292` select `src_idx` only for the pass that reads the ping-pong, `0` for aux-reading passes).

### 3.2 "Masking comes for free": **CONFIRMED**

`compose_filter_arm` passes `None` for the effect's own mask (`compositor.rs:4437`) and instead does snapshot (`:4389`) → effect → `mix(before, after, mask)` (`:4444-4453`) via `snapshot_parent_accum` / `lerp_parent_accum_with_mask`. That scaffolding is keyed off `LayerNode::composites_in_place()` (`layer.rs:715-721`). To be accurate: its *body* is a `match` on `LayerNode::Layer(Layer::Filter(_))`, but the **placement** is what matters and it is right, because the type answers the question about itself rather than a consumer asking what kind it is. Nothing there needs changing, and it applies unchanged to an accumulator-driving veil.

A genuine centralized kind branch does survive one level up: `CompositionContext::compose_layer` opens `if let Layer::Filter(f) = layer` (`compositor.rs:330-345`). §5 Step 6 removes it rather than renaming it.

### 3.3 "Scissor is a non-issue": **CONFIRMED for correctness, UNDERSTATED for cost**

Both filter render paths begin a full-target pass with `LoadOp::Clear` and never call `set_scissor_rect`: `MaskedFilterPipeline::render` (`gpu/effect.rs:388-406`) and `ParamFilter::render` (`gpu/param_filter.rs:352-370`). The compose scissor is always the full canvas at the top level (`compositor.rs:3328`, `:3413`). So there is no scissor to violate.

**But the cost claim needs correcting.** A canvas-space effect runs at *canvas* resolution, not viewport resolution. A 4096×4096 canvas is ~8× the pixels of a 1440p viewport, and Darkly re-composites the whole tree every dirty frame. `painting` declares `perf_scale_factor() == 0.7` precisely because it costs 169 taps/pixel (`gpu/veils/painting.rs:109-114`). Canvas-space effect layers therefore **need** the resolution-scaling machinery, which is why §5 Step 4 extracts it rather than leaving it inside `VeilChain`.

Krita solves the analogous problem with incremental region merges: `neededRect`/`changedRect` (`krita/libs/image/filter/kis_filter.h:81-93`; blur grows `neededRect` by 2× the kernel half-size at `krita/plugins/filters/blur/kis_blur_filter.cpp:105-114` but `changedRect` by only 1× at `:116-125`), with `useTempProjections = walker.needRectVaries()` (`krita/libs/image/kis_async_merger.cpp:175`) allocating scratch devices when reads exceed writes. **Darkly is immune to that entire class of bug today**, because it brute-forces a full-canvas pass per effect per frame. This plan does not change that trade, and does not introduce `neededRect`/`changedRect`. Recording it here so a future incremental-compositing effort knows the prior art exists.

### 3.4 "Work that looks genuinely required": **PARTLY WRONG**

- **A per-`LayerId` cache with invalidation**: correct, and there is an exact precedent to copy. `filter_caches: HashMap<LayerId, (Vec<ParamValue>, EffectCache)>` (`compositor.rs:687`) is pruned to live layers and rebuilt on a param fingerprint mismatch (`compositor.rs:3993-4017`). The *additional* need (invalidation when accum textures are recreated) also has a precedent one function away: `set_canvas_rect` already does `self.mask_snapshot_state.clear(); self.projection_states.clear(); self.blend_bind_groups.clear();` (`compositor.rs:1868-1874`) for exactly this reason.
- **Animation scheduling**: correct. `needs_animation` / `update_time` are overridden by `grain` (`gpu/veils/grain.rs:109,130`), `vhs` (`:138,154`) and `rainy_glass` (`:158,172`), and are driven only from `VeilChain` today (`compositor.rs:2984-3019`). A canvas-space effect must set `needs_composite`, not `needs_present`: the void path already models this (`compositor.rs:3011-3015`).
- **`perf_scale_factor` extraction**: correct that it must move, **wrong about who uses it**. Only `painting` overrides it (`gpu/veils/painting.rs:109`); `watercolor` and `lens_blur` do **not**. The global `rendering.veil_scale` downscale/upscale path (`gpu/veil_chain.rs:542-641`) applies to all veils regardless.
- **"A layer kind + serialization + undo, since veils bypass all of that"**: **WRONG. All three already exist.** `document/layer_kinds/filter.rs` is a complete, registered layer kind whose entire document state is `pipeline: String` + `params: Vec<ParamValue>` (`layer.rs:235-250`), with serializer/deserializer/`remap_ids` and round-trip tests (`document/layer_kinds/filter.rs:43-56`, `:122-271`). `add_filter_layer` pushes an `EntityAddAction` (`engine/layers.rs:797`) and `update_filter_params` coalesces a `PropertyAction` (`engine/layers.rs:855+`). **A veil placed in the tree needs no new layer kind, no new serialization and no new undo**: it is an existing filter layer whose `pipeline` id happens to name a veil. This is the single largest scope reduction versus the earlier estimate.

### 3.5 "Counter-proposal: explicit space property"; **REJECTED**, see §2.

### 3.6 "A full `Veil`/`FilterEffect` trait merge is blocked": **CONFIRMED**, but a *composition* is available

`FilterEffect::render(device, encoder, src, mask, out, format, cache)` (`gpu/filter.rs:58-67`) takes arbitrary source/output views, a format selector, and a real mask binding. It genuinely serves things a pre-baked veil cache cannot:

- destructive apply over an arbitrary node texture, at that node's own size and format: `engine/filters/apply.rs:60-130`, where `format` comes from `self.compositor.node_texture(node_id).format()` (`:75-77`);
- R8 mask and selection filtering: `engine/filters/mask.rs:86`, `:260-268`; `engine/filters/selection.rs:817`, `:912`.

So the two traits must both survive. **However, the containment is one-directional and total: every `FilterEffect` can act as a `Veil`** (one full-target pass from `views[src]` to `views[dst]`, `Rgba8Unorm`, no mask), which is *literally what `compose_filter_arm` already does*. That single observation is what makes the unification cheap: one blanket adapter, and every consumer that drives an accumulator sees one trait.

The duplication the user complained about is real and severe. `gpu/black_and_white.rs` is a shared core (identity, `PARAMS`, `pack_uniform`, `SHADER_LIB`), consumed by `gpu/veils/black_and_white.rs` (160 lines) and `gpu/filters/black_and_white.rs` (70 lines); the two WGSL files differ only in `textureSampleLevel` vs `textureLoad` plus a masked entry point. Same story for `chromatic_aberration` (149 + 443 lines; shaders differ by ~14 substantive lines).

On the frontend, `VeilPickerModal.svelte` and `FilterPickerModal.svelte` (82 and 84 lines) have **byte-identical `<style>` blocks**: 917 bytes, about 40 lines of each file. Their `<script>` bodies genuinely differ, though: the veil picker calls `app.addVeil` then `app.selectVeil(app.veilList.length - 1)` (`frontend/src/ui/veils/VeilPickerModal.svelte:18-27`), against the filter-layer path. **Extracting a shared picker grid is a ~60-line change available today, independent of any registry merge**, so the merge should not be credited with that saving, and the LOC table in §7 no longer is.

CLAUDE.md's DRY "stop-sign phrases" rule applies directly. `gpu/filter.rs:15-20` even contains the apology:

> `FilterEffect` is deliberately distinct from `Veil`: … The two share the `EffectCache` and the `ParamDef`/`ParamValue` schema (where the real reuse lives) not the invocation contract.

That statement was true when written. §3.1 shows the invocation contract *is* now shared; the comment has been overtaken.

**Verdict: the trait merge does not belong in this plan (and never will, the two traits describe different capabilities). The *registry* merge does, and is not optional.**

### 3.7 Findings the earlier scan missed

**(a) Six of ten veils hard-code `alpha = 1.0`.** This is the most substantive gap.

```
shaders/veils/vhs.wgsl:118          return vec4f(col, 1.0);
shaders/veils/rainy_glass.wgsl:226  return vec4f(col, 1.0);
shaders/veils/watercolor.wgsl:75    return vec4f(cmyk_to_rgb(cmyk), 1.0);
shaders/veils/frozen.wgsl:79        return vec4f(r, g, b, 1.0);
shaders/veils/lens_blur.wgsl:83     return vec4f(result.rgb / result.a, 1.0);
shaders/veils/painting.wgsl:127     return vec4f(out.rgb / out.w, 1.0);
```

Correct for a viewport veil, which runs on an already-opaque presented image. **Fatal in canvas space**: such an effect layer would force the entire canvas rect opaque, destroying transparency below it and filling unpainted areas with colour. `black_and_white` (`:24`), `grain` (`:95`) and `chromatic_aberration` carry alpha through correctly.

Note the accumulator is **straight alpha**, not premultiplied: `shaders/source_over.wgsl:1-2` ("premultiplied foreground onto straight-alpha background. Returns straight-alpha result") and `shaders/present.wgsl:62-66`. So there is no premultiplication hazard; the problem is purely alpha *authorship*.

An earlier draft proposed declaring this as an `authors_alpha()` capability and having the caller restore alpha. The review refuted that, and the refutation holds:

- **The write-mask remedy is unimplementable for `watercolor`.** All three passes share one pipeline (`gpu/veils/watercolor.rs:322`, bound at `:340`, `:363`, `:384`), and passes 0/1 store **CMYK in RGBA with K in the alpha slot** (`shaders/veils/watercolor.wgsl:67-69`, `:72-76`). Masking alpha writes would corrupt K. `EffectPipeline` holds exactly one `wgpu::RenderPipeline` (`gpu/effect.rs:3-6`), so there is no per-pass mask to set.
- **Restoring alpha does not fix `lens_blur` at all.** Its RGB normalizer *is* the alpha channel: it accumulates `exp(s * inv_t)` over the full `vec4` (`shaders/veils/lens_blur.wgsl:73`) then divides `result.rgb / result.a` (`:82-83`), with the shader's own comment at `:79-81` noting "Alpha input is 1.0, so each sample contributes exp(1/threshold) to acc.a". With varying alpha the *colour* is wrong, not just the alpha. Restoring source alpha would yield a correct alpha over a broken image.

**So the six shaders get fixed directly** (§5 Step 0). Five are one line each; `lens_blur` is ~4. All six stay bit-identical in the viewport, because the chain's input alpha is already 1.0 (`shaders/present.wgsl:71-72` returns `vec4f(composed, 1.0)`). This deletes the capability, the extra pass, and (importantly) a caller-side branch that would otherwise have made `ScaledEffect::encode` depend on both the effect's answer *and* which space its caller was in, which is precisely the centralized branching the Modularity Principle wants gone.

**A residual issue this plan names but does not solve.** Because the accumulator is straight alpha, spatial effects (`lens_blur`, `frozen`, `painting`, `watercolor`) will pull the arbitrary RGB of fully-transparent texels across alpha edges: expect dark fringing in canvas space regardless of alpha authorship. Not a blocker for shipping; Test 5 characterizes it at an edge texel so the behaviour is at least known rather than discovered later.

**(b) `FilterLayer::blend` is stored but never read, and should be implemented.** `FilterLayer` carries `blend: BlendProps { opacity, blend_mode }` (`layer.rs:238`), serialized (`document/layer_kinds/filter.rs:67-68`) and exposed to the UI, but `compose_filter_arm` (`compositor.rs:4359-4454`) never reads it. That is a live Document Authority violation: a document field with no realization.

An earlier draft read Krita as authority for *removing* the field. That reading was wrong. `krita/libs/image/kis_adjustment_layer.cc:40` sets `COMPOSITE_COPY` as a **default only**: the preceding comment cites bugs 324505/294122 and calls it "more natural for users". Opacity and blend mode remain fully user-settable and are honoured: `krita/libs/image/kis_layer_projection_plane.cpp:73` runs `painter->setOpacityU8(m_d->layer->projectionLeaf()->opacity())`, and `krita/libs/ui/dialogs/kis_dlg_layer_properties.cc:98-110` wires both properties for every node type with no adjustment-layer special case.

Wiring it is cheap: the arm already holds "before" (`views[src]`) and "after" (`views[dst]`), and opacity is the constant-factor case of the mask lerp it already runs (`compositor.rs:4444-4453`). ~40 lines, in scope at §5 Step 6a. A "30% grain layer" is the obvious first request once veils stack, so leaving it unread would read as a bug.

**(f) `requires` never records filter-layer pipeline ids (and unification would turn that into a regression.** `requires_from_doc` (`engine/save.rs:430-465`) collects `layer_kind`, `blend_mode`, `modifier` (from `Entity::Filter`) the *mask/selection* registry, `:445-447`) and `veil` (`:450-455`). A filter layer's `pipeline` id is **never recorded**. `document/layer_kinds/filter.rs:82-91` validates `blend_mode` on deserialize but not `pipeline`, and `compose_filter_arm` early-returns on an unknown id (`compositor.rs:4370-4377`): a silent no-op. The module doc comment claiming the opposite (`document/layer_kinds/filter.rs:11-13`: "surfaces as a `LoadError::CorruptManifest` rather than a silent fallback") is simply wrong today.

This matters here because after unification, `painting` or `watercolor` named in a layer's `pipeline` would go from *covered* (as `requires.veil`) to *silently dropped*. Recording filter-layer pipeline ids in `requires` (~15 lines) is therefore in scope at §5 Step 7, and the stale doc comment gets corrected with it.

**(c) `gpu::filter` vs `document::filter` is an existing naming collision the code apologises for**: `gpu/filter.rs:107-108`: "Distinct from the `layerFilters` catalog of `crate::document::filter`, which registers mask and selection modifiers rather than colour adjustments." Renaming the GPU side to "effect" removes the collision as a side effect of this work.

**(d) Convergent evolution worth noting.** Krita's `KisProjectionLeaf::dependsOnLowerNodes()` returns true for exactly one node type (`KisAdjustmentLayer` (`krita/libs/image/kis_projection_leaf.cpp:276-279`)) and it is what drives the "re-run this node when something below it dirties" logic. Darkly's `LayerNode::composites_in_place()` (`layer.rs:715-721`) is the same predicate, already expressed the way CLAUDE.md wants (a method, not a kind enumeration). Nothing to change; confirmation that the existing shape is right.

**(e) GIMP unified "filter" and "layer" under one abstraction, and it worked.** `GimpLayer` **is-a** `GimpFilter` (`gimp/app/core/gimplayer.c:279` → `GimpDrawable` `:227` → `GimpItem` `gimp/app/core/gimpitem.c:182` → `GIMP_TYPE_FILTER`), and the layer stack **is** a filter stack: `GimpItemStack : GimpFilterStack` (`gimp/app/core/gimpitemstack.c:47`) → `GimpDrawableStack` (`gimpdrawablestack.c:63`) → `GimpLayerStack` (`gimplayerstack.c:59`). `GimpFilterStack::get_graph` chains all active filters tail→head between input and output proxies (`gimp/app/core/gimpfilterstack.c:187-227`): the same ping-pong-through-a-stack shape Darkly's accumulator has.

**Scope of what this supports.** The hierarchy claims above are exact, but the unification is **document-side only** and says nothing about view-level effects: `GimpColorDisplay` is a plain `GObject` (`gimp/libgimpwidgets/gimpcolordisplay.c:93`), no display-filter module is a GEGL op, and `grep -rn "GimpFilterStack" app/display/` returns zero; they cannot be spliced into a filter stack. So GIMP is evidence that *unifying "a filter" and "a layer" inside the document works well*, which supports treating an effect layer as an ordinary tree node (§4.1). It is **not** evidence for merging a viewport catalog with a layer catalog; §2 states what carries that argument instead.

Also correcting an over-claim: `gimp/app/core/gimpgrouplayer.c:1248-1255` is labelled "Older explanation for #4634". The live rationale is `:1230-1246`, which the maintainers annotate `XXX It feels like the parent's get_bounding_box() implementation ... is bugged`: weaker authority than an earlier draft implied.

---

## 4. Recommended architecture

### 4.1 Feature semantics (precise)

**An *effect* is a registered, parameterized image transform.** One catalog, one picker, one preview mechanism. An effect declares up to two capabilities:

| capability | shape | serves |
|---|---|---|
| **accumulator** | per-instance, prepared against a ping-pong pair at a known size; may run multiple passes into aux textures; may animate | effect layers (canvas space) **and** viewport effects (screen space) |
| **node** | shared, stateless, one pass over arbitrary `src`/`out` views with an optional mask and a format selector | destructive apply to a raster layer, R8 mask/selection filtering |

Every node-capable effect is automatically accumulator-capable via a blanket adapter (§4.3). An accumulator-only effect (`watercolor`, `grain`, `rainy_glass`, `vhs`, `pixelate`, `lens_blur`, `frozen`, `painting`) simply does not appear in the destructive Colors menu, because the menu is driven by the presence of the node capability, not by a list.

**An effect layer** is an existing `filter` layer whose `pipeline` id names any registered effect. It:
- lives anywhere in the layer tree, at any depth, in any group;
- transforms **whatever is in the accumulator it composites into**, no more, no less. Passthrough decides which accumulator that is:
  - inside an **isolated** (non-passthrough) group, that group owns its accumulator, so the effect sees only its lower siblings within the group, over transparent, and does not reach outside;
  - inside a **passthrough** group (which is the **default** (`layer.rs:512`)) the group inlines into its nearest non-passthrough ancestor's accumulator, so the effect *does* reach past the group boundary, down through everything below it in that ancestor.

  This is not a new rule and not a design choice this plan makes: it is exactly what filter layers do today, stated by the existing code at `compositor.rs:4343-4351` ("lower siblings + everything beneath the group, since a passthrough group inlines into its non-passthrough ancestor's accumulator"). It needs no prior-art justification: it falls out of the accumulator model. An earlier draft flattened this to "affects only its parent group, matching Krita", which asserts the opposite of the default behaviour and cited `kis_filter_mask.h:16-21`, a line about `KisFilterMask` vs `KisAdjustmentLayer` scoping that says nothing about passthrough. Both retracted;
- runs in **canvas space** at canvas resolution, pre-view-transform;
- honours a visible attached mask via the existing snapshot+lerp path;
- is saved, undoable, and included in exports;
- animates by driving `needs_composite`.

**A viewport effect** is an entry in the existing `VeilChain`. It:
- runs in **screen space** at viewport resolution, post-view-transform;
- is view-only: never exported, never part of the document image;
- keeps its current save behaviour (`manifest.veils`) and its pinned position above the layer tree in the panel;
- animates by driving `needs_present`.

**Nothing about the effect's own implementation differs between the two.** One shader, one param schema, one preview.

### 4.2 Principle-by-principle assessment

- **DRY.** Removes two duplicated effect implementations (four `.rs` files → two, four `.wgsl` files → two), two duplicated registries, two duplicated preview mechanisms, and two duplicated picker modals. Directly addresses the stop-sign condition.
- **Modularity.** One registry, one `gpu/effects/` directory, one `build.rs` scan. Adding an effect stays "drop one file". Capability presence/absence is declared *in that file* and every consumer asks the registry, never the type id. The blanket adapter means the accumulator consumers never learn which capability an effect actually has.
- **Type-owned dispatch.** The diagnostic question: "would adding a variant force me to edit this code?": is answered no everywhere: adding an accumulator-only effect adds no arm to `compose_effect_arm`, and adding a node-capable one adds no arm to the Colors menu.
- **Ownership.** The effect instance's parameters live on the instance; the compositor owns only the derived GPU cache. The document owns `pipeline` + `params`; the compositor's `effect_layers` map is purely derived and rebuildable.
- **Document Authority.** Unchanged and strengthened. The document already carries the whole effect-layer state; the compositor map is a derived realization cleared on canvas resize. No new doc/compositor mirror is introduced. (§3.7(b) flags a *pre-existing* smell, unchanged by this plan.)
- **Prior Art.** GIMP's `GimpFilterStack`/`GimpItemStack` unification (§3.7e) supports treating an effect as an ordinary tree node; Krita's and GIMP's strict document/display separation (§2) supports keeping the two rendering stages distinct; Krita's per-group scoping supports §4.1's group-scoping rule. **The registry merge itself has no prior-art support in either editor** (§2): it rests on DRY and Modularity, which is enough.
- **No Migrations.** `.darkly` bodies and the `requires` manifest change shape; no upgrade path is written, per the pre-release rule.
- **No Blocking GPU Readbacks.** Nothing in this plan reads back from the GPU.
- **Engineering.** The pre-existing awkwardness (two registries for one concept) is what makes (B) hard; the plan restructures rather than adding a third surface.

### 4.3 The shape, concretely

```rust
// crates/darkly/src/gpu/effect.rs (traits live beside EffectCache/EffectPipeline)

/// An effect prepared against a ping-pong accumulator pair. (Today's `Veil`.)
pub trait AccumEffect: std::fmt::Debug {
    fn type_id(&self) -> &'static str;
    fn clone_boxed(&self) -> Box<dyn AccumEffect>;
    fn param_values(&self) -> Vec<ParamValue>;
    fn create_cache(&mut self, device: &wgpu::Device, queue: &wgpu::Queue,
                    ping_pong_views: &[wgpu::TextureView; 2], sampler: &wgpu::Sampler,
                    render_width: u32, render_height: u32) -> EffectCache;
    fn perf_scale_factor(&self) -> f32 { 1.0 }
    fn needs_animation(&self) -> bool { false }
    fn update_time(&mut self, _queue: &wgpu::Queue, _cache: &EffectCache, _dt: f32) {}
    fn preview_at(&mut self, _queue: &wgpu::Queue, _cache: &EffectCache, _t: f32) -> bool { true }

    /// `device` is threaded through so a `NodeEffect` can be driven here: it
    /// builds its bind group per call rather than pre-baking one.
    fn encode(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder,
              cache: &EffectCache, src_idx: usize, dst_view: &wgpu::TextureView);
}

/// An effect that transforms arbitrary texels in one pass. (Today's `FilterEffect`.)
pub trait NodeEffect: Send + Sync {
    fn ensure(&self, device: &wgpu::Device, queue: &wgpu::Queue,
              params: &[ParamValue], cache: &mut EffectCache);
    fn render(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder,
              src: &wgpu::TextureView, mask: Option<&wgpu::TextureView>,
              out: &wgpu::TextureView, format: wgpu::TextureFormat, cache: &EffectCache);
}
```

Widening `encode` with `&wgpu::Device` is the one signature change that makes the composition possible. All ten existing veils ignore the new parameter (they bind pre-baked groups). `wgpu::TextureView` is `Clone` in wgpu 29 (used at `brush/gpu_context.rs:588`, `gpu/overlay.rs:475`), so the adapter can retain the pair it was prepared with.

Registration, with capabilities as sub-structs so the shape stays legible:

```rust
pub struct AccumCapability {
    pub create_pipeline: fn(&wgpu::Device, wgpu::TextureFormat) -> EffectPipeline,
    pub from_params: fn(&[ParamValue], Arc<EffectPipeline>) -> Box<dyn AccumEffect>,
}

pub struct NodeCapability {
    pub create_pipeline: fn(&wgpu::Device) -> Arc<dyn NodeEffect>,
    /// Action id that applies this destructively to the active node.
    pub hotkey_action: &'static str,
}

/// What an effect can do. An effect with *neither* capability cannot be
/// realized by any consumer, so the enum makes that state unrepresentable
/// rather than leaving two `Option`s that can both be `None` and silently
/// no-op: the same failure mode §3.7(f) documents for unknown pipeline ids.
pub enum Capabilities {
    /// Ping-pong only: no destructive apply, absent from the Colors menu.
    Accum(AccumCapability),
    /// Arbitrary views/format/mask. Reaches accumulators via `NodeAsAccum`.
    Node(NodeCapability),
    Both(AccumCapability, NodeCapability),
}

pub struct EffectRegistration {
    pub type_id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub icon: &'static str,
    pub params: &'static [ParamDef],
    pub preview: Option<PreviewAnim>,
    pub preview_at: Option<fn(f32) -> Vec<ParamValue>>,
    pub capabilities: Capabilities,
}
```

`Capabilities::accum()` and `::node()` return `Option<&_>` so the registry's two accessors read exactly as before; the difference is that the "neither" branch no longer exists to be got wrong. A `crates/darkly/tests/schema_contracts.rs` case asserts every registered effect is constructible through at least one capability, which is now a type-level tautology but guards the accessors.

The registry holds the one line that makes containment invisible to callers:

```rust
impl EffectRegistry {
    /// Build an accumulator instance for `type_id`. An effect with no accumulator
    /// capability is served by its node capability through `NodeAsAccum`, so
    /// callers never learn which capability an effect actually declares.
    pub fn accum_instance(&mut self, type_id: &str, params: &[ParamValue],
                          device: &wgpu::Device, format: wgpu::TextureFormat)
        -> Option<Box<dyn AccumEffect>> { /* ... */ }

    /// The shared node transform, or `None` for an accumulator-only effect.
    pub fn node_effect(&mut self, type_id: &str, device: &wgpu::Device)
        -> Option<Arc<dyn NodeEffect>> { /* ... */ }
}
```

`NodeAsAccum` (≈80 lines) holds `type_id`, `Arc<dyn NodeEffect>`, `params`, and the cloned `[TextureView; 2]`; `create_cache` clones the views and runs `ensure` into a fresh `EffectCache`; `encode` calls `render(device, encoder, &self.views[src_idx], None, dst_view, Rgba8Unorm, cache)`.

### 4.4 What is deliberately *not* done

- **The viewport `VeilChain` is not folded into the layer tree.** §2 shows it cannot be done honestly. It keeps its own list, its own `needs_present` path, and its own save slot; it merely resolves instances from the unified registry.
- **`manifest.veils` is not dropped** (§8 Q2, resolved: keep).
- **`neededRect`/`changedRect` are not introduced.** Darkly's full-target-pass model makes them unnecessary today (§3.3).
- **The two traits are not merged** (§3.6).
- **Straight-alpha edge fringing on spatial effects is not fixed** (§3.7a): characterized by a test, not solved.
- **The layer kind is not renamed** `filter` → `effect` (§5 Step 8, deferred to a fourth PR).

---

## 5. Implementation steps

Ordered, and grouped into the three PRs of §7. Each step leaves the workspace compiling.

### Step 0: Fix alpha authorship in six shaders *(PR 1, standalone)*

Six effects hard-code `alpha = 1.0` (§3.7a). Correct for an opaque viewport image, fatal in canvas space. Fix the shaders rather than declaring a capability:

- One line each (return the sampled `.a` instead of `1.0`) for `vhs` (`shaders/veils/vhs.wgsl:118`), `rainy_glass` (`:226`), `frozen` (`:79`), `painting` (`:127`), and `watercolor`'s **final pass only** (`:75`; passes 0/1 legitimately carry K in alpha and must not be touched: `shaders/veils/watercolor.wgsl:67-76`). Neither `vhs` nor `rainy_glass` references `.a`/`alpha` anywhere today, so nothing else is affected.
- ~4 lines for `lens_blur`: its normalizer *is* the alpha channel (`shaders/veils/lens_blur.wgsl:73`, `:82-83`). Accumulate the normalizer from a constant `exp(inv_t)` weight and carry alpha separately.

**All six stay bit-identical in the viewport**, because the chain's input alpha is already 1.0 (`shaders/present.wgsl:71-72`). That makes this step verifiable against the existing veil path today, before any of the restructuring lands, which is why it goes first.

### Step 1: Move the traits, widen `encode`

- Move `Veil` → `AccumEffect` and `FilterEffect` → `NodeEffect` into `crates/darkly/src/gpu/effect.rs`, beside `EffectCache`/`EffectPipeline` (which both already serve both).
- Widen `encode` with `device: &wgpu::Device`.
- Update the 10 files in `crates/darkly/src/gpu/veils/` and the 7 in `crates/darkly/src/gpu/filters/` to the new names/signature. Mechanical.
- Update call sites: `gpu/veil_chain.rs:375, 387`, `gpu/veil.rs:371` (preview session), `gpu/compositor.rs:4433`.

### Step 2: One registry

- New `crates/darkly/src/gpu/effect_registry.rs`: `AccumCapability`, `NodeCapability`, `EffectRegistration`, `EffectRegistry`, `NodeAsAccum`, `catalog()`, and a single `preview_mechanism()`.
- Fold in the two existing preview mechanisms (`VeilMechanism`/`VeilSession` (`gpu/veil.rs:272-373`) and `FilterMechanism`/`FilterSession` (`gpu/filter.rs:267-352`)) into one `EffectMechanism`. It opens an `accum_instance` and drives `preview_at`, which is the veil session's shape; node-only effects reach it through the adapter, so the filter session's separate shape disappears. Update `PreviewRegistries` (`gpu/preview.rs`) to carry one `effects` field instead of `veils` + `filters`.
- Delete `crates/darkly/src/gpu/veil.rs` and `crates/darkly/src/gpu/filter.rs`.
- `crates/darkly/build.rs`: replace the two `generate_catalog_registry` calls at `:81-86` (`gpu/veils`) and `:95-100` (`gpu/filters`) with one for `gpu/effects`, type `crate::gpu::effect_registry::EffectRegistration`.

### Step 3: Merge the effect modules and shaders

- `git mv` all 17 files from `gpu/veils/` and `gpu/filters/` into `gpu/effects/`, then collapse the two duplicate pairs:
  - `black_and_white.rs`: one file with both capabilities; keeps using the shared `gpu/black_and_white.rs` core.
  - `chromatic_aberration.rs`: likewise.
- Same for `shaders/veils/*` + `shaders/filters/*` → `shaders/effects/*`. Each collapsed shader keeps a sampled entry point (accumulator/viewport) and the `textureLoad` + masked entry points (node path); the shared math already lives in `shaders/lib/`.
- Doing the `git mv` as its own commit keeps the review diff rename-detected. **This is the step that dominates the line count; almost none of it is new logic.**

### Step 4: Extract resolution scaling

`VeilChain`'s scaling is space-agnostic and belongs where both callers can reach it (CLAUDE.md "place functionality where it generalizes"). Move `VeilScaling`, `create_veil_resources`, `create_veil_scaling`, and `blit_pass` (`gpu/veil_chain.rs:19-27, 542-667`) into a new `crates/darkly/src/gpu/effect_scaling.rs` as a generic `ScaledEffect`:

```rust
/// One prepared effect instance plus the reduced-resolution scaffolding its
/// `perf_scale_factor` and the global scale ask for. Owns the downscale →
/// effect → upscale sequencing so both the viewport chain and the layer-tree
/// arm get it identically.
pub struct ScaledEffect { /* cache + Option<Scaling> */ }
impl ScaledEffect {
    pub fn prepare(device, queue, effect: &mut dyn AccumEffect, ping_pong: &[TextureView;2],
                   sampler, pipelines: &ScalingPipelines, format, width, height, scale) -> Self;
    pub fn encode(&self, device, encoder, effect: &dyn AccumEffect,
                  pipelines: &ScalingPipelines, ping_pong: &[TextureView;2],
                  src_idx: usize, dst_view: &TextureView);
}
```

`VeilChain` keeps its config key (`rendering.veil_scale`); the layer arm reads a sibling key `rendering.effect_layer_scale` (default `1.0`: canvas-space output is document content, so full resolution is the right default; `perf_scale_factor` still applies).

Register the new key in the appropriate `crates/darkly/src/config/sections/*.rs` file.

### Step 6: Compositor: drive `AccumEffect` from the tree

In `crates/darkly/src/gpu/compositor.rs`:

- Replace `filter_caches: HashMap<LayerId, (Vec<ParamValue>, EffectCache)>` (`:687`) with:
  ```rust
  struct EffectLayerState {
      effect: Box<dyn AccumEffect>,
      scaled: ScaledEffect,
      params: Vec<ParamValue>,
      parent: LayerId,
  }
  effect_layers: HashMap<LayerId, EffectLayerState>,
  ```
- In `sync_projection_states` (`:3993-4017`), keep the prune-to-live-layers shape and extend the rebuild trigger from "params changed" to "params changed **or** parent group changed **or** entry absent". Build via `registry.accum_instance(...)` then `ScaledEffect::prepare(...)` against `group_state[&parent].accum.views`.
- Add `self.effect_layers.clear();` to `set_canvas_rect` alongside the existing `mask_snapshot_state.clear()` / `blend_bind_groups.clear()` (`:1868-1874`): the caches reference accumulator views that were just replaced. Also clear on `ensure_group_state` replacing a group's state.
- Rename `compose_filter_arm` → `compose_effect_arm` (`:4359`) and replace the `pipeline.render(...)` block (`:4419-4442`) with a `ScaledEffect::encode(...)` call. Snapshot/lerp masking, the histogram tap (`:4409-4417`) and the ping-pong advance are unchanged.
- **Remove the kind branch in `CompositionContext::compose_layer`** (`:330-345`, `if let Layer::Filter(f) = layer`) rather than renaming it. `LayerNode::composites_in_place()` already answers the question the branch is asking; route through it so a future in-place-compositing layer kind is purely additive (§3.2, F11).
- Animation: extend `any_animated_layer` (`:2832-2837`) and `tick_animated_layers` (`:2844-2857`) to also walk `effect_layers`, gated on `doc.effective_visible(id)`. They already set `needs_composite` via the `void_fires` branch (`:3011-3015`): reuse the mechanism rather than adding a parallel divisor, but **rename `animation.void_divisor`** to a neutral name (e.g. `animation.canvas_divisor`), since it would otherwise be a void-named config key governing effect-layer animation. Pre-release, so no migration (CLAUDE.md).

### Step 6a: Wire `FilterLayer::blend` *(PR 3)*

Read the blend props the document already carries (§3.7b). In `compose_effect_arm`, after the effect writes `views[dst]`, apply `opacity` and `blend_mode` between `views[src]` (before) and `views[dst]` (after). Opacity is the constant-factor case of the mask lerp the arm already runs (`compositor.rs:4444-4453`), so the scaffolding exists; the blend mode routes through the existing `blend_modes/` registry. ~40 lines. This closes a Document Authority violation that predates this work and becomes visible the moment veils stack.

### Step 7: Engine, protocol, frontend

Rust:
- `engine/layers.rs::add_filter_layer` (`:782`) and `filter_param_defs` now consult the unified registry, so a veil id is accepted with no code change beyond the registry swap.
- `engine/veils.rs`: `add_veil_layer` / `update_veil_layer` resolve through the unified registry (they already call `chain.registry_mut().create_veil(...)`; that becomes `accum_instance`).
- `engine/save.rs::requires_from_doc` (`:430-465`): **filter-layer `pipeline` ids are not recorded today** (§3.7f), add them, then collapse `requires.veil` and that new contribution into one `requires.effect` list; update `format/manifest.rs:153`. Without this, `painting`/`watercolor` named in a layer's `pipeline` regresses from covered to silently dropped. ~15 lines.
- Correct the stale doc comment at `document/layer_kinds/filter.rs:11-13`, which claims an unknown pipeline id "surfaces as a `LoadError::CorruptManifest` rather than a silent fallback". It does not; `compose_filter_arm` early-returns (`compositor.rs:4370-4377`). Either make the claim true by validating `pipeline` on deserialize, or fix the comment, validating is preferred now that `requires` covers the ids.
- `format/error.rs` messages (`:23`) already speak in `"veil/lens_flare"` terms: retarget to `"effect/…"`.
- `engine/filters/apply.rs`, `filters/mask.rs`, `filters/selection.rs`: resolve through `registry.node_effect(...)`, which returns `None` for accumulator-only effects, so those simply never apply destructively.

Frontend:
- Replace `ui/veils/VeilPickerModal.svelte` + `ui/filters/FilterPickerModal.svelte` with one `ui/effects/EffectPickerModal.svelte` taking a `target: 'canvas' | 'viewport'` prop. Both call sites already build the same grid over `app.entries(...)` and share an identical `<style>` block.
- `state/layerPicker.svelte.ts`: `LayerPickerKind` becomes `'effect-canvas' | 'effect-viewport' | 'void'`.
- `actions/index.ts`: `newFilterLayer` → `newEffectLayer` (Layer menu); `newVeil` → `newViewportEffect`, **moved to the View menu**; the menu location is itself the clearest statement that it is a view-level thing. (Judgment call; flagged in §8.)
- `ui/veils/VeilFolder.svelte`: retitle "Veils" → "Viewport Effects" with a tooltip stating it is view-only and not exported.
- Merge `ui/veils/VeilProperties.svelte` (21 lines) into `ui/filters/FilterProperties.svelte` (104 lines) as one params surface if they prove to be the same shape; otherwise leave and note why.
- Optional affordance with no prior-art precedent but near-zero cost, since both stacks hold `(type_id, params)`: "Move to canvas" / "Move to viewport" commands on an effect. **Recommend deferring** to keep this PR's surface area down.

### Step 8: *Optional:* rename the layer kind `filter` → `effect`

Renames `document/layer_kinds/filter.rs` → `effect.rs`, `Layer::Filter(FilterLayer)` → `Layer::Effect(EffectLayer)`, manifest body type id `"filter"` → `"effect"`, wire method `addFilterLayer` → `addEffectLayer`. Removes the `gpu::filter` / `document::filter` collision (§3.7c) and makes one word mean one thing throughout. **Separable**: the feature works without it, and it is ~320 lines of pure churn. Recommend as a follow-up PR unless the reviewer disagrees.

---

## 6. Tests

This is a feature, so these are feature tests (CLAUDE.md Testing Principle: "the test exists; it passes"). GPU-touching tests go in `crates/darkly/tests/` and must run under `--test-threads=1` with `--features darkly/testing`.

New file `crates/darkly/tests/effect_layers.rs`:

1. **`veil_effect_as_layer_transforms_composite`**: add a solid raster layer, add a `black_and_white` effect layer above it, read back; assert a **property**, not mere difference: every texel satisfies `r == g == b`. (A bare "the composites differ" assertion passes even if the effect writes garbage.) A second case with `pixelate` asserts block uniformity. *Verifies the core ask: a veil-only effect renders from the layer tree, correctly.*
2. **`effect_layer_only_affects_layers_beneath`**: raster A (red) at bottom, an `invert` effect layer, raster B (blue, opaque, covering the left half) on top. Assert the left half is still blue and the right half is inverted-red. *Verifies stacking position is honoured: the actual semantic the user asked for.*
3. **`node_only_effect_still_composites_through_adapter`**: `invert` as an effect layer inverts the composite. *Verifies `NodeAsAccum`; this is the existing filter-layer path re-routed through the new trait, so it also guards against regressing what works today.*
4. **`masked_effect_layer_confines_to_mask`**: attach a half-covering mask to a `black_and_white` effect layer; assert the masked half is grey and the rest untouched. *Verifies the snapshot+lerp path survives the arm rewrite.*
5. **`spatial_effect_preserves_transparency`**: a canvas with an opaque blob on transparent background plus a `frozen` (or `vhs`) effect layer above it. Assert texels well outside the blob still have `a == 0`, **and separately sample an alpha *edge* texel** and record its value, so the straight-alpha fringing of §3.7(a) is characterized rather than discovered later. *Verifies Step 0: the most likely thing to be got wrong.*
6. **`viewport_effect_moves_with_view_but_effect_layer_does_not`**: the *screen-level* assertion. A naive "composite is view-independent" test would be vacuous: §2 establishes the composite is produced before `fs_present` applies the view transform, so it is view-independent by construction for every layer kind and cannot fail. Instead assert at the surface: with a `grain` **viewport** effect, panning changes which screen texels the grain lands on; with an equivalent `grain` **effect layer**, the grain pans *with* the artwork. *Verifies the two spaces are actually different, which is the whole premise of §2.*
7. **`perf_scale_factor_output_is_canvas_sized`**: a `painting` effect layer (`perf_scale_factor == 0.7`) produces a composite at full canvas dimensions. *Verifies the downscale→effect→upscale path works on the tree arm.*
8. **`effect_layer_round_trips_through_save_load`**: save a document containing an effect layer naming a veil-only type, reload, assert `pipeline`/`params` survive and `requires.effect` lists the type. *Verifies serialization needs no new machinery (§3.4).*
9. **`effect_catalog_has_no_duplicate_type_ids`**: the effect catalog contains exactly one entry per `type_id`; specifically one `black_and_white` and one `chromatic_aberration`. *Directly asserts the user's reported problem is gone.* (Note: `crate::catalog::catalogs()` does **not** exist, `crates/darkly/src/catalog.rs:194` exposes `settings_catalogs()`. Enumerate through the effect registry or the correct catalog accessor.)
10. **`animated_effect_layer_requests_composite_not_present`**: a `grain` effect layer makes `needs_animation(doc)` true and drives `needs_composite`. *Verifies Step 6's animation wiring.*
11. **`effect_layer_survives_canvas_resize`**: add an effect layer, resize the canvas, composite, assert correct output. *Covers Risk 3 (stale accumulator bind groups), the highest-severity failure mode, which nothing else touches.* Extend the existing `crates/darkly/tests/canvas_resize.rs` rather than adding a file.
12. **`effect_layer_survives_reparenting`**: move an effect layer into and out of a group, compositing each time. *Covers the second invalidation trigger Step 6 introduces (`parent: LayerId` on `EffectLayerState`).*
13. **`effect_layer_scope_follows_passthrough`**: two cases over the same tree shape (a raster below a group, an `invert` effect layer inside the group): with the group **isolated**, assert the outside raster is untouched; with the group **passthrough** (the default), assert the outside raster *is* inverted. *Pins both halves of §4.1's scoping rule. A test covering only the isolated case would pin the rarer configuration and leave the default untested.*
14. **`effect_layer_opacity_and_blend_are_honoured`**: a `black_and_white` effect layer at 50% opacity produces a half-desaturated composite. *Verifies Step 6a (F3); would fail today, since `blend` is never read.*

Extend existing files rather than duplicating:
- `crates/darkly/tests/filters.rs`; retarget to the unified registry; existing assertions must keep passing.
- `crates/darkly/tests/chromatic_aberration.rs`: must keep passing against the single merged CA implementation. This is the sharpest guard that collapsing the duplicate pair did not change behaviour.
- `crates/darkly/tests/picker_preview.rs`, `shader_compile.rs`, `wgsl_validate.rs`, `docs_export.rs`, `schema_contracts.rs`: all enumerate registries and will need the merged catalog id; they double as coverage that the merge is complete.

Frontend (`vitest`, node environment (no DOM globals):
- `frontend/src/ui/__tests__/effect_picker.test.ts`) the picker's entry list has no duplicate `type` values.
- `frontend/src/actions/__tests__/menu_actions.test.ts`: update for the renamed actions.

---

## 7. LOC estimate

Lines **added / removed**, excluding pure relocation. CLAUDE.md treats this as the primary scope signal, so the relocation is called out separately rather than hidden.

| Area | + | − |
|---|---:|---:|
| Traits moved into `gpu/effect.rs`, `encode` widened | 70 | 60 |
| `gpu/effect_registry.rs` (registration, registry, one preview mechanism) | 330 |, |
| `NodeAsAccum` adapter | 80 |, |
| Delete `gpu/veil.rs` + `gpu/filter.rs` registry/preview halves |, | 600 |
| Collapse the two duplicate effect pairs (`.rs`) | 130 | 330 |
| Collapse the two duplicate shader pairs (`.wgsl`) | 20 | 80 |
| `gpu/effect_scaling.rs` extraction from `veil_chain.rs` | 170 | 130 |
| Shader alpha fixes (Step 0) | 20 | 10 |
| `FilterLayer::blend` wiring (Step 6a, F3) | 40 |, |
| `requires` records pipeline ids + `pipeline` validated on load (Step 7, F5) | 15 |, |
| Compositor: `effect_layers`, `compose_effect_arm`, animation, invalidation, `compose_layer` branch removal | 190 | 105 |
| Engine / protocol / save / load | 120 | 110 |
| Frontend: one picker, folder retitle, actions, properties | 190 | 250 |
| **Production subtotal** | **~1,375** | **~1,605** |

**Production net ≈ −230** (about 1,375 added, 1,600 removed).

Note: the shared picker `<style>` block (~60 lines) is *not* counted as a saving here; it is extractable today without any registry merge (§3.6, F10), so crediting it to this work would overstate the payoff.

| | + | − |
|---|---:|---:|
| Tests (14 Rust integration tests + 2 vitest + existing-test retargeting) | ~460 | ~40 |
| Generated (`mod.rs` regeneration, `protocol_gen.ts`) + docs (`CLAUDE.md` repo-layout block, `README.md` Features checklist, a short `docs/effects.md`) | ~200 | ~110 |

**Plus ~4,600 lines of mechanical relocation** (Step 3): 17 `.rs` files totalling ~4,100 lines and ~700 lines of WGSL move directories with header and import edits. Done as a dedicated `git mv` commit this is rename-detected and reviewable; done inline it will read as +4,600/−4,600 and swamp the diff.

### Honest scope assessment

**The feature the user asked for is small; the cleanup it forces is not.** Stacking veils (ask A) is genuinely cheap: §3.1 and §3.4 show the invocation contract, the layer kind, the serialization and the undo all already exist. If ask A alone were acceptable, this would be roughly **300 production lines**.

The remaining ~1,100 lines exist because ask B (no duplicate UI entries) requires one registry, and because the alternative (leaving two registries and having the layer arm branch between them) is forbidden by the Modularity Principle and would give the user a *third* place to find "Chromatic Aberration" rather than one.

### Ship it as three sequenced PRs

An earlier draft argued the registry merge was a *precondition* for stacking, because otherwise the layer arm must branch between two registries. That is only true **if stacking ships first**. Reversing the order dissolves the coupling entirely:

| PR | Contents | Production LOC | Risk |
|---|---|---:|---|
| **1, Alpha correctness** | Step 0 | ~20 / −10 | Very low. Viewport-bit-identical, verifiable against the existing veil path today. De-risks everything after it. |
| **2: Registry + module unification** | Steps 1-4, F6 | ~570 / −1,120 | Medium. Pure de-duplication, **no new user-visible behaviour**. This is exactly ask (B) (the duplicate catalog entries disappear here) and it stands alone. Carries the ~4,600-line relocation as its own `git mv` commit. |
| **3: Effect layers** | Steps 6, 6a, 7, F5, F13 | ~785 / −475 | Medium. The actual new capability, on a foundation two landed PRs already validated. |
| *(4: optional)* | Step 8, layer-kind rename | ~320 churn | Cosmetic. Defer. |

**Ask (B) (the user's stated annoyance) is fully resolved by PR 2**, before any new rendering behaviour exists. That is the sequencing's main virtue: the thing the user actually complained about ships first and independently, and if PR 3 turns out to be harder than estimated, PR 2 still stands on its own merits.

The earlier draft's proposed reduced-scope lever ("defer Step 3's shader/module merge") is the wrong one. It defers the churn while *keeping* the coupling, and reintroduces two registration types for one concept. Sequencing removes both. Deferring **Step 8** remains correct and is already excluded from the table above.

---

## 8. Risks and unresolved questions

**Risks**

1. **Alpha (§3.7a) is the highest-likelihood defect.** Six effects will look correct in the viewport and wrong as layers. Resolved by fixing the shaders (Step 0) rather than declaring a capability; Test 5 covers it, and shipping it as PR 1 means it is validated before anything depends on it. Residual: straight-alpha edge fringing on spatial effects, characterized but not fixed.
2. **Canvas-resolution cost.** A full-canvas pass per effect layer per frame, on a large canvas, with `painting`-class effects, may be slow enough to matter. Mitigated by `perf_scale_factor` + `rendering.effect_layer_scale`, but the ceiling is real and there is no incremental-region escape hatch (§3.3). Recommend measuring `painting` and `watercolor` at 4096² before shipping.
3. **Cache invalidation.** `effect_layers` entries reference accumulator views. Missing an invalidation site produces stale bind groups pointing at freed textures: a class of bug wgpu may or may not catch. Known sites: `set_canvas_rect`, `ensure_group_state`, layer reparenting. Now covered by Tests 11 and 12; the enumeration of sites is still the residual risk, since a test can only catch the triggers it knows to exercise.
4. **PR size** (§7). The relocation must be a separate `git mv` commit or review quality suffers.
5. **`preview_at` cache invalidation through the adapter.** `pixelate` rebuilds its cache mid-preview by answering `false` (`gpu/veil.rs:85-87`, `gpu/veils/pixelate.rs`). The merged preview mechanism must preserve that; `NodeAsAccum` always answers `true`, which is correct for it.
6. **Effects inside groups.** §4.1's scoping is inherited unchanged from today's filter layers, so there is no new behaviour to get wrong, but it is worth being explicit that the **default** (passthrough) case lets an effect reach *past* its group boundary, and only an isolated group confines it. Test 13 pins both halves.

**Unresolved questions**

1. **RESOLVED: `FilterLayer::blend` gets wired up** (§3.7b, Step 6a). The prior art cited for removing it was misread: Krita's `COMPOSITE_COPY` is a *default*, and opacity/blend stay user-settable and honoured (`kis_layer_projection_plane.cpp:73`, `kis_dlg_layer_properties.cc:98-110`). ~40 lines, in PR 3. **Surface to the user as a product call**: it is a behaviour change to an existing layer kind, not just plumbing.
2. **RESOLVED: keep the viewport stack in `.darkly`** (`format/manifest.rs:71`, `engine/save.rs:407-424`). The prior art cited for dropping it is not analogous: `KisDisplayFilter` is a colour-management device profile (`krita/libs/ui/canvas/kis_display_filter.h:31-49`, `program()`, `setupTextures()`, `filter(quint8*, quint32)` and nothing else), and GIMP's are proofing modules (gamma, colour-blindness, clip-warning). Both would be actively *wrong* to carry between machines. Darkly's viewport effects are authored artistic choices with parameters. The "why didn't my grain export?" confusion is a **labelling** problem: fix it at the folder title and in the export dialog, not by discarding the user's work.
3. **OPEN: where does "Add Viewport Effect" live in the menu?** Step 7 proposes moving it from Layer to View. A UX judgment, not a technical one, and plausibly the single highest-leverage change for the confusion the user reported: the menu location is itself the clearest statement of what the thing is. **Surface to the user.**
4. **RESOLVED: group scoping needs no decision.** It is inherited unchanged from today's filter layers and determined by the accumulator model: an effect transforms the accumulator it composites into, and passthrough (the default) decides which one that is (§4.1). No prior art is invoked and none is needed.

Lesser open points, listed for completeness: whether `rendering.veil_scale` and `rendering.effect_layer_scale` should be one key or two; whether "Move to canvas / Move to viewport" ships in this PR or later; and whether `VeilProperties` and `FilterProperties` genuinely merge or only appear to.

---

## 9. Prior-art citations

All line numbers from the checkouts under the project root.
Krita: `/mega/ARTEXP/darkly/krita`, HEAD `e18d0a10e8a5`. GIMP: `/mega/ARTEXP/darkly/gimp`, HEAD `11661ef`, branch `master`.

### Krita: document-embedded effects

- `KisAdjustmentLayer : public KisSelectionBasedLayer`: `krita/libs/image/kis_adjustment_layer.h:25`; base multiply-inherits the config holder, `krita/libs/image/kis_selection_based_layer.h:28`.
- Owns a `KisFilterConfigurationSP` via `KisNodeFilterInterface::m_filterConfiguration`: `krita/libs/image/kis_node_filter_interface.h:50` (accessors `:27`, `:42`); deep-cloned on copy, `krita/libs/image/kis_node_filter_interface.cpp:50-55`.
- Forces copy compositing: `setCompositeOpId(COMPOSITE_COPY); setUseSelectionInProjection(false);`, `krita/libs/image/kis_adjustment_layer.cc:40-41`.
- **It is the only node type that reads the projection below it**: `KisProjectionLeaf::dependsOnLowerNodes()` returns true only for `KisAdjustmentLayer`, `krita/libs/image/kis_projection_leaf.cpp:276-279`; consumed by the merger at `krita/libs/image/kis_async_merger.cpp:226-233`.
- The filtering itself: `KisUpdateOriginalVisitor::visit(KisAdjustmentLayer*)`; `krita/libs/image/kis_async_merger.cpp:62-121`; source is the accumulated projection (`:78`), registry lookup + `filter->process(...)` at `:98,112`, selection blit at `:115-118`, no-config degrades to passthrough at `:84-93`.
- `KisFilterMask : public KisEffectMask, public KisNodeFilterInterface`: `krita/libs/image/kis_filter_mask.h:23`. The scoping distinction is stated in the header: a filter mask "only works on its parent layer, while adjustment layers work on all layers below it in its layer group", `krita/libs/image/kis_filter_mask.h:16-21`. Work hook `decorateRect`: `krita/libs/image/kis_filter_mask.cpp:91-129` (returns the dirtied rect, `:127-128`).
- `KisEffectMask` is a near-empty marker whose only functional line is `using KisMask::apply;`: `krita/libs/image/kis_effect_mask.h:36`. The real contract is `KisMask::apply(projection, applyRect, needRect, maskPos, flags)`: `krita/libs/image/kis_mask.h:187-191`; `applyRect` is what must be written, `needRect` is what is already valid to read (`krita/libs/image/kis_mask.cc:354-355`); `changeRect` is computed by the walkers, not passed in (`kis_mask.cc:386-389`).
- Rect growth for kernel filters: `neededRect`/`changedRect` declared with rationale at `krita/libs/image/filter/kis_filter.h:81-93`, identity defaults at `krita/libs/image/filter/kis_filter.cc:91-103`. Blur grows need by ×2 (`krita/plugins/filters/blur/kis_blur_filter.cpp:105-114`) but change by ×1 (`:116-125`); Gaussian identical (`kis_gaussian_blur_filter.cpp:83-96` vs `:98-108`). `KisFilter::process` sizes its temporary from `neededRect` (`kis_filter.cc:51,64`) and writes back only `applyRect` (`:87`). Walker forward/backward passes: `krita/libs/image/kis_base_rects_walker.h:317-334` and `:351-433`; the merger allocates temp projections when `walker.needRectVaries()`, `krita/libs/image/kis_async_merger.cpp:175`, `:213`, `:249`.
- `.kra` serialization: node type string `ADJUSTMENT_LAYER = "adjustmentlayer"`, `krita/plugins/impex/libkra/kis_kra_tags.h:37`. XML writes only name + filter name + version (`krita/plugins/impex/libkra/kis_kra_savexml_visitor.cpp:195-208`, refusing a filterless layer at `:197-199`); params go to a side file via `saveFilterConfiguration` → `filter->toXML()` (`krita/plugins/impex/libkra/kis_kra_save_visitor.cpp:476-497`, core `:490-493`), internal selection at `:439-472`. Load dispatches on the string (`krita/plugins/impex/libkra/kis_kra_loader.cpp:967-968`), builds a default config with a null selection (`:1184-1230`, note `:1227`), and a second pass fills it in (`krita/plugins/impex/libkra/kis_kra_load_visitor.cpp:237-278`, esp. `:267-274`). Deprecated filter ids are remapped at load (`kis_kra_loader.cpp:1201-1211`).

### Krita: view-level display filters, and their separation

- `KisDisplayFilter : public QObject`: `krita/libs/ui/canvas/kis_display_filter.h:31-49`; pure virtuals are `program()` (GLSL source), `setupTextures()`, `filter(quint8*, quint32)`, and the approximate transforms. **No node, no selection, no rect API.** Class doc at `:27-30`.
- GPU path: canvas forwards to the renderer (`krita/libs/ui/opengl/kis_opengl_canvas2.cpp:189-192`); the renderer recompiles the display shader (`krita/libs/ui/opengl/KisOpenGLCanvasRenderer.cpp:318-319`, `:1045-1046`) and binds LUTs per frame (`:845-846`). The filter injects shader *text*: `fragHeader.append("#define USE_OCIO\n"); fragHeader.append(displayFilter->program()...)`: `krita/libs/ui/opengl/kis_opengl_shader_loader.cpp:151-155`, applied last at `krita/krita/data/shaders/highq_downscale.frag:120-124`.
- Separation is structural: `grep -rn "displayFilter\|DisplayFilter" libs/image/` → **zero hits**. Ownership is per-canvas (`krita/libs/ui/canvas/kis_canvas2.cpp:913-923`); persistence is in `kritarc`, not the document (`krita/libs/ui/kis_config.cc:1975-1992`); nothing OCIO-related appears in the `.kra` impex code.
- **No conversion path exists** between `KisDisplayFilter` and `KisFilter` in the node factories, the kra saver/loader, or `libs/image`. The only construction site is the LUT docker handing one to the canvas: `krita/plugins/dockers/lut/lutdocker_dock.cpp:374-400`.
- **A claim an earlier draft made here is false and is retracted.** It asserted that Krita "pays for" the separation with duplicated ASC-CDL math between `KisFilterASCCDL` (`krita/plugins/filters/asccdl/kis_asccdl_filter.h:21`) and `OcioDisplayFilter` (`krita/plugins/dockers/lut/ocio_display_filter_vfx2021.h:56`). It does not. The only CDL implementation in the tree is `krita/plugins/filters/asccdl/kis_asccdl_filter.cpp` (`KisASCCDLTransformation::transform`, `qPow((normalised[c]*m_slope[c])+m_offset[c], m_power[c])`); `ocio_display_filter_vfx2021.cpp:70-85` is pure delegation (`m_processorCPU->apply(img)`) with no CDL code, and a repo-wide `grep -rn "CDLTransform\|asccdl"` outside `plugins/filters/asccdl/` returns zero. **Krita's separation is not demonstrably costing it duplication**, and this plan does not claim otherwise (§2).

### GIMP: non-destructive filters and the filter/layer unification

- **`GimpFilterLayer` does not exist in this checkout.** Repo-wide grep for `GimpFilterLayer` / `gimp_filter_layer` / "adjustment layer" across `.c`/`.h`/`.md` returns zero matches; the only layer subclasses present are `gimplayer.c`, `gimpgrouplayer.c`, `gimplinklayer.c`, `gimptextlayer.c`.
- Adjustment-layer-like behaviour is instead: `GimpDrawableFilter` on a drawable's own `filter_stack`, spliced into `gimp_drawable_get_source_node` (`gimp/app/core/gimpdrawable.c:1777-1785`) i.e. strictly upstream of that layer's mode node, affecting only that layer. "Affect everything below" is done by grouping and filtering the group (`gimp/app/core/gimpgrouplayer.c:306`, `:1159-1165`). Filters on pass-through groups are explicitly discouraged: `gimp/app/core/gimpgrouplayer.c:1249-1255`.
- `GimpDrawableFilter : GIMP_TYPE_FILTER`: `gimp/app/core/gimpdrawablefilter.c:183`; instance struct (GeglNode, mask, opacity, paint mode, blend/composite space, region, clip, crop, applicator) at `:81-123`. Graph construction `:375-465`, with the op's output connected to the applicator's `"aux"` pad at `:455-463`. Attachment via `gimp_drawable_add_filter`: `gimp/app/core/gimpdrawable-filters.c:153-156` (note: `gimp_drawable_append_filter` does not exist under that name in this checkout). Non-destructive commit keeps the filter on the stack: `gimp/app/core/gimpdrawablefilter.c:1246-1305`, esp. `:1282-1292`.
- **The unification, which is the load-bearing prior art for §4:** `GimpFilter` is a derivable `GimpViewable` whose entire contract is one signal and one `get_node` vfunc, `gimp/app/core/gimpfilter.h:26-41`. Its only two direct subclasses are `GimpItem` (`gimp/app/core/gimpitem.c:182`) and `GimpDrawableFilter` (`gimpdrawablefilter.c:183`), so **`GimpLayer` is-a `GimpFilter`** via `GimpDrawable` (`gimp/app/core/gimpdrawable.c:227`) and `GimpLayer` (`gimp/app/core/gimplayer.c:279`), both overriding `get_node` (`gimpdrawable.c:307`→`:519-542`; `gimplayer.c:410`→`:794-830`). And **the layer stack is a filter stack**: `GimpItemStack : GimpFilterStack` (`gimp/app/core/gimpitemstack.c:47`) → `GimpDrawableStack` (`gimpdrawablestack.c:63`) → `GimpLayerStack` (`gimplayerstack.c:59`). `GimpFilterStack::get_graph` chains every *active* filter tail→head between input and output proxies: `gimp/app/core/gimpfilterstack.c:187-227`.
- Region/bounds: GIMP does **not** call `gegl_node_invalidated` anywhere (repo-wide grep: no matches). Bounding box comes from GEGL (`gimp/app/core/gimpdrawable.c:1096-1100`), diffed and re-dirtied in `gimp_drawable_update_bounding_box` (`:1817-1878`); parameter changes invalidate the **whole** filter area, not a sub-rect (`gimp/app/core/gimpdrawablefilter.c:1911-1956`); recomputation of what a blur needs is delegated to GEGL's own `get_required_for_output` via `gegl_node_blit` (`gimp/app/gegl/gimptilehandlervalidate.c:241-244`).
- Display filters are separate: `GimpColorDisplay` in `libgimpwidgets` (`gimp/libgimpwidgets/gimpcolordisplay.c:93`, vfunc dispatch `:318-334`), stacked in a `GimpColorDisplayStack` on the shell (`gimp/app/display/gimpdisplayshell-filter.c:42-69`), applied to an already-rendered buffer during render (`gimp/app/display/gimpdisplayshell-render.c:372`). Concrete filters are loadable modules iterating raw buffers, not GEGL ops (`gimp/modules/display-filter-gamma.c:132`, `:184-200`). **Zero shared code paths with `GimpDrawableFilter`.**
- XCF calls them "effects": version bumped to 22 when any layer has filters, `gimp/app/core/gimpimage.c:3010-3020`. Save writes name, icon, GEGL op name, op version, then each GEGL property individually as `PROP_FILTER_ARGUMENT`, plus the mask as a full channel: `gimp/app/xcf/xcf-save.c:2234-2302`, props at `:872-978`, type tags at `gimp/app/xcf/xcf-private.h:107-117`. Load **discards a filter whose stored op version mismatches** (`gimp/app/xcf/xcf-load.c:4221-4237`) and materializes with a non-destructive commit at `:1653-1713` (esp. `:1702`).
- UI: not tree sub-rows and not separate layers, a toggle column in the layer row (`gimp/app/widgets/gimpdrawabletreeview.c:197`, `:231-249`, state from `filters-changed` at `:581-602`) opening a modal popover titled **"Layer Effects"** containing a container list over the drawable's filter stack (`gimp/app/widgets/gimpdrawabletreeview-filters.c:156-175`, `:310-312`). `gimplayertreeview.c` contains zero occurrences of "filter". Rows are `GimpRowDrawableFilter`, possible because `GimpFilter` is a `GimpViewable` (`gimp/app/widgets/gimprow-utils.c:50-57`).
