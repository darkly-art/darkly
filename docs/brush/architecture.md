# Brush System: Runtime Architecture

This is the runtime side of the brush system: what actually happens from the
moment the artist puts a stylus down to the moment the layer texture changes
on-screen. For *authoring* (how to add a node or build a preset), see
[node-system.md](node-system.md).

## 30-second mental model

> A brush is a **compiled graph of nodes**. The **stroke engine** feeds the
> graph a sequence of **dabs** spaced along the pen path. Each dab runs the
> graph once: CPU nodes compute scalars, GPU nodes record render passes. All
> dab passes land in a **stroke scratch** (an RGBA texture the same size as
> the layer). At the end of each input event the scratch is pushed to the
> layer by the active terminal via its **`commit`** lifecycle hook:
> `color_output` source-over blends it onto the pre-stroke snapshot, `liquify`
> replaces the layer with the warped scratch, and future terminals do
> whatever their semantics require. The engine never decides how the commit
> works; it only provides the resources.

Four pieces to keep in mind:

1. **Graph** = static description. Nodes + wires + ports + params.
2. **Runner** = compiled graph. Knows the topological order and slot layout.
3. **Stroke engine** = per-stroke state. Pen smoothing, spacing, save points.
4. **Stroke buffer** = per-stroke scratch. Where dabs actually land during the
   stroke. The layer itself is only written once per event (at the composite).

## File map

| Responsibility | Path |
|---|---|
| Stroke lifecycle, stabilizer hookup, event plumbing | [`engine/painting.rs`](../../crates/darkly/src/engine/painting.rs) |
| Graph-side preview regen | [`engine/brush_graph.rs`](../../crates/darkly/src/engine/brush_graph.rs) |
| Stabilizer + dab spacing + save points | [`brush/stroke_engine.rs`](../../crates/darkly/src/brush/stroke_engine.rs) |
| Pre-stroke snapshot + scratch RT + composite | [`brush/stroke_buffer.rs`](../../crates/darkly/src/brush/stroke_buffer.rs) |
| Graph compile + per-dab eval | [`brush/eval.rs`](../../crates/darkly/src/brush/eval.rs) |
| GPU context passed into `evaluate_gpu` | [`brush/gpu_context.rs`](../../crates/darkly/src/brush/gpu_context.rs) |
| Pipelines, uniform rings, shared bind groups | [`brush/pipelines.rs`](../../crates/darkly/src/brush/pipelines.rs) |
| Pre-allocated dab RTs + brush-tip textures | [`brush/dab_pool.rs`](../../crates/darkly/src/brush/dab_pool.rs) |
| Node types (auto-discovered by `build.rs`) | [`brush/nodes/`](../../crates/darkly/src/brush/nodes/) |
| WGSL shaders | [`crates/darkly/shaders/brush/`](../../crates/darkly/shaders/brush/) |
| Built-in presets | [`brush/builtin_presets.rs`](../../crates/darkly/src/brush/builtin_presets.rs) |

## Stroke lifecycle

```
 begin_stroke(layer_id)
   │
   ├─► compile active graph → BrushGraphRunner
   └─► create StrokeBuffer (scratch + pre-stroke snapshot of the layer)

 for each pen event (brush_stroke_to):
   │
   └─► record the event; nothing stabilizes or renders

 each frame, and at pen-up (flush_stroke), if events arrived since the last:
   │
   ├─► first flush only: runner.begin_stroke(gpu_ctx)
   │      // every terminal's begin_stroke hook fires:
   │      //   color_output → clear scratch to transparent
   │      //   liquify      → copy layer into scratch
   ├─► StrokeEngine.take_divergence() → stabilizer.push_all(events since
   │      the last flush), then the earliest rendered vertex that moved
   │      since it was rendered, else append-only
   ├─► StrokeEngine.render_from_stabilized_range(gpu_ctx, first_new)
   │      │   (after restoring a checkpoint on divergence)
   │      ├─► for each dab position on the segment:
   │      │     ├─► runner.seed_sensors(PaintInformation)
   │      │     ├─► runner.execute_cpu()     // scalars (size, opacity, …)
   │      │     └─► runner.execute_gpu(ctx)  // render passes recorded in ctx.encoder
   │      │             - stamp: render tip into a dab-pool texture
   │      │             - color_output: composite dab onto stroke_scratch_view
   │      │             - liquify: sample scratch with displaced UVs, write back
   │      └─► submit encoder
   │
   └─► runner.commit(gpu_ctx)
          // every terminal's commit hook fires:
          //   color_output → source-over (scratch, pre_stroke) → layer
          //   liquify      → copy scratch → layer (replace)

 end_stroke:
   │
   ├─► flush_stroke: render the events no frame has, as one segment
   │     with no checkpoint (nothing can diverge after pen-up)
   ├─► save_point → undo ring (bbox + checkpoint)
   └─► drop StrokeBuffer
```

