//! Gate on the WGSL every built-in brush assembles.
//!
//! Two properties, both cheap and both easy to break by accident:
//!
//! 1. **Nothing changes unless it is meant to.** Every brush's `stroke_wgsl`
//!    and `cursor_preview_wgsl` hash to a committed value. A change to the
//!    shader skeleton that was supposed to affect one terminal but reached
//!    every brush shows up here as a wall of mismatches. On failure the test
//!    prints the fresh source so the diff is recoverable.
//!
//! 2. **Both variants actually compile.** `builtin_brushes_compile` only
//!    checks that assembly *succeeds*; it never validates the text. A terminal
//!    that samples a stroke-only `@group(3)` binding without overriding
//!    `compile_cursor_preview_body` emits an undeclared identifier into the
//!    preview variant, which assembles fine and then fails at first hover in
//!    the browser (see `docs/brush-preview-and-overlays.md`). Running both
//!    variants through naga catches that class at build time, for every brush,
//!    without anyone having to remember the rule.
//!
//! The hash is FNV-1a/64 rather than a cryptographic digest: this detects
//! accidental change, not adversarial collision, and it avoids a dependency
//! for six lines of arithmetic.

use darkly::brush::{builtin_brushes, compile_graph};

/// FNV-1a, 64-bit. Stable across Rust versions and platforms, unlike
/// `DefaultHasher`, which is why the committed table can be trusted.
fn fnv1a64(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in s.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// `(brush name, stroke_wgsl hash, cursor_preview_wgsl hash)`.
///
/// Regenerate with `DARKLY_WGSL_GOLDEN=print cargo test --test wgsl_golden`
/// and paste — but read the diff first. A change here means every brush's
/// shader moved, which is almost never what a terminal-local edit intended.
const EXPECTED: &[(&str, u64, u64)] = &[
    ("Airbrush", 0x8c629cae2d2a4f35, 0x23d6dabfff234e2c),
    ("Blur", 0x7f6103ff166a54cb, 0x6ab373694a807535),
    ("Calligraphy", 0x7e0a4389fe90a711, 0x18bf5407b984d7cc),
    ("Charcoal", 0x14c3e4508e2b82a7, 0x65298e2c9a28d6a8),
    ("Clone", 0x03601fe012c3caaa, 0xb01d9a5502dcd4e4),
    ("Deposit Probe", 0x177705ccd12260b4, 0xa359bd7fddde1aa4),
    ("Hair", 0xf957a848acd75371, 0xbac106d312caf662),
    ("Ink Pen", 0x6edd3639f30fa577, 0xa347f3eac8ea80a8),
    ("Liquify", 0x3da07a624c82f469, 0x6981bb1ee50d2579),
    ("Rough Ink", 0xa524bd2b7c2a7678, 0xb2830fff8e0585d5),
    ("Rough Watercolor", 0x2a84e6be61b10225, 0x5e11a9242f36df61),
    ("Round", 0xa677a77c540fa88b, 0xc9e0f4f204a40950),
    ("Smooth Watercolor", 0x7d00109f20748919, 0x4e4d58c8567377a1),
    ("Smudge", 0x3246d27e634f5039, 0xc6c3b4ad96783609),
    ("Sponge", 0x020787e64274e29e, 0x4fae403630521a59),
];

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
fn builtin_brush_wgsl_is_unchanged() {
    let actual = compiled_wgsl();
    assert!(!actual.is_empty(), "no built-in brushes found");

    if std::env::var("DARKLY_WGSL_GOLDEN").as_deref() == Ok("print") {
        println!("const EXPECTED: &[(&str, u64, u64)] = &[");
        for (name, stroke, preview) in &actual {
            println!(
                "    (\"{name}\", 0x{:016x}, 0x{:016x}),",
                fnv1a64(stroke),
                fnv1a64(preview),
            );
        }
        println!("];");
        panic!("DARKLY_WGSL_GOLDEN=print — table above, not a real failure");
    }

    let mut failures = Vec::new();
    for (name, stroke, preview) in &actual {
        let Some((_, want_stroke, want_preview)) = EXPECTED.iter().find(|(n, _, _)| n == name)
        else {
            failures.push(format!(
                "{name}: no committed hashes — new brush? add \
                 (\"{name}\", 0x{:016x}, 0x{:016x})",
                fnv1a64(stroke),
                fnv1a64(preview),
            ));
            continue;
        };
        if fnv1a64(stroke) != *want_stroke {
            failures.push(format!(
                "{name}: stroke_wgsl changed (want 0x{want_stroke:016x}, got 0x{:016x})\n\
                 --- fresh stroke_wgsl ---\n{stroke}\n--- end ---",
                fnv1a64(stroke),
            ));
        }
        if fnv1a64(preview) != *want_preview {
            failures.push(format!(
                "{name}: cursor_preview_wgsl changed (want 0x{want_preview:016x}, got 0x{:016x})\n\
                 --- fresh cursor_preview_wgsl ---\n{preview}\n--- end ---",
                fnv1a64(preview),
            ));
        }
    }
    for (name, _, _) in EXPECTED {
        if !actual.iter().any(|(n, _, _)| n == name) {
            failures.push(format!("{name}: committed hashes but the brush is gone"));
        }
    }

    assert!(
        failures.is_empty(),
        "built-in brush WGSL changed:\n\n{}",
        failures.join("\n\n"),
    );
}

/// Both shader variants of every built-in brush must survive naga.
///
/// This is the gate that catches a terminal sampling a stroke-only binding
/// from its preview body — the failure mode `docs/brush-preview-and-overlays.md`
/// warns about, which nothing else in the suite sees.
#[test]
fn builtin_brush_wgsl_validates() {
    let mut failures = Vec::new();
    for (name, stroke, preview) in compiled_wgsl() {
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
