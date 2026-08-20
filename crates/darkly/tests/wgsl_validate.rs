//! Every built-in brush's assembled WGSL must survive naga — both shader
//! variants, not just the one a stroke exercises.
//!
//! `builtin_brushes_compile` only checks that *assembly* succeeds; it never
//! looks at the text. So a terminal that samples a stroke-only `@group(3)`
//! binding without overriding `compile_cursor_preview_body` emits an
//! undeclared identifier into `cursor_preview_wgsl`, assembles fine, and then
//! fails when the preview pipeline is built — at first hover, in the browser,
//! far from the edit that caused it. `docs/brush-preview-and-overlays.md`
//! documents the rule; this is what enforces it.
//!
//! Deliberately *not* here: a table of committed hashes per brush. Shader
//! text is assembled from a shared skeleton, so an edit to `_prelude.wgsl`
//! moves every brush at once and the only available response is "regenerate
//! and paste" — a change detector with no signal in it. What matters is that
//! the emitted WGSL is *valid*, which naga answers directly.

use darkly::brush::{builtin_brushes, compile_graph};

/// `(brush name, stroke_wgsl, cursor_preview_wgsl)` for every builtin.
fn compiled_wgsl() -> Vec<(String, String, String)> {
    builtin_brushes::all()
        .into_iter()
        .map(|brush| {
            let runner = compile_graph(&brush.metadata.graph).unwrap_or_else(|e| {
                panic!("brush '{}' failed to compile: {e}", brush.metadata.name)
            });
            let compiled = runner
                .compiled_brush()
                .unwrap_or_else(|| panic!("brush '{}' produced no terminal", brush.metadata.name));
            (
                brush.metadata.name.clone(),
                compiled.stroke_wgsl.clone(),
                compiled.cursor_preview_wgsl.clone(),
            )
        })
        .collect()
}

#[test]
fn builtin_brush_wgsl_validates() {
    let brushes = compiled_wgsl();
    assert!(!brushes.is_empty(), "no built-in brushes found");

    let mut failures = Vec::new();
    for (name, stroke, preview) in brushes {
        for (variant, source) in [("stroke", &stroke), ("cursor_preview", &preview)] {
            match naga::front::wgsl::parse_str(source) {
                Err(e) => failures.push(format!(
                    "{name} / {variant}: {}\n--- source ---\n{source}\n--- end ---",
                    e.emit_to_string(source),
                )),
                Ok(module) => {
                    let mut validator = naga::valid::Validator::new(
                        naga::valid::ValidationFlags::all(),
                        naga::valid::Capabilities::all(),
                    );
                    if let Err(e) = validator.validate(&module) {
                        failures.push(format!(
                            "{name} / {variant}: {e:?}\n--- source ---\n{source}\n--- end ---"
                        ));
                    }
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "built-in brush WGSL failed validation:\n\n{}",
        failures.join("\n\n"),
    );
}