The `begin_stroke` / `commit` pair is the generic mechanism. The engine owns
no policy about what paint strokes vs warp strokes do; each terminal
declares its semantics in its own file, and `BrushGraphRunner` dispatches the
hooks at the right moments.

## Why the stroke buffer exists

The engine always creates a pair of stroke-scoped textures at stroke start:

- **`stroke_scratch_texture`** - the stroke's working surface. What it
  *means* is up to the active terminal: paint terminals fill it with
  accumulated dab contributions, warp terminals fill it with a progressively-
  deformed copy of the layer, blur / future terminals do something else.
- **`pre_stroke_texture`** - a snapshot of the layer at stroke start. Used
  both by the engine (as the rewind source) and by terminals that need the
  untouched canvas at commit time (e.g. `color_output` blends its scratch
  over this to avoid over-darkening overlaps).

Three independent reasons every stroke needs this pair:

1. **Alpha accumulation semantics.** Two overlapping paint dabs at full alpha
   must not read-modify-write each other; that would produce a darker
   overlap than a continuous stroke. Accumulating the *contributions* in a
   scratch and compositing once at commit time gets this right.
2. **Rewind / divergence handling.** The stabilizer can retroactively move
   previously-seen samples. The stroke engine rewinds to a save point and
   re-renders forward. On full rewind the engine calls `runner.begin_stroke`
   again, so each terminal re-initialises its scratch however it wants:
   clear, re-copy the layer, whatever. On a partial rewind it calls the
   same hook over just the rewound region, when the checkpoint's frame
   does not cover it.
3. **Atomic commit per event.** The artist only sees changes land when the
   active terminal's `commit` hook writes to the layer. That boundary lets
   commit apply blend modes (paint/erase), replace wholesale (warp), or
   anything else, without the per-dab render path knowing anything about
   it.

For a deeper trace look at
[`engine/painting.rs::brush_stroke_to`](../../crates/darkly/src/engine/painting.rs).

## How dabs accumulate in the scratch

The `paint` terminal writes the scratch from one compute dispatch per dab,
one thread per pixel of the dab's layer-clamped footprint, so the law that
combines overlapping dabs is *shader code* applied against the live ground:
each thread loads its texel, folds the dab in, and stores it. The scratch
is a packed ground (`PACKED_GROUND_FORMAT`, premultiplied RGBA8 in one
`r32uint` texel) because core WebGPU reads and writes storage only in the
32-bit single-channel formats. Dispatches in a pass are ordered and a
dispatch's stores are visible to the next, so every dab sees the previous
dab's output. Three laws exist, all in
[`shaders/brush/paint_accumulate.wgsl`](../../crates/darkly/shaders/brush/paint_accumulate.wgsl),
which the terminal emits into every brush it ends. The first two are the
ends of the accumulation dial, under the terminal's `Deposit` mode; the
third is its `Move` mode.

**`accumulate_build`** composites each dab over the last (premultiplied
source-over). Coverage accumulates as `1 - prod(1 - a_i)`, so a pixel's
density rises with however many dabs the spacing happened to stack on it.
Its density is therefore a function of `spacing`, not only of pressure: the
Pencil measured 0.714 peak alpha at `spacing: 0.10` and 0.984 at
`spacing: 0.01`, same path, same pressure.

