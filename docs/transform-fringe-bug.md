# Transform Edge Fringe Bug

## Symptom

Repeated transform cycles (enter transform mode, commit) on the same content produce dark borders that accumulate over time. Copy-paste exacerbates it since paste also goes through the transform commit path.

- On a perfectly rectangular, axis-aligned object, the artifact appears on specific sides depending on sub-pixel alignment — not always all four.
- The borders are invisible during the GPU preview (active transform) but appear after CPU-side commit.
- Each cycle darkens/thins the edges further.
- Reducing layer opacity also made content darker (fixed separately — see below).

## What was fixed

### Compositor blend (composite.wgsl) — FIXED, separate bug

The blend function was squaring the foreground alpha. `blended_rgb` was computed in premultiplied space (`fg.rgb * fg.a`), then `mix(bg_pre, blended_rgb, fg.a)` applied `fg.a` a second time. Effective opacity was `opacity²` instead of `opacity`. This caused:
- Layer opacity slider making content darker and more transparent than it should be
- Semi-transparent pixels (including transform edges) rendering incorrectly

Fix: rewrote blend to use standard PDF/SVG compositing — blend modes operate on straight-alpha colors, then Porter-Duff over composites with alpha once.

### Transform shader (transform.wgsl) — FIXED, same class of bug

Had the same double-alpha issue as the compositor. `fg.rgb * opacity` didn't account for `fg.a`, and the background wasn't premultiplied by `bg.a`. Fixed to use proper `a * fg.rgb + (1-a) * bg_pre` with division by `out_a`.

### CPU bilinear interpolation (transform.rs `sample_bilinear`) — PARTIALLY HELPED

**Premultiplied-alpha interpolation:** The original code interpolated RGBA channels independently in straight-alpha space. Interpolating `[255, 0, 0, 255]` (red) with `[0, 0, 0, 0]` (transparent) at 50% gave `[128, 0, 0, 128]` — dark red at half opacity — instead of `[255, 0, 0, 128]` — full red at half opacity. Converting to premultiplied before interpolation and un-premultiplying after fixed the color darkening component.

**Clamp-to-edge:** The bilinear kernel samples `(ix,iy)`, `(ix+1,iy)`, `(ix,iy+1)`, `(ix+1,iy+1)`. At the right/bottom edges, `ix+1` or `iy+1` falls outside content bounds. Returning transparent for these caused alpha erosion at those edges — the GPU doesn't have this issue because hardware samplers use ClampToEdge by default. Clamping the CPU sampler to match reduced the artifact.

### CPU/GPU pixel-center alignment (transform.rs) — FIXED, context 4

**The bug:** CPU rasterization sampled at integer pixel positions `(px, py)` while the GPU fragment shader samples at pixel centers `(px + 0.5, py + 0.5)`. For fractional translations, this caused the CPU bounds check to reject the first column/row of pixels that the GPU correctly renders.

**Example:** With translation `(89.416, 46.593)` from origin `(555, 328)`:
- CPU at pixel 644: `local_x = 89`, `src_x = 89 - 89.416 = -0.416` → **rejected** (`< 0`)
- GPU at pixel 644: fragment center at 644.5, `local_x = 89.5`, `src_x = 89.5 - 89.416 = 0.084` → **accepted**

Result: every fractional translation shaved ~1px off the top-left of the content. Confirmed via diagnostic logging showing origin drift per cycle.

**Fix:** Two changes to `sample_bilinear` and `rasterize_to_tiles`/`rasterize_to_mask`:
1. Rasterize functions now pass pixel-center coordinates: `local_x = px + 0.5 - origin_x`
2. `sample_bilinear` converts from pixel-center convention to texel-index space via `sx - 0.5` (the standard GPU `u·N − 0.5` mapping), with bounds check adjusted to `[-0.5, w-0.5]` to allow the half-texel clamp-to-edge border.

This makes the CPU rasterization pixel-identical to the GPU for acceptance/rejection decisions and produces matching bilinear interpolation weights.

### Accumulator precision (compositor.rs) — TESTED, NO EFFECT

Switched accumulator textures from `Rgba8Unorm` to `Rgba16Float` to test whether 8-bit quantization during multi-pass compositing caused accumulated precision loss. No visible difference — the fringe bug is not a precision issue. Change left in place as it's architecturally correct (blending in higher precision prevents banding in deep composite stacks).

### Bounds tightening (transformed_bounds) — MADE IT WORSE (prior context)

Changed corners from `(w, h)` to `(w-1, h-1)` thinking content pixel centers end at `w-1`. This shaved pixels off the right/bottom edges because the iteration range became too small — the last row/column of pixels was no longer reached by the rasterization loop. Reverted.

