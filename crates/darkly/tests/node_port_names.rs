//! A node's ports share one namespace across inputs and outputs: a
//! `PortRef` is a node id plus a port name, with no direction, and the
//! graph relies on that. `Graph::connect` retires every wire leaving
//! `to` when it drives `to`, which is how a settable-source input stops
//! carrying its old knob value downstream. An input and an output with the
//! same name would therefore have the output's wires deleted whenever the
//! input is connected, depending on the order the wires were made in.

use std::collections::HashSet;

use darkly::brush::registry;

#[test]
fn every_node_port_name_is_unique() {
    for reg in registry().types() {
        let mut seen = HashSet::new();
        for port in &reg.node.ports {
            assert!(
                seen.insert(port.name.as_str()),
                "node `{}` declares port `{}` more than once",
                reg.node.type_id,
                port.name
            );
        }
    }
}

/// The failure that rule prevents, end to end: a saved brush lists the
/// adjust node's outgoing wire before its incoming one, and loading it
/// must keep both.
#[test]
fn wire_order_does_not_drop_an_output_wire() {
    let yaml = "name: Order
description: Output wire listed first.
nodes:
  brush_settings:
    type: brush_settings
  circle:
    type: circle
  hsv:
    type: hsv
  paint:
    type: paint
  paint_color:
    type: paint_color
  pen_input:
    type: pen_input
  stamp:
    type: stamp
connections:
- circle.mask -> stamp.tip
- hsv.output -> stamp.color
- paint_color.color -> hsv.color
- pen_input.position -> paint.position
- stamp.dab -> paint.rgba
";
    let portable: darkly::brush::portable::PortableBrush =
        serde_yaml_ng::from_str(yaml).expect("yaml parses");
    let graph = portable.into_graph(registry()).expect("graph builds");
    let stamp = graph
        .nodes()
        .keys()
        .find(|id| id.as_str() == "stamp")
        .expect("stamp node")
        .clone();
    let color_driver = graph
        .inputs_for(&stamp)
        .find(|c| c.to.port == "color")
        .map(|c| format!("{}.{}", c.from.node.as_str(), c.from.port));
    assert_eq!(color_driver.as_deref(), Some("hsv.output"));
}
