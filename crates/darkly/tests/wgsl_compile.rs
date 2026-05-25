//! Framework tests for `crate::brush::wgsl_compile` — the brush-graph
//! → WGSL fragment shader compiler.
//!
//! Asserts:
//!
//! 1. **Non-compilable graphs fail cleanly** — a graph wiring a node
//!    that returns `Err` from `compile_wgsl` produces a `CompileError`
//!    rather than panicking.
//! 2. **Identical topologies hash to the same id** — two structurally
//!    identical graphs (independent of node ID allocation) hash to the
//!    same `topology_hash` so the per-brush pipeline cache shares
//!    pipelines.
//! 3. **The Perlin Ink builtin compiles end-to-end** — the framework
//!    handles a real graph with random + curve + circle + stamp +
//!    paint_compiled and produces non-empty WGSL.

use std::collections::HashMap;

use darkly::brush::eval::BrushNodeEvaluator;
use darkly::brush::wgsl_compile::{compile_brush_to_wgsl, CompileError};
use darkly::brush::wire::BrushWireType;
use darkly::brush::{default_evaluators, BrushNodeRegistry};
use darkly::nodegraph::{compile, Graph, PortRef};

fn registry() -> BrushNodeRegistry {
    BrushNodeRegistry::new()
}

fn evals() -> HashMap<String, Box<dyn BrushNodeEvaluator>> {
    default_evaluators()
}

#[test]
fn empty_graph_errors_cleanly() {
    let graph = Graph::<BrushWireType>::new();
    let reg = registry();
    let plan = compile(&graph, reg.as_map()).unwrap();
    let err = compile_brush_to_wgsl(&graph, &plan, &evals())
        .expect_err("empty graph has no terminal — must error");
    assert!(matches!(err, CompileError::NoTerminal));
}

#[test]
fn non_compilable_node_errors_with_type_id() {
    // `image` (or any node without a compile_wgsl impl) feeding the
    // graph should produce NodeNotCompilable with the offending
    // type_id.
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let img = graph.add_node(
        "image",
        reg.get("image").unwrap().ports.clone(),
        vec![darkly::gpu::params::ParamValue::String("dummy".into())],
    );
    let _ = img;
    let plan = compile(&graph, reg.as_map()).unwrap();
    let err =
        compile_brush_to_wgsl(&graph, &plan, &evals()).expect_err("image has no compile_wgsl impl");
    match err {
        CompileError::NodeNotCompilable { type_id, reason } => {
            assert_eq!(type_id, "image");
            assert!(!reason.is_empty());
        }
        other => panic!("expected NodeNotCompilable, got {other:?}"),
    }
}

#[test]
fn perlin_ink_brush_compiles_to_nonempty_wgsl() {
    // Lift the Perlin Ink graph straight from `builtin_brushes::all()`
    // — it's the canonical demo brush this framework was built to
    // support, and a quick smoke test that every per-node
    // `compile_wgsl` works in the context of a real graph.
    let perlin = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Perlin Ink")
        .expect("Perlin Ink brush registered");
    let reg = registry();
    let plan = compile(&perlin.metadata.graph, reg.as_map()).unwrap();
    let compiled =
        compile_brush_to_wgsl(&perlin.metadata.graph, &plan, &evals()).expect("compiles");
    assert!(compiled.wgsl.contains("@fragment"));
    assert!(compiled.wgsl.contains("fn fs_main"));
    assert!(compiled.wgsl.contains("shape_r_theta")); // perlin shape
    assert!(compiled.wgsl.contains("DabRecord"));
    assert!(compiled.wgsl.contains("Uniforms"));
    assert!(compiled.dab_record_size >= 16); // intrinsic header + pen
    assert!(compiled.uniform_size > 0); // intrinsic + paint_color
    assert!(compiled.topology_hash != 0);
}

#[test]
fn topology_hash_is_stable_for_identical_graphs() {
    let perlin_a = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Perlin Ink")
        .unwrap();
    let perlin_b = darkly::brush::builtin_brushes::all()
        .into_iter()
        .find(|b| b.metadata.name == "Perlin Ink")
        .unwrap();
    let reg = registry();
    let plan_a = compile(&perlin_a.metadata.graph, reg.as_map()).unwrap();
    let plan_b = compile(&perlin_b.metadata.graph, reg.as_map()).unwrap();
    let a = compile_brush_to_wgsl(&perlin_a.metadata.graph, &plan_a, &evals()).unwrap();
    let b = compile_brush_to_wgsl(&perlin_b.metadata.graph, &plan_b, &evals()).unwrap();
    assert_eq!(a.topology_hash, b.topology_hash);
    assert_eq!(a.dab_record_size, b.dab_record_size);
    assert_eq!(a.uniform_size, b.uniform_size);
}

#[test]
fn paint_compiled_only_graph_falls_through_to_disc() {
    // pen_input → paint_compiled with no upstream graph: terminal's
    // `rgba` input is unwired, so the fallback "opaque white modulated
    // by local_dist" path runs. Smoke test that this compiles too.
    let reg = registry();
    let mut graph = Graph::<BrushWireType>::new();
    let pen = graph.add_node(
        "pen_input",
        reg.get("pen_input").unwrap().ports.clone(),
        vec![],
    );
    let term = graph.add_node(
        "paint_compiled",
        reg.get("paint_compiled").unwrap().ports.clone(),
        vec![],
    );
    graph
        .connect(
            PortRef {
                node: pen,
                port: "position".into(),
            },
            PortRef {
                node: term,
                port: "position".into(),
            },
        )
        .unwrap();
    let plan = compile(&graph, reg.as_map()).unwrap();
    let compiled = compile_brush_to_wgsl(&graph, &plan, &evals())
        .expect("paint_compiled with no rgba wire still compiles");
    assert!(compiled.wgsl.contains("local_dist"));
    assert!(compiled.wgsl.contains("vec4<f32>(1.0, 1.0, 1.0, 1.0)"));
}
