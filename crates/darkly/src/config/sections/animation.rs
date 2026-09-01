use crate::config::schema::{Pref, PrefKind, SchemaSection, WidgetHint};

// Note: these keep the historic snake_case key names to preserve existing
// Rust callers. New prefs should use camelCase dot-paths.
const PREFS: &[Pref] = &[
    Pref {
        key: "animation.screen_divisor",
        display_name: "Viewport animation divisor",
        description: Some(
            "How often animated viewport-only effects tick, as a fraction of the master frame rate. 1 = every frame, 2 = every other, 4 = every fourth.",
        ),
        kind: PrefKind::Int { min: 1, max: 16 },
        widget: WidgetHint::Auto,
    },
    Pref {
        key: "animation.overlay_divisor",
        display_name: "Overlay animation divisor",
        description: Some("Divisor for marching-ants selection overlays."),
        kind: PrefKind::Int { min: 1, max: 16 },
        widget: WidgetHint::Auto,
    },
    Pref {
        key: "animation.canvas_divisor",
        display_name: "Canvas animation divisor",
        description: Some(
            "How often animated document content — void layers and effect layers below the \
             viewport line — re-renders, as a fraction of the master frame rate. Both share \
             one clock so their ticks line up.",
        ),
        kind: PrefKind::Int { min: 1, max: 16 },
        widget: WidgetHint::Auto,
    },
];

pub fn register() -> SchemaSection {
    SchemaSection {
        id: "animation",
        display_name: "Animation",
        description: Some("Tick rates for animated overlays and effects."),
        icon: Some("fa6-solid:stopwatch"),
        order: 60,
        prefs: PREFS,
    }
}