## What's still broken

The **dark fringe at content edges** persists. The pixel-center fix resolved the spatial drift (content no longer shifts/shrinks per cycle), but semi-transparent edge pixels still render with dark borders after CPU commit.

### What we know

- Paint strokes at partial opacity render correctly — the compositor blend is not the issue.
- Content looks correct during GPU transform preview — the fringe appears only after CPU commit writes to tiles.
- The fringe color comes from bilinear interpolation at content boundaries (confirmed by changing fallback color from transparent-black to transparent-white → fringe turned white).
- Clamp-to-edge in the CPU sampler did not eliminate the fringe — the interpolation itself creates semi-transparent edge pixels that render incorrectly.

### The remaining question

Why do semi-transparent pixels at content edges render as visibly dark borders? The CPU bilinear now interpolates in premultiplied space and un-premultiplies correctly. The compositor blend is correct (PDF/SVG spec). Yet the committed edge pixels still appear darker than they should.

Possible remaining causes:

1. **The `out_a < 0.5` threshold in `sample_bilinear`.** Any interpolated pixel with alpha below 0.5 (128) is silently discarded, replaced with fully transparent. This creates a hard cutoff at edges instead of a smooth falloff, which may interact with the compositor to produce visible banding.

2. **Straight-alpha tile storage + bilinear un-premultiply rounding.** The bilinear interpolates in premultiplied space, then divides by alpha to get straight. For very low alpha values (edge pixels), this division amplifies rounding errors in the RGB channels. The resulting stored pixel may have RGB values that don't accurately represent the original color, and when the compositor re-premultiplies for blending, the error becomes visible.

3. **GPU hardware bilinear on `Rgba8Unorm` source texture.** The transform shader samples the source texture with the hardware bilinear sampler. For `Rgba8Unorm`, the hardware interpolates in straight-alpha space (not premultiplied). At content edges, this produces the classic "dark halo" artifact where interpolating between an opaque colored pixel and a transparent-black pixel darkens the color. The GPU preview might look correct only because the blend formula compensates, while the CPU commit path stores the raw bilinear result and the compensation doesn't happen.

## Possible approaches not yet tried

- **Premultiplied source texture.** Upload the source as premultiplied `Rgba8Unorm` (or `Rgba16Float`) and have the transform shader treat the sample as premultiplied. This matches what the CPU bilinear does and eliminates the straight-alpha hardware interpolation problem at edges.
- **Skip bilinear for identity/integer-translation transforms.** If the matrix is identity or pure integer translation, copy source tiles directly — zero interpolation artifacts.
- **Premultiplied pipeline end-to-end.** Store tile data as premultiplied internally. Eliminates all straight↔premultiplied conversion errors. Larger architectural change.

## Experimental findings (2026-03-13)

Systematic isolation testing to trace the source of the black fringe color:

### Confirmed: bilinear fallback is the source of the fringe color

- **Accumulator clear → RED:** Artifacts stayed black. Proves the black is baked into layer pixel data, not from compositor blending or accumulator background.
- **Layer GPU texture init → transparent white:** No effect.
- **BLANK_TILE → transparent white:** No effect.
- **RgbaData::default → transparent white:** No effect.
- **Bilinear out-of-bounds fallback → transparent white:** Artifacts turned WHITE. This is the source.

### Clamp-to-edge did NOT fix it

Replacing the fixed fallback with clamp-to-edge (snapping out-of-bounds coords to the nearest valid pixel, matching GPU hardware samplers) did not eliminate the fringe. The interpolation itself produces semi-transparent edge pixels regardless of the fallback strategy.

---

## Context 3 analysis (2026-03-13)

### Hypothesis: inconsistent alpha convention across the pipeline

Investigated whether Darkly has a consistent straight-alpha vs premultiplied-alpha convention. Traced every alpha touchpoint:

| Component | Convention |
|---|---|
| CPU tile storage (RgbaData) | Straight |
| Paint/brush (paint.rs composite) | Straight (premul internally, divides by out_a) |
| Clipboard copy/paste | Straight |
| Compositor shader (composite.wgsl) | Straight input, premul internally, straight output |
| Transform preview shader (transform.wgsl) | Straight input |
| Present/blit shaders | Straight passthrough |
| Overlay shader (overlay.wgsl) | **Premultiplied output** (mismatch, but renders to final surface, not accumulator) |

**Conclusion:** The pipeline is consistently straight-alpha. The overlay shader is premultiplied but renders directly to the surface with hardware alpha blending, so it doesn't interact with the accumulator. There is no systemic convention mismatch.

