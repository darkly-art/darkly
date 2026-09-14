# Brush Node System: Authoring Guide

This is the practical reference for **authoring brushes** (YAML under
[`crates/darkly/brushes/`](../../crates/darkly/brushes/)) and **defining new
node types** ([`nodes/`](../../crates/darkly/src/brush/nodes/)). For the runtime
architecture, see [`architecture.md`](architecture.md).

## Model

A brush is a directed graph of **nodes** whose **ports** carry typed values
(scalars, colors, textures, etc.) between them. Per dab, the runtime:

1. Seeds sensor nodes (`pen_input`, `paint_color`) from the pen event.
2. Walks CPU nodes in topological order; each reads inputs, writes outputs.
3. Walks GPU nodes in topological order; each records render passes.
4. Whatever reached the terminal gets composited onto the canvas.

Nodes are defined in individual `.rs` files under `crates/darkly/src/brush/nodes/`.
`build.rs` auto-discovers them: drop in a file with a `pub fn register()` and
it shows up. There is no central list to edit, and that includes the evaluator.

## Exposing a tunable value: one rule

A port **is** the knob. To ship a brush with a non-default value on a port, set
the value on the port instance. To let the artist adjust it at runtime, give
the port a brush-bar entry.

In a brush's YAML, `inputs` sets instance values and `exposed_ports` lists the
brush-bar entries, keyed `node_id.port_name`:

```yaml
nodes:
  brush_settings:
    type: brush_settings
    inputs:
      size: 0.2
      spacing: 0.01
  multiply_2:
    type: multiply
    inputs:
      a: 0.5
    ranges:
      a: [-1.0, 1.0]        # re-range this instance's slider
exposed_ports:
  brush_settings.size: {}   # inherits the port's own label, unit and icon
  multiply_2.a:
    label: Twirl            # per-entry overrides, all optional
    description: Twirliness factor
    icon: tabler:galaxy
    invert: true            # mirror which way the control reads
```

An entry with `{}` inherits the port's **label, unit, icon, range, and
description** from the node definition
(`PortDef::input(...).with_label(...).with_unit(...)` etc.). Everything under
the entry overrides that for this brush only. The same edits are available at
runtime through the brush bar's entry modal, so an author never has to
hand-edit YAML to rename or re-range a control.

### When to use a `user_input` node instead

The port-default path covers almost everything. One case it does not:

**Fan-out.** One dial drives several ports at once. A brush-bar entry is a
single `(node, port)` pair, so a scrub writes exactly one port's value. To make
one control reach many sinks it has to be a thing in the graph with an output,
which is what [`user_input`](../../crates/darkly/src/brush/nodes/user_input.rs)
is: a node whose entire content is one settable-source scalar, exposed by
default. Place it, wire its `value` port to as many inputs as you like, and the
painter gets one dial that moves all of them.

The other two reasons this section used to give are gone, absorbed by
per-instance state on the entry itself:

- **Range rescaling** is `ranges:` in the YAML, or the entry modal's Advanced
  block (`Graph::set_port_range`).
- **Per-brush custom label** is `label:` on the entry (`ExposedPortMeta`),
  alongside `description`, `icon` and `invert`.

Fan-out is also expressible without the node, by exposing a math node's input
and wiring its output around: place a `multiply`, leave `b` at its identity
`1.0`, expose `a`. That works, and predates `user_input`, but it costs a node
whose arithmetic is a no-op and whose header says "Multiply" when it means
"knob". Prefer `user_input` when the knob is the point, and a math node when
the arithmetic is.

## Defining a node type

Every node is a single file that does two things:

```rust
// 1. Declare ports. This is the schema the graph editor sees.
pub fn register() -> BrushNodeRegistration { ... }

// 2. Implement the evaluator: how the node computes its outputs.
pub struct MyNodeEvaluator;
impl BrushNodeEvaluator for MyNodeEvaluator { ... }
```

That is the whole registration. `BrushNodeRegistration` carries the evaluator
constructor (`BrushNodeRegistration::compute(node, || Box::new(MyEvaluator))`),
`build.rs` regenerates `nodes/mod.rs` around your file, and the registry picks
it up. Nothing outside your file changes; in particular no consumer should ever
learn your node's `type_id`.

