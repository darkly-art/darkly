use crate::config::schema::{Pref, PrefDefault, PrefKind, SchemaSection, WidgetHint};

const PREFS: &[Pref] = &[Pref {
    key: "edit.activateTransformAfterPaste",
    display_name: "Activate transform tool after paste",
    description: Some(
        "When pasting, enter transform mode so you can immediately reposition, \
         scale, or rotate the pasted content before it commits.",
    ),
    kind: PrefKind::Bool,
    default: PrefDefault::Bool(true),
    widget: WidgetHint::Auto,
}];

pub fn register() -> SchemaSection {
    SchemaSection {
        id: "edit",
        display_name: "Editing",
        description: Some("Behavior of edit operations like paste and transforms."),
        icon: Some("fa-solid fa-pen-to-square"),
        order: 25,
        prefs: PREFS,
    }
}
