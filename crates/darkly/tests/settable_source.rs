//! Settable-source ports + the `brush_settings.size` signal — brush-level guards.
//!
//! Covers the migration's user-visible invariants across every builtin: the
//! base-size knob is exposed on the `brush_settings` node (never a terminal),
//! each brush's base size survived the move (the silent-shrink guard), and
//! every builtin still compiles.

use darkly::brush::builtin_brushes;
use darkly::brush::nodes::brush_settings;

/// Expected base size per builtin after the migration. The four non-default
/// brushes are the ones whose base didn't come from the registration default
/// (0.1) — liquify/blur via terminal registration defaults, charcoal/calligraphy
/// via a terminal `inputs.size` that had to be relocated. A wrong value here
/// means a brush silently changed size.
fn expected_base(name: &str) -> f32 {
    match name {
        "Liquify" => 0.3,
        "Blur" => 0.2,
        "Charcoal" => 0.12,
        "Calligraphy" => 0.05,
        _ => 0.1,
    }
}

#[test]
fn builtins_own_base_size_on_brush_settings_and_preserve_its_value() {
    let brushes = builtin_brushes::all();
    assert_eq!(brushes.len(), 12, "expected all 12 builtins");

    for brush in brushes {
        let name = brush.metadata.name.clone();
        let graph = &brush.metadata.graph;
        let settings_id = brush_settings::node_id(graph)
            .unwrap_or_else(|| panic!("{name}: builtin must have a brush_settings node"));

        // (a) The size knob is exposed on brush_settings...
        assert!(
            graph.is_port_exposed(settings_id, "size"),
            "{name}: brush_settings.size must be exposed in the brush bar",
        );

        // ...and NOT on the terminal (the terminal's `size` is unexposed
        // per-touch modulation now).
        let term_id = darkly::brush::find_terminal(graph).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(
            !graph.is_port_exposed(term_id, "size"),
            "{name}: the terminal must not expose a size knob",
        );

        // (b) The base value survived the relocation (the silent-shrink guard).
        let base = brush_settings::base_size(graph);
        let want = expected_base(&name);
        assert!(
            (base - want).abs() < 1e-6,
            "{name}: base size {base}, expected {want}",
        );

        // (c) The brush still compiles end-to-end.
        darkly::brush::compile_graph(graph)
            .unwrap_or_else(|e| panic!("{name}: brush must compile: {e:?}"));
    }
}
