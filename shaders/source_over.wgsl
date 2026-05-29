// Porter-Duff source-over: premultiplied foreground onto straight-alpha background.
// Returns straight-alpha result.
//
// This is the SINGLE SOURCE OF TRUTH for straight-alpha compositing in Darkly.
// Include it in any shader that composites onto a straight-alpha target. Never
// inline this formula — use this function.
//
// Usage: prepend this file to your shader source via concat! in Rust:
//   concat!(include_str!("source_over.wgsl"), "\n", include_str!("my_shader.wgsl"))
//
// See docs/lessons-learned/compositing-lessons-learned.md for the full rationale.

fn source_over(fg_pre: vec3f, fg_a: f32, bg: vec4f) -> vec4f {
    let out_a = fg_a + bg.a * (1.0 - fg_a);
    let out_rgb = select(
        vec3f(0.0),
        (fg_pre + (1.0 - fg_a) * bg.a * bg.rgb) / out_a,
        out_a > 0.001,
    );
    return vec4f(out_rgb, out_a);
}

/// Porter-Duff destination-out (erase): remove foreground coverage from background.
/// fg_a is the eraser's alpha (how much to remove). bg is straight-alpha.
fn destination_out(fg_a: f32, bg: vec4f) -> vec4f {
    let out_a = bg.a * (1.0 - fg_a);
    return vec4f(bg.rgb, out_a);
}
