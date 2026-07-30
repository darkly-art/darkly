use crate::config::schema::{Pref, PrefKind, SchemaSection, WidgetHint};

const PREFS: &[Pref] = &[
    Pref {
        key: "recording.enabled",
        display_name: "Record process",
        description: Some(
            "Passively record the canvas as you paint, for timelapse export. \
             The recording is saved inside your .darkly file.",
        ),
        kind: PrefKind::Bool,
        widget: WidgetHint::Auto,
    },
    Pref {
        key: "recording.minIntervalSeconds",
        display_name: "Capture interval (seconds)",
        description: Some(
            "Minimum time between captured frames. Changes made inside the \
             window are recorded by a trailing capture when it closes.",
        ),
        kind: PrefKind::Float {
            min: 0.5,
            max: 10.0,
        },
        widget: WidgetHint::Auto,
    },
    Pref {
        key: "recording.maxLongEdge",
        display_name: "Recording resolution",
        description: Some(
            "Longest edge of the recorded video. Capped by what your \
             browser's video encoder supports.",
        ),
        kind: PrefKind::Enum {
            options: &[
                ("1280", "1280 (HD)"),
                ("1920", "1920 (Full HD)"),
                ("2560", "2560 (QHD)"),
                ("3840", "3840 (4K)"),
            ],
        },
        widget: WidgetHint::Auto,
    },
];

pub fn register() -> SchemaSection {
    SchemaSection {
        id: "recording",
        display_name: "Recording",
        description: Some("Passive process recording for timelapse export."),
        icon: Some("fa6-solid:video"),
        order: 16,
        prefs: PREFS,
    }
}
