# Unified effect scale

## Independent Review

Reviewed against the repository at `better-veils`. Every claim below was checked
against source. The standing decision (one scale, both spaces) is not
relitigated; what follows challenges *how* the plan implements it.

### Verified correct

- **(B) fingerprint gap — confirmed.** `structural_match` (`crates/darkly/src/gpu/compositor.rs:4723-4728`)
  compares exactly `pipeline_id`, `space`, `render_size`, `target_generation`.
  For a canvas instance `render_size` is the parent accumulator's dimensions
  (`compositor.rs:4705`), which a scale change does not move. `ScreenRun::scale()`
  (`gpu/screen_run.rs:102-104`) has exactly one consumer, `compositor.rs:4714`;
  `screen_scale()`/`canvas_scale()` have exactly three (`screen_run.rs:66,127`,
  `compositor.rs:4706`). The ownership move is clean and nothing else breaks.
- **(B) wake-up gap — confirmed, and both call sites are necessary.**
  `sync_resolution_scale` is called only from `Compositor::render`
  (`compositor.rs:5608`); `render_offscreen` early-returns at `!needs_composite`
  (`compositor.rs:3916`). `DarklyEngine::render` returns before ever touching the
  compositor in headless mode (`engine/rendering.rs:696-706`). `config_set`
  (`frontend/wasm/src/config_bridge.rs:29-45`) and `ConfigStore.set`
  (`frontend/src/config/store.svelte.ts:156-163`) neither hold an engine handle
  nor request a frame, so polling is indeed the only mechanism. Note for the
  implementer: the `render` call site cannot be dropped in favour of the
  `render_offscreen` one, because `render` early-returns on `!has_pending_work`
  (`compositor.rs:5612`) *before* reaching `render_offscreen`. The plan's two
  call sites are both load-bearing.
- **`mark_dirty()` is sufficient.** `render_offscreen` → `sync_projection_states`
  (`compositor.rs:3937`) → `sync_effect_instances` (`compositor.rs:4542`), so
  setting `needs_composite` does reach the rebuild. It is mildly over-broad —
  `mark_dirty` also does `content_bounds.invalidate_all()` (`compositor.rs:2208`),
  and for a *screen-space-only* scale change it forces a full canvas recomposite
  that the old code did not (the old code bumped `target_generation` + set
  `needs_present`). On a config edit that is fine; flag it in the method's doc
  comment rather than working around it.
- **Prior art — all spot-checked citations hold.** `kis_filter.cc:24`
  (`m_supportsLevelOfDetail(false)` in the ctor) and `:105-115`;
  `kis_stroke_strategy.cpp:124-128` (`createLodClone` returns 0);
  `kis_filter_stroke_strategy.cpp:371-379`; `kis_canvas2.cpp:1386-1404`;
  `kis_paint_device.cc:596` (`m_lodData`) and `:521-533` (`currentData`);
  `kis_strokes_queue.cpp:302-306` (`setLodBuddy`); the opt-out trio
  (`kis_unsharp_filter.cpp:44`, `KisResetTransparentFilter.cpp:38`,
  `KisPropagateColorsFilter.cpp:32`) and the opt-in list. GIMP:
  `gimpdrawablefilter.c:19-21`, `:787-800`, `:1246-1281`; `gimpprojection.c:303`;
  `gimpzoompreview.c:900-926`. No citation failed. Worth adding: the comment
  immediately above `kis_unsharp_filter.cpp:44` says LoD devices "can still appear
  when the filter is used in Adjustment Layer" and opts out *anyway* — i.e. Krita
  faced precisely this plan's situation (a reduced-resolution adjustment layer)
  and let the filter refuse. That is directly on point for finding 2.
- **(C) — the recommendation is right, and the LOC pricing is roughly right.**
  `sync_effect_instances` builds from the registry
  (`compositor.rs:4744-4749` → `EffectRegistry::instance`, `gpu/effect.rs:570-580`,
  which calls `from_params` and therefore discards everything not in the params);
  `rainy_glass.rs:118` (`time: 0.0`) and `grain.rs:98` (`frame_count: 0.0`)
  confirm the clocks reset. `grain::create_cache` (`gpu/effects/grain.rs:177-255`)
  does allocate two render-sized RGBA8 textures and upload a CPU `vec![0u8; w*h*4]`
  of PCG noise into both. Autosave 120 s / recorder 1.5 s / `recording.enabled: true`
  confirmed at `crates/darkly/presets/defaults.yaml:119-125`; export
  (`engine/export.rs:39`), save (`engine/save.rs:146`) and the recorder
  (`engine/process_recording.rs:275`) all route through `render_offscreen`. Two
  qualifications in finding 6 below.
- **CONFIG_VERSION deviation — accepted.** `validateOverrides`
  (`frontend/src/config/validate.ts:31-50`) drops unknown keys with a warning and
  the caller rewrites the cleaned file (`store.svelte.ts:86-98`); a version
  mismatch discards the whole file including every `hotkeys.*` key. Not bumping is
  correct. But `config/mod.rs:18-26` is self-contradictory as written — it lists
  "a pref key is renamed" as requiring a bump *and* says "removed pref keys are
  dropped by `validateOverrides`" as not requiring one. **Add to the plan:** amend
  that doc comment so it says a rename needs a bump only when the old value must
  be carried across, otherwise the next person re-derives this argument from
  scratch. That is one or two lines, and it is the difference between a documented
  rule and a documented rule with a silent exception.

### Findings requiring revision

**1. The straight-alpha hazard is real, but the plan mis-states what is left over
(high).** Verified: `shaders/composite.wgsl:53-55` divides by `max(out_a, 0.001)`,
so accumulators hold straight alpha and fully transparent texels hold RGB 0;
`shaders/downscale.wgsl:44-48` takes an unweighted mean; `shaders/in_place_apply.wgsl:92`
is an exact passthrough at opacity 1 / mask 1 / Normal. The dark-fringe conclusion
follows and the alpha-weighted average is the right mitigation — the codebase
already contains that exact idiom, at `shaders/lib/aberration.wgsl:79-84`
(`acc_rgb += s.rgb * s.a` … `acc_rgb / max(acc_a, CA_EPS)`), with a header comment
saying why. Cite it in the new `downscale.wgsl` comment rather than re-deriving.

Two corrections:

- The residual is **not** only the magnification. Each of the four taps is a
  *bilinear* sample (`compositor.rs:977-984` — the shared sampler is
  `FilterMode::Linear`), so the hardware already unweighted-averages up to four
  straight-alpha texels *inside* each tap before the shader ever sees it. At
  scale 0.7071 the taps sit ±0.35 texel from centre, so intra-tap contamination is
  the same order as the inter-tap error the fix removes. Alpha-weighting the four
  taps roughly halves the artifact; it does not remove it. Either say so, or make
  the taps `textureLoad`-based with explicit weights (same pass, no extra
  textures) and be actually correct.
- The claim that fixing the magnification residual "needs premultiplied
  intermediates with un-premultiply passes around the effect … two more
  full-resolution passes per effect per frame" is wrong. `ScalingPipelines::upscale`
  is its own `EffectPipeline` (`gpu/effect_scaling.rs:61`) sharing only the
  bind-group layout, so it can be given a dedicated shader that does a manual
  alpha-weighted bilerp from four `textureLoad`s — one pass, no extra textures,
  ~15 lines. That is the same order of cost as the downscale fix the plan already
  accepts, and it closes the residual it declares unaffordable.

**2. The unification is applied to effects that cannot benefit from it, and this
must be settled before implementation, not deferred (high).** This is not a
challenge to "the scale applies in both spaces" — it is a challenge to "the scale
applies to *every effect*". `perf_scale_factor` (`gpu/effect.rs:314`) already
exists as the per-effect declaration and every shipped effect except `painting`
leaves it at 1.0, so today the question has never been asked in canvas space.
After this plan it is asked of all fourteen. I audited them:

- Scale-invariant by construction (radius as a fraction of `sqrt(area)`):
  `lens_blur` (`shaders/effects/lens_blur.wgsl:37-40,74-77`), `frozen`
  (`frozen.wgsl:42-45,64-66`), `rainy_glass` (`rainy_glass.wgsl:174-175,189-192`).
  `vhs` works in UV throughout. The plan's `lens_blur` claim checks out.
