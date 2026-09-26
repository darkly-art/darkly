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
    // The colors section of the radial palette popup owns a 120 degree arc
    // split evenly among its nodes, so every added swatch narrows all of them:
    // at 12, plus the spectrum leaf that opens the color wheel, each sector is
    // 9.2 degrees, still a landable target at the innermost ring. Twelve is
    // also exactly Krita's color-history depth (MAX_RECENT_COLOR,
    // libs/ui/kis_favorite_resource_manager.cpp). The floor is 2 rather than 1
    // because the section seeds the current foreground/background pair when
    // the recents run short, and a section that could hold only one of them
    // would be a number the widget promises and the builder cannot honour.
    Pref {
        key: "ui.palettePopup.recentColors",
        display_name: "Recent colors in the palette popup",
        description: Some("How many recently used colors are in the radial palette popup."),
        kind: PrefKind::Int { min: 2, max: 12 },
        widget: WidgetHint::Auto,
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
