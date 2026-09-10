# Compositor Caching Audit

An inventory of every mechanism the compositor uses to avoid redundant GPU work
— across raster layers, effect layers (veils), voids, and vector layers — plus
an assessment of which ones earn their keep. Merged from two independent
analyses; every claim below was verified against the source.

Written against `ddcd909a` (`better-veils`). Line numbers will drift; symbol
names won't.

> **Superseded in part.** `docs/plans/compositor-revision-registry.md` has since
> been implemented, consolidating the push-side mechanisms this audit inventories
> into one `gpu/revisions.rs` registry. `needs_composite`, both `needs_present`
> flags, `cache_valid_through`, `mark_effect_dirty`, `dirty_node_pixels`, the
> free-standing `target_generation` counter, and the generation maps in
> `ContentBoundsPass` / `HistogramPass` no longer exist. The *tiering* this audit
> describes still holds — each tier still answers a different question — but the
> per-tier mechanisms are now revision stamps compared at the point of
> consumption. Read the tier analysis, not the mechanism names.

## Summary

The caching splits into five tiers, and the tiering itself is sound — each tier
answers a different question and the tick scheduler composes cleanly with all of
them. But the compositor is well defended against **reallocating GPU objects**
(bind groups, pipelines, effect instances, projection states) and almost
undefended against **redoing GPU work** (blend passes, composite walks). The two
mechanisms designed to provide work caching are dead code, one fine-grained
invalidation path exists but is unwired, one animation gate is a live regression
introduced by the veils-as-layers refactor, and the divisor clock's delta
scaling assumes a frame rate it cannot rely on.

## 1. Inventory

### 1.1 Frame-level gates

These skip whole passes for a frame.

