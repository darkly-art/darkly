//! Verifies the stamp node exposes its size port under the user-facing name
//! `"size"` (label "Size") and keeps the dynamic per-dab signal as the
//! unexposed `"size_input"` port.

use darkly::brush::nodes::stamp;

#[test]
fn stamp_exposes_size_not_scale() {
    let reg = stamp::register();

    let size = reg
        .ports
        .iter()
        .find(|p| p.name == "size")
        .expect("stamp must have a port named `size`");
    assert_eq!(size.label, "Size");
    assert!(
        size.exposed,
        "the `size` port must be exposed in the brush bar"
    );

    let size_input = reg
        .ports
        .iter()
        .find(|p| p.name == "size_input")
        .expect("stamp must have a port named `size_input`");
    assert!(
        !size_input.exposed,
        "`size_input` is the dynamic per-dab signal and must NOT be exposed"
    );

    assert!(
        reg.ports.iter().all(|p| p.name != "scale"),
        "stamp no longer has a `scale` port — it was renamed to `size`"
    );
}
