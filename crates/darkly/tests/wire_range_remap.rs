//! End-to-end tests for wire-boundary range remap (`PortDef::natural_range`).
//!
//! The bug that motivated this: `random.value → circle.seed` produced the
//! same seed every dab because random's output (`[-1, 1]`) collapsed to 0
//! when seed cast it through `as u32` (range `[0, 1024]`). The fix is that
//! when both ends of a wire declare a `natural_range`, the runner remaps
//! the value at slot-read time.
//!
//! These tests exercise the runner-level remap path with hand-built graphs
//! that customize port natural ranges per-instance (the registration ports
//! are cloned into each `NodeInstance`, so we can declare a non-default
//! `natural_range` on a single test instance without touching any node's
//! registration).

use darkly::brush::eval::BrushGraphRunner;
use darkly::brush::paint_info::PaintInformation;
use darkly::brush::registry;
use darkly::brush::wire::ScalarValue;
use darkly::gpu::params::ParamValue;
use darkly::nodegraph::{Graph, PortDef, PortRef};

/// Per-instance params for the `random` node: mode=0 (per-dab, the default).
fn random_params() -> Vec<ParamValue> {
    vec![ParamValue::Int(0)]
}

fn read_scalar(runner: &BrushGraphRunner, type_id: &str, port: &str) -> f32 {
    let slot = runner
        .find_output_slot(type_id, port)
        .unwrap_or_else(|| panic!("no slot for {type_id}.{port}"));
    match runner.read_slot(slot).expect("slot has value") {
        ScalarValue::Scalar(v) => v,
        other => panic!("expected Scalar, got {other:?}"),
    }
}

fn run_one_dab(runner: &mut BrushGraphRunner, pressure: f32, dab_index: u32) {
    let info = PaintInformation {
        pressure,
        ..Default::default()
    };
    runner.seed_sensors(&info, [0.0, 0.0, 0.0, 1.0], 42, dab_index);
    runner.clear_slots();
    runner.seed_sensors(&info, [0.0, 0.0, 0.0, 1.0], 42, dab_index);
    runner.execute_cpu();
}

/// Regression for the original bug: `random` output is now in `[0, 1)` (the
/// natural PRNG range), no longer the old `[-1, 1]` remap, and produces a
/// fresh value each dab. The previous behavior — `random` outputting in
/// `[-1, 1]` and any downstream `as u32` cast collapsing it to 0 — is what
/// caused `random → circle.seed` to repeat.
#[test]
fn random_outputs_unit_range_and_varies_per_dab() {
    let registry = registry();
    let mut graph = Graph::new();
    let random_reg = registry.get("random").unwrap();
    graph.add_node("random", random_reg.ports.clone(), random_params());

    let mut runner =
        BrushGraphRunner::new(&graph, registry.as_map(), registry.evaluators()).unwrap();

    let mut samples = Vec::with_capacity(16);
    for dab in 0..16 {
        run_one_dab(&mut runner, 0.5, dab);
        let v = read_scalar(&runner, "random", "value");
        assert!(
            (0.0..1.0).contains(&v),
            "random output {v} outside [0, 1) on dab {dab}",
        );
        samples.push(v);
    }

    // Strong evidence the value actually varies per dab — at least 10 of 16
    // distinct values rules out "always returns the same thing."
    let mut sorted = samples.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    sorted.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    assert!(
        sorted.len() >= 10,
        "expected ≥10 distinct random values over 16 dabs, got {} (samples: {:?})",
        sorted.len(),
        samples,
    );
}

