# Canvas-space animated effects never animate — fix the animation gate

## Independent Review

Independently re-verified against the `better-veils` working tree; every claim
below was checked from source, not taken from the plan or the audit.

### Diagnosis: confirmed

- The gate hole is real. `update_animations` (compositor.rs:3376-3429) gates
  `canvas_fires` on `any_animated_layer` (compositor.rs:3406-3408), which
  matches only `LayerContent::Procedural` (compositor.rs:3203-3208) — effect
  layers are invisible to it. `tick_animated_effects(..., false)` runs only
  inside that gate (compositor.rs:3418-3424).
- `needs_animation` (compositor.rs:3434-3438) is `overlay || screen-effect ||
  animated-void` — no canvas-effect term. It feeds `frame_needs_more`
  (engine/rendering.rs:761-769), the value JS reschedules on
  (rendering.rs:752), so the loop genuinely idles.
- The fossil comment is as described: compositor.rs:3211-3212 claims a
  canvas-space effect "drives `needs_composite` through the layer path", which
  `any_animated_layer` contradicts.
- Provenance confirmed: `git show c2895130^:crates/darkly/src/gpu/compositor.rs`
  has `effect_fires` on `self.effect_chain.needs_animation()` and a
  `needs_animation` that includes the effect chain; the split into
  screen/canvas dropped the canvas half's predicate exactly as the plan says.
- All three animated effects answer `speed > 0.0`
  (effects/grain.rs:131-133, effects/rainy_glass.rs:159-161,
  effects/vhs.rs:139-141), true at schema defaults (grain speed 0.05 at
  grain.rs:11; rainy_glass/vhs speed 0.5 at their `PARAMS[0]`).

### Design: sound, and the lag analysis holds

- The instance map is the right authority (matches `Effect::needs_animation`'s
  instance-level contract, effect.rs:318-322); a registry-level answer would
  duplicate per-effect param logic. Consumers stay trait-dispatched — no
  `matches!`/kind-branching — and a new animated effect is purely additive.
- Sync coverage verified: `sync_effect_instances` (compositor.rs:4654) runs
  from `render_offscreen` via `sync_projection_states` (compositor.rs:3931 →
  4536) and from `present_and_screen_run` when the doc run is non-empty
  (compositor.rs:3702-3704). Every mutation that changes instance membership,
  space, or animation-relevant params marks `needs_composite`:
  `add_filter_layer` (engine/layers.rs:812), `update_filter_params`
  (layers.rs:885), `set_layer_visible` (layers.rs:1458),
  `set_screen_space_boundary` (layers.rs:1742). `frame_needs_more` is only
  evaluated after `compositor.render` in the same call (rendering.rs:735-752),
  so the "one frame, self-correcting" claim holds on every production path I
  could find. The headless early-returns (rendering.rs:696-727) never consult
  `needs_animation` at all.
- Stale-id safety confirmed: `Document::effective_visible` returns `false`
  for an id `find_node` can't resolve (document/mod.rs:426-429), and filter
  *layers* are `LayerNode::Layer(Layer::Filter)` and thus resolvable
  (layers.rs:878-881). One correction to the plan's prose: a **just-deleted**
  effect cannot keep the loop alive even for one frame — its stale instance
  fails `effective_visible` immediately. The one-frame window applies only to
  instances whose *space tag* lags (moved across the divider, divider moved),
  which is harmless as analyzed. Revise that sentence in §"Instance-map lag".
- First-composite realization (matters for the test): root `group_state` is
  created in the constructor (compositor.rs:1221, 1254-1255), so a
  root-anchored canvas effect's instance is built by the very first
  `sync_effect_instances` — no bootstrap gap for `test_readback_canvas`.
- The unified predicate is the simplest general shape: the alternative
  (a third hand-written copy of space/animated/visible) is exactly the drift
  that produced this bug. Nit, not blocking: step 5 iterates the map twice
  (`any_animated_effect(true) || any_animated_effect(false)`); fine at this
  n, and folding it would need a third filter form — keep as planned.

### Regression test: valid, two hardening notes

