# Rust Complexity Audit

A crate-wide survey of `crates/darkly/src/` answering one question: which area
is most overly complex, awkward, or bloated *by the codebase's own standards*
(CLAUDE.md's DRY, Modularity / type-owned dispatch, Ownership, and Document
Authority principles). Findings were gathered by three independent read-only
audits (gpu/compositor, engine/, brush/ + mask/selection) and cross-checked
against the repo's prior audits (`docs/compositor-caching-audit.md`,
`old-handoffs/handoff-gpu-cleanup-compositor-decompose.md`).

Written against the `better-veils` working tree after `c9e7e4c7`. Line numbers
will drift; symbol names won't.

## Verdict

**`gpu/compositor.rs` is the worst offender, and it isn't close.** At 5,855
lines it is a 3× outlier over the next-largest file in the crate, holds ~26
distinct responsibilities behind one 45-field struct, and violates every named
CLAUDE.md principle simultaneously. The repo's own prior audit
(`old-handoffs/handoff-gpu-cleanup-compositor-decompose.md`) called it a
"3194-LOC kitchen sink"; it has since nearly doubled.

Runners-up, in order:

1. **The dead CPU selection stack** (`mask.rs` + `tile.rs`) — the most
   egregious *pure bloat*: ~700 production lines + ~560 test lines with zero
   production callers, kept in agreement with the live GPU path by a dedicated
   test. Largest zero-risk deletion available.
2. **`ReadbackContext`** — the worst *dispatch* violation: an 18-variant enum
   feeding a 243-line central match, next door to `engine/protocol/` which
   solves the identical problem with a registry.

Areas that came back clean: `engine/protocol/`, `engine/load.rs`'s
registry-driven staging, the brush node registration/eval architecture, the
dual stroke/preview shader machinery, and the mask/selection modifier split.
The modular machinery works where it's applied; the compositor is where it
stopped being applied.

---

## 1. `gpu/compositor.rs` — god object

### 1.1 Responsibility inventory

~26 responsibilities behind one `impl Compositor` (line 852) and a 230-line
struct (620–850, ~45 fields, nine of them parallel `HashMap<LayerId, _>`
caches). Highlights, with approximate line ranges:

| Responsibility | Lines | ≈LOC |
| --- | --- | --- |
| Free geometry math + `run_filter_region` plumbing | 26–218 | 190 |
| Type definitions (`GroupState`, `LayerCache`, `EffectInstance`, …) | 220–618 | 400 |
| `Compositor` struct — 45 fields | 620–850 | 230 |
| Construction + inline pipeline/shader/texture setup | 852–1333 | 480 |
| Per-kind layer realization (`ensure_*_layer`) | 1371–1418, 2699–2738, 2931–3047 | 220 |
| Node-texture lifecycle (realloc, staging, swap, rescale, snapshot) | 1420–2010 | 590 |
| Void subsystem (source pixels, mips, external image, params) | 2498–2638, 3111–3225 | 260 |
| Vector/Vello subsystem | 2988–3109 | 120 |
| Animation scheduler + frame clock | 3227–3475 | 250 |
| Present + screen-space effect chain | 3747–3849 | 103 |
| `bake_subtree_to_layer` — Merge Down / Flatten, a *document op* | 3867–3971 | 105 |
| Test-only present harnesses in the production file | 4032–4214 | 183 |
| Composite tree walk + blend bind-group cache | 4216–4423 | 208 |
| Projection/effect sync + all compose arms | 4425–5694 | 1,270 |

The void subsystem, Vello subsystem, animation scheduler, merge/flatten
document op, and test harnesses total ~900 lines that have a better home. A
second `impl Compositor` already exists in `gpu/floating_preview.rs:44`,
proving the extraction pattern works and was simply not continued.

### 1.2 DRY violations

The single worst pattern: **~900 lines of hand-rolled wgpu descriptor
boilerplate across 62 sites**, while the fixes sit unused in the same module.
`gpu/mod.rs` exports `blit_region` (:57), `create_texture_with_view` (:28),
and `clear_view_transparent` (:5) — used by `floating_preview.rs` and
`preview.rs`, used **zero** times by compositor.rs, which instead hand-rolls
12 `copy_texture_to_texture` blocks, 6 `TextureDescriptor` literals, and 3
transparent-clear passes.