/// `random → seed-style port (natural_range [0, 1024])` produces values
/// spread across the destination range, varying per dab.
///
/// We can't observe `circle.seed` directly without a GPU, but the runner's
/// remap step happens at `gather_inputs` — same code path for CPU and GPU
/// nodes. So we exercise it through a CPU node (`mix`) with a per-instance
/// override on its `factor` port: `natural_range = Some((0, 1024))`. The
/// mix evaluator computes `a + (b - a) * factor` with the default `a=0`
/// and `b=1`, so `mix.result == factor_after_remap`.
#[test]
fn random_to_wide_range_input_remaps() {
    let registry = registry();
    let mut graph = Graph::new();

    let random_reg = registry.get("random").unwrap();
    let random = graph.add_node("random", random_reg.ports.clone(), random_params());

    // Clone mix's ports, then widen the `factor` input's natural_range to
    // simulate wiring random into something like `circle.seed`.
    let mix_reg = registry.get("mix").unwrap();
    let mut mix_ports = mix_reg.ports.clone();
    for p in mix_ports.iter_mut() {
        if p.name == "factor" {
            *p = std::mem::replace(p, PortDef::input("placeholder", p.wire_type))
                .with_natural_range(0.0, 1024.0);
        }
    }
    let mix = graph.add_node("mix", mix_ports, vec![]);

    graph
        .connect(
            PortRef {
                node: random,
                port: "value".into(),
            },
            PortRef {
                node: mix,
                port: "factor".into(),
            },
        )
        .unwrap();

    let mut runner =
        BrushGraphRunner::new(&graph, registry.as_map(), registry.evaluators()).unwrap();

    let mut samples = Vec::new();
    for dab in 0..16 {
        run_one_dab(&mut runner, 0.5, dab);
        let v = read_scalar(&runner, "mix", "result");
        assert!(
            (0.0..=1024.0).contains(&v),
            "mix.result {v} outside [0, 1024] on dab {dab}",
        );
        samples.push(v);
    }
    // Values should span a meaningful chunk of [0, 1024]: max - min > 500.
    let min = samples.iter().cloned().fold(f32::INFINITY, f32::min);
    let max = samples.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    assert!(
        max - min > 500.0,
        "expected wide spread across [0, 1024], got [{min}, {max}] (samples {samples:?})",
    );
}

/// `pen.pressure → frequency-style port (natural_range [1, 16])` — the
/// other half of the bug class. Today this works; before the fix, pen
/// pressure ∈ [0, 1] cast to a frequency in [1, 16] rounded to 1 forever.
#[test]
fn pen_pressure_to_frequency_range_remaps() {
    let registry = registry();
    let mut graph = Graph::new();

    let pen_reg = registry.get("pen_input").unwrap();
    let pen = graph.add_node("pen_input", pen_reg.ports.clone(), vec![]);

    let mix_reg = registry.get("mix").unwrap();
    let mut mix_ports = mix_reg.ports.clone();
    for p in mix_ports.iter_mut() {
        if p.name == "factor" {
            *p = std::mem::replace(p, PortDef::input("placeholder", p.wire_type))
                .with_natural_range(1.0, 16.0);
        }
    }
    let mix = graph.add_node("mix", mix_ports, vec![]);

    graph
        .connect(
            PortRef {
                node: pen,
                port: "pressure".into(),
            },
            PortRef {
                node: mix,
                port: "factor".into(),
            },
        )
        .unwrap();

    let mut runner =
        BrushGraphRunner::new(&graph, registry.as_map(), registry.evaluators()).unwrap();

    // pressure 0.0 → factor 1.0
    run_one_dab(&mut runner, 0.0, 0);
    let v = read_scalar(&runner, "mix", "result");
    assert!(
        (v - 1.0).abs() < 1e-4,
        "pressure 0 → factor expected 1.0, got {v}"
    );

    // pressure 0.5 → factor 8.5
    run_one_dab(&mut runner, 0.5, 1);
    let v = read_scalar(&runner, "mix", "result");
    assert!(
        (v - 8.5).abs() < 1e-4,
        "pressure 0.5 → factor expected 8.5, got {v}"
    );

    // pressure 1.0 → factor 16.0
    run_one_dab(&mut runner, 1.0, 2);
    let v = read_scalar(&runner, "mix", "result");
    assert!(
        (v - 16.0).abs() < 1e-4,
        "pressure 1.0 → factor expected 16.0, got {v}"
    );
}

