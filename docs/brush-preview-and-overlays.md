# Brush Preview & On-Canvas Overlays

The pointer-to-pixel diagram in [`CONTRIBUTING.md`](../CONTRIBUTING.md) is the *paint* path.
Most on-canvas feedback comes from two derived paths, and a lot of tool/UX bugs
live here: invisible unless you know the model below.

## Cursor preview (the hover dab)

On hover the frontend calls `refreshBrushCursorPreview`
([`frontend/src/tools/brush.svelte.ts`](../frontend/src/tools/brush.svelte.ts)).
The engine renders the brush's *preview* shader (see below) into a mask texture
via `render_compiled_cursor_preview`
([`crates/darkly/src/brush/wgsl/mod.rs`](../crates/darkly/src/brush/wgsl/mod.rs))
and returns a `BrushCursorPreviewInfo`, which the frontend draws as a
`KIND_MASKED_STAMP` overlay at the cursor. No preview info ⇒ no dab shown.

## On-canvas overlays

Tools build primitives with `OverlayBuilder`
([`frontend/src/canvas/gpu_overlay.ts`](../frontend/src/canvas/gpu_overlay.ts))
and call `setOverlay`, which **replaces the entire overlay set**; it is
single-slot, not additive. Marching ants, gradient/transform handles, and the
brush hover dab all share it; two features that each want an overlay must compose
into one push.

`app.toolCursor` is a single shared field (set to `'none'` when a GPU stamp
stands in for the native cursor). Modules that write it without coordinating will
fight frame-to-frame.

## Two shader variants

A brush graph compiles (`compile_brush_to_wgsl` → `assemble_shader`,
[`crates/darkly/src/brush/wgsl/mod.rs`](../crates/darkly/src/brush/wgsl/mod.rs))
into **two** WGSL fragment shaders on one `CompiledBrush`: `stroke_wgsl` and
`cursor_preview_wgsl`.

- **Non-terminal node bodies are spliced verbatim into both variants.** Only the
  *terminal* differs: it emits the stroke body from `compile_wgsl` and the preview
  body from `compile_cursor_preview_body`
  ([`crates/darkly/src/brush/eval.rs`](../crates/darkly/src/brush/eval.rs)). The
  default returns the same body; `watercolor`/`smudge`/`liquify` override it to
  emit a neutral fill that samples no stroke-only bindings.
- **The preview variant has no live stroke.** It drops `@group(2)` selection
  (hard-codes `sel = 1.0`) and swaps every stroke-only binding (scratch,
  selection, the frozen clone source snapshot) for a registry fallback (e.g. the
  1×1 white `_fallback` tile); stroke-seeded uniforms read their unseeded defaults.

**Gotcha:** a non-terminal node *cannot* render differently at hover; its single
body runs in both modes. A non-terminal that samples a stroke-only resource will,
at hover, sample the fallback with default uniforms and produce a meaningless (or
transparent) dab. Preview-specific behavior lives on the *terminal*; if upstream
data must change for the preview, that is a design constraint, not a small tweak.
