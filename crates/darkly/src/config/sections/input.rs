use crate::config::schema::{Pref, PrefDefault, PrefKind, SchemaSection, WidgetHint};

const PREFS: &[Pref] = &[Pref {
    key: "input.fingerPainting",
    display_name: "Finger painting",
    description: Some("Allow touch input to paint (not just pan/zoom)."),
    kind: PrefKind::Bool,
    default: PrefDefault::Bool(false),
    widget: WidgetHint::Auto,
    per_preset: &[],
}];

pub fn register() -> SchemaSection {
    SchemaSection {
        id: "input",
        display_name: "Input",
        description: Some("Stylus and touch behavior."),
        icon: Some("fa-solid fa-pen-to-square"),
        order: 40,
        prefs: PREFS,
    }
}