/// Source-side opt-out: `multiply.result` has no natural_range, so wiring
/// it into a port that does still passes the raw value through (math nodes
/// are intentionally unbounded — their range depends on inputs). This is
/// the invariant that lets us reshape random's behavior without disturbing
/// any existing math-node-based brush.
#[test]
fn math_node_output_passes_through_to_ranged_input() {
    let registry = registry();
    let mut graph = Graph::new();

    // multiply with explicit defaults: a=0.5, b=0.5 → result=0.25.
    let multiply_reg = registry.get("multiply").unwrap();
    let mut multiply_ports = multiply_reg.ports.clone();
    for p in multiply_ports.iter_mut() {
        if p.name == "a" || p.name == "b" {
            p.default = 0.5;
        }
    }
    let multiply = graph.add_node("multiply", multiply_ports, vec![]);

    // mix.factor with natural_range Some((0, 1024)) — if multiply were
    // mistakenly opted in to source-side remap, we'd see 0.25 stretched
    // to 256. The passthrough invariant says we should see 0.25 raw.
    let mix_reg = registry.get("mix").unwrap();
    let mut mix_ports = mix_reg.ports.clone();
    for p in mix_ports.iter_mut() {
        if p.name == "factor" {
            *p = std::mem::replace(p, PortDef::input("placeholder", p.wire_type))
                .with_natural_range(0.0, 1024.0);
        }
    }
    let mix = graph.add_node("mix", mix_ports, vec![]);

    graph
        .connect(
            PortRef {
                node: multiply,
                port: "result".into(),
            },
            PortRef {
                node: mix,
                port: "factor".into(),
            },
        )
        .unwrap();

    let mut runner =
        BrushGraphRunner::new(&graph, registry.as_map(), registry.evaluators()).unwrap();

    run_one_dab(&mut runner, 0.0, 0);
    let v = read_scalar(&runner, "mix", "result");
    // mix.result = a + (b - a) * factor with mix's own a=0, b=1, so it
    // mirrors the factor value reaching the evaluator.
    assert!((v - 0.25).abs() < 1e-4, "expected raw 0.25, got {v}");
}

/// Dest-side opt-out: wiring a normalized source (random, `[0, 1)`) into a
/// port without a natural_range passes the value through raw. This is the
/// invariant that lets `stamp.size` keep its over-drag behavior — even if
/// you wire pen.pressure into it, the value flows as raw `[0, 1)` instead
/// of being mapped onto the slider's `[0, 4]` hint.
#[test]
fn ranged_source_to_unranged_input_passes_through() {
    let registry = registry();
    let mut graph = Graph::new();

    let random_reg = registry.get("random").unwrap();
    let random = graph.add_node("random", random_reg.ports.clone(), random_params());

    // mix.factor — explicitly STRIP the natural_range we added so this
    // node-instance behaves as if the consumer hadn't opted in.
    let mix_reg = registry.get("mix").unwrap();
    let mut mix_ports = mix_reg.ports.clone();
    for p in mix_ports.iter_mut() {
        if p.name == "factor" {
            p.natural_range = None;
        }
    }
    let mix = graph.add_node("mix", mix_ports, vec![]);

    graph
        .connect(
            PortRef {
                node: random,
                port: "value".into(),
            },
            PortRef {
                node: mix,
                port: "factor".into(),
            },
        )
        .unwrap();

    let mut runner =
        BrushGraphRunner::new(&graph, registry.as_map(), registry.evaluators()).unwrap();

    for dab in 0..8 {
        run_one_dab(&mut runner, 0.5, dab);
        let v = read_scalar(&runner, "mix", "result");
        // Raw random value, unscaled — should stay in [0, 1).
        assert!(
            (0.0..1.0).contains(&v),
            "unranged dest should see raw random in [0, 1), got {v} on dab {dab}",
        );
    }
}

/// Identity-range remap (source range == dest range) is a no-op. Wiring
/// `pen.pressure → curve.input` (both `Some((0, 1))`) preserves the value
/// exactly, so the identity curve's output equals pressure.
#[test]
fn identity_range_is_a_noop() {
    let registry = registry();
    let mut graph = Graph::new();

    let pen_reg = registry.get("pen_input").unwrap();
    let pen = graph.add_node("pen_input", pen_reg.ports.clone(), vec![]);

    let curve_reg = registry.get("curve").unwrap();
    let curve = graph.add_node(
        "curve",
        curve_reg.ports.clone(),
        vec![darkly::gpu::params::ParamValue::Curve(vec![
            [0.0, 0.0],
            [1.0, 1.0],
        ])],
    );

    graph
        .connect(
            PortRef {
                node: pen,
                port: "pressure".into(),
            },
            PortRef {
                node: curve,
                port: "input".into(),
            },
        )
        .unwrap();

    let mut runner =
        BrushGraphRunner::new(&graph, registry.as_map(), registry.evaluators()).unwrap();

    for pressure in [0.0_f32, 0.25, 0.5, 0.75, 1.0] {
        run_one_dab(&mut runner, pressure, 0);
        let v = read_scalar(&runner, "curve", "output");
        // Identity curve has small LUT-quantization noise — same tolerance
        // the existing `curve_spline_identity` test uses.
        assert!(
            (v - pressure).abs() < 0.02,
            "identity remap should preserve pressure {pressure}, got {v}",
        );
    }
}

