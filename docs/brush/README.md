# Brush System Documentation

Darkly's brush system is a **node-graph brush engine** running on the GPU.
This directory holds the reference material; code lives in
[`crates/darkly/src/brush/`](../../crates/darkly/src/brush/) and
[`shaders/brush/`](../../shaders/brush/).

Start here:

- **[architecture.md](architecture.md)** — runtime: how strokes flow from tablet
  to canvas. Stroke engine, stroke buffer, terminals, per-dab GPU dispatch.
  **Read this first if you are touching anything in `brush/` or wondering why a
  new brush doesn't appear on the canvas.**
- **[node-system.md](node-system.md)** — authoring: how to define a new node
  type and wire up a preset. Ports, params, `PresetBuilder`, exposed knobs.
- **[stabilization.md](stabilization.md)** — stabilizer algorithms (Laplacian
  smoother) and their config surface.

Prior-art references (for research, not implementation detail):

- [krita-brush-system.md](krita-brush-system.md)
