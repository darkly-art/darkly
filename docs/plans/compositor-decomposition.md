# Compositor Decomposition

## Independent Review

Independent investigation of the working tree (`better-veils`, file now 5,861
lines), the cited prior-art checkouts, and the test suite. Roughly forty of
the plan's line/symbol citations were spot-checked directly; the diagnosis is
sound and unusually well-grounded. Findings below; substantive ones first.

**Verified accurate (sample):** the bake sentinel leak is real
(`compositor.rs:3891-3902` inserts a `GroupState` under `LayerId::from_ffi(0)`
behind a `contains_key` guard and nothing ever removes it, nor the
`blend_bind_groups` entries keyed by it); `compose_children` (:4381) and
`sync_projection_states` (:4516) already take `&Document`, so the `doc: &mut
Document → &Document` signature change is feasible; `padded_width/height` are
provably dead duplicates (all three assignments are identity copies, and the
field's "tile-aligned" doc comment is stale); the `isolated_node` mirror,
lockstep write, and mirror-agreement assertions exist
(`engine/layers.rs:1543-1568`, `tests/engine.rs:4837,4862`, plan line numbers
drifted slightly, symbols match); all four `doc.mask_filter(host).filter(|m|
m.common.visible)` re-derivations exist; all five `LayerContent::Procedural`
matches exist; the three `ensure_*_layer` bodies are near-verbatim triplicates;
every prior-art citation checks out verbatim
(`kis_abstract_projection_plane.h:17-31` quote exact, `startMerge` at
`kis_async_merger.cpp:172`, `gimpimage-merge.c:60/208/271`,
`kis_image.h:589/596/787`, `kis_update_scheduler.cpp` and `gimpprojection.c`
present); the `gpu/mod.rs` helpers exist at :5/:28/:57 and are already used by
`preview.rs:543,627` and `floating_preview.rs:371,516`. Boilerplate counts:
12 `begin_render_pass`, 11 `create_bind_group`, 10 `create_buffer`, 6
`create_texture`, 9 `BlendUniforms` literals all exact.

**R1 (revise: C5 as written risks a painting-path regression and widens a
tick's meaning).** C5 stamps `blend_bind_groups` entries with
`revisions.targets()` and has `swap_node_texture` bump `targets`. But
`targets` is already the structural-staleness input to the effect-instance
sync: `sync_effect_instances` treats any bump as "the textures behind my bind
groups may be freed" (`compositor.rs:4817 built_targets ==
self.revisions.targets()`) and rebuilds every instance
(`effect_rebuilds += 1`, fresh bind groups + `ScaledEffect::prepare`).
`swap_node_texture` is not rare: it fires from the painting path whenever a
stroke crosses a `LAYER_GROWTH_CHUNK` boundary (`engine/painting.rs:793,859` →
`resize_node_texture` → swap at :1511), plus rescale/flip/ortho (:1578, :1669,
:1705) and staged void resizes (:3188). A stroke that grows a layer would
repeatedly rebuild all effect instances and flush the entire blend-bg cache
mid-stroke: the exact churn `effect_rebuilds` exists to catch. The plan's
risk note says "bumps are rare (… node swap)" and misses the effect-instance
coupling entirely. Also `revisions.rs:65-69` deliberately scopes `targets` to
"accumulators, the screen-run pair, the canvas apply scratch"; C5 silently
widens that source's meaning for all its consumers. Revise C5: give
node-texture identity its own revision source (per-node ticks already exist in
`Revisions`, stamp entries with the parent's `targets` plus the child's
node-texture tick), or keep the targeted retain in `swap_node_texture` and
revision-stamp only the canvas-resize/group-recreation invalidation. The
current three retain/clear sites are at least correct and local.

**R2 (record the trade in B1).** The sentinel `GroupState` is an intentional
cross-bake cache (guarded insert; three canvas-sized textures reused every
merge). The fix converts "leak for the session" into "allocate + `bump_targets`
per bake", and per R1, each post-fix bake's create/remove `bump_targets` will
also rebuild all effect instances. That is fine at user-action rate, but the
plan should say so explicitly rather than frame the removal as pure cleanup.
The regression test design is sound: `group_state` count goes 1 → 2 on the
first merge and stays, so the "count unchanged" assertion fails pre-fix and
passes post-fix. Note the test also needs the engine-side forwarder
(pattern: `engine/mod.rs:998 test_node_texture_count`), not just the
compositor accessor.

**R3 (A5 overstates the third site).** `LayerTexture::new_for_format` cleanly
covers the two realloc/stage matches (:1471, :1530), but
`ensure_node_texture`'s match (:2707) dispatches *behavior*, not construction,
its Rgba8 arm delegates to `ensure_raster_layer` wholesale and its R8 arm also
builds the mask bind group. Scope A5 to the two constructor sites; the third
match is the same shape C4's `NodeSlot` reworks anyway.

**R4 (A7 is not purely mechanical).** The three `ensure_*` bodies diverge in
ways the plan doesn't list: raster guards on `node_textures.contains_key`
while void/vector guard on `layer_cache.contains_key`; raster calls
`mark_node_pixels_dirty` (thumbnail invariant, see comment at :1414) while
void/vector call `mark_dirty`; vector allocates via `with_bounds_storage` and
stores `LayerContent::Raster`. The collapse is still right, but the shared
helper must carry the guard and dirty-call as per-kind inputs (or unify them
deliberately and say so), and Stage A's "no structural change" framing should
flag this item as the one needing care.

**R5 (A2 placement).** `create_uniform_buffer` is genuinely generic and
belongs in `gpu/mod.rs`. The blend bind-group builder is specific to the blend
pipeline's 4-entry BGL: per the Modularity Principle it belongs beside that
pipeline (`gpu/blend.rs` or the compositor), not in `gpu/mod.rs` dressed as a
generic helper. `copy_texture_to_texture` is 10 call sites, not 12 (:172,
:198, :1486, :1541, :1943, :1987, :2616, :3950, :4353, :5409).

**R6 (C1 notes, no change to verdict on it).** The mirror deletion is right
and all `isolated_node` reads are walk-scoped (:2180, :4406, :4559, :4740), so
per-frame threading works. Two notes: (a) "stored into a walk-scoped field" is
still a compositor-held copy of session state, just push-per-frame instead of
lockstep dual-write, of the plan's two options, threading through
`CompositionContext`/parameters is the cleaner one where reachable; (b)
`engine/rendering.rs:996 sync_compositor_layers` independently realizes
isolation into per-layer `BlendUniforms.isolated` bits from
`engine.isolated_node`, bake's save/restore never covered that path (a baked
subtree composites through whatever isolated bits the uniforms already carry;
pre-existing quirk, unaffected by C1, record it so nobody "fixes" it
mid-refactor). Also the C1 test item resolves to "already present":
`tests/engine.rs:3941 isolate_skips_off_path_sibling_rasters` asserts isolation
against rendered pixels.

**R7 (B5 sub-struct is marginal).** `FrameClock` would own two `u64`/`f32`
fields while every method stays `impl Compositor` per the plan's own
borrow-model reasoning. The file move is the win; the two-field struct adds a
name without adding a boundary. Fine either way: consider dropping the struct
and moving only the impl block.

**Scope/architecture/tests otherwise:** stages are independently landable and
correctly ordered (A shrinks B's diff; C4 before C5 is called out); no stage
scatters state to appease the borrow checker; C3's rejection of the audit's
fingerprint claim is correct on inspection (the stored `params` drives a
three-way decision, not a validity check); C8 deferral is right (no second
copy exists); D1-D3 match the existing `LayerKindGpu::realize_in` /
`LayerNode::compose_into` (`layer.rs:699`) pattern; test inventory checks out
(`assert_matches_from_scratch` at `tests/compositor_revisions.rs:100`,
`layer_bake.rs` merge tests, isolation output test). LOC estimate is
plausible against the verified boilerplate counts; net-negative production
delta is credible.

Verdict: **revise**, rework C5's invalidation source (R1), record B1's
cache-removal trade (R2), and fold in the mechanical corrections (R3-R5);
everything else stands as planned.

## Revision log

Every review finding has been addressed in the plan body below:

- **R1 → C5 withdrawn in its original form.** Neither proposed mechanism
  survives scrutiny: stamping with `targets` couples painting-path node swaps
  to whole-cache flushes *and* effect-instance rebuilds (the regression R1
  describes), and a dedicated node-texture-identity revision source would add
  a new `Revisions` source plus a per-node tick map purely to replace three
  retain/clear lines the review confirms are correct and local. Per the
  minimalism principle, C5 now keeps the existing targeted invalidation and
  folds its rule into C7's documented lifecycle invariant. The C5 test is
  kept (reframed as a pin on swap-path correctness, mechanism-agnostic).
- **R2 → B1 records the trade** (intentional cross-bake cache removed;
  per-merge realloc + `bump_targets` → per-merge effect-instance rebuild,
  acceptable at user-action rate) and the test plan now includes the
  engine-side forwarder.
- **R3 → A5 rescoped** to the two constructor sites; the third match is C4's.
- **R4 → A7 divergences enumerated** (guard map, dirty call, storage usage);
  the helper takes them as per-kind inputs; Stage A's framing flags A7 as the
  one item needing care.
- **R5 → blend bind-group builder relocated** to live beside the blend
  pipeline in the compositor, not `gpu/mod.rs`; copy-block count corrected to
  10 here and in the problem statement.
- **R6 → C1 prefers threading through `CompositionContext`** where reachable;
  the pre-existing `BlendUniforms.isolated` engine-push quirk is recorded so
  nobody "fixes" it mid-refactor; the C1 test item now cites the existing
  output assertion (`tests/engine.rs:3941`) instead of proposing a new one.
- **R7 → B5's `FrameClock` struct dropped**; the two fields stay on
  `Compositor` and only the impl block moves to `gpu/frame_clock.rs`.

LOC estimate updated for the C5 withdrawal (Stage C: +85/−150, tests +35).

## Implementation outcome

All five stages (A-E, including optional E) are implemented. Actuals:

- `compositor.rs`: 5,861 → 2,932 lines. New siblings: `compose_walk.rs`
  (1,309), `void_content.rs` (449), `effect_layers.rs` (401),
  `compositor_test_harness.rs` (180), `frame_clock.rs` (169),
  `vector_content.rs` (142), `bake.rs` (129). No gpu file except the walk
  exceeds 450 lines.
- Net production delta ≈ **−36 lines** (tracked src +639/−3,454, new files
  +2,779): flatter than the estimated −535 because ~2,600 lines moved
  (vs. the estimated ~1,570; the void/effect subsystems were larger than
  the plan's table) and each relocated file carries import/module-doc
  overhead. The Stage A deletions inside `compositor.rs` were real; the
  offset is relocation overhead plus the new shared helpers
  (`create_uniform_buffer`, `blit_region_mip`, `blend_bind_group`,
  `draw_blend_pass`, `BlendUniforms::for_extent`,
  `LayerTexture::new_for_format`, `insert_content_layer`,
  `node_view_info`).
- Tests +38/−4. `merge_down_leaks_no_group_state` was written first and
  confirmed failing (group-state count 1 → 2) before the B1 fix.
- Material deviations, all recorded in-line during implementation: C1
  needed no stored field (pure parameter threading, which also fixed a
  latent stale-mirror class where engine paths cleared
  `engine.isolated_node` without telling the compositor);
  `apply_uniforms_for` / `sync_effect_instances` also thread `isolated`
  (reachable from the present path); `host_active_mask_for_projection`
  was deleted outright in D2 (callers use `Document::visible_mask_of`);
  one extra `visible_mask_of` site was found and converted
  (`Document::collect_masked_in_place`); the C2/C4/C6/C7 items landed as
  specified; C3/C5/C8 remain no-ops per the review.

Refactor plan for `crates/darkly/src/gpu/compositor.rs`. Written against the
`better-veils` working tree after `c9e7e4c7`; every line number below was
re-verified against that tree (they will drift: symbol names won't). Findings
cross-checked against `docs/rust-complexity-audit.md` §1,
`docs/compositor-caching-audit.md`, and
`old-handoffs/handoff-gpu-cleanup-compositor-decompose.md`.

**Scope:** structural refactor, no behavior changes, with one exception, the
`bake_subtree_to_layer` sentinel `GroupState` leak, which is a real bug and
gets a regression test that fails before the fix.

## Problem statement

`gpu/compositor.rs` is 5,855 lines: a 45-field struct (620-850) with one
`impl` (852) holding ~26 responsibilities. It violates every named CLAUDE.md
principle at once:

- **DRY**: ~550-600 lines of hand-rolled wgpu descriptor boilerplate across
  ~60 sites while `gpu/mod.rs` exports the exact helpers
  (`clear_view_transparent` :5, `create_texture_with_view` :28, `blit_region`
  :57) that `floating_preview.rs` and `preview.rs` already use. Verified
  counts in compositor.rs: 10 `copy_texture_to_texture` blocks, 12
  `begin_render_pass` descriptors, 11 `create_bind_group` descriptors, 10
  `create_buffer` descriptors, 6 `create_texture` descriptors, 9
  `BlendUniforms {..}` literals, plus a verbatim present-pipeline pair
  (1155-1182 vs 1185-1213, differing only in target format, while the
  `in_place_apply_pipelines` block ten lines up already solves this with a
  `make(format)` closure, 1061-1097).
- **Ownership**: a document op (`bake_subtree_to_layer`, 3867-3971) that
  leaks a sentinel `GroupState`; a frame scheduler (3411-3464); 183 lines of
  test harnesses (4032-4214); session state (`cached_view_transform`,
  `viewport_bg`, `pixel_filter` 805-814, `histogram_target` 823-827); and four
  helpers that exist only to hand-split borrows of the oversized struct
  (3689, 3721, 4259, 5473).
- **Document/session authority**: `isolated_node` (792) is a mirror of
  `engine.isolated_node` (`engine/mod.rs:427`) written in lockstep
  (`engine/layers.rs:1504-1509`) with a test accessor (compositor.rs:2166,
  asserted at `tests/engine.rs:4837`) whose only job is to check the two
  copies agree. `LayerCache.opacity/.blend_mode/.isolated` (307-317) mirror
  the document's `layer.blend`. `padded_width`/`padded_height` (729-730) are
  only ever assigned `= width`/`= height` (970-971, 1295-1296, 2060-2061):
  dead duplicates of `canvas_width`/`canvas_height` (15 references).
- **Type-owned dispatch**: `CompositionContext::compose_layer` opens with
  `if let Layer::Filter(f)` (342), inside the dispatch hop
  (`layer.rs:699 compose_into`) that exists to prevent exactly that. The
  "host's visible mask filter" query is written four times (3729-3733,
  4431-4435, 4713-4716, 5551-5554). Five sites pattern-match
  `LayerContent::Procedural` (3172, 3235, 3286, 3320, 3343) around the
  `procedural_content()` helpers (2835/2843) documented as centralizing that
  lookup.
- **Cache-invalidation sprawl**: nine parallel `HashMap<LayerId, _>` fields
  (`group_state` 624, `node_textures` 636, `mask_bind_groups` 646,
  `blend_bind_groups` 655, `layer_cache` 661, `mask_snapshot_state` 672,
  `projection_states` 681, `effect_instances` 751, `vector_scenes` 849), each
  lifecycle event hand-picking a subset: `dispose_node_texture` (2855) hits
  six maps + revisions; `set_canvas_rect` (2047) clears three and rebuilds
  group states; `swap_node_texture` (1587) rewrites a uniform, retain-filters
  one map, rebuilds another. This is exactly the failure mode
  `gpu/revisions.rs` was written to abolish, applied to only part of the
  state.

**Root cause:** the extraction pattern that keeps the rest of the GPU
subsystem healthy (a second `impl Compositor` in a sibling file, proven at
`gpu/floating_preview.rs:44`) simply stopped being applied as features
landed. The prior decomposition handoff
(`old-handoffs/handoff-gpu-cleanup-compositor-decompose.md`, written at 3,194
LOC) was partially executed: its named dead methods (`render_to_view`,
`update_overlay_time`, the `update_layer_uniforms` wrapper, the tautological
`or_else` in `request_content_bounds`) are all gone from the current file, but
its headline item (the decomposition itself) never happened, and the file
has since nearly doubled.

## Prior art

Read from the checkouts at the repo root, not from memory:

- **The merger knows nothing about node internals.**
  `krita/libs/image/kis_abstract_projection_plane.h:17-31`: Krita's
  compositing walk (`KisAsyncMerger::startMerge`,
  `krita/libs/image/kis_async_merger.cpp:172`) talks to every node through
  `KisAbstractProjectionPlane` (`recalculate()` / `apply()` / rect queries),
  "Compositing system KisAsyncMerger knows nothing about the internals of the
  layer." This is the direction Darkly's `LayerKindGpu::realize_in` (382-421)
  and `LayerNode::compose_into` (`layer.rs:699`) already point; Stage D
  finishes the job.
- **Scheduling, walking, and per-node realization are separate objects.**
  Krita splits update scheduling (`kis_update_scheduler.cpp`), the merge walk
  (`kis_async_merger.cpp`), and per-node projection state (per-layer
  projection planes) into distinct classes; the animation clock hangs off the
  image, not the merger (`KisImageAnimationInterface`,
  `krita/libs/image/kis_image.h:787`). Supports Stage B's frame-clock and
  subsystem extractions.
- **Merge/flatten is an image (document) operation, not a projection
  operation.** GIMP: `gimp_image_merge_layers`
  (`gimp/app/core/gimpimage-merge.c:60`), `gimp_image_flatten` (:208),
  `gimp_image_merge_down` (:271) all take `GimpImage*`;
  `gimp/app/core/gimpprojection.c` is a pure renderer. Krita:
  `KisImage::mergeDown` / `flattenLayer` (`krita/libs/image/kis_image.h:589,
  596`). Darkly already has the document side in `engine/merge.rs` /
  `engine/flatten.rs`; the compositor's `bake_subtree_to_layer` should be a
  stateless "composite these sources into that texture" GPU service (it must
  not take `&mut Document` (it only reads it) verified) and must not leak
  render state across calls.

## Staged implementation

Four stages, each an independent PR that leaves the tree green
(`cargo check` mid-iteration; full gate at the end of each stage). Ordered so
each shrinks the surface the next has to reason about. An optional fifth
stage is listed last.

Extraction mechanics used throughout: sibling files in `gpu/` holding
`impl Compositor` blocks over `pub(super)` fields, the pattern
`gpu/floating_preview.rs` already proves. `gpu/mod.rs` is handwritten (not
build.rs-generated; only the ★ dirs `veils/`, `voids/`, `blend_modes/` are
generated), so adding `pub mod` lines there is fine. Sub-structs are
introduced only where the state is cohesive and never cross-borrowed against
the rest of the compositor (Ownership Principle: don't scatter a concept to
appease the borrow checker).

### Stage A: mechanical boilerplate collapse + dead-state deletion

No structural change; pure DRY: with one exception: A7 is the item needing
care, since the three bodies it collapses diverge in ways enumerated there.
Drops the file by ~450 net lines before any extraction begins, making Stage
B/C diffs reviewable.

1. **Use the existing `gpu/mod.rs` helpers.**
   - `blit_region` replaces the 10 hand-rolled `copy_texture_to_texture`
     blocks (172-217, 1486-1508, 1541-1567, 1943-1961, 1987-2005, 2616-2635,
     3946-3964, 4349-4367, 5403-5417; the mip-loop in `copy_void_source`
     keeps its loop but each iteration becomes one `blit_region` call: it
     needs a `mip_level` parameter added to `blit_region`, defaulting the
     existing callers via a thin wrapper or an extra arg at all 3 current
     callers).
   - `clear_view_transparent` replaces the three empty clear-only passes
     (3911-3924 bake clear, 4319-4332 accum clear, 5043-5055 proj clear).
   - `create_texture_with_view` absorbs `make_accum_texture` (854-879),
     `create_ortho_scratch` (93-116, keeping its usage-flags constant), the
     snapshot texture (1924-1937), and the two test-target textures
     (4043-4057, 4176-4190).
2. **Add one generic helper to `gpu/mod.rs`, one local builder to the
   compositor.**
   - `create_uniform_buffer::<T: bytemuck::Pod>(device, label) -> wgpu::Buffer`
     in `gpu/mod.rs`: genuinely generic (the same shape recurs in
     `paint_target.rs`, `selection.rs`); collapses the 10
     `BufferDescriptor { UNIFORM | COPY_DST, size_of::<T>() }` sites
     (911-916, 1100-1105, 1227-1232, 1394-1399, 1842-1847, 2757-2761,
     2966-2971, 3019-3024, 4479-4486, 4892-4897).
   - a `blend_bind_group(device, layout, bg_view, layer_view, sampler,
     uniform)` builder for the 4-entry blend BGL descriptor written four times
     (4225-4246, 4270-4293, 5487-5508, 1850-1871). It is specific to the
     blend pipeline's layout, so it lives beside that pipeline in the
     compositor (a private fn), not in `gpu/mod.rs` dressed as generic.
     `create_blend_bind_group` and `get_or_create_blend_bind_group` become
     thin callers of it (the doc comment at 4249 already admits one is "the
     cached entry point for" the other, then re-types the body).
3. **A `draw_blend_pass` helper** for the copy-pasted
   begin-pass(Load)/scissor/pipeline/bg0/bg1/bg2/draw sequence at 5059-5078,
   5082-5101 (apply-mask pipeline: pipeline is a parameter), 5105-5124,
   5248-5266, 5619-5637. Signature: `(encoder, target_view, load_op,
   pipeline, bg0, mask_bg, canvas_bg: Option<&BindGroup>, scissor)`.
4. **Collapse the present-pipeline pair** (1155-1213) with the same
   `make(format)` closure `new()` already uses for `in_place_apply_pipelines`
   (1061) and `paint_target.rs:996` uses for 14 pipelines. ~57 lines → ~30.
5. **`LayerTexture::new_for_format(device, queue, format, extent)`** on
   `gpu/atlas.rs`: the `match format { R8Unorm => new_mask_with_extent,
   Rgba8Unorm => with_bounds }` *constructor* dispatch written twice
   (1471-1477, 1530-1536). The format is `LayerTexture`'s own fact; the
   panic arm lives once, on the type. The superficially similar match in
   `ensure_node_texture` (2707-2737) is out of scope here: it dispatches
   behavior (its Rgba8 arm delegates to `ensure_raster_layer` wholesale, its
   R8 arm also builds the mask bind group) and is reworked by C4's
   `NodeSlot`.
6. **Delete `padded_width` / `padded_height`** (729-730). Replace all 15
   references with `canvas_width` / `canvas_height`. They have provably never
   diverged (all three assignments are identity copies).
7. **Collapse the three `ensure_*_layer` bodies** (`ensure_raster_layer`
   1371-1418, `ensure_void_layer` 2931-2986, `ensure_vector_layer`
   2998-3047) into one private
   `ensure_content_layer(device, queue, id, texture: LayerTexture,
   content: LayerContent, extra: ...)` that allocates the uniform buffer,
   builds default `BlendUniforms`, inserts into `node_textures` +
   `layer_cache`, and marks dirty. The three public entry points shrink to
   the per-kind tokens (texture constructor, content variant, the void's
   `create_cache` call, the vector's `vector_scenes` insert):
   `LayerKindGpu::realize_in` (382-421) already gives each variant the place
   to supply them. ~110 lines removed.

   **This is the one Stage A item that is not purely mechanical.** The three
   bodies diverge beyond the listed tokens: raster guards on
   `node_textures.contains_key` while void/vector guard on
   `layer_cache.contains_key`; raster calls `mark_node_pixels_dirty` (the
   thumbnail invariant documented at 1414) while void/vector call
   `mark_dirty`; vector allocates via `with_bounds_storage` and stores
   `LayerContent::Raster`. The shared helper takes the guard predicate and
   dirty call as per-kind inputs: do not silently unify them; if
   implementation reveals they *should* unify, that is a recorded decision,
   not a drive-by.
8. **`BlendUniforms::for_extent(opacity, blend_mode, isolated, extent)`
   constructor** replacing the 9 struct-literal sites.
9. **Share the lookup behind the two async-pass facades**: the content-bounds
   block (2296-2349) and histogram block (2351-2434) both start from "resolve
   node id → (view, w, h, format)". Extract
   `fn node_view_info(&self, id) -> Option<(&TextureView, u32, u32,
   wgpu::TextureFormat)>` and let both `request_*` paths use it. The
   forwarding one-liners (`poll_*`, `has_pending_*`) stay: a generic facade
   over two passes with different result types is machinery the DRY win
   doesn't pay for (decision recorded here for the reviewer).

Verification: the from-scratch byte-equality tests in
`tests/compositor_revisions.rs` (`assert_matches_from_scratch`,
`composite_runs`) are precisely the guard this stage needs; run the full GPU
suite single-threaded per CLAUDE.md.

### Stage B: the bake leak fix, then subsystem relocation

**B1. Fix the `bake_subtree_to_layer` sentinel leak (the one behavior
change).** Current behavior: the first Merge Down / Flatten inserts a
`GroupState` under `bake_parent = LayerId::from_ffi(0)` (3887-3899) into
`group_state` and never removes it, three canvas-sized RGBA8 textures
(~two accum + one composite cache) held for the rest of the session, plus
`blend_bind_groups` entries keyed `(bake_parent, child, idx)` created by the
walk. Fix, in `bake_subtree_to_layer` after `queue.submit`:

```rust
self.group_state.remove(&bake_parent);
self.blend_bind_groups.retain(|(p, _, _), _| *p != bake_parent);
self.revisions.bump_targets();
```

(the retain is required: without it the cached bind groups outlive the
textures they reference; the `bump_targets` matches the existing convention
that target recreation/destruction bumps that source). Also change the
signature `doc: &mut Document` → `doc: &Document`: the body only reads it
(verified: it reaches `sync_projection_states(&Document)` and
`compose_children(&Document)`); callers `engine/merge.rs:106,271` and
`engine/flatten.rs:61,234` update trivially.

**The trade, stated plainly:** the sentinel is an *intentional* cross-bake
cache, the guarded insert reuses three canvas-sized textures across merges.
This fix converts "leak for the session" into "allocate + `bump_targets` per
bake", and that bump also rebuilds every effect instance on the next sync.
Merge/flatten are user-action-rate operations, so the cost is acceptable,
but it is a cache removal, not pure cleanup.

Regression test (written FIRST, confirmed failing against unfixed code):
add `#[cfg(any(test, feature = "testing"))] pub fn test_group_state_count()`
beside `test_node_texture_count` (3363), an engine-side forwarder following
the `engine/mod.rs:998 test_node_texture_count` pattern, and in
`tests/layer_bake.rs`:

```text
merge_down_leaks_no_group_state:
  build 2-layer doc → record test_group_state_count() (== 1, root)
  → merge down → settle → assert count unchanged
  → merge again with two fresh layers → assert still unchanged
```

Fails before the fix (count becomes 2 and stays); passes after.

**B2-B7. Relocate the misplaced subsystems.** Each is a file move of an
`impl Compositor` block plus its private types; fields referenced across
files become `pub(super)` (most already are). No sub-struct is forced where
it would fight the borrow checker; two are introduced where the state is
self-contained:

| Move | New file | What goes | ≈lines |
| --- | --- | --- | --- |
| B2 | `gpu/bake.rs` | `bake_subtree_to_layer` (post-B1). Stays a Compositor service per prior art (GIMP/Krita: doc op in the image/engine (already `engine/merge.rs`/`flatten.rs`) renderer provides a stateless bake). | 110 |
| B3 | `gpu/void_content.rs` | `LayerContent`, `ProceduralContent`, `procedural_content(_mut)`, `ensure_void_layer`, `create_void_box`, `set_void_source_pixels`, `copy_void_source`, `update_void_layer_params`, `update_void_layer_transform`, `void_content_extent`, `void_persistent_frame_size`, `resync_voids_to_canvas`, `upload_void_external_image`, `encode_dirty_layer_content` (2498-2638, 2831-2848, 2919-2986, 3111-3225, 3300-3358) | 420 |
| B4 | `gpu/vector_content.rs` | `VectorContent` (renamed `VectorScene`), a **`VectorSubsystem` sub-struct** owning `vector_renderer` + `vector_scenes` (the two fields are only ever used together: 844-849, 3068-3109; dispose touches one map), plus `ensure_vector_layer`'s vector-specific tail, `set_vector_scene`, `ensure_vector_renderer`, `realize_dirty_vector_layers` | 160 |
| B5 | `gpu/frame_clock.rs` | the `impl Compositor` scheduler block: `update_animations`, `needs_animation`, `any_animated_layer`, `any_animated_effect`, `effect_animates`, `tick_animated_layers`, `tick_animated_effects`, `frame_count()` (3227-3298, 3383-3475). The `frame_count` + `last_wall_time` fields (830-834) stay on `Compositor`: a two-field sub-struct whose every method stays `impl Compositor` would add a name without a boundary; the file move is the win. | 260 |
| B6 | `gpu/compositor_test_harness.rs` (`#[cfg(any(test, feature = "testing"))] mod` in `gpu/mod.rs`) | `present_into_target`, `test_present_to_canvas`, `test_present_to_viewport`, `test_present_through_screen_run` (4032-4214) | 190 |
| B7 | `gpu/effect_layers.rs` | `EffectSpace`, `EffectInstance` (538-572), `sync_effect_instances` (4743-4944), `sync_effect_scale` (3606-3626), `apply_uniforms_for` (4704-4741), `effect_rebuilds`/`effect_reduced_size` accessors | 430 |

After Stage B, `compositor.rs` holds: free geometry fns + `run_filter_region`
(exemplary; stays), the struct, construction, the node-texture lifecycle
(realloc/swap/stage/upload/dispose), canvas rect + selection + projection
sync, the compose walk, and present. ≈4,000 → ≈3,600 lines in the main file
with ~1,570 relocated.

### Stage C: kill mirrors, unify the cache lifecycle

**C1. Delete the `isolated_node` mirror.** Isolation is Session state whose
single home is `engine.isolated_node` (`engine/mod.rs:427`). Instead of the
lockstep write (`engine/layers.rs:1509` → `compositor.set_isolated_node`),
pass it per frame: `Compositor::render` and `render_offscreen` gain an
`isolated: Option<LayerId>` parameter, threaded through
`CompositionContext` and function parameters wherever reachable; a
walk-scoped field is the fallback only where threading is impractical, since
it is still a compositor-held copy of session state (just push-per-frame
instead of lockstep dual-write). Consumers: `is_in_isolation_path` (2179)
and `sync_projection_states`' `isolated_node == Some(mask_id)` check (4555,
4736). `bake_subtree_to_layer` passes `None`, deleting its save/restore
dance (3883, 3968), which was only ever compensating for the mirror.

Pre-existing quirk, recorded so nobody "fixes" it mid-refactor:
`engine/rendering.rs:996 sync_compositor_layers` independently realizes
isolation into per-layer `BlendUniforms.isolated` bits from
`engine.isolated_node`. Bake's save/restore never covered that path: a
baked subtree composites through whatever isolated bits the uniforms already
carry. C1 leaves that behavior exactly as it is.

Delete:
`set_isolated_node` (2160), `test_isolated_node` (2166), the field (792),
`engine/layers.rs:1521 test_compositor_isolated_node`, and rework
`tests/engine.rs:4837-4862` to assert on `engine.isolated_node()` (the
behavior under test (isolation cleared when its node is deleted,
`engine/layers.rs:1189`) is engine-side and survives). Callers: engine's
render entry (`engine/rendering.rs:752`) passes `self.isolated_node`; export
and every other `render_offscreen` caller passes the same engine field, so
behavior (including "export honors isolation", today's semantics) is
unchanged.

**C2. Collapse the `LayerCache` blend mirror.** Replace the three loose
doc-fact copies (`opacity`, `blend_mode`, `isolated`, 307-317) with one
`last_uniforms: BlendUniforms`: a shadow of the *GPU buffer's last written
contents* (a write-only resource that legitimately needs a CPU shadow; three
copies of a document fact do not). `swap_node_texture` (1602-1613) patches
`layer_offset`/`layer_size` and rewrites; `update_layer_uniforms`
(3529-3566) overwrites whole; `sync_projection_states` (4551-4554) and the
floating-preview mirror (`floating_preview.rs`) read fields off the shadow.
Net: same information, one struct, and the resize path stops re-deriving a
`BlendUniforms` by hand.

**C3. `EffectInstance` fingerprints: evaluated, mostly kept.** The audit
reads `params`/`pipeline_id` as document mirrors that should be tick
comparisons per `gpu/revisions.rs`. Rejected, with reasoning the reviewer
should challenge: the stored `params` is not a validity mirror but the
*input record* the sync needs to decide between three outcomes; no-op,
in-place `set_params` (one buffer write), or rebuild-preserving-clocks via
`clone_boxed` (4844-4848). A per-layer document tick can answer "changed?"
but not "changed *how*", so the diff would survive any revision scheme.
`built_targets` already is a `Tick`. The structural fields (`space`,
`render_size`, `applied_scale`) describe compositor-internal resources, not
document facts. No change beyond the B7 relocation.

**C4. Fold `mask_bind_groups` into the node-texture slot.** A mask bind
group is derived 1:1 from an R8 node texture; keeping it in a parallel map is
the two-places-for-one-fact shape. Change `node_textures` to
`HashMap<LayerId, NodeSlot>` where
`struct NodeSlot { texture: LayerTexture, mask_bg: Option<wgpu::BindGroup> }`;
`ensure_node_texture` (2699) builds the bind group into the slot,
`swap_node_texture` rebuilds it in place (deleting the contains_key dance at
1623-1634), `dispose_node_texture` drops both with one `remove`. The public
`node_texture(id) -> Option<&LayerTexture>` accessor keeps its signature.
Two maps become one; the "did you evict both?" question becomes
unrepresentable.

**C5. `blend_bind_groups` invalidation: evaluated, kept as-is (revised
after review).** The original proposal stamped entries with
`revisions.targets()` and had `swap_node_texture` bump that source. The
review (R1) showed this is a painting-path regression: `swap_node_texture`
fires whenever a growing stroke crosses a `LAYER_GROWTH_CHUNK` boundary
(`engine/painting.rs:793,859`), and a `targets` bump makes
`sync_effect_instances` rebuild every effect instance
(compositor.rs:4817) besides flushing the whole blend-bg cache; the exact
churn `effect_rebuilds` exists to catch. It would also silently widen the
documented meaning of the `targets` source (`revisions.rs:65-69`). The
alternative (a dedicated node-texture-identity revision source) would add
a new `Revisions` source plus per-node ticks purely to replace three
retain/clear sites the review confirms are correct and local
(`swap_node_texture` 1618, `dispose_node_texture` 2862, `set_canvas_rect`
2095). That machinery doesn't pay for itself. Decision: keep the targeted
push-invalidation; its rule ("blend bind groups are evicted wherever the
texture they reference is replaced or destroyed") is codified in C7's
lifecycle doc so the tenth cache has a rule to follow. A swap-path pin test
is still added (see Tests) so any future change of mechanism has a guard.

**C6. Stop reading config at construction.** `pixel_filter_from_config()`
(36-38, used at 1321) makes the compositor reach into global config; the
engine already owns the push path (`set_pixel_filter`,
`engine/rendering.rs:159`). Initialize the field to auto and have engine
setup push the persisted value once at handle creation. The animation
divisor reads (3424-3426) stay: they are per-frame tunables read where
consumed, and moving them buys nothing.

**C7. Codify the lifecycle invariant.** After C4 the per-node maps are:
`group_state`, `node_textures` (slot), `layer_cache`, `mask_snapshot_state`,
`projection_states`, `effect_instances`, `vector_scenes` (inside
`VectorSubsystem`), plus the `blend_bind_groups` cache. Each is owned by
exactly one subsystem file with exactly one of three lifecycle disciplines:
**disposed in `dispose_node_texture`** (slot, layer_cache, vector,
projection, revisions), **swept against the document in its own sync phase**
(projection stale-sweep 4525-4533, effect retain 4772-4773, snapshot ensure
4621), or **evicted wherever its referenced texture is replaced or
destroyed** (`blend_bind_groups`, per C5). Document this in the struct's
doc comment so the tenth map has a rule to follow instead of a precedent to
copy.

**C8. (Deferred, recorded)** `histogram_target` / `node_histogram_target`
are UI-session facts stored on the compositor. There is no second copy
(engine doesn't mirror them) so unlike C1 nothing is duplicated; moving them
to the engine adds per-frame plumbing and deletes nothing. Leave them,
flagged here so the reviewer can disagree.

### Stage D: type-owned dispatch

1. **Move the Filter branch into the type.** Give `Layer` a
   `compose_into(&self, ctx: &mut CompositionContext)` in `layer.rs` (beside
   `LayerNode::compose_into`, 699) holding the variant match; the arms stay
   compositor-private, reached via the existing `pub(crate)` methods on
   `CompositionContext` (`compose_effect_arm` gains one). Delete the
   `if let Layer::Filter(f)` at compositor.rs:342; `compose_layer` becomes
   `layer.compose_into(self)`. A fifth in-place layer kind then edits
   `layer.rs` only.
2. **One home for "host's visible mask".** Add
   `Document::visible_mask_of(&self, host: LayerId) -> Option<LayerId>`:
   it's a pure document fact (`mask_filter(host).filter(visible)`). Replace
   the four re-derivations: `effective_mask_bind_group_fields` (3729-3733),
   `host_active_mask_for_projection` (4431-4435, becomes a one-line call or
   is deleted), `apply_uniforms_for` (4713-4716), `compose_group_arm`
   (5551-5554). The `compose_group_arm` masked/pure-passthrough choice then
   reads `doc.visible_mask_of(group_id).is_some()`: the branch on
   `group.passthrough` itself stays, because it *is* the group variant's own
   arm inspecting its own field, not consumer-side classification;
   `needs_before_snapshot` (`layer.rs:773`) remains the walk-level
   predicate.
3. **Route the five `LayerContent::Procedural` matches through owned
   iteration.** Add `procedural_entries(&self)` /
   `procedural_entries_mut(&mut self)` iterators next to
   `procedural_content` and rewrite 3172, 3235, 3286, 3320. Site 3343
   (`encode_dirty_layer_content`'s inline match) is a documented
   borrow-split against `node_textures` and moves to a field-explicit helper
   in `gpu/void_content.rs` where both fields are visible: the match
   collapses into the same helper the iterators use.

### Optional Stage E: extract the compose walk

Move `compose_group`, `compose_children`, the three compose arms,
`compose_layer_through_projection`, `compose_passthrough_masked`,
`snapshot_parent_accum`, `apply_in_place`, `encode_in_place_apply`, and
`sync_projection_states` (~1,150 lines, the audit's "irreducible" core) to
`gpu/compose_walk.rs` as an `impl Compositor` block: a pure file move
mirroring Krita's `kis_async_merger.cpp` split. Leaves `compositor.rs` at
roughly 1,800-2,000 lines: struct, construction, node-texture lifecycle,
present. Recommended, but it is reviewable noise with zero net LOC; do it
only if Stages A-D land cleanly and the user wants the final cut.

## Tests

**Existing coverage relied on (all must stay green, run with
`--features darkly/testing -- --test-threads=1`):**

- `tests/compositor_revisions.rs` (828 lines): frame gating,
  `composite_runs`, and the from-scratch byte-equality harness
  (`assert_matches_from_scratch`) that makes Stage A's mechanical rewrites
  safe.
- `tests/layer_bake.rs` (664 lines): merge/flatten pixel correctness; hosts
  the new leak regression test.
- `tests/effect_space.rs`, `tests/effect_scale.rs`, `tests/veil_alpha.rs`,
  `tests/blend_modes.rs`: effect-instance sync, scaling, screen run (B7/C
  guard).
- `tests/engine.rs`: isolation behavior (4837-4862, reworked in C1),
  dispose/leak cycles via `test_node_texture_count`.
- `tests/canvas_resize.rs`, `tests/image_rescale.rs`, `tests/flip_rotate.rs`:
  the `swap_node_texture` / `set_canvas_rect` paths C4/C5 touch.
- `tests/stroke.rs`, `tests/transform.rs`, `tests/paste_mask.rs`, floating
  preview + mask paths (C2 guard).

**New tests:**

1. **(B1, regression: written first, must fail pre-fix)**
   `merge_down_leaks_no_group_state` in `tests/layer_bake.rs`, plus the
   `test_group_state_count()` accessor. Optionally assert
   `test_node_texture_count` style symmetry after repeated merges.
2. **(C5)** `a_node_swap_invalidates_cached_blend_bind_groups`: paint, settle,
   resize a layer via the engine (drives `swap_node_texture`), render, and
   `assert_matches_from_scratch`, mechanism-agnostic pin that the swap path
   ends up compositing from the new texture, guarding both today's retain
   and any future change of invalidation scheme. Lives in
   `tests/compositor_revisions.rs` where the harness is.
3. **(C1)** rework of the two isolation assertions in `tests/engine.rs`
   (4837-4862) to engine-side state. No new output assertion needed:
   `tests/engine.rs:3941 isolate_skips_off_path_sibling_rasters` already
   asserts isolation against rendered pixels.

No new tests for Stages A/B2-B7/D: they are behavior-preserving moves and
rewrites under an existing byte-equality + full GPU suite; per the Testing
Principle features need tests, and no feature is added.

## Risks

- **GPU byte-drift from mechanical rewrites (Stage A).** Mitigated by the
  from-scratch equality harness; every helper substitution is
  argument-for-argument identical.
- **`pub(super)` widening during extraction (Stage B)** can quietly grow the
  compositor's internal surface. Rule: fields touched by exactly one sibling
  file stay `pub(super)`; anything needing wider visibility is a sign the
  move is cutting across a seam, stop and re-cut.
- **B1 cost**: each merge/flatten now reallocates the bake `GroupState` and
  bumps `targets`, which rebuilds effect instances on the next sync.
  User-action rate, acceptable, but if profiling ever disagrees, the fix is
  a bake-scoped state kept *and disposed* by the bake call, not a return to
  the session-lifetime sentinel.
- **C1 threading**: `render_offscreen` has several callers (render, export,
  test harnesses, bake); missing one changes isolation semantics silently.
  Grep-verified list at implementation time; the compiler enforces the new
  parameter.
- **B5 frame clock**: `update_animations` mutates `tool_overlay`,
  `effect_instances`, `layer_cache`; it must remain `impl Compositor`. Only
  the impl block moves; the two clock fields stay on `Compositor`.
- **Sequencing**: C4 touches `swap_node_texture` / `dispose_node_texture`
  (the same sites that hold C5's retained eviction lines); land C4 first so
  those lines settle against the final map shape.

## Unresolved questions

1. Should Stage E (compose-walk extraction) ship in this series or as a
   follow-up? (Plan: follow-up unless the user says otherwise.)
2. C8: do `histogram_target` / `node_histogram_target` stay on the
   compositor? (Plan: yes; no duplication exists; reviewer may disagree.)
3. Whether `apply_effect_to_region`'s inline bind-group/uniform build
   (1832-1899) should also route through the Stage A helpers: it shares the
   layouts but has region-local semantics. (Plan: yes for the descriptor
   helpers, no restructuring.)

## LOC estimate (added / removed, not touched)

Production numbers exclude pure relocation; "moved" counts lines that change
file but not content (they appear as +N/−N in the diff).

| Stage | Production added | Production removed | Moved | Tests added | Tests removed | Docs |
| --- | --- | --- | --- | --- | --- | --- |
| A (boilerplate | +150 | −600 | 0 | 0 | 0 | 0 |
| B) bake fix + relocation | +45 | −35 | ~1,570 | +55 | 0 | 0 |
| C (mirrors + lifecycle | +85 | −150 | 0 | +35 | −25 | 0 |
| D) dispatch | +55 | −85 | 0 | 0 | 0 | 0 |
| **Total (A-D)** | **+335** | **−870** | **~1,570** | **+90** | **−25** | this plan |
| E (optional) | +10 | −10 | ~1,150 | 0 | 0 | 0 |

Net production: **≈ −535 lines**, with `compositor.rs` going from 5,855 to
≈3,600 (A-D) or ≈2,400 (with E), and no file in the new set exceeding ~450
lines except the core walk. The dominant signal: this is a large-diff,
small-net-code refactor; most of the diff is deletion and relocation, and
the only semantic change (B1) is a three-line fix behind a failing-first
regression test.