/// Bipolar destination range: `random [0, 1] → [-100, 100]` spans both
/// halves, verifying the affine remap handles a negative `dst_min`.
///
/// Note: radian-typed ports (`stamp.rotation`, `circle.phase`, etc.)
/// deliberately do NOT have a `natural_range` — radians are a unit, not
/// a normalized signal, and wires like `pen.drawing_angle → rotation`
/// must preserve them exactly. This test uses an abstract `[-100, 100]`
/// to exercise the math without conflating "negative remap target" with
/// "angular signal."
#[test]
fn unit_source_to_bipolar_dest_spans_full_range() {
    let registry = registry();
    let mut graph = Graph::new();

    let random_reg = registry.get("random").unwrap();
    let random = graph.add_node("random", random_reg.ports.clone(), random_params());

    let mix_reg = registry.get("mix").unwrap();
    let mut mix_ports = mix_reg.ports.clone();
    for p in mix_ports.iter_mut() {
        if p.name == "factor" {
            *p = std::mem::replace(p, PortDef::input("placeholder", p.wire_type))
                .with_natural_range(-100.0, 100.0);
        }
    }
    let mix = graph.add_node("mix", mix_ports, vec![]);

    graph
        .connect(
            PortRef {
                node: random,
                port: "value".into(),
            },
            PortRef {
                node: mix,
                port: "factor".into(),
            },
        )
        .unwrap();

    let mut runner =
        BrushGraphRunner::new(&graph, registry.as_map(), registry.evaluators()).unwrap();

    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    for dab in 0..32 {
        run_one_dab(&mut runner, 0.5, dab);
        let v = read_scalar(&runner, "mix", "result");
        assert!(
            (-100.0..=100.0).contains(&v),
            "value {v} outside [-100, 100] on dab {dab}",
        );
        if v < min {
            min = v;
        }
        if v > max {
            max = v;
        }
    }
    // We should see both halves of the range — at least one negative and
    // one positive sample across 32 dabs.
    assert!(
        min < 0.0,
        "expected at least one negative sample, min was {min}"
    );
    assert!(
        max > 0.0,
        "expected at least one positive sample, max was {max}"
    );
}

/// Radian-unit ports (`pen.drawing_angle → stamp.rotation`) are
/// unit-preserving identity wires — both speak radians, so the value
/// must pass through raw without any range remap. This is the regression
/// the broader [`rotation.rs`] integration test covers; this minimal
/// CPU-only test pins the contract at the runner level.
#[test]
fn radian_to_radian_wire_passes_through() {
    let registry = registry();
    let mut graph = Graph::new();

    let pen_reg = registry.get("pen_input").unwrap();
    let pen = graph.add_node("pen_input", pen_reg.ports.clone(), vec![]);

    // mix.factor with no natural_range (radians-style dest).
    let mix_reg = registry.get("mix").unwrap();
    let mut mix_ports = mix_reg.ports.clone();
    for p in mix_ports.iter_mut() {
        if p.name == "factor" {
            p.natural_range = None;
        }
    }
    let mix = graph.add_node("mix", mix_ports, vec![]);

    graph
        .connect(
            PortRef {
                node: pen,
                port: "drawing_angle".into(),
            },
            PortRef {
                node: mix,
                port: "factor".into(),
            },
        )
        .unwrap();

    let mut runner =
        BrushGraphRunner::new(&graph, registry.as_map(), registry.evaluators()).unwrap();

    // drawing_angle defaults to 0 in PaintInformation. Set it explicitly
    // via a custom PaintInformation to a known radian value and confirm
    // mix sees the same raw value (no [0, TAU] → [???] remap).
    let info = PaintInformation {
        drawing_angle: std::f32::consts::PI,
        ..Default::default()
    };
    runner.seed_sensors(&info, [0.0, 0.0, 0.0, 1.0], 42, 0);
    runner.clear_slots();
    runner.seed_sensors(&info, [0.0, 0.0, 0.0, 1.0], 42, 0);
    runner.execute_cpu();

    let v = read_scalar(&runner, "mix", "result");
    assert!(
        (v - std::f32::consts::PI).abs() < 1e-5,
        "drawing_angle PI rad should pass through to rotation as PI rad, got {v}",
    );
}
