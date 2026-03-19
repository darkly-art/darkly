# Compositing Lessons Learned

## 1. Straight vs premultiplied alpha: the convention mismatch that darkens everything

**Problem**: Copy-paste with a feathered selection produced visibly darker content. The darkening accumulated with repeated transform cycles, creating progressively worse dark borders at content edges. Took five debugging sessions across bilinear interpolation, compositor blending, accumulator precision, and pixel-center alignment before finding the actual cause.

**Root cause**: The selection-coverage copy path in `ImageClip::from_layer` (`clipboard.rs`) scaled both RGB and alpha by coverage, producing premultiplied pixel values that were stored into the straight-alpha tile grid:

```rust
// Bug: scales RGB by coverage — produces premultiplied output
let a = (pixel[3] as f32 * coverage).round() as u8;
let ratio = a as f32 / pixel[3] as f32;  // ≈ coverage
out[0] = (pixel[0] as f32 * ratio).round() as u8;
out[3] = a;
```

For `[255, 0, 0, 255]` at 50% coverage this produced `[128, 0, 0, 128]`. As premultiplied that's correct (full red at half opacity). But stored as straight alpha, the compositor reads it as color=128, alpha=0.5 — dark red at half opacity. The correct straight-alpha result is `[255, 0, 0, 128]`.

**Why it was hard to find**: The bug was in the copy path, but the symptom appeared during transform/paste. The darkened pixels looked identical to what you'd expect from a compositing or interpolation error, so every debugging session focused on the transform and compositor pipelines — which were all correct. The key clue was that pure transforms (no copy/paste) didn't produce the fringe.

**Fix**: In straight alpha, coverage scaling only affects the alpha channel. The RGB channels are the actual color and don't change:

```
new_straight_R = (R · α · coverage) / (α · coverage) = R
```

Just copy RGB through, scale only alpha.

**Takeaway**: When manipulating pixel alpha in a straight-alpha pipeline, never touch the RGB channels. The color is the color — alpha is a separate, independent quantity. If you find yourself multiplying RGB by an alpha-related factor and storing the result as straight alpha, you're producing premultiplied values in a straight-alpha container. This is the single most common alpha compositing bug and it's invisible until something downstream reveals it (blending, interpolation, repeated operations).

## 2. Premultiplied interpolation for GPU texture sampling

**Problem**: Hardware bilinear filtering on `Rgba8Unorm` textures operates in straight-alpha space by default. At content edges (opaque pixel adjacent to transparent-black), interpolating `[255, 0, 0, 255]` with `[0, 0, 0, 0]` at 50% gives `[128, 0, 0, 128]` — which in straight alpha means dark red at half opacity, not bright red at half opacity. This is the classic "dark halo" artifact.

**Fix**: Upload source texture data as premultiplied (multiply RGB by alpha before `write_texture`), and adjust the shader to treat sampled values as premultiplied in the blend formula. The CPU bilinear sampler already did this correctly — the GPU path needed to match.

**Takeaway**: Any time you sample a texture with hardware bilinear filtering and the texture contains transparency boundaries, either upload as premultiplied or use a premultiplied texture format. Straight-alpha + linear filtering = dark halos. This applies to transform previews, texture atlases with transparent regions, and any filtered sampling near alpha edges.

## 3. Porter-Duff source-over: know which space you're in

The correct Porter-Duff source-over formula depends on whether inputs are straight or premultiplied:

