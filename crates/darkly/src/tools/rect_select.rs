use crate::tool::ToolRegistration;

pub fn register() -> ToolRegistration {
    ToolRegistration {
        type_id: "rect_select",
        display_name: "Rectangle Select",
        icon: "boxicons:square-dashed",
        description: "Select a rectangular region.",
        hotkey_action: "rectSelectTool",
        params: &[],
    }
}
