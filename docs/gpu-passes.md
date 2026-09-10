# WebGPU Passes: what they are, and where they cost us

## What a pass is

A render pass is the unit of work the GPU executes between memory barriers. In wgpu, it's the scope opened by `begin_render_pass` and closed by dropping the returned pass object.

**Inside the pass:**
- Draws write to the attachments declared at `begin_render_pass`.
- The target texture is locked write-only: no shader running inside the pass can sample it.
- Many draw calls can share the pass (cheap).

**At the pass boundary:**
- The GPU drains its pipeline and inserts a memory barrier so writes from pass N are visible to pass N+1.
- Tile-based GPUs (mobile, Apple Silicon) also flush their tile cache here.

This is why "fewer passes" is a real optimization knob: each pass-end is a serialization point.

## The "no read+write same texture in one pass" rule

Falls straight out of the locking above. A texture is bound either as a color attachment (write) **or** as a sampled input (read) for the duration of the pass: never both.

To do a read-modify-write effect, end the pass, `copy_texture_to_texture` to a mirror texture, start a new pass that reads the mirror while writing the original. See [crates/darkly/src/brush/scratch.rs](../crates/darkly/src/brush/scratch.rs).

## Minimal working example

Fill a texture with one color. The GPU "hello world."

### `shader.wgsl`

```wgsl
// Runs once per vertex. We draw 3 vertices forming one big triangle
// that covers the whole render target. No vertex buffer: positions
// computed from the vertex_index the GPU hands us.
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let x = -1.0 + 4.0 * f32(i == 1u);   // i=0:-1, i=1:3, i=2:-1
    let y = -1.0 + 4.0 * f32(i == 2u);   // i=0:-1, i=1:-1, i=2:3
    return vec4f(x, y, 0.0, 1.0);        // clip space: [-1,1] = visible
}
// Runs once per pixel covered by the triangle. Hardcoded pink.
@fragment fn fs() -> @location(0) vec4f { return vec4f(1.0, 0.3, 0.5, 1.0); }
```

### Rust

Given `device`, `queue`, and `target: &wgpu::TextureView`:

```rust
// Compile WGSL → driver IR. Done once at startup.
let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
    label: None,
    source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
});

// A pipeline bakes together every piece of fixed state the GPU needs to
// draw: which shader, entry points, output format, blend mode. Built
// once; binding it later for a draw is cheap.
let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
    label: None,
    layout: None,                              // no bind groups → auto-derive
    vertex: wgpu::VertexState {
        module: &shader,
        entry_point: Some("vs"),
        buffers: &[],                          // no vertex attribute buffers
        compilation_options: Default::default(),
    },
    fragment: Some(wgpu::FragmentState {
        module: &shader,
        entry_point: Some("fs"),
        targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())], // must match target's format
        compilation_options: Default::default(),
    }),
    primitive: Default::default(),             // TriangleList
    depth_stencil: None,
    multisample: Default::default(),
    multiview_mask: None,
    cache: None,
});

// CPU-side scratchpad. Commands recorded here don't run until submit.
let mut enc = device.create_command_encoder(&Default::default());

{
    // begin_render_pass: "the next commands draw into `target`."
    // Inside this scope `target` is locked write-only.
    let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),  // wipe before drawing
                store: wgpu::StoreOp::Store,                    // keep the result
            },
        })],
        ..Default::default()
    });
    pass.set_pipeline(&pipeline);
    pass.draw(0..3, 0..1);   // 3 vertices, 1 instance → one triangle
}   // ← `pass` drops here. THIS is what ends the render pass.

queue.submit([enc.finish()]);   // NOW the GPU actually runs the recorded commands.
```

The whole flow: compile shader → bake into pipeline → record "clear `target`, run pipeline once" into encoder → submit. The closing brace of the `{ … }` block defines the boundary of one pass.

## Vocabulary

| Term | What it is |
|---|---|
| **Shader module** | Compiled WGSL. Compile once, reuse forever. |
| **Bind group layout** | The *shape* of inputs the shader expects (slot 0 = uniform, slot 1 = texture, …). |
| **Bind group** | The *concrete* resources fulfilling that shape. Swappable between draws. |
| **Pipeline layout** | Ordered list of bind group layouts; matches `@group(N)` in WGSL. |
| **Pipeline** | Shader + layout + output format + blend state, baked together. |
| **Command encoder** | CPU-side scratchpad. Records commands; nothing runs until `queue.submit`. |
| **Pass** | Scope opened by `begin_render_pass`, closed by dropping it. Writes to the attachments declared at the start. |
| **Draw call** | `pass.draw(verts, instances)`. Many draws can share one pass (cheap). |

## Applied: where dab rendering spends its time

Each dab in our brush pipeline issues roughly:

1. **Dab-gen pass**: `stamp.wgsl` or the inlined `shape` node rasterizes the brush mark into a pool texture.
2. **Read-mirror sync**: `copy_texture_to_texture` from scratch's write side to its read mirror.
3. **Composite pass**: `color_output` shader reads the dab + reads the mirror + writes scratch.

So ~2 passes + 1 copy per dab, minimum. Smudge adds another. At a few hundred dabs per frame, the GPU likely spends most of its time at pass boundaries rather than in shader code.

## Optimizations available

### 1. Instanced batching

`pass.draw(0..3, 0..N)` draws N dabs in one pass with one barrier at each end. Pack per-dab data into a storage buffer, draw all of them in one call.

**Constraint:** instanced dabs execute in parallel within the pass, so dab `i+1` cannot read what dab `i` wrote.

- **Works for:** procedural shapes onto a fresh scratch with hardware blending (`PREMULTIPLIED_ALPHA_BLENDING` for additive accumulation).
- **Breaks for:** smudge, watercolor pickup, anything sampling the cumulative read mirror.

Practical version: walk the stabilized polyline, group consecutive non-overlapping dabs (or those whose blend mode is associative + commutative), emit each group as one instanced draw. Flush with a `copy_texture_to_texture` when the next dab overlaps or needs a fresh mirror. Roughly what Krita does in `KisDabRenderingQueue`.

### 2. Inline procedural dab-gen into composite

For procedural shapes (the `shape` node), the intermediate dab texture is pure overhead: `r(θ)` could be evaluated directly inside the composite fragment shader, eliminating the dab-gen pass. Halves pass count for the procedural path. Stamp brushes (user-image tip) can't do this.

### 3. Compute-pass smudge

A compute shader can read and write the same buffer via explicit `storageBarrier()`. One workgroup looping over all dabs sequentially trades parallelism for eliminating pass boundaries entirely. Only worth it if sequential dependency is the actual bottleneck: i.e. smudge specifically.

## Priority order

**Confirm pass boundaries are actually the bottleneck first.** `BrushPerfCounters` in [crates/darkly/src/brush/gpu_context.rs](../crates/darkly/src/brush/gpu_context.rs) already buckets the relevant timings, if `read_mirror_copy_us` dominates `stamp_pass_us`, the prioritization changes.

If passes are the bottleneck:

1. **Instanced batching** of consecutive non-overlapping additive dabs: biggest expected win, no shader rewrites.
2. **Inline procedural dab-gen**: straightforward, halves pass count for procedural-shape brushes.
3. **Compute-pass smudge**: bigger lift, only worth it after the above.
