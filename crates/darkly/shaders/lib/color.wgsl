// Color-math atoms shared across every shader path that needs them.
//
// These are format-agnostic helpers operating on the `vec4f` a `textureLoad`
// yields. Any shader (a destructive adjustment pass, a veil, a void, or a
// future adjustment-layer composite) `include_str!`-prepends this file (the
// same concatenation trick `shaders/voids/noise.wgsl` uses for `lib/fbm.wgsl`)
// so the math lives in exactly one place.

// Invert RGB, preserve alpha. For an R8 mask a `textureLoad` yields
// `vec4(r, 0, 0, 1)`; this returns `(1 - r, …)` and the R8 target stores only
// `.r = 1 - r`, so the one atom serves RGBA8 and R8 alike.
fn invert_color(c: vec4f) -> vec4f {
    return vec4f(1.0 - c.rgb, c.a);
}