### Hypothesis: compositor blend function double-applies alpha

Analyzed `composite.wgsl` `blend()`:
```
let fg_pre = fg.rgb * fg.a;          // premultiply
...
case 0u: { blended_rgb = fg_pre; }   // Normal: blended = fg.rgb * fg.a
...
let out_rgb = mix(bg_pre, blended_rgb, fg.a) / max(out_a, 0.001);
//                                     ^^^^
// mix applies fg.a AGAIN as lerp factor
// For Normal: mix(bg_pre, fg.rgb * fg.a, fg.a) = bg_pre*(1-fg.a) + fg.rgb*fg.a*fg.a
// Effective: fg.a² — alpha is squared
```

This is the **double-alpha bug** described in the "What was fixed" section. The code in the current codebase still has this bug — either the fix was reverted or was never applied to the actual file.

**Fix applied:** Rewrote blend to use PDF/SVG compositing. Blend modes operate on straight-alpha colors (not premultiplied). Porter-Duff compositing applies alpha exactly once:
```
var Cs: vec3f;
switch mode {
    case 0u: { Cs = fg.rgb; }                           // Normal
    case 1u: { Cs = fg.rgb * bg.rgb; }                  // Multiply
    ...
}
let out_a = fg.a + bg.a * (1.0 - fg.a);
let out_rgb = (fg.a * mix(fg.rgb, Cs, bg.a) + (1.0 - fg.a) * bg.a * bg.rgb)
           / max(out_a, 0.001);
```

### Hypothesis: transform shader has wrong blend formula

Analyzed `transform.wgsl`:
```
let a = fg.a * u.opacity;
let out_rgb = fg.rgb * u.opacity + bg.rgb * (1.0 - a);
```

Two errors:
1. `fg.rgb * u.opacity` — doesn't multiply by `fg.a`. For a semi-transparent source pixel, the full RGB value is used regardless of alpha.
2. `bg.rgb * (1.0 - a)` — doesn't multiply by `bg.a`. Treats background as if it's always fully opaque.
3. No division by `out_a` — output is in an inconsistent space (not straight, not premultiplied).

**Fix applied:** Proper straight-alpha Porter-Duff over:
```
let out_a = a + bg.a * (1.0 - a);
let out_rgb = (fg.rgb * a + bg.rgb * bg.a * (1.0 - a)) / max(out_a, 0.001);
```

### Hypothesis: CPU bilinear produces premultiplied output stored as straight

The channel-wise lerp in `sample_bilinear` treats all 4 RGBA channels identically. Lerping `[200, 0, 0, 255]` (straight) with `[0, 0, 0, 0]` at 50% gives `[100, 0, 0, 128]`. This result is premultiplied (RGB scaled by alpha). But it's written to tile storage which is straight-alpha, and the compositor does `fg.rgb * fg.a` again — double-darkening.

**Fix applied:** Premultiply pixels before interpolation, interpolate in premultiplied space, un-premultiply the result back to straight alpha for storage. Also added clamp-to-edge in `get_pixel` (OOB kernel coordinates snap to nearest valid pixel instead of returning transparent).

Also fixed `rasterize_to_mask` which was un-premultiplying `sample_bilinear` output — no longer needed since `sample_bilinear` now returns straight alpha.

### Result: bug still present

All three fixes (compositor blend, transform shader, CPU bilinear) were applied. The fringe artifact persists. This means either:

1. **The fixes are correct but insufficient.** The remaining issues from "What's still broken" (alpha erosion from rounding, bounds instability, edge asymmetry) are the dominant cause, and the alpha convention fixes only addressed the color component of the fringe, not its existence.

2. **There's another alpha touchpoint not yet identified.** Something else in the pipeline between `sample_bilinear` output and final display is mishandling semi-transparent pixels.

3. **The fringe is inherent to bilinear interpolation at content boundaries.** Even with perfect alpha handling, interpolating an opaque edge pixel with a clamped copy at a sub-pixel offset produces a slightly different alpha/color from rounding. Over repeated cycles this accumulates. The real fix may not be "fix the math" but "don't re-interpolate edges" (identity bypass, stable bounds, or direct tile copy).

---

## Context 4 analysis (2026-03-13)

### Fixed: CPU/GPU pixel-center misalignment (the "shaving" bug)

See "CPU/GPU pixel-center alignment" in the "What was fixed" section above. This was a separate bug from the dark fringe — content was losing ~1px per transform cycle due to the CPU sampling at integer positions instead of pixel centers.

### Tested: Rgba16Float accumulators — no effect on fringe

