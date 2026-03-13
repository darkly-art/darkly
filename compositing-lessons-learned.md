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
