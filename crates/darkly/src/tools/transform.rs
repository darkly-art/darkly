use crate::tool::ToolRegistration;

pub fn register() -> ToolRegistration {
    ToolRegistration {
        type_id: "transform",
        display_name: "Transform",
        icon: "fa6-solid:up-down-left-right",
        description: "Move, scale and rotate the active layer or selection.",
        hotkey_action: "transformTool",
        params: &[],
    }
}