- All claimed entry points exist with the claimed semantics:
  `test_frame_needs_more` (engine/mod.rs:874), `test_readback_canvas`
  (mod.rs:1106 — calls `render_offscreen`, which early-returns on
  `!needs_composite` at compositor.rs:3910, so a frozen clock yields
  byte-identical reads of the same cached texture), `test_tick_animations`
  (mod.rs:1177), `test_clear_needs_present` (mod.rs:888),
  `test_flush_readbacks` (mod.rs:1400). Helpers `test_engine`/`fill_layer`/
  `settle` exist in tests/effect_space.rs:17-46. `canvas_divisor: 2` at
  presets/defaults.yaml:129. Param order (speed, color, opacity) matches
  grain's `read_params` (grain.rs:51-65).
- Pre-fix failure verified by tracing: (b) cannot pass — with no animated
  void, `canvas_fires` is false, `needs_composite` stays false, and the second
  readback returns the identical cached composite. Post-fix inequality is
  sound: `canvas_fires` sets `needs_composite` and grain's `update_time`
  bumps the seed uniform (grain.rs:166-170); at speed 1.0 every texel
  reshuffles per tick.
- **Harden (a)**: as written, (a) could pass vacuously pre-fix if any pending
  flag lingered (it is an `assert!(needs_more)`, satisfied by leftovers).
  Restructure to assert baseline quiescence *before* adding the effect layer —
  the exact pattern canvas_resize.rs:735-741 already proves reachable — then
  add the effect, composite once, and assert (a). ~4 extra lines; makes the
  failure attribution airtight instead of probabilistic.
- The visibility-off assertion is safe post-fix: `set_layer_visible` →
  `mark_dirty` sets only `needs_composite` (compositor.rs:2201-2209), which
  `frame_needs_more` does not consult, and `invalidate_all` creates no pending
  content-bounds work (content_bounds.rs:145-161).
- Coverage gap, acknowledged not blocking: the screen predicate's enumeration
  swap (step 2) has zero test coverage anywhere (`test_frame_needs_more` is
  used only in canvas_resize.rs). Headless engines never realize Screen
  instances (`screen_run.views()` is `None` → skip at compositor.rs:4703-4709),
  so a screen-side gate test would need `test_readback_screen_run`
  (mod.rs:1156) to size the run first. Optional hardening; the one-frame
  analysis above is verified and the plan already documents the zero-change
  fallback.

### Scope, prior art, estimate

- Scope is minimal and correctly bounded: §3.2 dt-scaling and grain's
  dt-ignoring `update_time` are rightly excluded; audit doc left as a
  point-in-time report; no document/engine/WASM/frontend changes needed.
- `any_animated_screen_effect` has exactly the three uses the plan replaces
  (compositor.rs:3213, 3394, 3436) — nothing else consumes it.
- Prior art: external-editor research is not applicable; the authoritative
  prior art is this repo's own pre-regression design at `c2895130^`, which the
  plan cites and I verified.
- LOC estimate (~+22/−18 production, ~+60 tests) is honest; add ~4 test lines
  for the baseline-quiescence restructure.

### Verdict: accept

Two minor revisions to fold in during implementation, neither changing the
approach: (1) restructure the test to assert baseline quiescence before adding
the effect; (2) correct the "just-deleted effect keeps the loop alive one
frame" sentence — deletion is inert immediately via `effective_visible`; only
space-tag lag has the one-frame window.

### Post-review addendum — `handoff-viewport-boundary.md` fold-in

The PR 4 session handoff (`handoff-viewport-boundary.md` §3.1) independently
confirms this bug from actual use and reaches the **identical fix shape** (a
space-parameterised `any_animated_effect(doc, screen: bool)` replacing
`any_animated_screen_effect`) — treated here as convergent validation of the
design. Folded in from it:

- Field symptom detail added to the Problem section: the effect's *pass* runs
  every dirty frame at t=0 (refraction correct, raindrops frozen) — only the
  clock is dead.
- The handoff's requested second regression case — the same effect **above**
  the divider must keep passing, "which is what pins the two spaces to one
  mechanism rather than two" — added as a companion test. This also closes
  the review's noted coverage gap on the screen predicate's
  enumeration-source swap.
- Independence confirmed by the handoff: the animation gating survives the
  pending divider-as-a-node redesign (handoff §2) unchanged, so this fix
  correctly ships first.

## Problem

An effect layer that declares `needs_animation()` (`rainy_glass`, `grain`, `vhs`
— all three answer `self.speed > 0.0`, true at their schema defaults) does not
animate when it sits **below** the screen-space divider. Its clock never
advances and the rAF loop is not kept alive. It animates only if an animated
void layer coincidentally exists in the same document, because the canvas-space
tick rides the void gate.

