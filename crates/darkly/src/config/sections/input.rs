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
        key: "input.predictionLead",
        display_name: "Stroke prediction",
        description: Some(
            "Dabs drawn ahead of the pen to hide input latency. 0 disables prediction.",
        ),
        kind: PrefKind::Int { min: 0, max: 32 },
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
