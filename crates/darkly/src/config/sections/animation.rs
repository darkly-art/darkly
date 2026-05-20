use crate::config::schema::{Pref, PrefDefault, PrefKind, SchemaSection, WidgetHint};

// Note: these keep the historic snake_case key names to preserve existing
// Rust callers. New prefs should use camelCase dot-paths.
const PREFS: &[Pref] = &[
    Pref {
        key: "animation.veil_divisor",
        display_name: "Veil animation divisor",
        description: Some(
            "How often animated veils tick, as a fraction of the master frame rate. 1 = every frame, 2 = every other, 4 = every fourth.",
        ),
        kind: PrefKind::Int { min: 1, max: 16 },
        default: PrefDefault::Int(2),
        widget: WidgetHint::Auto,
    },
    Pref {
        key: "animation.overlay_divisor",
        display_name: "Overlay animation divisor",
        description: Some("Divisor for marching-ants selection overlays."),
        kind: PrefKind::Int { min: 1, max: 16 },
        default: PrefDefault::Int(4),
        widget: WidgetHint::Auto,
    },
];

pub fn register() -> SchemaSection {
    SchemaSection {
        id: "animation",
        display_name: "Animation",
        description: Some("Tick rates for animated overlays and veils."),
        icon: Some("fa-solid fa-stopwatch"),
        order: 60,
        prefs: PREFS,
    }
}
