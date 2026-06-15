use crate::config::schema::{Pref, PrefKind, SchemaSection, WidgetHint};

const PREFS: &[Pref] = &[
    Pref {
        key: "autosave.enabled",
        display_name: "Enable autosave",
        description: Some(
            "Periodically snapshot unsaved documents for crash recovery. \
             Snapshots stay in the browser and are never written to your file.",
        ),
        kind: PrefKind::Bool,
        widget: WidgetHint::Auto,
    },
    Pref {
        key: "autosave.intervalSeconds",
        display_name: "Autosave interval (seconds)",
        description: Some("How often to refresh the recovery snapshot of the active document."),
        kind: PrefKind::Int { min: 30, max: 1800 },
        widget: WidgetHint::NumberInput,
    },
];

pub fn register() -> SchemaSection {
    SchemaSection {
        id: "autosave",
        display_name: "Autosave",
        description: Some("Crash-recovery snapshots of unsaved work."),
        icon: Some("fa6-solid:clock-rotate-left"),
        order: 15,
        prefs: PREFS,
    }
}
