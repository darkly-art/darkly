//! Switch node: routes one of two inputs through to the output based on
//! a static `select` flag, or removes the wire entirely so downstream falls
//! back to its own port default.
//!
//! Generalises a gate: wire only `in_0_X`, leave `in_1_X` unconnected, and
//! the switch behaves as an enable-on toggle (when `select` flips to 1 the
//! chosen side is unconnected, so downstream sees no wire). Wire both sides
//! and it becomes a 2-way mux.
//!
//! `select` is a Bool input port (not a `ParamDef`) so the brush author can
//! expose it to the painting artist as a toggle in BrushOptions. Per-instance
//! `label`/`description` overrides (set via `Graph::set_port_label` /
//! `set_port_description`) let the author rename the toggle for the artist
//! without inventing a parallel mechanism.
//!
//! The switch is dispatched at graph-rewrite time, before compilation, by
//! [`apply_to`]: that walks the graph and, for every switch whose `select`
//! port is *unconnected* (i.e. driven by its own default), splices the
//! chosen upstream directly into the downstream, drops the other side, and
//! removes the switch node. Switches whose `select` is dynamically wired
//! are left in place and rejected by [`SwitchEvaluator::compile_wgsl`] in
//! v1: a runtime WGSL `select(...)` fallback is a follow-up if anyone
//! actually wires it dynamically.

use crate::brush::eval::{BrushNodeEvaluator, EvalContext};
use crate::brush::input_value::InputValue;
use crate::brush::node::BrushNodeRegistration;
use crate::brush::wgsl::{CompileWgslCtx, NodeWgsl};
use crate::brush::wire::{BrushWireType, ScalarValue};
use crate::nodegraph::{Connection, Graph, NodeRegistration, PortDef, PortDir, PortRef};

pub const TYPE_ID: &str = "switch";

/// `(in_0, in_1, out)` port-name triplet per wire-type. The switch declares
/// one triplet for each [`BrushWireType`]; the brush author wires whichever
/// triplets they need and leaves the others unconnected.
const TRIPLETS: &[(&str, &str, &str)] = &[
    ("in_0_scalar", "in_1_scalar", "out_scalar"),
    ("in_0_int", "in_1_int", "out_int"),
    ("in_0_bool", "in_1_bool", "out_bool"),
    ("in_0_vec2", "in_1_vec2", "out_vec2"),
    ("in_0_vec4", "in_1_vec4", "out_vec4"),
];

const SELECT_PORT: &str = "select";

pub fn register() -> BrushNodeRegistration {
    BrushNodeRegistration::compute(
        NodeRegistration {
            type_id: TYPE_ID,
            category: "math",
            display_name: "Switch",
            description: "Toggle that picks one of two inputs. Wire one side only and it acts as an on/off switch for that branch.",
            ports: vec![
                PortDef::input("in_0_scalar", BrushWireType::Scalar)
                    .with_description("Routed to out_scalar when select=0"),
                PortDef::input("in_1_scalar", BrushWireType::Scalar)
                    .with_description("Routed to out_scalar when select=1"),
                PortDef::output("out_scalar", BrushWireType::Scalar)
                    .with_description("Selected scalar"),
                PortDef::input("in_0_int", BrushWireType::Int)
                    .with_description("Routed to out_int when select=0"),
                PortDef::input("in_1_int", BrushWireType::Int)
                    .with_description("Routed to out_int when select=1"),
                PortDef::output("out_int", BrushWireType::Int).with_description("Selected int"),
                PortDef::input("in_0_bool", BrushWireType::Bool)
                    .with_description("Routed to out_bool when select=0"),
                PortDef::input("in_1_bool", BrushWireType::Bool)
                    .with_description("Routed to out_bool when select=1"),
                PortDef::output("out_bool", BrushWireType::Bool).with_description("Selected bool"),
                PortDef::input("in_0_vec2", BrushWireType::Vec2)
                    .with_description("Routed to out_vec2 when select=0"),
                PortDef::input("in_1_vec2", BrushWireType::Vec2)
                    .with_description("Routed to out_vec2 when select=1"),
                PortDef::output("out_vec2", BrushWireType::Vec2).with_description("Selected vec2"),
                PortDef::input("in_0_vec4", BrushWireType::Vec4)
                    .with_description("Routed to out_vec4 when select=0"),
                PortDef::input("in_1_vec4", BrushWireType::Vec4)
                    .with_description("Routed to out_vec4 when select=1"),
                PortDef::output("out_vec4", BrushWireType::Vec4).with_description("Selected vec4"),
                PortDef::input(SELECT_PORT, BrushWireType::Bool)
                    .with_value(InputValue::Bool(false))
                    .with_label("Select")
                    .with_description(
                        "Routes one of two inputs to the output. If the \
                         chosen input is unconnected, downstream falls back \
                         to its own port default.",
                    ),
            ],
            is_gpu: false,
            is_terminal: false,
            supports_erase: true,
            preview_staging: None,
        },
        || Box::new(SwitchEvaluator),
    )
}