Switched all accumulator textures from `Rgba8Unorm` to `Rgba16Float`. No visible change to the dark fringe artifact. This rules out 8-bit quantization during compositing as a cause. The fringe is baked into the pixel data at commit time, not introduced during GPU compositing.

### Tested: brush opacity slider — paint strokes render correctly

Added brush opacity slider to test whether semi-transparent pixels render correctly in general. They do — painting at 50% opacity shows correct color with no dark fringe. This confirms the compositor blend formula is correct and the issue is specific to the transform commit path.

### Narrowed: the fringe is created during CPU bilinear, not during GPU display

The dark fringe is visible on committed pixels even when painted content at the same alpha renders correctly. The bilinear interpolation at content edges creates edge pixels whose stored RGBA values, when later composited by the (correct) GPU blend, produce a visible dark border. The issue is in what values the CPU writes, not in how the GPU reads them.

---

## Context 5 analysis (2026-03-13)

### Applied: premultiplied source texture for GPU transform preview

Uploaded source texture data as premultiplied alpha so hardware bilinear interpolation operates in premultiplied space (matching the CPU bilinear path). Updated `transform.wgsl` to treat sampled values as premultiplied in the Porter-Duff blend. **No effect on the fringe** — expected, since the bug manifests after CPU commit, not during GPU preview.

### Verified: CPU bilinear math is correct

Exhaustive trace of the premultiplied interpolation pipeline confirms the math is sound. For any input pixel, the premultiply → interpolate → un-premultiply roundtrip preserves color channels exactly (within ±1 from u8 rounding, which is non-directional). The compositor blend formula, tile upload path, and sampler alignment are all correct. There is no systematic darkening in the transform commit path itself.

### Verified: repeated resampling causes expected edge softening, not darkening

Repeated bilinear resampling with sub-pixel offsets progressively erodes edge alpha (edges get softer/more transparent). This is inherent to resampling and not a bug — but it makes any pre-existing color error at edges more visible over cycles.

### New lead: the dark fringe originates in copy/paste, not transform

Key observation: **pure transform (no copy/paste) does not introduce the dark fringe.** The fringe is already present in the content when it arrives via copy or paste. Repeated transforms then exacerbate the existing edge artifacts through resampling softening.

This means the source of the dark color is in either:
1. **`ImageClip::from_layer`** — the copy/extraction path (`clipboard.rs`). Possibly the selection-coverage multiplication (`pixel[3] * coverage`, `pixel[c] * ratio`) introduces rounding errors that darken edge pixels.
2. **The paste commit path** — `rasterize_to_tiles` in Paste mode does Normal blend onto existing content. If the blend formula or the bilinear output has a subtle error specific to paste, it could bake dark edges into the tiles.
3. **The clipboard encode/decode roundtrip** — if clipboard data goes through a PNG or other lossy encoding, the alpha handling there could introduce dark halos (classic straight-alpha PNG problem).

Next step: isolate whether the fringe appears after copy (inspect extracted tiles before any transform), or only after paste commit (inspect tiles after paste). This narrows it to one of the three paths above.

### FOUND: selection-coverage copy produces premultiplied output stored as straight

**Root cause:** `ImageClip::from_layer` in `clipboard.rs` (lines 119-133) applied selection coverage by scaling BOTH alpha AND RGB channels:

```rust
let a = (pixel[3] as f32 * coverage).round() as u8;
let ratio = a as f32 / pixel[3] as f32;  // ≈ coverage
out[0] = (pixel[0] as f32 * ratio).round() as u8;  // WRONG: scales color
out[3] = a;
```

For pixel `[255, 0, 0, 255]` with 50% selection coverage, this produced `[128, 0, 0, 128]` — a **premultiplied** result stored as **straight alpha**. The compositor reads this as straight (color=128, alpha=0.5) and renders dark red instead of bright semi-transparent red.

**The math:** In straight alpha, scaling by coverage should only affect alpha:
- `premul_R = R · α · coverage`
- `new_α = α · coverage`
- `new_straight_R = premul_R / new_α = R`  (unchanged)

**Fix:** Remove the RGB scaling — only scale alpha, copy RGB as-is:
```rust
out[0] = pixel[0];  // color unchanged
out[3] = a;         // only alpha scaled by coverage
```

This explains why:
- The fringe appeared on copy/paste but not pure transform
- The fringe color matched the "darkened" version of the content color
- Repeated transforms exacerbated it (resampling softened edges that already had wrong colors)
- The bilinear fallback color test (transparent-black vs transparent-white) changed the fringe color — the darkened edge pixels were being interpolated with the fallback