- Scale-**variant**, and the plan lists only one of them:
  - `pixelate` — correctly flagged (`gpu/effects/pixelate.rs:96-99`).
  - `grain` — **missed**. The noise state textures are render-sized and sampled
    1:1 in UV (`gpu/effects/grain.rs:195-196,218`, `shaders/effects/grain.wgsl:73-75`),
    so grain frequency is tied to render resolution: at 0.7071 the grain becomes
    1.41× coarser and is then bilinearly magnified. Grain size becoming a function
    of a performance preference is a real quality decision.
  - `chromatic_aberration` — **missed**. `offset_px` / `blur_px` are consumed as
    `ab.offset_px / dims` where `dims = textureDimensions(tex)`
    (`shaders/lib/aberration.wgsl:112-114`) and nothing normalizes them by
    resolution CPU-side (`gpu/effects/chromatic_aberration.rs:219-221`). At 0.7071
    every aberration is 1.41× stronger than the user dialled.
  - `painting` — kernel taps are in texels (`shaders/effects/painting.wgsl:41,68`),
    so its footprint also grows relative to the image; that is deliberate and
    already priced into its 0.7.
- The sharpest case, which the plan does not raise at all: the pure per-pixel
  colour operators — `invert`, `curves`, `levels`, `hsv`, `black_and_white`,
  `brightness_contrast`. These are mathematically scale-invariant, so the reduced
  round trip buys nothing and costs on both axes. Cost: `Full` is one full-res
  1-tap pass; `Reduced` at 0.7071 is a 4-tap pass at 0.5 area, plus the effect at
  0.5 area, plus a full-res blit — call it 3.5× the sampling work, *plus* two new
  textures per instance (on a 4000×4000 document, ~64 MB per canvas effect
  layer — a VRAM cost the plan's risk list does not mention). Quality: because
  `in_place_apply` replaces `after` wholesale including `after.a`
  (`in_place_apply.wgsl:92`), the round trip resamples the **alpha channel** of
  everything below the effect, not just its colour. A curves layer over a
  transparent-backed document would feather every silhouette edge by ~1.4 px, and
  that ships into the export. Making the document's geometry a function of a perf
  slider is a different and larger claim than "effects are softer".

  Unresolved question 1 should therefore be answered *in this plan*, in the
  direction its own prior art points: `fn scales_with_resolution(&self) -> bool { true }`
  on `Effect`, overridden `false` in `pixelate.rs` and in the per-pixel colour
  operators, consulted in `effective_scale`. The plan itself prices this at ~6
  production lines. Krita's `supportsLevelOfDetail` defaults to *false* and three
  shipped filters opt out; Darkly can keep the opposite default and still let an
  effect refuse. Without it, "one knob, both spaces" also means "a knob that makes
  the cheapest effects slower and blurrier", which is not the trade the owner
  agreed to.

**3. The steady-state guard the plan leans on does not exercise the code path
(medium).** `effect_space.rs::effect_instances_are_not_rebuilt_every_frame`
(`crates/darkly/tests/effect_space.rs:502-546`) uses a headless engine, and
`DarklyEngine::render` returns at `engine/rendering.rs:696-706` without touching
the compositor when there is no surface. Neither `fill_layer` nor the painting
loop calls `render_offscreen` (the only engine-side callers are export, save,
preview, the recorder, `test_readback_canvas`, and the `sample_merged` branch at
`engine/painting.rs:983-986`). The painting phase of that test therefore never
reaches `sync_effect_instances`, and its assertion is very likely comparing 0
against 0. It cannot guard `sync_effect_scale` against dirtying every frame. Add
an explicit test that does: call `test_readback_canvas()` twice with no mutation
in between and assert `test_effect_rebuilds()` is unchanged *and* that the second
`render_offscreen` performed no work (it returns `bool`). While there: verify the
existing test still means what it claims, or say plainly in the plan that it does
not.

**4. `sync_effect_scale` can loop if an instance exists but cannot be rebuilt
(medium).** The plan calls the method "idempotent and self-terminating: once
`sync_effect_instances` rebuilds, drift is zero." That holds only when the rebuild
is reachable. `sync_effect_instances` has three `continue`s that skip an
*existing* instance before `structural_match`: missing parent group state
(`compositor.rs:4698-4703`), no screen-run views (`:4710-4713`), and zero size
(`:4717-4719`). The reachable one is the second: `ScreenRun::resize(0, 0)` drops
the textures (`screen_run.rs:138-147`) and `ensure_resources` refuses to recreate
them at zero size (`screen_run.rs:161-164`), while `effect_instances` retains the
screen entries. In that state a scale change would mark dirty on every frame
forever, and `has_pending_work` keeps the frame loop alive. Either compare drift
only against instances the sync can actually reach, or drop instances whose space
has no resources. One or two lines, but it belongs in the plan.

**5. The two regression tests cannot "fail first" as written (medium,
procedural).** Both are specified against `rendering.effect_scale`, a key that
does not exist until step 1 renames it, so run against unfixed code they fail with
a missing/defaulted config key rather than for the diagnosed reason — which is not
a regression demonstration under CLAUDE.md's Testing Principle. The accessors are
separable from the fix (step 8 touches nothing the fix touches), so the honest
sequence is: add `test_effect_reduced_size` + both tests written against
`rendering.canvas_effect_scale`, demonstrate the two distinct failures, *then* do
steps 1-7 and rename the key in the tests. State that sequence in the plan. The
claim that test 1 isolates the fingerprint and test 2 the wake-up is otherwise
correct: painting a dab does set `needs_composite`, so test 1 reaches
`sync_effect_instances` and still fails on `structural_match`, while test 2 fails
earlier at `compositor.rs:3916`.

**6. (C): right answer, partly over-argued (low).** Two of the four runtime
arguments are weaker than presented.

- "It is not a rare path" is a consequence of the plan's own choice to wrap save,
  the recorder and `bake_subtree_to_layer` as well as export. The minimal shape a
  proponent would actually propose is export-only (`engine/export.rs:39`), which
  is one call site, is user-initiated, and never meets the recorder's 1.5 s
  cadence. Argue against *that* shape, or the argument reads as defeating a
  strawman.
- "Animated effects visibly reset" is presented as inherent to a rebuild. It is
  inherent to *this* rebuild: `sync_effect_instances` goes to the registry
  (`compositor.rs:4744-4749`), but `Effect::clone_boxed` (`gpu/effect.rs:272`)
  exists and `RainyGlass`/`Grain` both derive `Clone` with their clocks as fields.
  Reusing `inst.effect.clone_boxed()` when `pipeline_id` matches would preserve
  animation state across any rebuild — one line, and it also retires this plan's
  own listed risk "Animated effects reset on a scale change" (which under (A)+(B)
  fires every time the user drags the new slider, on both spaces at once). Worth
  folding into (A) regardless of (C).

Even so the verdict stands on the criterion the owner set: ≈30 production lines is
not "a few lines", and export-only is still ~20 plus a mode flag with no owner in
the document/session/compositor taxonomy. Ship (A)+(B); export inherits the scale.

**7. Bake finding (item 8 in the risk list) is a real, user-visible correctness
bug — state it as such (high, out of scope but must not be soft-pedalled).**
Confirmed end to end. `bake_subtree_to_layer` composes into a sentinel `GroupState`
keyed by `LayerId::from_ffi(0)` (`compositor.rs:3821-3833`) and calls
`compose_children(..., bake_parent, ...)` (`compositor.rs:3869`), which builds a
`CompositionContext { parent_group: bake_parent }` and dispatches
`compose_effect_arm(parent_group = bake_parent)` (`compositor.rs:340-354`,
`:5176-5257`). The instance it looks up was prepared against
`EffectSpace::Canvas { parent: doc.accumulator_host_of(id) }`
(`compositor.rs:4681-4683`) — for a top-level effect that is the *root's*
accumulator — so `inst.scaled.encode(...)` reads the root accumulator's views
while `apply_in_place` writes the bake accumulator. Concretely: flatten a document
that is one red raster with an `invert` effect above it. The live composite is
cyan and lives in the root accum. The bake composites red into the bake accum,
then runs `invert` over the *root* accum (cyan) into the scratch, and the apply
pass replaces the bake accum with the result — red. Flatten returns the
un-inverted image. Merge Down with an effect layer as source
(`engine/merge.rs:106`) is the same defect. And when no live render has happened
(headless, or a freshly loaded document) there is no instance at all, so
`compose_effect_arm` returns at `compositor.rs:5188-5190` and the effect is
silently dropped from the bake instead.

`crates/darkly/tests/layer_bake.rs:612-650` covers only *screen-space* effects
surviving flatten; nothing asserts that flattening a canvas-space effect layer
reproduces the composite. Agreed that fixing it is out of scope here, but the
plan should (a) call it a confirmed bug rather than "this looks wrong", (b) record
the two-line reproduction above so the follow-up plan starts from a failing test,
and (c) note that it is aggravated by this plan, not merely adjacent: once canvas
instances carry reduced-resolution scaffolding, the bake will also run that
scaffolding against the wrong accumulator.

**8. Collision with `docs/plans/composite-prefix-cache.md` is larger than the
plan's section admits (medium).** That plan's step 9 (`composite-prefix-cache.md:510`)
**deletes** `cache_valid_through` and its four assignments, replacing the
invalidation channel with a `composite_epoch` bumped inside `mark_dirty`
(`:226`, `:495`). The invalidation argument here survives — `mark_dirty` still
invalidates everything — but the *justification text* in this plan ("clears every
group's `cache_valid_through`, `compositor.rs:2201-2209`", and the whole
"Interaction with composite-prefix-cache" section) goes stale the moment that plan
lands, and the recommendation it makes to the prefix cache is already what the
prefix cache does. Rewrite that section in terms of `mark_dirty`'s contract, not
its current implementation. Two harder conflicts to sequence explicitly:

- That plan routes all six `target_generation += 1` sites through a new
  `bump_target_generation()` and enumerates `:5609` among them
  (`composite-prefix-cache.md:267-268`, `:501-502`) — the exact line this plan
  deletes. Whichever lands second must adjust.
- Both plans edit the head of `render_offscreen` (`compositor.rs:3910-3918`) and
  the `structural_match` block (`:4723-4728`, which that plan cites at `:371-372`
  as its template and describes as a five-field compare — it is four today and
  becomes five here). Textual conflicts, not semantic ones, but they should be
  merged deliberately.

### Smaller notes

- `defaults.yaml:131-132`, `config/sections/rendering.rs:3-6,9,22`,
  `effect_scaling.rs:11-14,30-38,74-77,108-138`, `compositor.rs:548-569,3809-3812`,
  `engine/mod.rs:1108,1169`, `screen_run.rs:5-6,19,41,102-134`,
  `gpu/effects/painting.rs:135`, `gpu/effect.rs:314` — all quoted line references
  in the plan land where it says they do.
- Add the VRAM cost of unification to Risks: two reduced textures per canvas
  effect instance where today there are none (`effect_scaling.rs:188-209`).
- Prefer an epsilon comparison for the new `applied_scale` clause in
  `structural_match`. Both sides come from the same `effective_scale` call so `==`
  would work today, but an exact float compare in a fingerprint is a trap for the
  next person who introduces a second path to the value.
- `render` calling `sync_effect_scale` and then `render_offscreen` calling it
  again is a harmless double poll; worth one word in the doc comment so it does
  not read as an oversight.

### Verdict: revise

The diagnosis is correct, the ownership move is the right one, and the (C) and
CONFIG_VERSION calls are sound. Revision is needed for: the per-effect refusal
(finding 2) settled in-plan rather than deferred; the corrected characterization
of the straight-alpha residual and the cheaper upscale fix (finding 1); a real
steady-state guard (finding 3); the drift-loop edge case (finding 4); the
fail-first sequencing for the regression tests (finding 5); and the bake bug
restated as a confirmed defect with a reproduction (finding 7). None of these is a
rethink — the shape of the fix survives all of them.

## Revision

Every review finding is dispositioned below. Findings accepted are folded into the
body sections; the one rejection is an owner ruling, recorded with its
consequences rather than argued.

**1. Straight-alpha residual — accepted, and the scope grows slightly.** The
review is right on both corrections. The shared sampler is `FilterMode::Linear`
(`compositor.rs:977-984`), so each of the four downscale taps already
unweighted-averages up to four straight-alpha texels in hardware before the
shader sees them; alpha-weighting the taps halves the artifact rather than
removing it. And the claim that fixing the upscale side costs "two extra
full-resolution passes" was wrong — `ScalingPipelines::upscale` is its own
`EffectPipeline` (`effect_scaling.rs:61`) sharing only a bind-group layout, so it
can take a dedicated shader doing a manual alpha-weighted bilerp from four
`textureLoad`s in the same pass. Both fixes are in scope: the downscale becomes
`textureLoad`-based with explicit weights (not bilinear taps), and the upscale
gets the matching shader. The idiom to cite in both headers is
`shaders/lib/aberration.wgsl:79-84`. Adds ~15 production lines over the original
estimate and closes the residual the plan had declared unaffordable.

**2. Per-effect refusal — rejected by owner ruling.** The reviewer's audit is
factually correct and is preserved above; the proposed `scales_with_resolution()`
opt-out is not adopted. The owner's ruling: *the effect scale is a global
setting and applies to every veil; it is not an effect-specific setting.* No
`Effect` trait method is added and no effect opts out. `perf_scale_factor` stays
exactly as it is — a per-effect cost declaration that composes with the global
scale, not a veto.

The consequences are accepted deliberately, and are recorded here so they are not
rediscovered as bugs:

- `grain`'s noise state textures are render-sized and sampled 1:1
  (`gpu/effects/grain.rs:195-196,218`, `shaders/effects/grain.wgsl:73-75`), so
  grain frequency tracks the scale — at 0.7071 the grain is 1.41× coarser.
- `chromatic_aberration` consumes `offset_px / textureDimensions(tex)`
  (`shaders/lib/aberration.wgsl:112-114`), so every aberration is 1.41× stronger
  than the dialled value.
- `pixelate` computes its blocks on the reduced texture
  (`gpu/effects/pixelate.rs:96-99`), which softens the `soft: false` hard-edge
  option.
- The per-pixel colour operators (`invert`, `curves`, `levels`, `hsv`,
  `black_and_white`, `brightness_contrast`) pay the round trip for no
  proportional win: ~3.5× the sampling work of a single full-resolution pass,
  plus two reduced textures per instance.

All four behaviours are already live in screen space today; unification makes
them uniform rather than introducing them. The alpha-resampling concern the
reviewer raises alongside them (`in_place_apply.wgsl:92` replacing `after.a`
wholesale, feathering silhouettes) is a real cost and is what finding 1's
down- *and* up-scale fixes exist to minimize; it is not a reason to exempt any
effect.

**3. Steady-state guard — accepted.** `effect_space.rs::effect_instances_are_not_rebuilt_every_frame`
cannot guard `sync_effect_scale`: headless `DarklyEngine::render` returns at
`engine/rendering.rs:696-706` without touching the compositor, so that test's
painting loop never reaches `sync_effect_instances`. Added to the test list:
`a_steady_frame_does_not_rebuild_or_redirty` — call `test_readback_canvas()`
twice with no mutation between, assert `test_effect_rebuilds()` is unchanged and
that the second `render_offscreen` returns `false` (it already reports whether it
did work). The implementation must also state plainly in the existing test's doc
comment what it does and does not cover, rather than leaving a test that reads as
a guard it is not.

**4. Drift loop — accepted.** `sync_effect_scale` must compare drift only against
instances `sync_effect_instances` can actually reach, or the three `continue`
paths (`compositor.rs:4698-4703`, `:4710-4713`, `:4717-4719`) leave an instance
permanently drifted and mark dirty every frame forever. The reachable case is a
0×0 viewport (`screen_run.rs:138-164`) with screen entries retained. Fix: skip
instances whose space currently has no resources. The method's doc comment states
the termination argument explicitly instead of asserting idempotence.

**5. Fail-first sequencing — accepted.** Both regression tests were specified
against `rendering.effect_scale`, which does not exist until the rename, so they
would fail for the wrong reason. Corrected sequence, stated in the test section:
add `test_effect_reduced_size` and both tests written against
`rendering.canvas_effect_scale`; demonstrate the two distinct failures
(fingerprint, then wake-up); then perform steps 1-7 and rename the key inside the
tests. Step 8 touches nothing the fix touches, so the accessors are safely
separable.

**6. (C) — verdict unchanged, one argument retracted, one improvement adopted.**
The "not a rare path" argument is retracted: it depends on this plan's own choice
to wrap save and the recorder, and the minimal proponent shape is export-only,
which is user-initiated and never meets the recorder's cadence. The verdict still
stands on the owner's stated criterion — export-only is still ~20 production
lines plus a mode flag with no owner in the document/session/compositor taxonomy.

Adopted into (A) regardless of (C): preserve animation state across rebuilds by
reusing `inst.effect.clone_boxed()` (`gpu/effect.rs:272`) when `pipeline_id`
matches, instead of always going to the registry (`compositor.rs:4744-4749`).
`RainyGlass` and `Grain` both derive `Clone` with their clocks as fields, so this
retires the plan's own "animated effects reset on a scale change" risk — which
under (A)+(B) would otherwise fire on every drag of the new slider, in both
spaces at once. ~2 production lines.

**7. Bake defect — confirmed real, but the reproduction is different from the
review's.** The review's specific claim was tested directly and does *not*
reproduce. Flattening a 16×16 document of one red raster with a canvas-space
`invert` above it returns cyan (correct). `merge_down` of the effect returns cyan
(correct). `flatten_node` of a group holding raster + effect, with a second
visible layer outside the group so the bake and root accumulators genuinely
differ, returns cyan (correct). The wrong-accumulator read is evidently masked on
these paths.

What does reproduce, and is a real defect with data loss:

```
canvas 16×16; raster filled red; canvas-space `invert` added;
NO composite has run (no render_offscreen, so no effect instance exists)
flatten_image() → composite reads [0, 0, 0, 0]
```

The result is not "the effect was skipped" — it is an empty image. The control
without the effect layer returns `[255, 0, 0, 255]` correctly, so the unrealized
effect destroys the source pixels rather than merely being dropped at
`compositor.rs:5188-5190`. Whether a cold flatten is reachable in the shipping
app (where a frame always renders before the user can invoke Flatten) is **not
established** and is the first question the follow-up plan must answer.

Still out of scope here, and still aggravated by this plan: once canvas instances
carry reduced-resolution scaffolding, the bake runs that scaffolding too.
`crates/darkly/tests/layer_bake.rs:612-650` covers only screen-space effects and
asserts tree structure, never pixels — so no existing test would catch any of
this. Recorded for a separate plan, which should start from the failing cold-flatten
case above.

**8. Prefix-cache collision — accepted.** The "Interaction with
composite-prefix-cache" section is rewritten in terms of `mark_dirty`'s
*contract* ("a mutation that marks dirty invalidates every derived cache") rather
than its current implementation, so it does not go stale when that plan deletes
`cache_valid_through`. Two sequencing conflicts recorded for whichever lands
second: that plan routes `compositor.rs:5609` through a new
`bump_target_generation()` while this plan deletes that line, and both plans edit
the head of `render_offscreen` and the `structural_match` block.

**Smaller notes — all accepted.** Epsilon comparison rather than `==` for the new
`applied_scale` clause in `structural_match`; the double poll (`render` then
`render_offscreen`) documented as intentional; the VRAM cost of unification added
to Risks (two reduced textures per canvas effect instance where today there are
none, `effect_scaling.rs:188-209`); and the `config/mod.rs:18-26` doc comment
amended so the rename-versus-removal rule states its own exception instead of
contradicting itself.

### Revised LOC estimate

The original estimate was production ≈ +115 / −65. Revisions add: the
`textureLoad`-based downscale and the new alpha-weighted upscale shader (~+15,
replacing the ~+9 already budgeted), `clone_boxed` reuse (~+2), the drift-loop
guard (~+3), the `config/mod.rs` doc amendment (~+2). Nothing is removed by the
owner's ruling on finding 2, because the opt-out was never in the estimate.

**Revised: production ≈ +130 / −65; tests ≈ +260 / 0** (the steady-frame guard
test, plus the fail-first sequencing needing both regression tests written twice
against two key names); docs/generated ≈ +2 / −3. **Total ≈ +392 / −68.**

## Problem and semantics

Effect layers are the same object in both spaces — one shader, one param schema, one `EffectInstance` in `Compositor::effect_instances` — but they run at two different resolutions. `/mega/ARTEXP/darkly/crates/darkly/src/gpu/effect_scaling.rs:30-38` exposes two getters:

- `screen_scale()` reads `rendering.screen_effect_scale`, default `0.7071` (`crates/darkly/presets/defaults.yaml:131`, commented there as "sqrt(.5) … roughly half the processing power compared to 1.0").
- `canvas_scale()` reads `rendering.canvas_effect_scale`, default `1.0` (`defaults.yaml:132`).

`ScaledEffect::prepare` (`effect_scaling.rs:108-138`) computes `effective = (scale * effect.perf_scale_factor()).clamp(MIN_SCALE, 1.0)` and, when `effective >= 1.0 - FULL_SCALE_EPSILON`, returns `ScaledEffect::Full` — the caller's own ping-pong pair, no intermediate textures, no extra passes. So with today's canvas default of `1.0` and every shipped effect except `painting` (`gpu/effects/painting.rs:135`, `0.7`) leaving `perf_scale_factor` at its default `1.0` (`gpu/effect.rs:314`), canvas space gets no downscale/upscale wrapping at all.

`Compositor::sync_effect_instances` (`gpu/compositor.rs:4660-4832`) picks `(size, scale)` per space at `4696-4716`: canvas takes the parent group's accumulator dimensions plus `canvas_scale()`; screen takes `screen_run.viewport_size()` plus `screen_run.scale()`.

**The decision.** The repository owner has ruled that the reduced-resolution scale is a deliberate quality/performance trade-off that applies regardless of which space an effect is in. This overrides the justification currently written into the module doc at `effect_scaling.rs:11-14` ("canvas space is document content and defaults to 1.0, since shipping a layer's pixels through a reduced-resolution round trip would bake the loss into what the user exports") and the mirror of it in `config/sections/rendering.rs:3-6`. Both comments are deleted, not argued with. The consequence — exported pixels carry the downscale — is accepted; §"(C)" prices the alternative and recommends against it.

**Resulting semantics.** One config key, `rendering.effect_scale`, default `0.7071`, clamped to `[MIN_SCALE, 1.0]`, multiplied by the effect's own `perf_scale_factor()`. Both spaces read it. `ScaledEffect::Full` remains the representation of "no reduction needed" rather than becoming a scale of 1.0 — that distinction is what keeps the full-scale path free (`effect_scaling.rs:74-77`), and it stays correct.

**Why 0.7071 as the single default.** It is the value the owner already chose deliberately for the space where the trade-off was being made; the canvas `1.0` is the value being overridden, so keeping it as the merged default would be the opposite of the instruction. `1/sqrt(2)` halves the texel count on a two-dimensional cost curve — the honest "half the work" point — and the `downscale.wgsl` header (`crates/darkly/shaders/downscale.wgsl:1-12`) documents that the multi-tap filter was written precisely because a single-tap blit "aliases hard below about 0.7", i.e. the filter is tuned for exactly this ratio. The `Pref` range stays `min: 0.25, max: 1.0`, so a user who wants full-resolution documents sets one slider to 1.0 and pays for it in both spaces — which is the point of a unified knob.

**What does not change.** The destructive apply path (`apply_filter_typed` → `filter_node_region`) and the preview path do not go through `ScaledEffect` at all — the only two `effect_scaling::` consumers are `compositor.rs:4706` and `compositor.rs:4772` (grep: `ScaledEffect|effect_scaling::` across `crates/darkly/src`). A user who wants an effect baked at full resolution already has an always-full-resolution route: apply it destructively. That materially weakens the case for (C).

## (B) The latent bug — confirmed real

**Diagnosis.** `ScreenRun::sync_resolution_scale` (`gpu/screen_run.rs:126-134`) re-reads `screen_scale()`, compares it against the cached `applied_scale` field (`screen_run.rs:41`), and on drift sets `needs_present` and answers `true`; its one caller, `Compositor::render` (`compositor.rs:5605-5610`), bumps `target_generation`, which invalidates every instance's fingerprint at `compositor.rs:4727`.

The canvas side has no counterpart. `structural_match` (`compositor.rs:4723-4728`) compares `pipeline_id`, `space`, `render_size` and `target_generation`. For a canvas instance `render_size` is the *parent accumulator's* dimensions (`compositor.rs:4705`), which a scale change does not move. Nothing else in the canvas path reads the config. Therefore: **changing `rendering.canvas_effect_scale` today leaves every canvas-space instance prepared at the old scale indefinitely** — until something unrelated bumps `target_generation` (a canvas resize, a new group state, a scratch reallocation) or the layer is edited structurally.

Today this is latent-but-harmless in the default configuration, because the canvas default is `1.0` and the instance is `ScaledEffect::Full`; a user who lowers the value sees nothing happen. Unification makes it load-bearing: the one knob that now governs document content would not take effect on document content.

There is a second, independent gap the fix must close. `sync_resolution_scale` is called only from `Compositor::render` (`compositor.rs:5608`), which is the surface path. `DarklyEngine::render` returns early in headless mode before reaching it (`engine/rendering.rs:695-700`), and `render_offscreen` — the entry point used by export (`engine/export.rs:39`), save (`engine/save.rs:146`), process recording (`engine/process_recording.rs:275`), previews (`engine/preview.rs:262`) and `test_readback_canvas` (`engine/mod.rs:1108`) — never polls the config at all. It also early-returns on `!self.needs_composite` (`compositor.rs:3916`), so even a correct fingerprint would not be consulted after a config change unless something marks the composite dirty. The config layer cannot push: `config_set` is a free function with no engine handle (`frontend/wasm/src/config_bridge.rs:29-45`), and `ConfigStore.set` (`frontend/src/config/store.svelte.ts:156-161`) does not request a frame. Polling is therefore the only available mechanism, and it must run somewhere both the surface path and the offscreen path reach.

**Root cause.** "What scale is this instance at" was never recorded on the instance. It was cached on `ScreenRun` — a resources object that does not otherwise use the value (the run's own textures are always native viewport size; `sync_resolution_scale` does not drop them) — so the one space whose resources object happened to hold a cached copy got change detection and the other did not. The fix is to move the fact to its owner.

**Fix.** Add `applied_scale: f32` to `EffectInstance` (`compositor.rs:548-569`), beside `render_size` and `target_generation`, and include it in `structural_match`. Delete `ScreenRun::applied_scale`, `ScreenRun::scale()`, `ScreenRun::sync_resolution_scale` and `screen_run.rs:19 SCALE_EPSILON` — with one scale there is one watcher, not two, and it does not belong to either space's resources.

To keep one formula in one place, `effect_scaling` grows:

```rust
/// The scale an effect actually renders at: the configured scale composed with
/// the effect's own declared cost, floored so it always has texels to work with.
pub fn effective_scale(base: f32, perf_scale_factor: f32) -> f32
```

`ScaledEffect::prepare` uses it (replacing the inline expression at `effect_scaling.rs:120`) and so does the fingerprint comparison, so a clamped configuration compares equal to itself.

The wake-up is one method on the compositor, and it reads the instances rather than caching a second copy of the scale:

```rust
/// Wake the pipeline when the configured effect scale has drifted from what the
/// realized instances were built at. The instances are the record of the scale
/// in force, so nothing else caches it; `sync_effect_instances` does the actual
/// rebuilding once a frame is running.
fn sync_effect_scale(&mut self) { … }
```

It computes `effect_scale()` once, asks whether any instance's `applied_scale` differs from `effective_scale(base, inst.effect.perf_scale_factor())` by more than `SCALE_EPSILON`, and if so calls `self.mark_dirty()` (which sets `needs_composite` and clears every group's `cache_valid_through`, `compositor.rs:2201-2209`) and `self.screen_run.mark_needs_present()`. It does **not** bump `target_generation` — the fingerprint now catches the drift, and bumping would rebuild instances that have not changed. It is idempotent and self-terminating: once `sync_effect_instances` rebuilds, drift is zero.

Two call sites, for the two entry points that gate on dirtiness:

- `Compositor::render`, replacing the `screen_run.sync_resolution_scale()` block at `compositor.rs:5605-5610`, before the `has_pending_work` early return.
- `Compositor::render_offscreen`, before the `!self.needs_composite` early return at `compositor.rs:3916` — this is what makes export, save, recording, headless and tests see a change.

This is the same two-entry-point shape `sync_effect_instances` already has, and for the same documented reason (`compositor.rs:3702-3710`).

**Regression tests (must fail before the fix).** In the new `crates/darkly/tests/effect_scale.rs`:

1. `changing_the_scale_rebuilds_a_canvas_instance` — build a canvas-space effect on a 64×64 canvas at the default scale, settle, record `engine.test_effect_reduced_size(fx)`; `config::set("rendering.effect_scale", ConfigValue::Float(0.5))`; force a composite by any ordinary means (paint a dab) and read back. Assert the reduced pair is now `(32, 32)`. **Fails before the fix** because `structural_match` holds — `pipeline_id`, `space`, `render_size` (the accumulator, unmoved) and `target_generation` are all unchanged — so the instance is reused at the old scale even though the composite ran. This isolates the fingerprint defect from the wake-up defect.
2. `a_scale_change_alone_wakes_the_canvas` — same setup, change the config, then call `engine.test_readback_canvas()` with nothing else dirtied. Assert the reduced size moved. **Fails before the fix** for the second reason: `render_offscreen` returns at `!needs_composite` and never reaches the sync. Both must pass after.

## Architectural impact

- **Ownership.** "The scale this instance is realized at" moves onto the instance, next to the other two facts of the same kind (`render_size`, `target_generation`). `ScreenRun` goes back to owning only resources, which is what its module doc already claims (`screen_run.rs:5-6`).
- **DRY.** Two getters collapse to one; two config keys to one; the `(size, scale)` match in `sync_effect_instances` collapses to `size`, with the scale hoisted out of the loop. The `effective_scale` formula stops being an inline expression readable from exactly one place.
- **Modularity / type-owned dispatch.** Unchanged and preserved: `Effect` still never learns why it was given a size (`gpu/effect.rs:279-286`), `perf_scale_factor` remains the one per-effect override, and no consumer branches on effect type.
- **Document Authority.** The scale is a config-derived rendering knob, not document state. Nothing about it serializes; instances are compositor state, rebuildable from the document plus the config on the next frame. Correct as-is.
- **No migrations.** `rendering.canvas_effect_scale` is deleted and `rendering.screen_effect_scale` is renamed in one pass across `config/sections/rendering.rs`, `presets/defaults.yaml`, `gpu/effect_scaling.rs` and `gpu/screen_run.rs`. There are no other producers or consumers (grep for `effect_scale` across the repo hits only those files plus historical `docs/plans/` and handoff notes, which are records and stay as written). No shim, no alias.
- **CONFIG_VERSION.** `config/mod.rs:26` documents "bump whenever … a pref key is renamed", but the mechanism it describes already handles this case: `validateOverrides` (`frontend/src/config/validate.ts:32-50`) drops unknown keys with a warning and rewrites the cleaned file. **Recommendation: do not bump.** Bumping discards the user's entire settings file including every hotkey (`store.svelte.ts:86`), to save one stale float. The user who had customized the viewport scale loses that one override and inherits `0.7071`, which is the value they most likely had anyway. Flagged for the reviewer as a deliberate deviation from the letter of that comment.

### The one genuinely new hazard: straight alpha through the reduced round trip

The screen-space run operates on an opaque presented image. Canvas accumulators do not: `composite.wgsl:48-57` un-premultiplies (`out_rgb = (…) / max(out_a, 0.001)`), so accumulators hold **straight (non-premultiplied) alpha**, and fully transparent regions hold RGB `0`. `downscale.wgsl:44-49` takes an unweighted mean of four bilinear taps. Averaging straight-alpha texels across an alpha edge pulls RGB toward black: opaque red `(1,0,0,1)` averaged with empty `(0,0,0,0)` yields `(0.5,0,0,0.5)`, which as straight alpha is *dark* red at half coverage, not red at half coverage. The upscale blit and the in-place apply then write that into the accumulator, where at opacity 1 and no mask the apply is an exact passthrough (`in_place_apply.wgsl:11-14`). The result is a dark fringe at every transparency edge in the document — a colour defect, not merely softness, and one that exports.

The minimal correct mitigation is an alpha-weighted downscale: RGB as `sum(rgb_i * a_i) / max(sum(a_i), eps)`, alpha as the existing mean. In screen space, where alpha is 1 everywhere, this reduces to the current formula exactly, so it is a no-op there. About nine lines of WGSL, no Rust, no extra passes. **Included in the plan as part of (A)**, priced separately so it can be dropped if the reviewer disagrees.

Two refinements from review, both folded in. The four downscale taps are *bilinear* samples (the shared sampler is `FilterMode::Linear`, `compositor.rs:977-984`), so the hardware already unweighted-averages straight-alpha texels inside each tap; alpha-weighting the taps alone would only halve the artifact. The downscale therefore reads via `textureLoad` with explicit weights instead. And the upscale side is cheap to fix after all: `ScalingPipelines::upscale` is its own `EffectPipeline` (`effect_scaling.rs:61`) sharing only a bind-group layout, so it takes a dedicated shader doing a manual alpha-weighted bilerp from four `textureLoad`s — one pass, no extra textures, no premultiplied intermediates, and no un-premultiply passes around the effect (which expects straight alpha; `invert` on premultiplied RGB would be wrong). Both shaders cite `shaders/lib/aberration.wgsl:79-84`, which is the same idiom already in the tree.

## Implementation steps

1. **`gpu/effect_scaling.rs`.** Replace `screen_scale`/`canvas_scale` with `effect_scale()` reading `rendering.effect_scale`. Add `pub fn effective_scale(base: f32, perf_scale_factor: f32) -> f32` and use it in `prepare`. Move `SCALE_EPSILON` here from `screen_run.rs` with a doc comment describing what it means for a fingerprint comparison. Rewrite the module doc: one scale, both spaces, why. Add `ScaledEffect::reduced_size(&self) -> Option<(u32, u32)>` (`#[cfg(any(test, feature = "testing"))]`) reading the actual texture dimensions from `Reduced::_textures[0]` — ground truth, no duplicated rounding formula.
2. **`gpu/screen_run.rs`.** Delete `SCALE_EPSILON`, the `applied_scale` field and its initializer, `scale()`, and `sync_resolution_scale`. Narrow the import to `ScalingPipelines`.
3. **`gpu/compositor.rs`.** Add `applied_scale: f32` to `EffectInstance` with a doc line in the same register as its siblings. In `sync_effect_instances`: compute `let base = effect_scale();` once above the loop; reduce the per-space match to `size`; extend `structural_match` with the `applied_scale` comparison via `effective_scale`; pass `base` to `ScaledEffect::prepare`; store `applied_scale: effective_scale(base, effect.perf_scale_factor())` on insert.
4. **`gpu/compositor.rs`.** Add `sync_effect_scale`. Call it at the top of `render_offscreen` (above the `needs_composite` gate) and at the top of `render` (replacing the `sync_resolution_scale` block).
5. **`config/sections/rendering.rs`.** One `Pref`: key `rendering.effect_scale`, display name "Effect scale", description covering both spaces *and* stating that the result is what gets exported. Rewrite the file header comment.
6. **`presets/defaults.yaml`.** One key, `rendering.effect_scale: 0.7071`; keep and reword the sqrt(.5) comment.
7. **`shaders/downscale.wgsl`.** Alpha-weighted RGB average; extend the header comment to say why (straight-alpha accumulators, canvas-space content).
8. **Test accessors.** `Compositor::test_effect_reduced_size(&self, id) -> Option<(u32, u32)>` and a `DarklyEngine::test_effect_reduced_size` passthrough beside `test_effect_rebuilds` (`engine/mod.rs:1166-1171`), both `#[cfg(any(test, feature = "testing"))]`.
9. **Tests.** New `crates/darkly/tests/effect_scale.rs` (below).
10. **Verify.** `cargo test -p darkly --test effect_scale --features testing -- --test-threads=1`, then the full gate from CLAUDE.md.

Sequencing: 1 → 2/3 together (2 breaks 3's call site) → 4 → 5/6 → 8/9 → 7 last (independent, droppable).

## (C) Full-resolution export — recommendation: **do not implement**

**Shape priced.** With `applied_scale` in the fingerprint, the override is genuinely cheap in concept: one `effect_scale_override: Option<f32>` field on `Compositor`, consulted by a private `fn effect_scale(&self)` that both `sync_effect_instances` and `sync_effect_scale` call instead of the free function; one scoped helper `bake_at_full_resolution(&mut self, f: impl FnOnce(&mut Self) -> R) -> R` that sets `Some(1.0)`, marks dirty, runs the closure, clears, marks dirty again; and call-site wrapping at `engine/export.rs:39`, `engine/save.rs:146`, `engine/process_recording.rs:275`, plus set/clear inside `Compositor::bake_subtree_to_layer` (which covers flatten and merge without touching `flatten.rs`/`merge.rs`). `engine/preview.rs:262` deliberately keeps the configured scale.

**Concrete production LOC for (C) alone, on top of (A)+(B): ≈ 30 added, 4 removed.** Field + doc 4; `effect_scale` accessor + doc 5; the scoped helper + doc 12; four call sites ≈ 9. That is over the 25-line threshold on its own.

It also fails the spirit of the second criterion. It does not create a second record of *what scale an instance is at* — that stays on the instance — but it creates a second source of truth for *what scale should be in force*, as a mode flag with no owner in the document/session/compositor taxonomy, whose correctness depends on every early return restoring it (`bake_subtree_to_layer` has one at `compositor.rs:3809-3812`).

**Runtime cost — the decisive argument.** A rebuild is not a re-parameterization. `sync_effect_instances` constructs a *fresh* effect from the registry (`compositor.rs:4744-4749`) and calls `Effect::create_cache` at the new size, twice per bake (down to 1.0, back to configured):

- **Allocation spike.** `grain::create_cache` (`gpu/effects/grain.rs:172-250`) allocates two render-sized RGBA8 textures, builds a `vec![0u8; w*h*4]` of PCG noise on the CPU, and `write_texture`s it into both. On a 4000×4000 canvas that is a 64 MB CPU buffer plus 128 MB of uploads per rebuild — ~256 MB of traffic and hundreds of milliseconds for one export, per animated grain layer. Every effect's aux textures are re-created at the larger size on the way out and at the smaller size on the way back.
- **Two extra full-tree recomposites.** `mark_dirty` clears `cache_valid_through` on every group (`compositor.rs:2204-2206`), so the restore forces a complete recomposite on the next frame in addition to the full-resolution one.
- **Animated effects visibly reset.** Animation clocks live on the `Effect` struct, not the cache — `rainy_glass.rs:95-96` (`time`), `grain.rs:166-168` (`frame_count`, `noise_idx`). A rebuild discards them. So a full-resolution export would bake the animation at t = 0 rather than the frame the user was looking at (the export is *less* faithful, not more), and the on-screen animation would jump backwards after every export.
- **It is not a rare path.** `SavePurpose::Snapshot` autosaves every 120 s (`defaults.yaml:120-121`), and the process recorder captures every 1.5 s by default (`recording.enabled: true`, `recording.minIntervalSeconds: 1.5`) through the same `render_offscreen`. With (C), a document with one animated canvas effect would rebuild it twice and reset its clock every 1.5 seconds. That is disqualifying regardless of LOC.
- **The recorder does not even want it**: it immediately soft-downscales the composite to `recording.maxLongEdge: 1920` (`engine/process_recording.rs:277-300`).

**The trade-off, stated plainly.** Full-resolution export means the exported image is *not* bit-identical to what the user previewed — the preview shows a downscaled round trip, the file shows something sharper, and for animated effects a different moment in time. Scaled export means the deliberate quality/performance trade-off ships into the file: a user who exports a document with a canvas-space blur gets pixels that went through a 0.7071 round trip, and no amount of zooming into the exported PNG recovers what the shader could have produced at full resolution. **Recommendation: ship (A)+(B); export inherits the scale.** The user-facing escapes are honest and already exist: set `rendering.effect_scale` to 1.0 before exporting, or apply the effect destructively, which never goes through `ScaledEffect` at all.

## Prior art

**Krita — level of detail / Instant Preview.** Directly analogous, and unambiguous about which side of the line reduced resolution lives on.

- LOD is a *viewport* decision derived from canvas zoom: `KisCanvas2::notifyLevelOfDetailChange` computes `lod = KisLodTransform::scaleToLod(effectiveZoom, maxLod)` and pushes it to the image (`krita/libs/ui/canvas/kis_canvas2.cpp:1386-1404`). LOD *n* means working zoom `2^-n` (`krita/libs/image/kis_image.h:729-734`).
- Reduced-resolution pixels live in a **separate plane** from document pixels. `KisPaintDevice::Private` holds `mutable QScopedPointer<Data> m_lodData` (`krita/libs/image/kis_paint_device.cc:596`) alongside the authoritative data; `currentData()` returns the LOD plane only while `defaultBounds()->currentLevelOfDetail()` is non-zero and the authoritative `currentNonLodData()` otherwise (`kis_paint_device.cc:521-533`). Authority flows one way: `createLodDataStruct` / `updateLodDataManager` downsample *from* `currentNonLodData()` into the LOD plane (`kis_paint_device.cc:688-722`, `724-760`). The document is never overwritten with preview-quality pixels, so export needs no restore dance — there is nothing to restore.
- Reduced-resolution work is always a *companion* to full-resolution work, never a replacement. `KisStrokesQueue::startStroke` pairs every LODN stroke with a LOD0 buddy — `stroke->setLodBuddy(buddy)` (`krita/libs/image/kis_strokes_queue.cpp:281-330`, especially `302-306`). The LODN stroke exists for immediate feedback; the LOD0 stroke computes the committed result.
- Participation is **opt-in per operation, defaulting to off**: `KisStrokeStrategy::createLodClone` returns `0` by default (`krita/libs/image/kis_stroke_strategy.cpp:124-128`), which forces the legacy full-resolution path.
- Filters specifically: `KisFilter::supportsLevelOfDetail` defaults to `false` (`krita/libs/image/filter/kis_filter.cc:24`, `105-115`), and `KisFilterStrokeStrategy::createLodClone` refuses to build an LOD clone unless the filter opts in (`krita/libs/ui/tool/strokes/kis_filter_stroke_strategy.cpp:371-379`). Most filters opt in — `kis_blur_filter.cpp:31`, `kis_gaussian_blur_filter.cpp:36`, `kis_convolution_filter.cpp:28`, `kis_pixelize_filter.cpp:50` — and some deliberately do not: `kis_unsharp_filter.cpp:44`, `KisResetTransparentFilter.cpp:38`, `KisPropagateColorsFilter.cpp:32` all call `setSupportsLevelOfDetail(false)`. Filter *masks* read the LOD off the device they render into and adjust their needed/changed rects accordingly (`krita/libs/image/kis_filter_mask.cpp:127`, `186-192`, `225-237`).

**GIMP — preview versus applied operation.** Two different answers in two eras, both instructive.

- Modern non-destructive filters: the on-canvas preview *is* the final graph. `GimpDrawableFilter` documents itself as "manipulation of drawable data, with live preview on screen" (`gimp/app/core/gimpdrawablefilter.c:19-21`); `preview_enabled` is a visibility boolean (`gimpdrawablefilter.c:787-800`), not a quality knob. `gimp_drawable_filter_commit` explicitly turns off the preview-only modifiers — split view off, preview on — *before* merging, so the committed result is defined as "the preview with preview-only affordances disabled" (`gimpdrawablefilter.c:1246-1281`). Resolution reduction lives in the display instead: the projection carries a mip pyramid ("The pyramid levels constitute a geometric sum with a ratio of 1/4", `gimp/app/core/gimpprojection.c:303`) used when zoomed out.
- Legacy plug-in previews are the other pattern — the one (C) would be adopting. `gimp_zoom_preview_get_source` feeds the plug-in `gimp_drawable_get_sub_thumbnail_data(...)`, i.e. a *thumbnail-scaled* copy of the visible region (`gimp/libgimp/gimpzoompreview.c:900-926`), and the plug-in re-runs on the full-resolution drawable when the user confirms. Preview and result are computed twice, at two resolutions, and are not guaranteed to match.

**How this informs the decisions.** Both editors treat reduced resolution as a *view-side* concern with a full-resolution authority behind it, which is an argument against the owner's unification. The owner has ruled otherwise, and that ruling is what this plan implements. But the prior art is decisive on (C): neither editor achieves full-quality output by *re-running the preview machinery at a different scale and putting it back* — Krita keeps two planes so the question never arises, GIMP either uses the same graph for both or runs the operation a second time from the untouched source. (C) as priced is the one shape neither of them chose: mutate the shared realization, bake, mutate back. That, plus the animation-clock reset and the recorder's 1.5 s cadence, is why the recommendation is to ship (A)+(B).

Krita's per-filter opt-out (`supportsLevelOfDetail` defaulting to false) is also a pointed observation about Darkly's `perf_scale_factor`, which can only reduce and never pin — see Unresolved questions.

## Tests

New file `crates/darkly/tests/effect_scale.rs`, using the established idioms: `test_device()` + `GpuContext::new_headless` + `DarklyEngine::new` (`tests/effect_space.rs:18-22`), `effect()`/`fill_layer()`/`settle()` helpers (`effect_space.rs:26-63`), `paste_image` for pixel patterns (`tests/filters.rs:83`), and `darkly::config::set(key, ConfigValue::Float(..))` for config (the idiom in `tests/engine.rs:5390`). Run: `cargo test -p darkly --test effect_scale --features testing -- --test-threads=1`.

Every test sets the key explicitly and resets it at the end — `config` is a thread-local store (`config/mod.rs:58-60`) shared across tests in the binary, so a leaked override would leak sideways, exactly as `engine.rs:5388-5392` warns.

Features:

1. `canvas_space_effect_renders_at_the_configured_scale` — 64×64 canvas, one canvas-space `invert`, scale `0.5`; assert `test_effect_reduced_size(fx) == Some((32, 32))`. The direct proof that (A) works.
2. `both_spaces_render_at_the_same_scale` — one canvas-space and one screen-space effect, 64×64 canvas and viewport, scale `0.5`; drive both (`test_readback_canvas` and `test_readback_screen_run`) and assert both instances report `(32, 32)`. Pins "one knob, both spaces" against a future re-split.
3. `a_scale_of_one_skips_the_reduced_path` — scale `1.0`; assert `test_effect_reduced_size(fx) == None`, i.e. `ScaledEffect::Full`. Guards the free common path.
4. `per_effect_factor_composes_with_the_configured_scale` — a `painting` effect (`perf_scale_factor` `0.7`) at scale `0.5` on a 100×100 canvas → `Some((35, 35))`. Pins that the two multiply rather than one overriding the other, in canvas space where that composition is new.
5. `a_reduced_canvas_effect_actually_resamples_the_composite` — paste a one-pixel checkerboard, add a canvas-space `invert`, read back at scale `1.0` (exact per-pixel inverse) and at `0.5` (materially different from the exact inverse). Proves the reduced path is really in the canvas *encode*, not merely prepared.
6. `a_reduced_canvas_effect_does_not_darken_transparent_edges` — only if the `downscale.wgsl` change is kept. Paste an image that is opaque red on the left half and fully transparent on the right, add a canvas-space `invert` at scale `0.5`, and assert that every pixel with `a > 0` still reads as inverted red (`r` near 0) rather than being pulled toward black. Fails against the unweighted downscale.

Regressions for (B), as specified above: `changing_the_scale_rebuilds_a_canvas_instance` and `a_scale_change_alone_wakes_the_canvas`. Both are written to fail first and for distinct reasons (fingerprint; wake-up), and both may additionally assert `engine.test_effect_rebuilds()` increased, which is the cheap corroborating signal (`engine/mod.rs:1166-1171`).

Existing coverage that must keep passing unchanged: `effect_space.rs::effect_instances_are_not_rebuilt_every_frame` — `sync_effect_scale` must not report drift on a steady frame, or it would turn every frame into a recomposite; that test is the guard. Also `effect_space.rs::screen_space_effect_is_visible_only_after_the_present_pass` and both animation tests, which exercise the screen path whose `applied_scale` cache is being deleted.

Constraints honoured: no blocking readback in production code — `reduced_size` and both `test_*` accessors are `#[cfg(any(test, feature = "testing"))]` and read texture metadata, not pixels; the pixel assertions go through the existing test-only readbacks.

## Risks

- **Every existing document gets softer canvas effects on first launch after this lands.** That is the requested behaviour, but it is a silent visual change to saved work. Mitigated only by the pref being one slider away and documented in the pref description.
- **Straight-alpha fringing** — see above. The alpha-weighted downscale addresses the dominant term; the magnification residual remains.
- **Effects whose output is not scale-invariant.** `lens_blur` is safe: its radius is expressed as a fraction of `sqrt(area)` (`shaders/effects/lens_blur.wgsl:37-40`, `72-77`), so a smaller render target produces the same visual blur. `pixelate` is not: `num_halvings` operates on the render-size texture (`gpu/effects/pixelate.rs:96-99`), so at 0.7071 the blocks are computed on 1.41× fewer texels and then bilinearly magnified — which specifically defeats the `soft: false` "hard pixel edges" option. This already happens in screen space today; unification extends it to the document. See Unresolved questions.
- **Animated effects reset on a scale change.** Changing the pref rebuilds instances, which discards `rainy_glass::time` and `grain::frame_count`. Acceptable for a deliberate settings change (the screen side already behaves this way via the `target_generation` bump) and worth one line in the pref description.
- **`sync_effect_scale` runs per frame in `render_offscreen`.** It iterates `effect_instances` — a handful of entries — and does one config lookup. Negligible, but it is now on the offscreen path where it was not before; if profiling ever objects, the config read is the part to hoist, not the iteration.
- **Pre-existing, out of scope, observed while tracing the bake path:** `Compositor::bake_subtree_to_layer` composites into a sentinel `GroupState` keyed by `LayerId::from_ffi(0)` (`compositor.rs:3821-3833`) and calls `compose_children(..., bake_parent, ...)` (`compositor.rs:3869`), but effect instances are realized against `EffectSpace::Canvas { parent: doc.accumulator_host_of(id) }` (`compositor.rs:4681-4683`) — the document's structural parent, i.e. the *root's* accumulator, not the bake accumulator. So `compose_effect_arm` during a flatten or merge encodes an effect that reads the root's accum views while writing into the bake accum (`compositor.rs:5222-5257`). This looks wrong and would be masked in the common "flatten everything" case by the two accumulators holding similar content. **Not diagnosed further and not planned here** — flagged for a separate investigation, and noted because it is the same code (C) would have leaned on.

## Unresolved questions

1. **Should an effect be able to refuse the scale? Settled: no.** Owner ruling — the effect scale is a global setting that applies to every veil, not an effect-specific one. No `Effect` trait method is added, no effect opts out, and `perf_scale_factor` remains a cost declaration composed with the global scale rather than a veto. The scale-variant behaviours this implies (`grain` frequency, `chromatic_aberration` strength, `pixelate` block edges) are enumerated under Revision finding 2 and are accepted; all of them are already live in screen space today. Krita's contrary default (`supportsLevelOfDetail` opt-in) is recorded in Prior art as a road not taken, not as an open question.
2. **CONFIG_VERSION**: not bumped, contrary to the letter of `config/mod.rs:18-26`, because `validateOverrides` auto-cleans a dropped key and a bump would discard every hotkey. Reviewer to confirm.
3. **The magnification residual** (bilinear upscale of straight alpha darkening a one-texel edge band). Left unfixed; the proper fix is premultiplied intermediates with un-premultiply passes around the effect, which costs two extra full-resolution passes per effect per frame and would partly negate the reason the scale exists.
4. **Keep the alpha-weighted downscale in scope?** It is nine lines of WGSL and the difference between "the document is softer" and "the document has dark fringes". The plan keeps it; a reviewer who reads it as scope creep can strike step 7 and test 6 without touching anything else.

## Interaction with composite-prefix-cache

Independent and complementary. That plan attacks *how often* a canvas-space effect forces a full-tree recomposite; this one attacks *how much it costs* when it does (0.7071 ≈ half the texels through the effect's own passes). The gains multiply.

One concrete coupling, stated against `mark_dirty`'s *contract* rather than its current implementation, so it survives that plan deleting `cache_valid_through`: `sync_effect_scale` signals a scale change by calling `Compositor::mark_dirty`, and the contract that must hold is *a mutation which marks dirty invalidates every derived composite cache*. Whatever channel the prefix cache uses — today's per-group field, or that plan's `composite_epoch` — a scale change invalidates it for free provided the channel is driven from `mark_dirty` rather than sitting beside it. A prefix cache with validity state outside that sweep would hold pixels produced at the old scale.

Two sequencing conflicts for whichever plan lands second: that plan routes all six `target_generation += 1` sites through a new `bump_target_generation()` and enumerates `compositor.rs:5609` among them — the exact line this plan deletes; and both plans edit the head of `render_offscreen` (`compositor.rs:3910-3918`) and the `structural_match` block (`:4723-4728`, which that plan cites as its template and describes as a five-field compare — it is four today and becomes five here). Textual conflicts, not semantic ones, but they must be merged deliberately.

No file is shared between the two plans' edits except `gpu/compositor.rs`; within it, this plan touches `EffectInstance`, `sync_effect_instances`, `render`, `render_offscreen` and adds `sync_effect_scale`. `docs/plans/composite-prefix-cache.md` is not edited here.

## LOC estimate

Lines **added / removed**, not touched.

**Production ≈ +115 / −65**

| File | + | − | |
|---|---|---|---|
| `gpu/effect_scaling.rs` | 39 | 20 | module doc rewrite, one getter for two, `effective_scale`, `SCALE_EPSILON` moved in, gated `reduced_size` |
| `gpu/screen_run.rs` | 1 | 28 | delete `applied_scale`, `scale()`, `sync_resolution_scale`, `SCALE_EPSILON` |
| `gpu/compositor.rs` | 48 | 14 | `applied_scale` field, fingerprint clause, hoisted scale, `sync_effect_scale` + 2 call sites, gated accessor |
| `config/sections/rendering.rs` | 9 | 20 | two prefs → one |
| `engine/mod.rs` | 9 | 0 | gated `test_effect_reduced_size` passthrough |
| `shaders/downscale.wgsl` | 9 | 3 | alpha-weighted average (droppable: −9/+3 if struck) |

Roughly a third of the additions are doc comments, per house style; the net line count is about +50.

**Tests ≈ +230 / 0** — one new `crates/darkly/tests/effect_scale.rs`: module header, four shared helpers reused in spirit from `effect_space.rs`, six feature tests, two regression tests.

**Generated / docs ≈ +2 / −3** — `presets/defaults.yaml` only (one key instead of two, reworded comment). No `build.rs`-generated `mod.rs` changes (no new module file). No frontend changes: the settings panel is schema-driven and nothing in `frontend/` names either key.

**Total ≈ +347 / −68.** Excluding the optional alpha-weighted downscale: ≈ +316 / −65. Adding (C) would be ≈ +30 / −4 more, in production, on the hot bake path — not recommended.
