use crate::tool::ToolRegistration;

pub fn register() -> ToolRegistration {
    ToolRegistration {
        type_id: "brush",
        display_name: "Brush",
        icon: "fa6-solid:paintbrush",
        description: "Paint strokes with the active brush.",
        hotkey_action: "brushTool",
        params: &[],
    }
}