**Straight alpha** (what Darkly's compositor uses):
```
out_a = fg.a + bg.a * (1 - fg.a)
out_rgb = (fg.a * fg.rgb + (1 - fg.a) * bg.a * bg.rgb) / out_a
```

**Premultiplied alpha**:
```
out_a = fg.a + bg.a * (1 - fg.a)
out_rgb_pre = fg_pre.rgb + bg_pre.rgb * (1 - fg.a)
```

Common errors:
- **Double-alpha**: Using `fg.rgb * fg.a` when `fg.rgb` is already premultiplied. Effective opacity becomes α².
- **Missing un-premultiply**: Blending in premultiplied space but storing the result without dividing by `out_a`. The stored pixel has dimmed RGB.
- **Cross-space blend**: Treating one input as premultiplied and the other as straight in the same formula. Both inputs must be in the same space before blending.

**Takeaway**: Every alpha touchpoint in the pipeline should have a comment stating which convention the input is in and which convention the output produces. When in doubt, trace the math with a concrete example: `[255, 0, 0, 255]` blended 50/50 with transparent should produce visually bright red at half opacity, not dark red.

## 4. Hardware alpha blending writes premultiplied to straight-alpha targets

**Problem**: GPU brush dabs had visible dark outlines, especially on transparent canvas regions. A white brush stroke appeared with grey edges. The dab texture was correctly stored as premultiplied (per lesson #2), but the dark fringe persisted.

**Root cause**: Hardware alpha blending — both `SrcAlpha/OneMinusSrcAlpha` (straight source) and `One/OneMinusSrcAlpha` (premultiplied source) — inherently produces premultiplied output. The blend equation is:

```
out.rgb = src_color * src_alpha + dst.rgb * (1 - src_alpha)
out.a   = src_alpha + dst.a * (1 - src_alpha)
```

On a transparent canvas `(0, 0, 0, 0)`, painting white at α=0.5 stores `(0.5, 0.5, 0.5, 0.5)`. The compositor reads layer textures as straight alpha, so it interprets the color as grey — not white at half opacity. The correct straight-alpha value is `(1.0, 1.0, 1.0, 0.5)`.

This is invisible at full opacity (premultiplied = straight when α=1) and on fully opaque backgrounds (the RGB just works out). It's only visible in the soft-edge region of dabs on transparent or partially transparent canvas — exactly where it manifests as a dark outline.

The existing paint pipeline (`paint_circle.wgsl`) has the same theoretical issue, but its 1px softness edge makes it imperceptible. The brush system has wide softness (up to half the dab radius), making the artifact very visible.

**Why hardware blending can't do straight-alpha source-over**: The correct straight-alpha formula requires dividing by `out_a`:

```
out.rgb = (fg.a * fg.rgb + (1-fg.a) * bg.a * bg.rgb) / out_a
```

Fixed-function blend hardware can only do `A*src + B*dst` — no division, no access to the result alpha. Straight-alpha compositing is fundamentally impossible with a single hardware-blended render pass.

**Fix — shader-side Porter-Duff with canvas copy**: Before the composite render pass, `copy_texture_to_texture` copies the dab's bounding rect from the canvas layer to a pre-allocated 512×512 temp texture (`canvas_copy_texture` in `BrushPipelines`). The composite shader then:

1. Samples the dab texture (premultiplied — correct for bilinear filtering, per lesson #2)
2. Samples the canvas copy (straight alpha — the existing layer data)
3. Computes Porter-Duff source-over manually in the shader (premultiplied fg, straight bg → straight output)
4. Outputs with **REPLACE** blend (no hardware alpha blending)

This produces correct straight-alpha values in the canvas layer. The GPU copy is a fast blit (same-format, no conversion), and the shader math adds only a texture sample + a few ALU ops per fragment.

**Takeaway**: You cannot use hardware alpha blending to write into a straight-alpha render target and get correct results on partially transparent backgrounds. The output is always premultiplied regardless of source convention. If the target must stay straight alpha (as Darkly's layer textures do), the shader must read the destination, compute the blend manually, and write with REPLACE. The "copy destination before compositing" pattern makes this possible in a render pass without compute shaders or storage textures.

## 5. REPLACE blend + shared uniforms = one submit per dab

**Problem**: Brush strokes showed square artifacts at fast stroke speeds. The artifacts were position-dependent — dabs appeared to jump or composite at wrong locations.

**Root cause**: `queue.write_buffer()` in wgpu stages buffer writes that execute *before* the next `queue.submit()`, not inline with the command encoder. When multiple dabs were recorded into a single command encoder and submitted together, all `write_buffer` calls for uniform data were batched and only the last write survived:

```text
write_buffer(uniforms, dab_1_data)    ← overwritten
  encoder: record proc pass 1
  encoder: record composite pass 1
write_buffer(uniforms, dab_2_data)    ← this one wins
  encoder: record proc pass 2
  encoder: record composite pass 2
submit(encoder)
  → GPU sees: write dab_2_data, then runs all 4 passes with dab_2_data
```

All render passes used the last dab's position, UV mapping, and canvas copy region. With the old hardware alpha blending this was mostly invisible (each dab just accumulated onto the canvas at slightly wrong positions). With REPLACE blend, it was catastrophic — each pass overwrote the canvas with data computed from wrong uniforms, producing visible rectangular artifacts.

**Why it wasn't caught earlier**: Before the shader-side Porter-Duff fix (lesson #4), the composite pass used hardware alpha blending (`SrcAlpha`). Incorrect uniform data still produced roughly correct results because alpha blending is additive — painting the same dab twice at the wrong position just made it slightly brighter/shifted, not destructively wrong. REPLACE blend has no such forgiveness: wrong data overwrites correct data completely.

The full-screen triangle trick (vertices at unit coords (0,0), (2,0), (0,2)) also contributed: the triangle extends 2× beyond the intended quad, so fragments outside the quad read stale data from the canvas copy texture and overwrote the canvas. The fix was to use a proper 6-vertex quad (two triangles) instead of the oversized triangle — the geometry now covers exactly the intended region.

**Fix**: Two changes:

1. **Proper quad geometry**: Replace the full-screen triangle trick with a 6-vertex quad (two triangles at unit corners (0,0), (1,0), (0,1), (1,1)). The rendered region now matches the intended composite area exactly. This is better than a scissor rect because future effects (e.g. liquify) may legitimately need a larger output region — the quad and canvas copy should grow together, not be artificially clipped.

2. **Per-dab submit**: `BrushGpuContext::submit_and_reset()` finishes the current encoder, submits it, and creates a fresh encoder. Called after each dab in `place_dab()`. This ensures each dab's `write_buffer` uniforms are consumed by `submit()` before the next dab overwrites them.

```rust
pub fn submit_and_reset(&mut self) {
    let finished = std::mem::replace(
        &mut self.encoder,
        self.device.create_command_encoder(...),
    );
    self.queue.submit([finished.finish()]);
}
```

**Takeaway**: `queue.write_buffer()` is not a command encoder operation — it's a queue-level staging write that flushes at the next `submit()`. If multiple render passes in one encoder depend on different uniform values written via `write_buffer`, only the last write is visible to all of them. When using REPLACE blend (or any pattern where correct per-pass uniform data is critical), submit after each logical unit of work. The per-submit overhead is negligible compared to the GPU work in each dab.
