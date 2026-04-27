# Brush Node System — Authoring Guide

This is the practical reference for **authoring brush presets** (code in
[`builtin_presets.rs`](../../crates/darkly/src/brush/builtin_presets.rs)) and
**defining new node types** ([`nodes/`](../../crates/darkly/src/brush/nodes/)).
For the runtime architecture, see [`architecture.md`](architecture.md).

## Model

A brush is a directed graph of **nodes** whose **ports** carry typed values
(scalars, colors, textures, etc.) between them. Per dab, the runtime:

1. Seeds sensor nodes (`pen_input`, `paint_color`) from the pen event.
2. Walks CPU nodes in topological order — each reads inputs, writes outputs.
3. Walks GPU nodes in topological order — each records render passes.
4. Whatever reached `color_output` gets composited onto the canvas.

Nodes are defined in individual `.rs` files under `crates/darkly/src/brush/nodes/`.
`build.rs` auto-discovers them — drop in a file with a `pub fn register()` and
it shows up. No central list to edit (but see [Gap](#gap) below about evaluators).

## Exposing a tunable value — one rule

A port **is** the knob. To ship a preset with a non-default value on a port,
set the default on the port instance. To let the user adjust it at runtime,
mark the port exposed.

```rust
b.set_port(node, "port_name", value);        // preset-specific default, not user-adjustable
b.expose_port(node, "port_name", value);     // preset-specific default + toolbar slider
```

Both helpers live on `PresetBuilder`. The port's **label, unit, icon, range,
and description** come from the node definition (set once in
`PortDef::input(...).with_label(...).with_unit(...)` etc.) — the preset does
not re-specify them.

**Example** — the canvas-brush preset:

```rust
fn canvas_brush() -> PresetBundle {
    let mut b = PresetBuilder::new();
    b.add_circle(0.4);
    b.wire(b.pen, "pressure", b.stamp, "size");
    b.wire(b.paint_color, "color", b.stamp, "color");

    let tex = b.add_texture_overlay(0); // Multiply
    b.add_pattern("canvas_grain.png", tex);

    b.set_port(tex, "scale", 1.0);       // preset-specific default, hidden
    b.expose_port(tex, "strength", 0.6); // preset-specific default + toolbar slider

    b.build_with_resources("Canvas Brush", "painting", /* ... */)
}
```

`strength` appears in the toolbar at 60%. `scale` is pinned to 100% without a UI. No extra nodes, no wires for either knob.

### When to use a `user_input` node instead

The port-default path breaks for three cases. These are the *only* legitimate
reasons to add a `user_input` node:

1. **Range rescaling.** The port expects 0–1 but the user should see a range
   in different units (e.g. pixels 1–500). `user_input` normalizes its
   displayed value back to 0–1 before feeding the port.
2. **Fan-out.** One slider drives multiple ports (or one port through some
   math first). The knob must exist as a node to be wired to more than one
   place.
3. **Per-preset custom label.** The port's built-in label isn't right for
   this preset (e.g. calling `strength` "Grain" in a canvas brush). Ports
   don't yet support per-instance label override — `user_input` is the
   workaround.

If none of these apply, use `set_port` / `expose_port`.

## Defining a node type

Every node is a single file that does three things:

```rust
// 1. Declare ports + params. This is the schema the graph editor sees.
pub fn register() -> BrushNodeRegistration { ... }

// 2. Implement the evaluator: how the node computes its outputs.
pub struct MyNodeEvaluator;
impl BrushNodeEvaluator for MyNodeEvaluator { ... }
```

Then add a one-line entry in [`brush/mod.rs::default_evaluators()`](../../crates/darkly/src/brush/mod.rs)
mapping `"type_id"` to `MyNodeEvaluator`.

### Port def checklist

```rust
PortDef::input("my_knob", BrushWireType::Scalar)
    .with_range(0.0, 1.0, 0.5)        // min, max, default
    .with_label("My Knob")            // shown in UI
    .with_unit(UnitType::Percent)     // Percent | Pixels | Degrees | Raw
    .with_icon("fa-solid fa-droplet") // Font Awesome class
    .with_description("What it does") // tooltip
    .exposed()                        // optional: show on node by default (preset can override)
```

`.exposed()` on the node definition means the port is user-facing *by default*;
the preset can still call `set_port_exposed(..., false)` to hide it.
Presets that don't mark the node-def as exposed can still call
`set_port_exposed(..., true)` per-instance — use `expose_port`.

### Param vs port

A **port** carries per-dab data (can change each dab — pressure, position).
A **param** is a graph-editor constant (does NOT change per-dab — a curve's
control points, an image node's file name, a blend-mode enum). Use a port
for anything that could be pressure-sensitive or wired from elsewhere. Use
a param for graph-authoring choices that are fixed once the brush is saved.

## Gap: evaluator registration is still central

Node *definitions* are auto-discovered by `build.rs`, but node *evaluators*
still have to be inserted by hand in `brush/mod.rs::default_evaluators()`.
This is a pre-existing architectural gap — if we want modular evaluators the
`NodeRegistration` struct would need to carry a constructor for the
evaluator. Not done yet.

## Removed: the `constant` node

A `constant` node used to exist as a standalone value source (one param,
one output). It was redundant — a port default carries the same value with
less graph clutter — so it was removed. Any saved preset that referenced
`"constant"` will fail to load. The built-in presets have all been migrated.

## Patterns to avoid

- **Don't** use `set_port` when the port isn't wired anywhere and has a
  useful node-def default. It's just noise. `expose_port` is different —
  exposing is always a preset choice.
- **Don't** stack port-default + `user_input` wired to the same port.
  The wire wins; the default is dead code.
- **Don't** reach for `user_input` reflexively. Check the three legitimate
  reasons above. If none apply, `expose_port` is simpler.