**`accumulate_wash`** applies the deposit ceiling (below, "How the commit
decides") per dab against the live ground, so a pixel takes its strongest
dab rather than the sum of its dabs. For one pigment this is exactly a
per-pixel maximum of coverage: a dab no stronger than what is already
there finds no room and changes nothing, bit for bit, and a stronger one
re-targets the pixel to its own coverage. A stroke cannot darken itself by
crossing back over its own path, density stops depending on spacing
(identical to the byte across a 30x spacing spread), and pressure becomes
the only thing setting it.

**`accumulate_move`** lerps the pixel toward the dab by the finger's
coverage, `src + dst * (1 - cov)`, where `cov` comes from the terminal's
`coverage` port (the tip mask, times a sampler's coverage) rather than
from the dab's alpha. The dab is what a finger brought from elsewhere (a
live canvas sampler's output), which may be bare paper, and the pixel
gives up `cov` of what it had whatever arrives. The same `cov` accumulates
in a `coverage` channel as a bare alpha under source-over, and the commit
lays the ground over `1 - coverage` of the pre-stroke layer
(`source_over_covering`): source-over with the coverage decoupled from the
alpha. That is what lets a smear thin an edge as well as thicken it. This
ground alone is stored straight rather than premultiplied: at a soft tip's
edge a dab brings a few percent of coverage, and a dark colour times that
coverage rounds to zero in a premultiplied 8-bit texel while the alpha
does not, so the pigment the finger carried turned black and drew a thin
dark line along every stroke edge on a dark surface. Under
plain source-over a smear of a half-transparent field along itself raised
its alpha with every pass, which showed as a darkening of every soft edge
on a transparent layer. On an opaque layer the two laws agree exactly. A
dab whose sampler has nothing to bring (a finger that has not moved, a
sample point off the layer) reports zero coverage and moves nothing.

### The accumulation dial

The commit lays the wash slot through the ceiling and the build slot over
it, and a single ground mixing both halves could not be split there, so
there is no midpoint between the two laws. What *is* continuous is the
input.

The `paint` terminal's `buildup` port is a scalar in `[0, 1]`, and the
share it names splits every dab between two accumulations that each run
one law untouched:

| `buildup` | the scratch (the ground) | second accumulation | the dab |
| --- | --- | --- | --- |
| `0` | `accumulate_wash` | none | all of it washes |
| `1` | `accumulate_build` | none | all of it stacks |
| between | `accumulate_wash` | `build` (a second packed ground, `accumulate_build`) | `1 - b` washes, `b` stacks |

Both halves ride the one dispatch as two storage textures, so 1px spacing
stays as affordable as it was, and a brush at either end declares no
channel and pays nothing. The port is stroke-constant: its value picks the
laws and the storage bindings when the brush compiles, so no per-dab wire
can drive it, and `PortDef::stroke_constant` is what says so.

Each half has its own per-dab intensity, `wash_flow` ("Flow (Wash)") and
`build_flow` ("Flow (Build-up)"), so an author can scale one without the
other. They matter because the two ends deposit different amounts on
untouched ground: the ceiling takes one dab where source-over stacks every
dab that lands, which at 1px spacing is tens of them. A pass on fresh
ground therefore darkens as the dial rises (measured on the Pencil at
pressure 0.7, darkest over white: 149 / 74 / 30 / 9 / 1 across the dial),
and lowering `build_flow` against `wash_flow` is how a brush holds it
level. The terminal does not correct it automatically: the only correction
that would is a per-dab normalisation by the overlap count, and that
removes the per-dab stacking the top of the dial exists to provide.

What the dial buys is that the two places overlap can happen agree.
Retracing a path inside one stroke and drawing a second stroke over it
build the same amount at every setting (measured surcharge, median alpha
at spacing 0.30: 2 / 25 / 43 / 54 / 62 within a stroke, 0 / 24 / 44 / 56 /
63 across strokes). The wash half refuses at both sites, the stacking half
compounds at both, and the commit is what keeps them in step.

### What `Wash` requires, and what it changes

**Whole pigments, any chroma.** The ceiling deposits a dab's colour as a
whole: room is measured along the dab's own pigment, so a graph that varies
dab colour per dab (via `random`, `split_color`, or an `image` tip) lays
each colour down intact, and a later pigment far from an earlier one finds
room and replaces it rather than fringing. (The `Max` blend state the
instanced terminal ran took each channel's maximum from a different dab,
which is why such brushes once had to stay on `Build-up`.)

**Canvas-space fields survive; dab-space fields do not.** For one pigment
the law is a per-pixel maximum of coverage, and anything fixed per canvas
pixel factors straight out of it (`max_i(g(x) * k_i) = g(x) * max_i k_i`
for `g >= 0`), so canvas-space noise and the selection mask modulate the
finished mark instead of being saturated through. That is an improvement
for selection in particular: under source-over a 50%-selected region still
converges toward 1 as dabs accumulate, where under the ceiling it converges
to `0.5 * max a_i`, so feathering is properly respected. The converse is
the trap: a *dab*-space field draws an independent sample per dab, and with
17 to 35 samples the max saturates it to near 1 across the whole dab
interior. Dab-space grain goes inert under `Wash`. The Pencil's paper grain
is authored in canvas space for exactly this reason.

**Erase inherits the law.** Erase reads the same scratch
(`composite.wgsl`'s `destination_out` branch takes `fg_a` from it), so a
`Wash` brush used as an eraser stops punching further through where its
own dabs overlap. That is the same promise in both directions and is
deliberate.

### The ceiling also applies across strokes

`Wash` caps overlapping deposit at both places it accumulates. The per-dab
law above handles one stroke's own dabs; the commit
(`shaders/brush/composite.wgsl`) caps a stroke against what the layer already
holds. Both call the same `ceiling_t`
([`shaders/lib/deposit_ceiling.wgsl`](../../crates/darkly/shaders/lib/deposit_ceiling.wgsl)),
one against the premultiplied ground and one against the straight-alpha
layer. One invariant covers both: **a pixel never takes more deposit than a
single pass over it would have laid down.**

The two are different mechanisms and have to be, and the ceiling is hard at
both. Softening it per dab would compose to `1 - (1 - s*k)^n` with
`n = diameter/spacing`, putting tone back under the spacing slider, which is
the defect it exists to remove; softening it only at the commit would let
separate strokes build while a stroke crossing its own path stayed capped.
Keeping it hard at both sites is what makes crossing your own stroke and
crossing an earlier one give the same result. The dial changes how much of
each dab this law receives, never how strictly it then applies, which is why
that parity survives at every setting.

**The commit takes two foregrounds**, one per law, each with its own stroke
opacity where zero means the slot is absent. The wash slot goes through the
ceiling below; the build slot is then composited on top with plain
source-over. The order is load-bearing: the ceiling reads the ground to find
room, so a build half laid underneath would let a stroke's own build-up shrink
its own wash. Under erase each slot removes its own coverage and neither
consults the ceiling, because removal must be able to reach zero. Which
accumulation fills which slot is the terminal's knowledge; the shader knows
two slots and two laws, and watercolor fills the build slot alone.

**How the commit decides.** A pass carrying pigment `C` at coverage `s` lands,
starting from blank, a fixed fraction of the way to `C`. Everything past that
is refused. So rather than asking what the pass would add, the shader asks how
much room is left between where the pixel already sits and where this pass
saturates, and deposits exactly that:

```
origin = gamut corner opposite C        // the deposit scale's zero
reach  = chebyshev(origin, C)           // a full-strength deposit's span
ground = bg.rgb * bg.a + origin * (1 - bg.a)
d      = chebyshev(ground, C)           // how far this pixel still is from C
t      = max(0, 1 - (1 - s) * reach / d)
out    = source_over(C * t, t, bg)
```

It stays ordinary source-over, with an effective coverage computed from how
close the pixel already is to the pigment. On untouched ground `d == reach` and
`t == s`, the full deposit. At saturation `t == 0`, and the pixel is returned
exactly as it was rather than re-derived through a zero-alpha source-over
(whose division would perturb it). A heavier pass shrinks the saturation
distance and reopens room, so pressure still works.

Three properties fall out of that shape, and each was a bug in an earlier
version of this code:

- **Layer transparency does not change the result.** Room is read from the
  pixel's colour composited over the deposit's origin, which is the one
  quantity a transparent layer and an opaque one holding the same visible mark
  agree on. An earlier version capped `max(bg.a, fg_a)` instead and was
  therefore completely inert on an opaque layer, which is what Darkly's own
  fresh document hands the artist.
- **No assumption that pigment is dark.** `origin` is derived from `C`, so a
  white pencil on black ground behaves exactly like a black one on white. An
  earlier attempt measured room along a luminance axis and only worked for dark
  pigments.
- **Saturation is per pigment, not global.** Room is distance to the colour
  being laid down, so a red mark is nowhere near saturated for blue and takes
  it normally. A design that capped alpha alone could not express this.

**The max-norm is load-bearing.** Under it `d <= reach` holds for every colour
in the cube, so a pass can only ever be reduced, never amplified, and `t`
collapses to exactly `s` on any untouched ground. Under a Euclidean norm that
is false: white is not red's antipode, so a red pencil on white paper would
saturate at a weaker mark than graphite does at the same pressure. This is also
where a move to OKLab would land, since distance there is Euclidean and
perceptually uniform, which is what this actually wants; it needs a different
reference than the cube corner to keep the `d <= reach` guarantee, so it
belongs with the colour-system rewrite rather than before it.

**The refusal is absolute, within its half.** `t` is the whole answer: a pixel
at the saturation level takes nothing more of the washing half, at any
pressure, from any number of later strokes. There is no partial ceiling. A
commit-side dial that relaxed it used to exist and was removed: it could only
soften the *commit*, so a stroke crossing its own path stayed fully capped
while separate strokes built, and the two sites agreed only at its zero. That
made it a second, weaker copy of source-over accumulation. What varies with
`buildup` is how much of each dab is handed to this law at all, not how
strictly the law then applies.

**What the ceiling costs.** At the bottom of the dial, crosshatch
intersections do not build, abutting hatch strokes leave a seam at the join,
and tone cannot be built by repeated passes at one pressure; pressure is the
only tonal control. That is the opposite of how graphite behaves, and it is
the trade the law makes. Raising `buildup` is how a brush buys some of that
back, and the two shipped Pencils sit at the two ends.

**What it still cannot know is history.** The layer stores appearance, not what
made it. A pixel already close to the pigment reads as saturated whether this
brush put it there, another brush did, or it came in with a pasted image. That
is a real limitation, but it is a smaller one than the alpha cap's: it is
scoped to the pigment being laid down and it behaves the same everywhere,
rather than silently doing nothing on opaque ground.

**Erase is deliberately not capped.** `destination_out` returns before the
ceiling, so removal stays fully accumulative across strokes and an eraser can
always reach zero. Within a stroke the ground's wash law still applies, so a
soft eraser stops punching further through on self-overlap. Deposit saturates;
removal does not.

Prior art informs the shape but not the default. Krita's `KoCompositeOpGreater`
(Nicholas Guttenberg) is a destination-aware ceiling, though it back-solves an
effective source alpha and so cancels colour along with coverage; Krita's
`KoCompositeOpAlphaDarken` and `KoCompositeOpMarker` run separate colour and
alpha laws, as does GIMP's `GimpLayerCompositeMode`. But every
cross-destination ceiling in either codebase is a user-selectable blend mode,
never a paintop default: both editors cap only within a stroke.

## Terminal nodes

The graph is free-form, but a stroke only produces visible output if at
least one **terminal** node is reachable. A terminal is any GPU node whose
job is to put something on the layer (stroke mode) or on the preview mask
(preview mode). Terminals participate in the stroke *lifecycle* by
overriding `begin_stroke` / `commit` in addition to per-dab `evaluate_gpu`.

Non-terminal nodes (`stamp`, `circle`, `user_input`, …) don't override the
lifecycle hooks; their default impls are no-ops.

### `paint` (paint terminal)

Its registration declares `dab_pass: DispatchPerDab` and
`scratch_format: PACKED_GROUND_FORMAT`, which is what makes the stroke
buffer allocate the ground as read-write storage and the assembler emit
the compute skeleton.

- `begin_stroke` (framework, `Lifecycle::ClearScratchToTransparent`):
  clears the ground to transparent.
- `evaluate_gpu` (per dab): records the dab's layer-clamped footprint,
  pushes its size onto the batch's per-dab meta, and queues the dab record.
- `flush_dabs` (per event): uploads the records once, opens one compute
  pass, and per dab binds the dab's index through a static index buffer at
  a dynamic offset and dispatches `ceil(w / 8) x ceil(h / 8)` workgroups
  over the footprint. Each thread derives the footprint's corner from the
  record exactly as the CPU did, rejects pixels off the layer and past the
  dab's bbox, evaluates the compiled graph at the pixel centre, and stores
  through the brush's law. Selection is sampled per dab inside the law.
- `commit`: `commit_brush_dab` with the ground (and the build channel,
  inside the dial) as packed foregrounds, through `composite.wgsl`'s
  `fs_packed` entry. Applies `gpu.blend_mode` (paint / erase toggle).

**The appearance mirror.** A graph that samples the stroke at other
pixels (the canvas sampler's Live source, which the Smudge feeds into
`stamp.color`) cannot read the ground from inside the dab's dispatch: the
thread owning the texel it reads runs in the same dispatch, and WebGPU
orders nothing within one. Such a graph requests
`LiveSource::StrokeAppearance`, and `paint` runs a second dispatch before
each dab's (`brush/appearance_snapshot.rs`): the ground (and the build
channel) laid on the pre-stroke snapshot through `lib/commit_law.wgsl`, the
law the commit uses, at full strength in paint mode, written into a
layer-sized `rgba8unorm` mirror on `Scratch` under the dab's read region.
The read region is the footprint grown by the graph's per-dab
`read_reach` (the sampler's `|motion|` plus a texel). The dab then samples
the mirror as an ordinary graph texture. Opacity and erase stay at the
commit. The mirror is derived per dab, so it is never cleared,
checkpointed or restored. An instanced terminal cannot order work
between its dabs, so the compiler rejects a graph that asks for the
appearance under one (`DabPass::can_refresh_between_dabs`).

The hover preview is a fragment module for every terminal; `paint`'s
preview body shows the one dab as the stroke would deposit it on blank
ground.

### `liquify` (warp terminal)

- `begin_stroke`: `copy_texture_to_texture(layer_texture → stroke_scratch_texture)`.
  The scratch starts as a copy of the real canvas.
- `evaluate_gpu` (per dab): `ensure_canvas_copy` snapshots the current
  scratch; the liquify shader samples the copy with displaced UVs (`pos -
  motion * falloff * strength`) and writes the warped value back to the
  scratch. Each dab sees the cumulative warp from the prior dabs.
- `commit`: `copy_texture_to_texture(stroke_scratch_texture → layer_texture)`.
  Replace: the scratch already represents the finished image.

`gpu.blend_mode` is ignored; a warp isn't paint.

### `preview_output` (preview-only terminal)

- `evaluate_gpu` (per dab): if `render_mode == Preview`, blits the upstream
  dab texture into `preview_mask_view`. Otherwise bails.
- `begin_stroke` / `commit`: no-op (preview doesn't participate in strokes).

Graphs without a `preview_output` have no hover preview; the overlay falls
back to the tool's generic cursor ring.

### One graph, two render modes

Presets typically wire both a stroke-writing terminal (`color_output` or
`liquify`) and `preview_output` from shared upstream nodes:

```
stamp.dab ─┬─► color_output.dab    (render_mode = Stroke)
           └─► preview_output.dab  (render_mode = Preview)
```

Running the graph with `render_mode == Stroke` fires `color_output`'s
lifecycle hooks and `evaluate_gpu`; `preview_output` is a no-op. Running
with `render_mode == Preview` flips the roles. Non-terminal nodes are
mode-agnostic and run identically in either pass.

## The per-dab GPU context

`BrushGpuContext` ([`gpu_context.rs`](../../crates/darkly/src/brush/gpu_context.rs))
bundles everything `evaluate_gpu` needs. Every durable surface is exposed
by its *real identity*: nodes pick what they need, the engine never
secretly swaps resources behind a single misleading name.

| Field | Purpose |
|---|---|
| `encoder` | Command encoder shared across all dabs in a segment |
| `device`, `queue` | Standard wgpu handles |
| `dab_pool` | Pre-allocated 512×512 RTs for stamp / circle outputs |
| `pipelines` | `BrushPipelines` with shaders + uniform rings |
| `layer_view`, `layer_texture` | The actual layer.  Warp terminals read/write it; paint terminals use it only at `commit`. |
| `stroke_scratch_view`, `stroke_scratch_texture` | Stroke-scoped scratch.  `Some` during a stroke, `None` in preview mode. |
| `pre_stroke_texture` | Layer snapshot taken at stroke start.  `color_output::commit` uses it as the source-over background. |
| `scratch_bind_group`, `pre_stroke_bind_group` | Pre-built bind groups exposing the scratch/snapshot for reuse with the composite pipeline (used by `color_output::commit`). |
| `preview_mask_view`, `preview_mask_size` | Preview-mode render target.  `Some` in preview, `None` in stroke. |
| `selection_bind_group` | Active selection mask (or 1×1 white when unset) |
| `resource_handles` | Named texture handles for `image` nodes |
| `blend_mode` | Engine-level paint/erase toggle.  Honoured by `color_output::commit`; ignored by warp terminals. |
| `canvas_copy_origin` | Per-dab cache for `ensure_canvas_copy`. Reset to `None` in `place_dab` before each dab. |
| `render_mode` | `Stroke` or `Preview`, terminals switch on this. |

### Uniform batching

Each pipeline owns a `DynamicUniformRing` (~256 slots). A dab's uniform block
is written to the next slot; the dynamic offset is passed to `set_bind_group`.
This means all dabs in a stroke segment go through **one** encoder and **one**
`queue.submit()`, instead of per-dab submission. When any ring nears capacity
the engine flushes mid-stroke (cheap: a few per 1000 dabs). Dab-batching
terminals hold their queue on the CPU in a buffer sized to
`MAX_DABS_PER_PHASE`; `StrokeEngine::place_dab` flushes the terminals and
submits when a phase reaches it, which only a whole path rendered in one
phase does: the brush preview, or a stroke no frame ran during.

### `ensure_canvas_copy`

WebGPU disallows sampling and writing the same texture in one pass. So before
sampling the scratch (for Porter-Duff bg, or for liquify warp source), we do
`copy_texture_to_texture` from `stroke_scratch_texture` into
`canvas_copy_texture`. Keyed on the integer copy origin so multiple nodes
within one dab don't re-copy. Reset by `place_dab` before each dab so the
next dab sees fresh data.

Sampler is **linear**: composite reads at pixel centres (equivalent to
nearest), liquify reads with arbitrary UV displacement and needs bilinear.

## Graph compilation and evaluation

`compile_graph(&graph)` → `BrushGraphRunner`. Compilation does:

- Topological sort of nodes respecting wire dependencies.
- Slot-table allocation: one flat `Vec<Option<ScalarValue>>` entry per output
  port in the graph. No per-node HashMaps on the hot path.
- Pre-resolve the `pen_input` slots (so `seed_sensors()` can write directly
  without any lookup) and the `paint_color` slot.
- Precompute curve LUTs for nodes with `Curve` params.

Per dab:

1. `clear_slots()` - `None` every slot.
2. `seed_sensors(&paint_info)` - direct writes to `pen_input` slots.
3. `execute_cpu()` - walk steps, gather inputs by slot, dispatch to evaluator,
   write outputs.
4. `execute_gpu(ctx)` - same walk for GPU nodes; each records render passes.

Evaluator dispatch is a `HashMap<type_id, Box<dyn BrushNodeEvaluator>>` lookup
per step (~5-15 steps per dab), so the HashMap cost is noise compared to the
render pass.

## Dab spacing

The stroke engine places dabs at a fixed distance along the straight
segments between consecutive stabilized vertices, every field interpolated
linearly along the segment. The distance is derived from the brush's **own
reported `dab_size`**: the runner caches, at build time, the slot of
whichever terminal publishes a `dab_size: Vec2` output, and the stroke engine
reads it after every dab (`BrushGraphRunner::last_dab_size`). A terminal that
wants its footprint to drive spacing only has to publish that port.

## Undo and save points

Two cooperating structures:

- **`StrokeBuffer::save_pre_stroke`** - a full snapshot of the layer taken at
  `begin_stroke`. Owned by the stroke buffer; the layer's original pixels can
  be read back from here during undo.
- **`SavePoints`** - a per-dab log of bounding boxes + render-state
  checkpoints. Its `full_bbox()` gives the total damage rect for the stroke.

At `end_stroke`, the damage rect is registered with the undo ring. Undo
restores only the damaged region from the pre-stroke snapshot.

## Preview regen

When the brush tool is active and the cursor hovers without pressing, the
engine calls `regenerate_brush_preview()`
([`engine/brush_graph.rs`](../../crates/darkly/src/engine/brush_graph.rs)). It:

1. Short-circuits if `runner.has_preview_terminal() == false`.
2. Allocates (or re-uses) a 128×128 overlay preview mask texture.
3. Builds a `BrushGpuContext` with `render_mode: Preview`, `canvas_view`
   pointing at the preview mask.
4. Runs `seed_sensors` with a synthetic paint event, `execute_cpu`,
   `execute_gpu`.
5. Reads `BrushPreviewInfo` back from the `preview_output` node's resolved
   input slots (so the overlay knows the canvas-space half-extent and
   rotation).

If the graph has no `preview_output`, the preview mask is cleared and the
overlay draws nothing brush-shaped; the cursor just gets the tool's generic
ring.

## Warp brushes (and other non-paint terminals)

Terminals that transform the layer rather than depositing pigment
(liquify, blur, displacement, future effects) fit the system
through the **same** `begin_stroke` / `evaluate_gpu` / `commit` lifecycle
as paint, without any warp-specific code in the engine.

**Liquify as the worked example** ([`nodes/liquify.rs`](../../crates/darkly/src/brush/nodes/liquify.rs)):

- `begin_stroke`: copy `layer_texture` → `stroke_scratch_texture`. The
  scratch now starts as the *real* canvas, not a transparent surface.
- `evaluate_gpu`: `ensure_canvas_copy` snapshots the current scratch into
  `canvas_copy_texture`. The liquify shader samples the copy with a
  displaced UV (`canvas_pos - motion * falloff * strength`) and writes the
  warped value back into the scratch. Successive dabs compound because
  each one reads the scratch after the previous dab has mutated it.
- `commit`: `copy_texture_to_texture(scratch → layer_texture)`. The layer
  atomically becomes the warped image. No blend: the scratch already
  holds the finished pixels.

Stabilizer rewind works out of the box: on a full rewind the engine calls
`runner.begin_stroke` again, re-copying the layer and wiping prior warps.
Partial rewind from a checkpoint re-seeds only the rewound region from the
pre-stroke layer where the checkpoint's frame does not cover it, then
restores the scratch's bytes over that region directly (the checkpoint
doesn't care whether those bytes represent accumulated pigment or a
warped layer).

**Designing a new non-paint terminal:**

1. Add a node under [`crates/darkly/src/brush/nodes/`](../../crates/darkly/src/brush/nodes/)
   with `is_gpu: true`.
2. Implement `evaluate_gpu` for the per-dab work. Read whatever it needs
   (scratch via `ensure_canvas_copy`, layer directly, dab_pool textures,
   …) and write to `stroke_scratch_view`.
3. Implement `begin_stroke` to initialise the scratch however the effect
   wants: clear, layer-copy, something else entirely.
4. Implement `commit` to push the scratch onto the layer: source-over,
   replace, destination-out, custom blend, or whatever matches the effect.
5. Register the evaluator in [`brush/mod.rs::default_evaluators`](../../crates/darkly/src/brush/mod.rs).
6. Write a preset that wires `pen_input` to the new terminal, plus a
   `preview_output` subtree so hover feedback works.

No engine changes needed.

**Prefer a sampler on `paint` to a new read-mirror terminal.** An effect
that reads the canvas the stroke is changing (a smear, a blur, a pickup)
can be a node feeding `paint` that requests
`LiveSource::StrokeAppearance` and reports its per-dab `read_reach`;
`paint` then supplies the ordering, the dial, flow, pressure size,
selection, the hover preview, the commit and undo. The Smudge is built
that way; the `blur` terminal predates it.

## Performance anchors

- One `queue.submit` per stroke segment, not per dab (dynamic uniform ring).
- `canvas_copy` cached per-dab (not per-node within a dab).
- Dab pool returns RTs to a free list after each dab: zero allocation during
  a stroke.
- Stabilizer divergence triggers partial re-render from the nearest save
  point, not full stroke re-render.

See [gpu-lessons-learned.md](../lessons-learned/gpu-lessons-learned.md) for the specific
pitfalls (pixel-centre offsets, readback deadlock on WebGPU, NDC stretch on
padded textures, copy-origin UV math).

## Extending the system

- **New node type** → [node-system.md](node-system.md).
- **New stabilizer algorithm** → [stabilization.md](stabilization.md).
- **New terminal behaviour** (warp, smudge, filter) → re-read the "Warp
  brushes" section above and design the engine-level routing *first*, not the
  node.