| Literal | Sites | ≈total lines |
| --- | --- | --- |
| `copy_texture_to_texture` + copy-info + `Extent3d` | 12 | 240 |
| `create_bind_group(&BindGroupDescriptor{...})` | 11 | 200 |
| `begin_render_pass(&RenderPassDescriptor{...})` | 12 | 170 |
| `create_texture(&TextureDescriptor{...})` | 6 | 108 |
| `BlendUniforms { .. }` literals | 9 | 81 |
| `create_buffer(&BufferDescriptor{...})` | 10 | 70 |
| Duplicated present-pipeline descriptors (1155/1185) | 2 | 57 |

Roughly 550–600 of these lines are mechanically removable — ~10% of the file.

Specific duplications:

- **Two present pipelines are a verbatim copy differing in one field**
  (compositor.rs:1155–1182 vs 1185–1213) — 60 lines after `new()` solves the
  identical problem with a `let make = |format| {...}` closure (1061–1097),
  and `gpu/paint_target.rs:996` does the same for 14 pipelines.
- **The three `ensure_*_layer` bodies are the same ~45-line function three
  times** (1371–1418, 2931–2986, 2998–3047): allocate texture, build
  `BlendUniforms`, create + write uniform buffer, insert into `node_textures`
  + `layer_cache`, mark dirty. Only the texture constructor and `LayerContent`
  variant differ; `LayerKindGpu::realize_in` (382–421) already gives the
  variant a place to supply those tokens. ~110 lines removable.
- **The 4-entry blend bind-group descriptor is written four times**
  (4225–4246, 4271–4293, 5487–5508, 1850–1871); the doc comment at 4249
  admits the second is "the cached entry point for" the first, then re-types
  the body.
- **The blend draw sequence is copy-pasted at four sites** (5059–5078,
  5105–5124, 5248–5266, 5619–5637), while its apply-pass counterpart was
  already extracted (`encode_in_place_apply`, 5473).
- **The `match format { R8Unorm => …, Rgba8Unorm => … }` texture dispatch is
  written three times** (1471–1477, 1530–1536, 2707–2737).
- **Two structurally identical 5-method async-pass facades** (content-bounds
  2296–2349, histogram 2353–2434) — 137 lines of the same
  get/request/poll/has_pending/is_pending shape.

### 1.3 Type-owned dispatch violations

- compositor.rs:342 — `if let Layer::Filter(f)` inside
  `CompositionContext::compose_layer`, i.e. *inside the dispatch hop that
  exists to prevent it*. `layer.rs:715 composites_in_place()` answers exactly
  this question and is not consulted. A fifth in-place layer variant forces an
  edit here.
- compositor.rs:5547–5554 — `compose_group_arm` branches on
  `group.passthrough` and re-derives "does this group have a visible mask",
  when `layer.rs:773 needs_before_snapshot()` exists specifically so (per its
  own doc comment) "the compositor never branches on which kind it is looking
  at".
- The "host's visible mask filter" query is re-implemented four times
  (3729–3733, 4432–4434, 4713–4716, 5551–5554) instead of calling
  `host_active_mask_for_projection` (4431).
- Five sites pattern-match `LayerContent::Procedural` (3172, 3235–3238,
  3286–3288, 3320–3327, 3343–3352) despite `procedural_content()` /
  `procedural_content_mut()` (2835/2843) being documented as centralizing that
  lookup so "the rest of the compositor never pattern-matches on
  `LayerContent` directly".

### 1.4 Document Authority violations

- **`isolated_node` (compositor.rs:792)** — comment says "Mirrored from
  `engine.isolated_node`" (`engine/mod.rs:427`, written in lockstep at
  `engine/layers.rs:1504–1509`). A `test_compositor_isolated_node()` accessor
  (2166) exists solely to assert the two copies agree — a test for a mirror
  that shouldn't exist. Verbatim CLAUDE.md anti-pattern.
- **`LayerCache.opacity` / `.blend_mode` / `.isolated` (307–313)** — the same
  fact in three places: document `layer.blend`, this CPU mirror, and the GPU
  uniform buffer. Justified at 305–306 as convenience for the floating-preview
  path — "the same logical fact stored in two places 'for ergonomics'".
- **`padded_width` / `padded_height` (729–730)** — only ever assigned
  `= width` / `= height` (1295–1296, 2060–2061); two dead fields duplicating
  `canvas_width`/`canvas_height`, referenced at 14 sites.