### Port def checklist

```rust
PortDef::input("my_knob", BrushWireType::Scalar)
    .with_range(0.0, 1.0, 0.5)        // min, max, default
    .with_label("My Knob")            // shown in UI
    .with_unit(UnitType::Percent)     // Normalized | Percent | Degrees | Raw | Pixels
    .with_icon("fa6-solid:droplet")   // iconify name
    .with_description("What it does") // tooltip
    .exposed()                        // optional: brush-bar entry by default
    .source()                         // optional: also wirable *from* (see below)
```

`.exposed()` on the node definition means the port gets a brush-bar entry when
the node is placed; a brush can still drop the entry. A port with no
`.exposed()` can be surfaced per-instance instead, which is what the node
editor's eye toggle does.

`.source()` makes an **input** port also a wire source: it keeps its scrubbable
authored value *and* other nodes can wire from it. That is what lets one
control feed many sinks (`user_input.value`, `brush_settings.size`). Driving
such a port with an incoming wire retires its outgoing wires, since the
authored value no longer means anything.

`.persist_in_thumbnail()` exempts an exposed port from the neutralization that
runs before a brush's picker thumbnail is baked. Reach for it when the authored
value is part of the brush's identity rather than a momentary scrub.

### Size is a bound: the dab footprint rule

A shape node's silhouette is **bounded by the nominal dab radius**, up to a
small, declared, bounded perturbation. Two rules follow, and both are load
bearing far outside the node that breaks them:

- **An anisotropy knob is a contraction, never a stretch.** A knob that
  squashes a tip into an ellipse inscribes that ellipse in the dab radius
  (semi-axes `aspect` and `1`), so the nominal size is always the tip's
  semi-major axis. The area-preserving alternative (`aspect` and `1/aspect`)
  makes a *shape* knob change the *size*, which is how `circle.aspect` once
  reached ten times its nominal radius. Clamp the emitted value at **both**
  ends: a wired input can deliver more than 1.0, and an unclamped high end
  turns the same expression back into a stretch.
- **A bounded perturbation declares its bound through the extent protocol.**
  `r = 1 + A*sin(n*theta)` swings `+A` out and `-A` in, so it may return
  `ExtentContribution::Multiply(1 + A_max)` where `A_max` comes from the
  port's own declared range. The requirement is that a bound exists and is
  declared; the specific figure is whatever the range implies. A polar radius
  with no bound (the Gielis superformula as `n1 -> 0`) satisfies neither rule
  and needs its parameters constrained at compile time instead.

Why it matters beyond the shape: the declared extent sizes the rasterized
quad, the CPU dab bbox, the shader's write footprint, the save-point restore
region, and every preview render canvas. A node that can grow its own
footprint forces *every* one of those to reserve its worst case, whether or
not the artist's brush uses the knob.

Prior art agrees, and is worth reading before proposing an exception. Krita's
autobrush ratio sets `height() = diameter * ratio`
(`libs/image/kis_base_mask_generator.cpp:201-227`) and its image brushes
`scaleY() = scale * ratio` (`libs/brush/kis_dab_shape.h:35-38`); GIMP's
`gimp_brush_transform_get_scale` (`app/core/gimpbrush-transform.cc:732-748`)
pins one axis at exactly `scale` and multiplies the other by a factor in
`[0, 1]`. Neither preserves dab area, and in neither can the ratio control
make a dab larger than its size.

## Removed: the `constant` node

A `constant` node used to exist as a standalone value source (one param,
one output). It was redundant: a port default carries the same value with
less graph clutter, so it was removed.

`user_input` is not that node coming back. `constant` was a value source with
no artist-facing control, and what replaced it was the port default. What
`user_input` adds is the control and its fan-out, neither of which a port
default can express.

## Patterns to avoid

- **Don't** set an instance value when the port isn't wired anywhere and the
  node-def default is already right. It's just noise. A brush-bar entry is
  different, since exposing is always a brush-level choice.
- **Don't** wire into a port that also carries an instance value and expect
  the value to matter. The wire wins; the value is dead.
- **Don't** reach for `user_input` reflexively. If one control feeds one port,
  a brush-bar entry on that port is simpler, and it can be renamed, re-ranged
  and inverted without a node.
