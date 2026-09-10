use crate::action::{ActionCategory, ActionDef};

const ACTIONS: &[ActionDef] = &[
    ActionDef {
        id: "commitFloating",
        display_name: "Commit Floating",
        description: "Stamp the floating content down into its layer, ending the transform.",
        icon: "fa6-solid:check",
    },
    ActionDef {
        id: "cancelFloating",
        display_name: "Cancel Floating",
        description: "Discard the floating content and leave the layer as it was.",
        icon: "fa6-solid:xmark",
    },
];

pub fn register() -> ActionCategory {
    ActionCategory {
        id: "transform",
        actions: ACTIONS,
    }
}