- **`EffectInstance` fingerprint fields (551–571)** — `params`, `pipeline_id`,
  `space`, `render_size`, `applied_scale` are a field-by-field mirror of the
  document's `FilterLayer`, hand-diffed in `sync_effect_instances`
  (4807–4819). `gpu/revisions.rs:9–20` states validity should be a tick
  comparison, not a stored copy; only `built_targets` uses a `Tick`.

### 1.5 Ownership violations

- **`bake_subtree_to_layer` (3867–3971)** — a document operation (Merge Down /
  Flatten) living on the compositor, taking `&mut Document`, saving/restoring
  `isolated_node` around itself, and **leaking a permanent sentinel
  `GroupState`** (`bake_parent = LayerId::from_ffi(0)` inserted at 3898, never
  removed — three canvas-sized RGBA8 textures held for the rest of the session
  after the first merge).
- **`update_animations` (3411–3464)** — a frame scheduler reading three global
  config keys per frame; session state.
- **`cached_view_transform`, `viewport_bg`, `pixel_filter` (805–814)** —
  viewport transform is Session state per CLAUDE.md; the compositor also reads
  global config at construction (`pixel_filter_from_config()`, 36–38).
- **`histogram_target` / `node_histogram_target` (823–827)** — "which panel
  the user has open" is UI state.
- **Test harnesses (4032–4214, 183 lines)** — `present_into_target`,
  `test_present_to_canvas`, `test_present_to_viewport`,
  `test_present_through_screen_run` belong with `gpu/test_utils`.
- **Borrow-split symptom:** four helpers exist purely to hand-split borrows of
  the oversized struct — `effective_mask_bind_group_fields` (3721),
  `get_or_create_blend_bind_group` (4259), `encode_in_place_apply` (5473),
  `split_overlay_and_selection` (3689). "Don't let the borrow checker dictate
  the data model."

### 1.6 Cache-invalidation sprawl

Nine parallel `HashMap<LayerId, _>` fields (`group_state`, `node_textures`,
`mask_bind_groups`, `blend_bind_groups`, `layer_cache`, `mask_snapshot_state`,
`projection_states`, `effect_instances`, `vector_scenes`). Every lifecycle
event must touch the right subset by hand: `dispose_node_texture` (2855–2872)
hits six maps plus `revisions`; `set_canvas_rect` (2047–2119) clears three and
rebuilds all group states; `swap_node_texture` (1587–1635) rewrites a uniform,
retains `blend_bind_groups`, rebuilds a mask bind group. "Did you remember to
evict from all nine?" is an unenforceable invariant — the failure mode
`gpu/revisions.rs` was written to abolish, applied to only two of the nine.

### 1.7 Essential vs accidental

Irreducible (~1,600 lines): ping-pong accumulator management and the recursive
group walk (`compose_group` 4302, `compose_children` 4377, `compose_group_arm`
5535); the three-pass leaf-mask projection (4951–5125);
`sync_effect_instances` *as a phase* (the device/queue-before-encode split it
enforces is a real WebGPU constraint); `run_filter_region` (133–218, which is
exemplary); the `Revisions` integration and frame gates;
`ortho_extent_about` / `scaled_extent_about` with their tests.

Accidental (~2,000+ lines): ~900 of descriptor boilerplate, ~900 of misplaced
subsystems, ~350 of duplicated bodies, plus mirror/dead fields.

---

## 2. Dead CPU selection stack (`mask.rs`, `tile.rs`, select tools)

> **Resolved.** `tile.rs` is deleted, the tile-based `AlphaMask` impl blocks and
> their tests are gone from `mask.rs` (1,811 → 926 lines), the three orphaned
> `pub fn rasterize` bodies are stripped from the select tools, and
> `contour_segments_r8_matches_tile_version` no longer exists — there is one
> marching-squares implementation. `mask.rs` now holds only the live R8 free
> functions and their shared helpers. The one survivor worth noting:
> `contour_segments_r8` is still public and still tested, but has no production
> caller (only `contour_polylines_r8` does), so it is a candidate for a future
> speculative-deletion pass.

Selections were migrated to GPU R8, but the entire pre-migration CPU
implementation is still compiled, tested, and maintained. Zero production
callers outside `mask.rs`/`tile.rs` (verified by grep):

