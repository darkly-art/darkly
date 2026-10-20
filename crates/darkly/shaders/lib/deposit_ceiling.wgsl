// The deposit ceiling's room arithmetic: how much coverage a pass may
// still lay on a pixel. One law, two compositing conventions: the stroke
// commit (`brush/composite.wgsl`) applies it to a straight-alpha layer, the
// paint terminal's per-dab wash law (`brush/paint_accumulate.wgsl`) to the
// premultiplied stroke ground. Both include this file so the room is
// computed once, here.
//
// A pass carrying pigment `C` at coverage `s` lands, starting from blank, a
// fixed fraction of the way to `C`. Everything past that is refused. So
// instead of asking what this pass would add, ask how much room is left
// between where the pixel already sits and where this pass saturates, and
// deposit exactly that. A pixel already at the saturation level takes
// nothing; one that has never been touched takes the full `s`; a heavier
// pass moves the saturation level and reopens room. No history is read: the
// room is a property of the pixel's current colour, so a transparent layer
// and an opaque one holding the same visible mark answer alike.
//
// `O` is the origin of the deposit scale, the gamut corner opposite `C`,
// which is what the distances are measured against. Deriving it from the
// pigment rather than assuming white is what lets a white pencil on black
// ground behave exactly like a black one on white.
//
// See docs/brush/architecture.md, "How the commit decides".

/// Max-norm distance. See `ceiling_t` for why the norm choice matters.
fn chebyshev(a: vec3f, b: vec3f) -> f32 {
    let v = abs(a - b);
    return max(v.x, max(v.y, v.z));
}

// The effective coverage the ceiling grants `fg` (premultiplied) over
// `bg` (premultiplied): 0 when there is nothing to deposit or no room.
fn ceiling_t(fg: vec4f, bg: vec4f) -> f32 {
    if fg.a <= 0.0 {
        return 0.0;
    }
    let pigment = fg.rgb / fg.a;
    let origin = select(vec3f(0.0), vec3f(1.0), pigment < vec3f(0.5));

    // The max-norm is load-bearing, not a cheap stand-in for a Euclidean one.
    // Under it, `d <= reach` holds for every colour in the cube, so a pass can
    // only ever be reduced, never amplified, and `t` collapses to exactly `s`
    // on any untouched ground. Under a Euclidean norm that is false: white is
    // not red's antipode, so a red pencil on white paper would saturate at a
    // weaker mark than graphite does at the same pressure.
    //
    // This is also where a move to OKLab would land. Distance there is
    // Euclidean and perceptually uniform, which is the property this actually
    // wants, but it needs a different reference than the cube corner to keep
    // the `d <= reach` guarantee. Worth revisiting with the colour-system
    // rewrite, not before.
    let reach = chebyshev(origin, pigment);
    let ground = bg.rgb + origin * (1.0 - bg.a);
    let d = chebyshev(ground, pigment);
    if d <= 0.0 {
        return 0.0;
    }
    // `t <= fg.a` always holds (see the max-norm note), so the ceiling can
    // only ever reduce what the pass carries, never amplify it.
    return max(0.0, 1.0 - (1.0 - fg.a) * reach / d);
}
