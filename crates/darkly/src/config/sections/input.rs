use crate::config::schema::{Pref, PrefKind, SchemaSection, WidgetHint};

const PREFS: &[Pref] = &[
    Pref {
        key: "input.fingerPainting",
        display_name: "Finger painting",
        description: Some("Allow touch input to paint (not just pan/zoom)."),
        kind: PrefKind::Bool,
        widget: WidgetHint::Auto,
    },
    Pref {
        key: "input.predictionHorizon",
        display_name: "Stroke prediction",
        description: Some(
            "Look-ahead in milliseconds (0 = off). Draws a short extrapolated \
             tail ahead of the pen to hide input latency. Only takes effect \
             while stabilization is active.",
        ),
        kind: PrefKind::Float {
            min: 0.0,
            max: 50.0,
        },
        widget: WidgetHint::Auto,
    },
];

pub fn register() -> SchemaSection {
    SchemaSection {
        id: "input",
        display_name: "Input",
        description: Some("Stylus and touch behavior."),
        icon: Some("fa6-solid:pen-to-square"),
        order: 40,
        prefs: PREFS,
    }
}