- `mask.rs:21,33,48` `boolean_add`/`boolean_subtract`/`boolean_intersect` —
  superseded by `gpu/selection.rs:383 combine`; also `invert` (:90),
  `bounding_rect` (:112), `pixel_bounding_rect` (:138), `clear` (:82),
  `rasterize` (:204), `feather` (:733), `contour_segments` (:835),
  `rasterize_r8` (:1190), `from_r8` (:1224), and `gaussian_kernel` (:706,
  reachable only from the dead `feather` yet still doc-linked from
  `engine/filters/selection.rs:598`).
- `tile.rs` (313 lines) — sole consumers are the dead code above. Its own
  header (:1–10, :242–250) states the deletion condition: *"If selections are
  ever migrated to GPU compute, this module can be deleted entirely."* They
  were.
- `tools/rect_select.rs:17`, `ellipse_select.rs:18`, `lasso_select.rs:18` —
  three `pub fn rasterize(...) -> AlphaMask` with no callers anywhere.
- The duplication is codified as a test:
  `tests/selection.rs:1059–1073 contour_segments_r8_matches_tile_version`
  exists purely to keep two implementations of marching squares in agreement —
  the "keep in sync" stop-sign expressed as a test. The near-verbatim pair:
  `mask.rs:835–919` (`AlphaMask::contour_segments`) vs `mask.rs:583–670`
  (`contour_raw_segments_r8`) — identical 16-arm lookup table, differing only
  in the sampler.

Scope: ~700 production lines + ~560 test lines (`mask.rs:1252–1810`).
The live R8 free functions and shared helpers (`mask.rs:922–1185`) stay.

---

## 3. Engine findings

### 3.1 `ReadbackContext` — worst dispatch violation in engine/

- `ReadbackContext` enum: `engine/mod.rs:153–333` — 18 variants, 180 lines.
- `handle_completed_readback`: `engine/rendering.rs:351–594` — a 243-line
  central match on those variants. Adding a readback means editing `mod.rs`,
  `rendering.rs`, and the requester.
- Five arms are copies of each other: `BrushStrokePreview` (449–475) vs
  `BrushThumbnailForSave` (476–497) are the same 14 lines;
  `BrushDabThumbnail` (498–507), `ActiveBrushDab` (563–575), `NodePreview`
  (576–592) are the same shape three more times.
- The `Thumbnail` arm (424–448) branches on `t.format() == R8Unorm` to pick a
  thumbnail encoder — consumer-side branching on a fact the pixel buffer owns.
- The correct answer is next door: `engine/protocol/mod.rs:476–544` routes
  payloads to N handlers via a registry with "a 2-arm match on `Option`, never
  on kind". Fix shape: the readback carries its own completion (closure or
  `ReadbackCompletion` trait); `drain_readbacks` becomes the whole dispatcher.
  Est. 150–200 lines removed, dispatch becomes additive.

### 3.2 `engine/types.rs` `LayerInfo`

`LayerInfo` (49–217) spells out 14 identical fields in all five variants, with
doc comments copy-pasted verbatim five times. `node_to_layer_info` (532–692)
re-types all 14 assignments per variant, including the identical `modifiers:`
filter_map closure five times. Flattening to a `NodeInfoCommon` + per-kind
`LayerKindInfo` (hung off `LayerKindRegistration`) removes ~220 lines and
makes a new layer kind additive on both the Rust and TS sides. Also:
`ParamInfo::from_def` (314–396) and `from_pref` (406–471) build the same
`scalar_default`/`renders_range`/`ParamDisplay` tail twice, and
`modifier_to_info` (:704–707) branches on `FilterKind` for `linked_to_host`,
which belongs on the filter kind (the same branch already leaked to
`document/pixel_transform.rs:44–45`).

### 3.3 Other engine findings

- **`duplicate.rs::clone_subtree` (97–335)** — a compile-time-exhaustive
  5-arm match on layer kind, each arm repeating an identical ~15-line
  epilogue. A sixth layer kind forces an edit here. `LayerKindRegistration`
  already carries `serialize`/`deserialize`/`remap_ids` fn-pointers; a
  `duplicate:` sibling makes this additive. ~180 lines out of engine/.
- **Property setters (`layers.rs:1356–1700`)** — seven setters hand-roll the
  gate → read-old → write → mark-dirty → `push_undo(PropertyAction)` dance
  that `undo/property.rs:52–93 Property::apply` already owns half of. With a
  `Property::read` counterpart, each collapses to one `set_property` call.
  `set_layer_visible` (1446–1456) and `set_node_locked` (1469–1479) are the
  same 11-line block differing in one field. ~130 lines.