pub struct SwitchEvaluator;

impl BrushNodeEvaluator for SwitchEvaluator {
    /// Safety net only. [`apply_to`] removes Switch nodes before the
    /// runner ever sees them; if one survives (dynamic `select` wiring),
    /// just route the chosen side so a CPU eval doesn't produce garbage.
    fn evaluate_cpu(&self, ctx: &EvalContext) -> Vec<(String, ScalarValue)> {
        let select_to_1 = ctx.input(SELECT_PORT).as_f32() >= 0.5;
        let mut out = Vec::with_capacity(TRIPLETS.len());
        for (in_0, in_1, out_name) in TRIPLETS {
            let chosen = if select_to_1 { *in_1 } else { *in_0 };
            out.push(((*out_name).to_string(), ctx.input(chosen)));
        }
        out
    }

    fn compile_wgsl(&self, _cctx: &CompileWgslCtx) -> Result<NodeWgsl, String> {
        // Reaching this means `apply_to` left the switch in place, which
        // only happens when `select` is dynamically wired. v1 rejects.
        Err(
            "Switch.select must be a static value; dynamic wiring is not \
             yet supported. Disconnect the wire on the `select` port and \
             toggle it via its exposed slider/value instead."
                .into(),
        )
    }
}

/// Rewrite every Switch node in `graph` in place: splice the chosen input
/// through to all downstreams (or drop the wire entirely if the chosen
/// input is unconnected), then remove the Switch.
///
/// Switches whose `select` port has an incoming connection are left alone;
/// those are dynamic and handled (rejected, in v1) by
/// [`SwitchEvaluator::compile_wgsl`].
///
/// Called by [`crate::brush::compile_graph`] on a throwaway clone of the
/// artist's graph: the persisted graph is never mutated.
pub(crate) fn apply_to(graph: &mut Graph<BrushWireType>) {
    let switch_ids: Vec<_> = graph
        .nodes()
        .iter()
        .filter(|(_, n)| n.type_id == TYPE_ID)
        .map(|(id, _)| id.clone())
        .collect();

    for switch_id in switch_ids {
        // Dynamic select → leave for WGSL compile to reject.
        let select_wired = graph
            .connections
            .iter()
            .any(|c| c.to.node == switch_id && c.to.port == SELECT_PORT);
        if select_wired {
            continue;
        }

        // Read the (possibly per-instance overridden) default of `select`.
        let select_to_1 = {
            let Some(node) = graph.nodes().get(&switch_id) else {
                continue;
            };
            node.ports
                .iter()
                .find(|p| p.name == SELECT_PORT && p.dir == PortDir::Input)
                .map(|p| p.value.as_f32() >= 0.5)
                .unwrap_or(false)
        };

        for (in_0, in_1, out_name) in TRIPLETS {
            let chosen_in = if select_to_1 { *in_1 } else { *in_0 };

            // Snapshot the chosen upstream and every downstream first so
            // we can mutate `graph.connections` without overlapping borrows.
            let upstream: Option<PortRef> = graph
                .connections
                .iter()
                .find(|c| c.to.node == switch_id && c.to.port == chosen_in)
                .map(|c| c.from.clone());
            let downstreams: Vec<PortRef> = graph
                .connections
                .iter()
                .filter(|c| c.from.node == switch_id && c.from.port == *out_name)
                .map(|c| c.to.clone())
                .collect();

            // Drop every edge touching this triplet's switch ports
            // (including the unchosen `in_X`, which now goes nowhere).
            graph.connections.retain(|c| {
                !(c.to.node == switch_id && (c.to.port == *in_0 || c.to.port == *in_1))
                    && !(c.from.node == switch_id && c.from.port == *out_name)
            });

            // Splice upstream → downstreams. If no upstream, the
            // downstreams are now unconnected and fall back to their
            // declared port defaults via `EvalContext::input` /
            // `CompileWgslCtx::input`.
            if let Some(up) = upstream {
                for down in downstreams {
                    graph.connections.push(Connection {
                        from: up.clone(),
                        to: down,
                    });
                }
            }
        }

        // remove_node also drops any stray edges still touching the switch
        // (e.g. anyone wiring `out_X` after we cleared it: shouldn't
        // happen, but cheap insurance).
        let _ = graph.remove_node(&switch_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush;
    use crate::nodegraph::{Connection, Graph, NodeId, PortRef};

    /// Helper: instantiate a node of `type_id` from the live registry,
    /// cloning the registration's input/output ports (which carry each
    /// input's default value).
    fn add_node(graph: &mut Graph<BrushWireType>, type_id: &str) -> NodeId {
        let registry = brush::registry();
        let reg = registry
            .get(type_id)
            .unwrap_or_else(|| panic!("no registration for {type_id}"));
        graph.add_node(type_id, reg.ports.clone())
    }

    fn pr(node: &NodeId, port: &str) -> PortRef {
        PortRef {
            node: node.clone(),
            port: port.into(),
        }
    }

    /// `paint_color → switch.in_0_vec4 → switch.out_vec4 → stamp.color`,
    /// select=0 → expect a direct `paint_color → stamp.color` after rewrite.
    #[test]
    fn enabled_default_splices_through() {
        let mut graph = Graph::<BrushWireType>::new();
        let paint_color = add_node(&mut graph, "paint_color");
        let switch_id = add_node(&mut graph, TYPE_ID);
        let stamp = add_node(&mut graph, "stamp");

        graph
            .connect(pr(&paint_color, "color"), pr(&switch_id, "in_0_vec4"))
            .unwrap();
        graph
            .connect(pr(&switch_id, "out_vec4"), pr(&stamp, "color"))
            .unwrap();

        apply_to(&mut graph);

        // Switch removed.
        assert!(
            graph.nodes().get(&switch_id).is_none(),
            "switch node should be removed after rewrite"
        );
        // Direct edge spliced through.
        assert!(
            graph
                .connections
                .iter()
                .any(|c| c.from == pr(&paint_color, "color") && c.to == pr(&stamp, "color")),
            "expected paint_color → stamp.color after splice; got {:?}",
            graph.connections
        );
    }

    /// Same wiring, `select=1` → chosen side (`in_1_vec4`) is unconnected,
    /// so `stamp.color` ends up disconnected after rewrite.
    #[test]
    fn disabled_drops_wire_so_downstream_uses_default() {
        let mut graph = Graph::<BrushWireType>::new();
        let paint_color = add_node(&mut graph, "paint_color");
        let switch_id = add_node(&mut graph, TYPE_ID);
        let stamp = add_node(&mut graph, "stamp");

        graph
            .connect(pr(&paint_color, "color"), pr(&switch_id, "in_0_vec4"))
            .unwrap();
        graph
            .connect(pr(&switch_id, "out_vec4"), pr(&stamp, "color"))
            .unwrap();

        // Flip select to 1: `in_1_vec4` is unconnected so the wire is dropped.
        graph
            .set_port_default(&switch_id, SELECT_PORT, 1.0)
            .unwrap();

        apply_to(&mut graph);

        assert!(graph.nodes().get(&switch_id).is_none());
        assert!(
            !graph
                .connections
                .iter()
                .any(|c| c.to == pr(&stamp, "color")),
            "stamp.color should be unconnected after rewrite; got {:?}",
            graph.connections
        );
    }

    /// Two inputs wired; toggling `select` routes one through and drops the other.
    #[test]
    fn mux_mode_routes_chosen_input_and_drops_unchosen() {
        // select=0 → in_0 wired through.
        {
            let mut graph = Graph::<BrushWireType>::new();
            let paint_color = add_node(&mut graph, "paint_color");
            let switch_id = add_node(&mut graph, TYPE_ID);
            let stamp = add_node(&mut graph, "stamp");

            // Both sides wired; in_0 from paint_color, in_1 from another
            // unrelated source. We don't have a "constant color" node handy
            // so we fake it by leaving in_1 connected to a second paint_color.
            let paint_color_b = add_node(&mut graph, "paint_color");
            graph
                .connect(pr(&paint_color, "color"), pr(&switch_id, "in_0_vec4"))
                .unwrap();
            graph
                .connect(pr(&paint_color_b, "color"), pr(&switch_id, "in_1_vec4"))
                .unwrap();
            graph
                .connect(pr(&switch_id, "out_vec4"), pr(&stamp, "color"))
                .unwrap();

            apply_to(&mut graph);

            assert!(graph.nodes().get(&switch_id).is_none());
            assert!(
                graph
                    .connections
                    .iter()
                    .any(|c| c.from == pr(&paint_color, "color") && c.to == pr(&stamp, "color")),
                "select=0 should route paint_color (in_0) → stamp.color"
            );
            assert!(
                !graph
                    .connections
                    .iter()
                    .any(|c| c.from == pr(&paint_color_b, "color")),
                "paint_color_b (in_1) should be disconnected"
            );
        }

        // select=1 → in_1 wired through.
        {
            let mut graph = Graph::<BrushWireType>::new();
            let paint_color = add_node(&mut graph, "paint_color");
            let switch_id = add_node(&mut graph, TYPE_ID);
            let stamp = add_node(&mut graph, "stamp");
            let paint_color_b = add_node(&mut graph, "paint_color");
            graph
                .connect(pr(&paint_color, "color"), pr(&switch_id, "in_0_vec4"))
                .unwrap();
            graph
                .connect(pr(&paint_color_b, "color"), pr(&switch_id, "in_1_vec4"))
                .unwrap();
            graph
                .connect(pr(&switch_id, "out_vec4"), pr(&stamp, "color"))
                .unwrap();

            graph
                .set_port_default(&switch_id, SELECT_PORT, 1.0)
                .unwrap();

            apply_to(&mut graph);

            assert!(graph.nodes().get(&switch_id).is_none());
            assert!(
                graph
                    .connections
                    .iter()
                    .any(|c| c.from == pr(&paint_color_b, "color") && c.to == pr(&stamp, "color")),
                "select=1 should route paint_color_b (in_1) → stamp.color"
            );
            assert!(
                !graph
                    .connections
                    .iter()
                    .any(|c| c.from == pr(&paint_color, "color")),
                "paint_color (in_0) should be disconnected"
            );
        }
    }

    /// A wire into the `select` port keeps the switch in the graph so the
    /// WGSL compile can reject it. There's no other Bool-output node in the
    /// registry today (the switch is the first), so we synthesize the
    /// connection directly: `apply_to` doesn't re-validate wire types.
    #[test]
    fn dynamic_select_leaves_switch_in_place() {
        let mut graph = Graph::<BrushWireType>::new();
        let paint_color = add_node(&mut graph, "paint_color");
        let switch_id = add_node(&mut graph, TYPE_ID);
        let stamp = add_node(&mut graph, "stamp");
        graph
            .connect(pr(&paint_color, "color"), pr(&switch_id, "in_0_vec4"))
            .unwrap();
        graph
            .connect(pr(&switch_id, "out_vec4"), pr(&stamp, "color"))
            .unwrap();
        // Synthetic dynamic-select wire (no real Bool source exists yet).
        graph.connections.push(Connection {
            from: pr(&paint_color, "color"),
            to: pr(&switch_id, SELECT_PORT),
        });

        apply_to(&mut graph);

        assert!(
            graph.nodes().get(&switch_id).is_some(),
            "switch with dynamic select should NOT be rewritten away"
        );
        assert!(
            graph
                .connections
                .iter()
                .any(|c| c.to == pr(&switch_id, SELECT_PORT)),
            "dynamic select wire should still be present"
        );
    }
}
