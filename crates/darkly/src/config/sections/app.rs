use crate::config::presets_gen::BASE_SETTINGS_OPTIONS;
use crate::config::schema::{Pref, PrefKind, SchemaSection, WidgetHint};

const PREFS: &[Pref] = &[Pref {
    key: "app.baseSettings",
    display_name: "Base settings",
    description: Some(
        "Starting point modeled after a familiar editor. Your own \
         customizations sit on top, and you can switch the base at any time.",
    ),
    kind: PrefKind::Enum {
        options: BASE_SETTINGS_OPTIONS,
    },
    widget: WidgetHint::Auto,
}];

pub fn register() -> SchemaSection {
    SchemaSection {
        id: "app",
        display_name: "Application",
        description: Some("Application-wide preferences."),
        icon: Some("fa-solid fa-gear"),
        order: -100,
        prefs: PREFS,
    }
}
