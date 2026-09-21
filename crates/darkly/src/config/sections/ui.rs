use crate::config::schema::{Pref, PrefKind, SchemaSection, WidgetHint};

const THEME_OPTIONS: &[(&str, &str)] = &[("dark", "Dark"), ("light", "Light")];

const TOOL_STRIP_EDGES: &[(&str, &str)] = &[
    ("left", "Left"),
    ("right", "Right"),
    ("top", "Top"),
    ("bottom", "Bottom"),
];

const PREFS: &[Pref] = &[
    Pref {
        key: "ui.theme",
        display_name: "Theme",
        description: Some("Dark or light."),
        kind: PrefKind::Enum {
            options: THEME_OPTIONS,
        },
        widget: WidgetHint::Auto,
    },
    // Brush builder pane state, persisted via the unified backend so it
    // survives reloads, but hidden from the Settings UI: it's UI state,
    // not a configurable preference.
    Pref {
        key: "ui.brushBuilder.previewVisible",
        display_name: "Brush preview pane visible",
        description: None,
        kind: PrefKind::Bool,
        widget: WidgetHint::Hidden,
    },
    Pref {
        key: "ui.brushBuilder.previewWidth",
        display_name: "Brush preview width",
        description: None,
        kind: PrefKind::Int { min: 160, max: 800 },
        widget: WidgetHint::Hidden,
    },
    Pref {
        key: "ui.brushBuilder.previewHeight",
        display_name: "Brush preview height",
        description: None,
        kind: PrefKind::Int { min: 60, max: 400 },
        widget: WidgetHint::Hidden,
    },
    Pref {
        key: "ui.toolStrip.edge",
        display_name: "Tool strip position",
        description: Some("Which edge of the canvas the tool strip docks to."),
        kind: PrefKind::Enum {
            options: TOOL_STRIP_EDGES,
        },
        widget: WidgetHint::Auto,
    },
    // Where along that edge the strip sits, as a fraction of the travel it has
    // (0 = flush against the start, 1 = flush against the end). A fraction
    // rather than pixels so the strip keeps its place when the canvas area
    // resizes. Set by dragging the strip, so it is not worth a Settings row.
    Pref {
        key: "ui.toolStrip.offset",
        display_name: "Tool strip offset along its edge",
        description: None,
        kind: PrefKind::Float { min: 0.0, max: 1.0 },
        widget: WidgetHint::Hidden,
    },
    Pref {
        key: "ui.toolStrip.autoHide",
        display_name: "Hide the tool strip until the pointer nears it",
        description: Some(
            "Tuck the tool strip against its edge, leaving a sliver, and slide it out on approach.",
        ),
        kind: PrefKind::Bool,
        widget: WidgetHint::Auto,
    },
];

pub fn register() -> SchemaSection {
    SchemaSection {
        id: "ui",
        display_name: "Interface",
        description: None,
        icon: Some("fa6-solid:display"),
        order: 30,
        prefs: PREFS,
    }
}