- **`floating.rs` two parallel commit machines** — `finish_transform_commit`
  (653–880) and `commit_floating` (885–1057) both do bounds → grow →
  `save_region` → undo-compound → dirty → clear-session.
  `FloatingMode` (gpu/transform.rs:66–72) has exactly one variant,
  destructured irrefutably at floating.rs:948 — dead generality keeping the
  duplicated path alive. ~120–150 lines.
- **`sync_void_persistent_frame` (`layers.rs:1098–1117`) flows data uphill** —
  reads `compositor.void_persistent_frame_size()` and mirrors it onto
  `VoidLayer::frame`; four call sites must remember to invoke it, and
  `mod.rs:1051–1055` documents the save-corruption hazard in a comment rather
  than designing it away. The repo's named anti-pattern, live.
- **`grow_layer`/`grow_filter` (`painting.rs:756–879`)** — the same function
  twice over kind-agnostic document accessors that already exist, routed by
  `doc.is_filter()` (the literal `fn is_foo() -> bool` consumer-side router;
  8 more call sites across clipboard/merge/layers).
- **`fill_background` (`painting.rs:200–286`)** hand-rolls the
  snapshot/mutate/commit dance that `region_undo_inplace` (115–166)
  generalizes — while its sibling `fill_background_color` (293–307) uses it
  properly and is 14 lines.
- **`config/mod.rs:337–346 kind_is_int`** — the textbook
  `fn is_foo(key) -> bool` classification API; its sole consumer is a JS
  number-coercion concern. `PrefKind::coerce(f64) -> ConfigValue` kills it and
  the `config_bridge.rs` pass-through.
- **`load.rs` pass 1a/1b (241–280 vs 283–320)** — the same 38-line
  allocate-then-fill block for nodes vs filters; ~35 lines.

Engine total: ~1,000–1,070 removable lines (~5% of engine/), and four items
(readbacks, duplicate, LayerInfo, `kind_is_int`) convert "edit a central file"
into "drop a file in the module dir".

---

## 4. Brush findings (minor)

- **Per-brush pipeline cache triplicated** — `nodes/paint.rs:295–353`
  (`PaintPipeline`) and `nodes/watercolor.rs:526–571` (`WatercolorPipeline`)
  are line-for-line identical modulo type name and id string;
  `read_mirror_terminal.rs:283–320` is the third copy, and the nine-field
  `BuildContext` helper is likewise duplicated (paint.rs:723–744 vs
  watercolor.rs:1345–1365). A `PerBrushPipelineCache<P>` generic absorbs all
  three; the repo already proved the move works when it unified
  smudge/liquify/blur behind `ReadMirrorTerminal`.
- **`eval.rs` sensor seeding branches on four hardcoded node type IDs**
  (:627–653 build-time slot resolution, :889–891 hot-loop disjunction,
  :841–856 a 15-arm port-name match living in the runner). The file itself
  documents removing exactly this pattern for terminals (:507–514); the same
  defaulted-trait-method move (`seed_sensor`) applies.
- **Duplicated WGSL vertex preamble** — `VsOut` + `quad_corner` repeated
  verbatim in `wgsl/mod.rs:1026–1043` and :1088–1104; belongs in
  `_prelude.wgsl`, which `assemble_shader` already prepends unconditionally.

Cleared of suspicion: the three "keep in sync" comment hits in brush/ are
negations (stating no sync burden exists, correctly); the dual stroke/preview
shader machinery is the prescribed defaulted-trait-method pattern, not
duplication; `nodes/watercolor.rs`'s 1,365 lines are ~150 of module docs, an
embedded WGSL shader with a documented reason to be inline, and a genuinely
different pipeline build.

---

## 5. Recommended attack order

1. ~~**Delete the dead CPU selection stack** (§2). Largest zero-risk deletion,
   no design work: `tile.rs`, the dead `impl AlphaMask` blocks, the three
   tool `rasterize` fns, their tests, and the keep-in-sync test.~~ **Done.**
2. **Resume the compositor decomposition** (§1) — see
   `docs/plans/` for the plan. The boilerplate collapse alone (~550–600
   mechanical lines) drops the file below `document/mod.rs` before any
   structural extraction begins.
3. **`ReadbackContext` → per-request completions** (§3.1).
4. The remaining engine and brush items as opportunistic cleanups, roughly in
   the order listed.