Confirmed in use (`handoff-viewport-boundary.md` §3.1): "move a veil below the
divider and it stops animating. `rainy_glass` still refracts the colour beneath
it correctly, but the raindrops never move." The *pass* re-runs every dirty
frame — at t=0. `Effect::update_time` is never called; only the clock is
frozen, which is why the effect still looks correct on static content.

All claims below were verified directly against the source (paths and line
numbers are for the current `better-veils` working tree).

### Root cause

`Compositor::update_animations`
(`crates/darkly/src/gpu/compositor.rs:3376-3429`) gates its three subsystems:

```rust
let screen_fires = screen_divisor > 0
    && self.any_animated_screen_effect(doc)
    && self.frame_count.is_multiple_of(screen_divisor);
...
let canvas_fires = canvas_divisor > 0
    && self.any_animated_layer(doc)          // <-- voids only
    && self.frame_count.is_multiple_of(canvas_divisor);

if canvas_fires {
    self.tick_animated_layers(queue, dt * canvas_divisor as f32, doc);
    self.tick_animated_effects(queue, dt * canvas_divisor as f32, doc, false);
    self.needs_composite = true;
}
```

- `any_animated_layer` (compositor.rs:3203) inspects only
  `LayerContent::Procedural` — animated **voids**. It knows nothing about
  effect layers.
- The canvas half of `tick_animated_effects(..., screen = false)`
  (compositor.rs:3227) runs only inside `canvas_fires`, so it is gated on an
  unrelated fact.
- `Compositor::needs_animation` (compositor.rs:3434) has the same hole:
  `tool_overlay || any_animated_screen_effect || any_animated_layer` — there is
  no canvas-effect predicate. `DarklyEngine::frame_needs_more`
  (`crates/darkly/src/engine/rendering.rs:761`) consumes this to keep the JS
  rAF loop scheduling frames, so a document whose only animated content is a
  canvas-space effect goes idle.

The `update_animations` doc comment (compositor.rs:3366) already promises the
correct behavior — "Document content — void layers **and canvas-space effect
layers**: every `canvas_divisor`-th frame" — the gate just doesn't deliver it.
The comment on `any_animated_screen_effect` (compositor.rs:3210-3212), "a
canvas-space animated effect drives `needs_composite` through the layer path
instead", is false: the "layer path" is `any_animated_layer`, which ignores
effects. That comment is a fossil of the regression.

### Provenance (verified via git)

`c2895130 "veils as normal layers wip"`. The parent commit
(`git show c2895130^:crates/darkly/src/gpu/compositor.rs`, lines 3334-3378) had
a dedicated gate:

```rust
let effect_fires = effect_divisor > 0
    && self.effect_chain.needs_animation()
    ...
pub fn needs_animation(&self, doc: &Document) -> bool {
    self.tool_overlay.needs_animation()
        || self.effect_chain.needs_animation()
        || self.any_animated_layer(doc)
}
```

When the single effect chain was split into two spaces, the screen half got
`screen_fires` / `any_animated_screen_effect`, and the canvas half was folded
under the void gate without a predicate of its own.

## Design

### Where the answer lives

`Effect::needs_animation` (`crates/darkly/src/gpu/effect.rs:320`) is an
**instance-level** answer — the three animated effects return
`self.speed > 0.0`, a function of their current parameters. It cannot be
answered from `EffectRegistration` metadata without duplicating each effect's
param logic in a second place (a DRY and type-ownership violation), and it
cannot be answered from the document alone (the document stores only
`pipeline: String` + params). The authority is the realized instance — exactly
what `any_animated_screen_effect` already consults
(`self.effect_instances.get(id).is_some_and(|inst| inst.effect.needs_animation())`).

So the new predicate consults `effect_instances`, keyed by the `space` tag each
instance already carries (`EffectSpace`, compositor.rs:541 — `Canvas { parent }`
or `Screen`), and filters by `doc.effective_visible` exactly as the existing
screen predicate and both tick paths do.

### Instance-map lag is bounded and self-correcting

