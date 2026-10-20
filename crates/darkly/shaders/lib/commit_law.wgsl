// The stroke commit's law over two foregrounds, each with a stroke
// opacity, and a straight-alpha background. Called by the commit
// (`brush/composite.wgsl`, one fragment per layer pixel) and by the
// appearance snapshot (`brush/appearance_snapshot.wgsl`, one thread per
// pixel of a dab's read region), so the stroke a sampler reads mid-stroke
// is the stroke the commit will show.
//
// Two slots, where an opacity of zero marks an absent slot, under one of
// two laws (`law`):
//   0, deposit: the first slot is committed through the per-pigment
//      deposit ceiling (wash) and the second composited on top with plain
//      Porter-Duff source-over (build).
//   1, move: the first slot's alpha is the coverage a finger displaced and
//      the second is the pigment it brought, straight alpha (see
//      `accumulate_move`); the pigment is laid over `1 - coverage` of the
//      background, so a smear can thin an edge as well as thicken it.
//
// The deposit slots are premultiplied and their opacity scales the whole
// texel; the move slots' opacity scales coverage and alpha and never the
// colour. The law applies the opacities itself for that reason.
//
// Requires `source_over.wgsl` and `lib/deposit_ceiling.wgsl`.

// The deposit ceiling: lay `fg` (premultiplied) onto `bg` (straight alpha),
// depositing only what the pixel can still take; the room is `ceiling_t`'s.
// A saturated pixel (`t == 0`) is left exactly as it is, where a
// `source_over` at zero alpha would re-derive it through a division.
fn deposit_through_ceiling(fg: vec4f, bg: vec4f) -> vec4f {
    let t = ceiling_t(fg, vec4f(bg.rgb * bg.a, bg.a));
    if t <= 0.0 {
        return bg;
    }
    return source_over(fg.rgb / fg.a * t, t, bg);
}

fn commit_law(wash_raw: vec4f, build_raw: vec4f, bg: vec4f, blend_mode: u32, law: u32,
              wash_opacity: f32, build_opacity: f32) -> vec4f {
    if law == 1u {
        let coverage = wash_raw.a * wash_opacity;
        let a = build_raw.a * build_opacity;
        if blend_mode == 1u {
            return destination_out(coverage, bg);
        }
        return source_over_covering(build_raw.rgb * a, a, coverage, bg);
    }
    let wash = wash_raw * wash_opacity;
    let build = build_raw * build_opacity;
    if blend_mode == 1u {
        // Erase: each slot removes its own coverage, composing to a removal
        // of `1 - (1 - wash.a) * (1 - build.a)`. Removal never goes through
        // the ceiling, so an eraser can always reach zero. No gate needed:
        // `destination_out(0, x)` is exactly `x`.
        return destination_out(build.a, destination_out(wash.a, bg));
    }

    // The wash slot first: the ceiling reads the ground to find room, and a
    // build slot laid under it would let a stroke's own build-up shrink its
    // own wash. An absent slot is skipped rather than composited at zero
    // alpha, because `source_over` at zero alpha is not an exact identity
    // (it divides `bg.a * bg.rgb` by `bg.a`, and zeroes rgb under 0.001).
    var out = bg;
    if wash_opacity > 0.0 {
        out = deposit_through_ceiling(wash, out);
    }
    if build_opacity > 0.0 {
        out = source_over(build.rgb, build.a, out);
    }
    return out;
}