| Flag | Location | Gates |
| --- | --- | --- |
| `Compositor::needs_composite` | [compositor.rs:700](../crates/darkly/src/gpu/compositor.rs#L700) | `render_offscreen` (the tree walk) |
| `Compositor::needs_present` | [compositor.rs:702](../crates/darkly/src/gpu/compositor.rs#L702) | present + screen-space run |
| `ScreenRun::needs_present` | [screen_run.rs:44](../crates/darkly/src/gpu/screen_run.rs#L44) | same, second copy |
| `DarklyEngine::frame_needs_more()` | [rendering.rs:761](../crates/darkly/src/engine/rendering.rs#L761) | whether JS reschedules rAF |

`has_pending_work()` ORs the first three; a false result returns from
`Compositor::render` before acquiring a surface texture. `render_offscreen`
re-checks `needs_composite` itself, so a present-only frame skips the tree walk
but still blits. The JS rAF loop is demand-driven off `frame_needs_more()`, so
when all gates are false, zero GPU work happens.

`ScreenRun::needs_present` exists because `ScreenRun` sets the flag from inside
its own `resize` / `sync_resolution_scale`, where it has no handle on the
compositor. Its lifetime is identical to `Compositor::needs_present`, both are
cleared together in `finish_present`, and
[layers.rs:1763-1764](../crates/darkly/src/engine/layers.rs#L1763-L1764) has to
set both by hand — the manual-double-mark shape the thumbnail write-site
invariant exists to prevent.

### 1.2 Per-object content dirt

These skip regenerating one object's pixels while the rest of the frame
proceeds. Three parallel schemes for the same concept:

- **Voids**: a sticky `DirtyFlag` embedded on the trait object
  ([void.rs:115](../crates/darkly/src/gpu/void.rs#L115)); state-changing methods
  (`update_params`, `update_time`, `upload_external_image`, `set_transform`)
  mark it, and `take_dirty()` returns-and-clears, so a void re-encodes exactly
  once per state change. Consumed by `encode_dirty_layer_content`
  ([compositor.rs:3276](../crates/darkly/src/gpu/compositor.rs#L3276)) at the top
  of `render_offscreen`.
- **Vector layers**: a separate compositor-side `VectorContent::dirty`
  ([compositor.rs:458](../crates/darkly/src/gpu/compositor.rs#L458)), consumed by
  `realize_dirty_vector_layers`
  ([compositor.rs:3048](../crates/darkly/src/gpu/compositor.rs#L3048)). Set when
  the engine pushes a new scene, never on view zoom or pan.
- **Raster layers**: none. Pixels arrive through paint and the node texture is
  authoritative, so there is no regeneration step to skip.
- **Effect layers**: none. An effect encodes every composite; its cost is the
  pass itself, which nothing gates below the frame level.

`dirty_node_pixels: HashSet<LayerId>`
([compositor.rs:712](../crates/darkly/src/gpu/compositor.rs#L712)) looks like a
fourth entry but is not a render gate — the engine drains it each frame to queue
thumbnail readbacks.

### 1.3 Fingerprint-based rebuild avoidance

These avoid reallocating GPU resources, not passes. The compare-and-skip shape
is hand-rolled independently at least six times, never with hashing (plain
equality everywhere; `gpu/hash.rs` is shader pseudo-randomness, unrelated):

- **`EffectInstance`** — the central one: a five-field fingerprint
  (`pipeline_id`, `space`, `render_size`, `target_generation`, `params`)
  compared in `sync_effect_instances`
  ([compositor.rs:4717](../crates/darkly/src/gpu/compositor.rs#L4717)). A
  structural match plus unchanged params is a no-op; a structural match with new
  params tries `Effect::set_params` in place (usually a uniform write) before
  falling through to a rebuild. `target_generation`
  ([compositor.rs:759](../crates/darkly/src/gpu/compositor.rs#L759)) is an epoch
  counter bumped whenever any accumulator is recreated, so one comparison covers
  canvas resize, crop, group creation, and screen-run scale changes.
  `effect_rebuilds`
  ([compositor.rs:753](../crates/darkly/src/gpu/compositor.rs#L753)) is the
  telemetry: steady state is one build per effect layer for the life of the
  document; growth with frame count means thrashing.
- **`ProjectionState`**: padded-dimension check in `ensure_projection_state`
  ([compositor.rs:4369](../crates/darkly/src/gpu/compositor.rs#L4369)).
- **`ScreenRun::applied_scale`**: epsilon comparison against the config value
  ([screen_run.rs:126](../crates/darkly/src/gpu/screen_run.rs#L126)).
- **`Pixelate::built_scale`**: private fingerprint of the pass-chain shape; a
  changed block size forces rebuild (each halving is an aux texture + pass).
- **`TexturedVoid`**: `canvas: CanvasRect` equality early-out in
  `set_canvas_rect`; aux-texture realloc only on source-dimension change
  ([textured_void.rs:680](../crates/darkly/src/gpu/textured_void.rs#L680)).
- **`canvas_apply_scratch`**: size-compare realloc
  ([compositor.rs:4590](../crates/darkly/src/gpu/compositor.rs#L4590)).

Plus two CPU mirrors that avoid GPU readbacks rather than reallocation:
`LayerCache`'s `opacity` / `blend_mode` / `isolated`
([compositor.rs:303](../crates/darkly/src/gpu/compositor.rs#L303)) and
`cached_view_transform`.

### 1.4 Bind-group and pipeline caches

- `blend_bind_groups: HashMap<(LayerId, LayerId, u8)>`
  ([compositor.rs:652](../crates/darkly/src/gpu/compositor.rs#L652)), keyed
  `(parent_group, child, src_accum_idx)` — both ping-pong sides get an entry per
  child because the source accumulator view flips every layer. Invalidated per
  node in `dispose_node_texture` / `resize_node_texture`.
- `mask_bind_groups`, `default_mask_bind_group`, `present_cache_bind_group`,
  `ScreenRun::blit_bind_groups`.
- Lazy pipeline caches in **both** registries, independently implemented:
  `EffectRegistry.pipelines` ([effect.rs:447](../crates/darkly/src/gpu/effect.rs#L447))
  and `VoidRegistry`'s per-entry `cached_pipeline`
  ([void.rs:480](../crates/darkly/src/gpu/void.rs#L480)).
- Brush side: `TextureRegistry` caches layouts per texture count;
  `BakedSourceCache` caches baked noise tiles keyed by `BakeSpec`.

### 1.5 Async derived-value caches

`ContentBoundsPass` ([content_bounds.rs](../crates/darkly/src/gpu/content_bounds.rs))
and `HistogramPass` ([histogram.rs](../crates/darkly/src/gpu/histogram.rs)) both
hold:

```rust
cached: HashMap<LayerId, T>,
generation: HashMap<LayerId, u64>,
pending: Vec<PendingX>,
```

with structurally identical `invalidate` / `invalidate_all` / `is_pending` /
`poll` bodies, including the "discard results whose generation moved" rule. The
histogram's module header says "Modeled on" `ContentBoundsPass` — a stop-sign
phrase per CLAUDE.md's DRY section. The bbox half of the overlap was already
extracted into `BboxReduction`
([bbox.rs](../crates/darkly/src/gpu/bbox.rs)) and shared with `DiffRectPass`;
the generation-cache half was not.

Related: the selection filter's CPU cache + `pixel_bounds`
([document/filters/selection.rs:25](../crates/darkly/src/document/filters/selection.rs#L25))
mirror the GPU R8 texture — the principled bulk-pixel exception — but
invalidation is manual and scattered across 8+ call sites (selection, mask,
canvas resize, canvas transform, transform commit); missing one is a standing
stale-cache bug class.

### 1.6 Stale-async-result rejection

Five hand-rolled "generation counter drops stale result" implementations:
`ContentBoundsTracker.generation`, `transform_setup_generation`
([engine/mod.rs:613](../crates/darkly/src/engine/mod.rs#L613)),
`TransformSession.preview_revision`
([floating.rs:77](../crates/darkly/src/engine/floating.rs#L77)), the brush
`graph_version` / `topology_version` pair
([brush_graph.rs](../crates/darkly/src/engine/brush_graph.rs)), and
`Document::revision` sampled by the process recorder. Same two-line pattern; a
shared abstraction would likely be ceremony — listed for completeness, not
action.

## 2. The structural gap: no composite-level work caching

Every one of the ~74 `mark_dirty()` call sites means the same thing: re-walk and
re-blend the entire tree. There is no layer-level, group-level, or region-level
skip. The two mechanisms built to provide one are inert, and the one built to
make screen-space edits cheap is unwired.

### 2.1 `cache_valid_through` is dead

`GroupState::cache_valid_through: Option<usize>`
([compositor.rs:290](../crates/darkly/src/gpu/compositor.rs#L290)) is documented
as "child index through which the cache is valid. None = cache is empty, must
composite from scratch."

It is assigned `None` in four places
([928](../crates/darkly/src/gpu/compositor.rs#L928),
[2205](../crates/darkly/src/gpu/compositor.rs#L2205),
[3839](../crates/darkly/src/gpu/compositor.rs#L3839),
[4229](../crates/darkly/src/gpu/compositor.rs#L4229)) and is **never read and
never set to `Some`**. Partial-composite caching (composite children `0..k`
once, then re-run only the children above the one that changed) was designed and
never wired up. It is the one mechanism here with large frame-time upside: it
would make animated-void ticks and painting-in-progress stop paying full-tree
recomposite whenever the changing layer sits above the bottom of the stack.

### 2.2 `scissor` is a constant threaded through the walk

The `scissor: (u32, u32, u32, u32)` parameter is carried through eight compose
functions and stored on `CompositionContext`
([compositor.rs:332](../crates/darkly/src/gpu/compositor.rs#L332)). Both call
sites pass the full canvas
([compositor.rs:3829](../crates/darkly/src/gpu/compositor.rs#L3829) for bake,
[compositor.rs:3914](../crates/darkly/src/gpu/compositor.rs#L3914) for
`render_offscreen`), so it is derivable from `self` at every point of use.
Dirty-rect compositing is the feature it was shaped for, and that feature does
not exist. It shares a fate with 2.1: delete both, or implement the region-level
caching they were built for.

### 2.3 `mark_effect_dirty` is unwired

[`Compositor::mark_effect_dirty`](../crates/darkly/src/gpu/compositor.rs#L3576)
implements exactly the right fine-grained invalidation — a screen-space effect
edit routes to `screen_run.mark_needs_present()` (re-present only), a
canvas-space edit to `mark_dirty()` — but has **zero callers**. The live path,
`Engine::update_filter_params`
([layers.rs:870](../crates/darkly/src/engine/layers.rs#L870)), calls
`compositor.mark_dirty()` unconditionally, so dragging a slider on a
viewport-only effect recomposites the entire canvas for nothing. Its doc comment
also claims resources are rebuilt "keyed by the param fingerprint" when the
actual mechanism is `EffectInstance`'s `Vec<ParamValue>` equality (1.3).

### 2.4 `composite_cache` is barely a cache

Each `GroupState` owns a canvas-sized `composite_cache` texture
([compositor.rs:286](../crates/darkly/src/gpu/compositor.rs#L286)) that
`compose_group` fills with an unconditional `copy_texture_to_texture` from the
final accumulator on every composite
([compositor.rs:4260](../crates/darkly/src/gpu/compositor.rs#L4260)).

It is read across frames in exactly one way: present-only frames (pan/zoom)
re-present by sampling root's `composite_cache` from the previous composite. But
that role only requires a **stable texture view** — `current_accum` flips an
unpredictable number of times during the walk, and a bind group must be built
against a fixed view. `blend_bind_groups` already solves that exact problem for
children by keying on `src_accum_idx` and selecting the variant at draw time.
Applying the same approach to a group's output would remove a full-canvas copy
per group per composite and one canvas-sized texture per group of VRAM.

Consumers to audit before changing this: `composited_texture()` /
`composited_view()`
([compositor.rs:3533](../crates/darkly/src/gpu/compositor.rs#L3533)) are read by
export, save, process recording, previews, engine, and painting. They would
return `accum.textures[current_accum]`, which is stable between the end of a
composite and the next one (accumulators are only touched inside
`compose_group`, which clears `views[0]` at entry).

## 3. Bugs in the tick path

### 3.1 Canvas-space animated effects never animate (live regression)

[compositor.rs:3406-3424](../crates/darkly/src/gpu/compositor.rs#L3406-L3424):

```rust
let canvas_fires = canvas_divisor > 0
    && self.any_animated_layer(doc)      // procedural VOIDS only
    && self.frame_count.is_multiple_of(canvas_divisor);

if canvas_fires {
    self.tick_animated_layers(queue, dt * canvas_divisor as f32, doc);
    self.tick_animated_effects(queue, dt * canvas_divisor as f32, doc, false);
    self.needs_composite = true;
}
```

`any_animated_layer`
([compositor.rs:3203](../crates/darkly/src/gpu/compositor.rs#L3203)) only
inspects `LayerContent::Procedural`, so it answers for voids and nothing else.
The canvas-space half of `tick_animated_effects` is gated on an unrelated fact.
`needs_animation()`
([compositor.rs:3434](../crates/darkly/src/gpu/compositor.rs#L3434)) has the
same hole: overlay, `any_animated_screen_effect` (screen side only), and
`any_animated_layer` — there is no `any_animated_canvas_effect`.

Consequence: `rainy_glass`, `grain`, and `vhs` (the three effects declaring
`needs_animation`) placed **below** the divider neither advance their clock nor
keep the rAF loop alive. They animate only as a side effect of an animated void
being present in the same document.

Provenance (verified): `c2895130 veils as normal layers wip`. The parent commit
had a dedicated `effect_fires` gate asking
`self.effect_chain.needs_animation()`; when the chain was split into two spaces,
the screen half got `screen_fires` and the canvas half was folded into the void
gate rather than getting its own predicate.

**Fix**: add `any_animated_canvas_effect(doc)`, symmetric with
`any_animated_screen_effect` (iterate `effect_instances`, keep
`space != Screen`, require `effect.needs_animation()` and
`doc.effective_visible`), and fold it into both `canvas_fires` and
`needs_animation()`.

**Regression test**: a document whose only animated content is a canvas-space
`grain` layer must report `frame_needs_more() == true` and must advance the
effect's clock across frames. This fails before the fix.

### 3.2 The divisor clock's delta scaling assumes uniform frame times

Each subsystem advances by `dt * divisor`, where `dt` is the delta since the
**previous rAF frame**, not since that subsystem last fired. With frames at
`t0..t3` and divisor 2, the fire on frame 2 advances by `(t2 - t1) * 2` when the
true elapsed time is `t2 - t0`. These agree only under a perfectly uniform frame
rate; under any variance (a stall, a long paint frame, a throttled tab)
animation speed jitters.

**Fix**: a per-subsystem dt accumulator — add `dt` every frame, consume and zero
on fire. Exact, allocation-free, and preserves the integer-divisor alignment
property described at
[compositor.rs:3369](../crates/darkly/src/gpu/compositor.rs#L3369).

Minor related wart: `grain`'s `update_time` ignores `dt` entirely (steps
`frame_count += 1.0` per tick), so its speed is tick-rate-dependent, unlike
`rainy_glass` and `vhs`.

### 3.3 Coarse invalidation is over-triggered

`mark_dirty()`
([compositor.rs:2200](../crates/darkly/src/gpu/compositor.rs#L2200)) nulls every
group cache **and** calls `content_bounds.invalidate_all()`;
`mark_node_pixels_dirty()` additionally calls `histogram.invalidate_all()` —
even though both are usually called with a specific node in hand and both passes
expose per-layer `invalidate`.

Worse, `poll_pending`
([rendering.rs:665](../crates/darkly/src/engine/rendering.rs#L665)) calls global
`mark_dirty()` whenever **any** readback completes — so a color-pick or
thumbnail readback landing forces a full recomposite and throws away every
layer's cached content bounds, for an event that changed no document pixels.

## 4. What the tick machinery gets right

Recorded so it does not get "optimized" away during cleanup. Any consolidation
must preserve all four properties:

- `frame_count` advances exactly once per `update_animations`, and every
  divisor-throttled subsystem tests the same counter, so a divisor-4 tick always
  coincides with a divisor-2 tick. No subsystem forces a frame another would not
  already produce. The counter is exposed via `frame_count()` so JS-side
  throttles (the camera void's upload) stay phase-locked to the same clock.
- On a non-firing frame nothing sets `needs_present`, `has_pending_work()`
  returns false, and `Compositor::render` returns before acquiring a surface. An
  overlay-only animation at divisor 4 costs three empty wasm calls and zero GPU
  work.
- Visibility is queried through `doc.effective_visible` at exactly the point the
  compose walk would drop the layer's output, rather than from a precomputed set
  that could drift from the document.
- Canvas-side firing sets `needs_composite` (document content needs a full
  composite); screen and overlay firing set only `needs_present`. That split is
  correct.

One consequence worth knowing (a design cost of the global flag, not a bug):
`canvas_fires` sets `needs_composite` whenever any visible animated void
*exists*, independent of whether the void produced a new frame this tick — an
unfrozen camera void with no fresh upload still recomposites the whole tree
every canvas tick. The `DirtyFlag` spares only the void's own encode, not the
walk. Fixing this properly is 2.1's partial-composite caching.

## 5. Duplication catalog

Beyond the items already covered (two `needs_present` flags in 1.1, three
per-layer dirty schemes in 1.2, six fingerprint sites in 1.3, twin generation
caches in 1.5, five stale-result counters in 1.6):

- **Two registries, one shape.** `EffectRegistry`
  ([effect.rs:447](../crates/darkly/src/gpu/effect.rs#L447)) and `VoidRegistry`
  ([void.rs:476](../crates/darkly/src/gpu/void.rs#L476)) independently implement
  registration map, lazy Arc'd pipeline cache, `static_type_id`, catalog,
  display metadata, and a preview-session pair. [void.rs:554](../crates/darkly/src/gpu/void.rs#L554)
  still says "Mirrors `VeilRegistry::static_type_id`" — a stale name and an
  explicit keep-in-sync marker.
- **Two in-place param-adoption protocols.** `Effect::set_params -> bool`
  (validity answer) vs `Void::update_params` (mutate in place, no answer) plus
  `Void::preview_at -> bool` carrying the validity answer only on the preview
  path. Same problem, three shapes.
- **Three copies of "pad params to schema defaults":** `pad_to_schema`
  ([param_effect.rs:114](../crates/darkly/src/gpu/param_effect.rs#L114)),
  `normalize_params`
  ([textured_void.rs:168](../crates/darkly/src/gpu/textured_void.rs#L168)), and
  the inline default-fill in `VoidSession::set_t`.
- **Belt-and-suspenders dirtying:** every engine void mutator both marks the
  void's `DirtyFlag` (inside the void) and calls `compositor.mark_dirty()`. The
  pair is intentional (flag = re-encode texture, mark_dirty = recomposite tree)
  but they always travel together from the engine.

## 6. Per-frame churn and naming

- `sync_effect_instances`
  ([compositor.rs:4654](../crates/darkly/src/gpu/compositor.rs#L4654)) clones a
  `String` + `Vec<ParamValue>` per filter layer per frame purely to compare
  against the stored fingerprint, calls `doc.all_filter_layers()` (allocating)
  twice, and `doc.screen_space_run().to_vec()` once. It runs **twice per frame**
  whenever the screen run is non-empty — once from the compose path, once from
  the present path ([compositor.rs:3703](../crates/darkly/src/gpu/compositor.rs#L3703));
  the second call is deliberate and documented, but it doubles the churn.
- `needs_animation` is recomputed 2-3× per frame (twice for gating in
  `update_animations`, again for `frame_needs_more`) — full O(layers + effects)
  scans, derived each time rather than tracked.
- `sync_effect_instances` is called from **inside** `sync_projection_states`
  ([compositor.rs:4536](../crates/darkly/src/gpu/compositor.rs#L4536)), which
  also ensures mask-snapshot states
  ([4532](../crates/darkly/src/gpu/compositor.rs#L4532)) and writes their apply
  uniforms. Three unrelated pre-walk syncs live under a name describing one of
  them; they are siblings ("make sure the encode-only walk finds everything
  built") and belong under a single pre-pass that calls all three.

## 7. Recommendations, ranked

1. **Fix the canvas-space animation gate** (3.1). A live regression with a small
   fix and a clear regression test.
2. **Wire `mark_effect_dirty` into `update_filter_params`** (2.3), and **stop
   `poll_pending` calling global `mark_dirty` for non-pixel readbacks** (3.3).
   Both are small and eliminate whole classes of pointless recomposites.
3. **Decide the fate of `cache_valid_through` + `scissor`** (2.1, 2.2): delete
   both, or implement partial-composite caching. Implementing is the only item
   here with large frame-time upside for animated-void and painting workloads;
   as they stand they read as an optimization that exists but doesn't.
   **Direction resolved**: `handoff-viewport-boundary.md` §3.2 records the
   user-endorsed shape — extend the voids' `DirtyFlag` protocol to effect
   encodes and finish per-layer dirty marking so the compose walk carries a
   running "anything below me changed" bit (measured motivation: `painting`
   costs +43.5ms/frame re-encoding on every dirty frame at 2048²). Needs a
   plan; not yet started.
4. **Per-subsystem dt accumulators** (3.2).
5. **Extract the generation-keyed async cache** shared by `ContentBoundsPass`
   and `HistogramPass` (1.5), the way `BboxReduction` was extracted, and switch
   the `invalidate_all()` calls in `mark_dirty` / `mark_node_pixels_dirty` to
   per-layer `invalidate` where the caller has the node id.
6. **Replace `composite_cache` with accum-indexed bind groups** (2.4), after
   auditing the `composited_texture()` consumers. Removes a canvas-sized copy
   per group per composite and one texture per group of VRAM.
7. **Collapse `ScreenRun::needs_present` into the compositor's flag** (1.1) —
   the internal setters can return a bool the compositor ORs in.
8. **Regroup the pre-walk syncs and stop cloning fingerprints every frame** (6);
   **merge the registries' shared shape and unify the per-layer dirty schemes**
   (5). Pure DRY consolidation, no behavior change.

Leave alone: the five stale-result generation counters (1.6 — a shared
abstraction would be ceremony), the six fingerprint sites individually (each
fingerprints genuinely different facts; the churn fix in 6 covers the costly
one), and the master-clock design itself (4).

## 8. Verdict

Not too many caches in the sense of redundancy — but the wrong ones. The
compositor is thoroughly defended against reallocating GPU objects and
undefended against redoing GPU work, with two dead subsystems standing where the
work caching was meant to go, one fine-grained invalidation path built but never
wired, and one animation gate that stopped working when veils became normal
layers.