`effect_instances` is rebuilt by `sync_effect_instances`
(compositor.rs:4654), called from the composite path (compositor.rs:4536, via
`render_offscreen`'s prepare step) and from the present path when the screen
run is non-empty (compositor.rs:3703). Adding, removing or moving an effect
layer marks the compositor dirty, so `needs_composite` is already true; the
next `render` composites, which syncs the instances, and `frame_needs_more` is
evaluated **after** `render` in the same call (rendering.rs:730-752). The rAF
loop therefore never observes a missing instance for a live effect. On the tick
side, `update_animations` runs before `render`'s sync, so a just-added effect
misses at most one tick (with a near-zero accumulated `dt`) — the same
tolerance every existing consumer of the instance map already accepts.
A just-deleted effect is inert immediately: its stale instance fails
`doc.effective_visible` (which returns `false` for an id `find_node` cannot
resolve, document/mod.rs:426-429), so it cannot keep the loop alive even
before the sync's `retain` drops it. The one-frame window applies only to
instances whose *space tag* lags (effect moved across the divider, or the
divider moved), which is harmless as analyzed.

### The change (all in `crates/darkly/src/gpu/compositor.rs`)

1. **One shared filter** for "does this effect instance participate in an
   animation tick for space X" — the triple condition currently written inline
   in `tick_animated_effects` (compositor.rs:3234-3240):

   ```rust
   /// Whether an effect instance participates in an animation tick for the
   /// given side of the divider: right space, animated at its current
   /// parameters, and effectively visible.
   fn effect_animates(inst: &EffectInstance, id: LayerId, doc: &Document, screen: bool) -> bool {
       (inst.space == EffectSpace::Screen) == screen
           && inst.effect.needs_animation()
           && doc.effective_visible(id)
   }
   ```

2. **Replace `any_animated_screen_effect(doc)` with
   `any_animated_effect(doc, screen: bool)`** — same shape as
   `tick_animated_effects`'s existing `screen: bool` parameter, built on the
   shared filter:

   ```rust
   fn any_animated_effect(&self, doc: &Document, screen: bool) -> bool {
       self.effect_instances
           .iter()
           .any(|(id, inst)| Self::effect_animates(inst, *id, doc, screen))
   }
   ```

   Note this changes the screen predicate's enumeration source from
   `doc.screen_space_run()` to the instance map's `space` tag. The two agree
   except during the one-frame window before a sync (analyzed above); the tag
   is also what `tick_animated_effects` itself keys on, so predicate and tick
   can no longer disagree about which instances are in scope — today they
   already read different sources.

3. **`tick_animated_effects`** — replace its inline space/animation/visibility
   checks with `Self::effect_animates(...)` so the predicate and the tick are
   the same condition by construction.

4. **`update_animations`** —
   `screen_fires`: `self.any_animated_effect(doc, true)`;
   `canvas_fires`: `(self.any_animated_layer(doc) || self.any_animated_effect(doc, false))`.
   (`tick_animated_layers` inside `canvas_fires` is a no-op when only an
   effect is animated — it filters on procedural content itself.)

5. **`needs_animation`** — add the canvas side:

   ```rust
   self.tool_overlay.needs_animation()
       || self.any_animated_effect(doc, true)
       || self.any_animated_effect(doc, false)
       || self.any_animated_layer(doc)
   ```

6. **Comments** — delete the false "drives `needs_composite` through the layer
   path" sentence; the replacement predicate's doc comment covers both spaces.

No document, engine, WASM, or frontend changes. No new consumer-side
kind-branching: the predicate asks each instance `needs_animation()` through
the trait; a new animated effect participates with zero scheduler edits.

### Alternatives considered

- **Separate `any_animated_canvas_effect` beside the existing screen one**
  (the audit's literal suggestion): works, but leaves three copies of the
  space/animated/visible condition (two predicates + the tick filter) that
  must stay in agreement — the exact drift that caused this bug. The unified
  predicate makes the agreement structural.
- **Doc-driven predicate** (walk `doc.all_filter_layers()`, ask the registry
  whether the type animates at given params): requires registration-level
  animation metadata duplicating instance logic per effect. Rejected on DRY /
  type-owned dispatch.

## Regression test (write first, must fail before the fix)

Location: `crates/darkly/tests/effect_space.rs` — the bug is a property of
which side of the divider a layer sits on, which is that file's stated domain,
and every helper needed (`test_engine`, `fill_layer`, `effect`, `settle`)
already lives there. **No new test accessor is needed**: the existing
test-only surface covers both assertions —

- `DarklyEngine::test_frame_needs_more()` (engine/mod.rs:874) — the exact
  value returned to JS.
- `DarklyEngine::test_tick_animations(wall_time)` (engine/mod.rs:1177) —
  drives `update_animations` directly, since headless `render()` early-returns
  before it (rendering.rs:696-710).
- `DarklyEngine::test_readback_canvas()` (engine/mod.rs:1106) — forces
  `render_offscreen`, which early-returns on `!needs_composite`
  (compositor.rs:3910), so a stale composite is byte-identical across calls.
  This is what makes "the clock advanced" observable end-to-end: without the
  fix, the tick neither writes the effect's uniform nor sets
  `needs_composite`, so a second readback returns the first frame's bytes.
- `test_clear_needs_present` / the settle-to-quiescence pattern proven by
  `dropped_present_keeps_requesting_frames`
  (`crates/darkly/tests/canvas_resize.rs:725`).

```rust
/// Regression: a canvas-space animated effect is the document's only animated
/// content. It must keep the frame loop alive and advance its clock across
/// frames — before the fix, the canvas tick and `needs_animation()` were both
/// gated on animated *voids* only, so the effect froze unless a void
/// coincidentally existed.
#[test]
fn canvas_space_animated_effect_animates() {
    let (cw, ch) = (32u32, 32u32);
    let mut engine = test_engine(cw, ch);
    let base = engine.add_raster_layer(None);
    fill_layer(&mut engine, base, 128, 128, 128);

    // Baseline quiescence BEFORE the effect exists: settle startup async work
    // and prove the loop goes idle, so assertion (a) below can only be
    // satisfied by the effect layer — not by a leftover pending flag.
    for _ in 0..8 {
        engine.render(0.0);
    }
    engine.test_flush_readbacks();
    engine.test_clear_needs_present();
    assert!(
        !engine.test_frame_needs_more(),
        "engine must be quiescent before the animated effect is added"
    );

    // Grain at full speed: needs_animation() == true, and every tick reseeds
    // the noise, so consecutive composites cannot be byte-identical.
    let fx = engine
        .add_filter_layer(
            "grain",
            vec![
                ParamValue::Float(1.0), // speed
                ParamValue::Float(0.0), // color
                ParamValue::Float(1.0), // opacity
            ],
            None,
        )
        .expect("grain should be addable as an effect layer");
    // screen_space_count defaults to 0 — the effect is canvas-space.

    // Composite once (realizes + syncs the effect instance), then clear the
    // transient flags so `frame_needs_more` reflects only animation demand.
    let before = engine.test_readback_canvas();
    engine.test_flush_readbacks();
    engine.test_clear_needs_present();

    // (a) The frame loop must stay alive for the canvas-space effect.
    assert!(
        engine.test_frame_needs_more(),
        "a visible canvas-space animated effect must keep the frame loop alive"
    );

    // (b) The effect's clock must advance across frames. Tick past several
    // divisor boundaries (canvas_divisor defaults to 2; a handful of ticks
    // stays correct for any small divisor), then recomposite.
    engine.test_tick_animations(1.0); // primes last_wall_time (dt == 0)
    for i in 1..=8 {
        engine.test_tick_animations(1.0 + i as f32 * 0.05);
    }
    let after = engine.test_readback_canvas();
    assert_ne!(
        before, after,
        "ticking the animation clock must advance a canvas-space effect and \
         re-composite; identical bytes mean the canvas gate never fired"
    );

    // Hiding the effect must silence the loop — the predicate honors
    // effective visibility like every other animation gate.
    engine.set_layer_visible(fx, false);
    assert!(
        !engine.test_frame_needs_more(),
        "a hidden canvas-space animated effect must not keep the loop alive"
    );
}
```

### Companion test — same effect above the divider (must pass before AND after)

Requested by `handoff-viewport-boundary.md` §3.1: "A second case with the same
effect above the divider must keep passing, which is what pins the two spaces
to one mechanism rather than two." It doubles as coverage for this plan's one
behavior-adjacent change — the screen predicate's enumeration source moving
from `doc.screen_space_run()` to the instance `space` tag — which the review
flagged as otherwise untested.

```text
screen_space_animated_effect_keeps_animating:
  1. Same setup (raster + fill + grain at speed 1.0), then
     set_screen_space_boundary(1) so the effect is screen-space.
  2. Realize the Screen instance: test_readback_screen_run(16, 16)
     — headless engines never realize Screen instances otherwise
     (screen_run.views() is None → sync skips them), per the review.
  3. Settle + clear flags as in the canvas test; assert
     test_frame_needs_more() — the screen predicate, now instance-tag
     driven, must still keep the loop alive.
  4. Tick across several divisor boundaries (screen_divisor defaults to 2),
     then assert a second test_readback_screen_run(16, 16) differs from
     step 2's frame — the screen clock advances.
```

This case passes today and must keep passing — it pins the no-regression half
of the unified predicate.

Failure mode before the fix, confirmed against the current code paths:

- (a) fails: `needs_animation` finds no overlay animation, no screen-run
  member, no animated void → `frame_needs_more()` is false once present/
  readback flags are cleared.
- (b) fails: `canvas_fires` is false (no animated void), so
  `tick_animated_effects(..., false)` never runs and `needs_composite` stays
  false; the second `test_readback_canvas` hits the early return at
  compositor.rs:3910 and returns the identical cached composite.
- The visibility assertion passes both before and after (vacuously before);
  it exists to pin the predicate's `effective_visible` clause after the fix.

Verification order per CLAUDE.md: add the test, run
`cargo test -p darkly --test effect_space --features testing -- --test-threads=1`,
show assertions (a)/(b) failing, then apply the fix and show it passing, plus a
full `cargo test --workspace --exclude darkly-wasm --features darkly/testing -- --test-threads=1`
at the end (GPU tests share one device; `--test-threads=1` is mandatory).

Minor details verified for the test:

- `grain` defaults (`gpu/effects/grain.rs:10-20`): speed 0.05, color 0.0,
  opacity 1.0 — animated even at defaults; the test pins speed 1.0 anyway so
  the pixel delta is maximal (grain's evolve pass replaces a `speed` fraction
  of pixels per tick).
- `animation.canvas_divisor` defaults to 2 (`presets/defaults.yaml:129`);
  `frame_count` starts at 0 and only `test_tick_animations` advances it in
  headless tests, so the tick loop crosses multiple divisor boundaries
  deterministically.
- `add_filter_layer(pipeline, params, anchor)` is the engine entry
  (`engine/layers.rs:799`); `set_layer_visible` (`engine/layers.rs:1444`) is
  already used by effect_space.rs tests.

## Risks and unresolved questions

- **Screen predicate source change**: `any_animated_effect(doc, true)`
  enumerates instances instead of `doc.screen_space_run()`. Divergence windows
  are one frame wide and self-correcting (analyzed above); the instance tag is
  what the tick itself uses, so this removes a latent disagreement rather than
  adding one. If review prefers zero behavior change on the screen side, the
  fallback is keeping `any_animated_screen_effect` as-is and adding only the
  canvas predicate — at the cost of a third copy of the filter condition.
- **Pixel-inequality assertion**: `assert_ne!` on full buffers could in theory
  pass vacuously if grain rendered nothing — mitigated by pinning opacity 1.0
  over a mid-gray fill; grain reseeds from `frame_count`, so consecutive
  composites differ with overwhelming probability. If flakiness appears, the
  deterministic alternative is a tiny `test_needs_composite()` accessor, but
  it is not needed to demonstrate the regression.
- **Audit §3.2** (divisor dt scaling assumes uniform frame times) and grain's
  dt-ignoring `update_time` are adjacent but distinct defects; this plan
  deliberately does not touch them.
- **Divider-as-a-node redesign** (`handoff-viewport-boundary.md` §2, decided
  but unplanned): independent of this fix — the animation gating consults the
  instance `space` tag and `effective_visible`, neither of which the redesign
  changes. The companion test's `set_screen_space_boundary(1)` call will need
  a mechanical swap to a divider move when that redesign lands; semantics are
  unaffected.
- `docs/compositor-caching-audit.md` §3.1 describes this bug as live; the
  audit is a point-in-time report, not living documentation, so it is left
  unedited.

## LOC estimate

- **Production** (`gpu/compositor.rs` only): ~+22 / −18
  (shared `effect_animates` filter ~10, predicate replacing
  `any_animated_screen_effect` ~8/−12, tick body −4/+2, two gate edits +2,
  comment fixes ±2).
- **Tests** (`tests/effect_space.rs`): ~+90 / −0 (regression test with the
  review's baseline-quiescence restructure, plus the handoff-requested
  screen-side companion test, plus one `use` addition).
- **Generated / docs**: 0 (no module added, `mod.rs` untouched; this plan file
  only).
