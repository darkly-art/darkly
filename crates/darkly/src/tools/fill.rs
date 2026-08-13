use crate::tool::ToolRegistration;

pub fn register() -> ToolRegistration {
    ToolRegistration {
        type_id: "fill",
        display_name: "Fill",
        icon: "fa6-solid:fill-drip",
        description: "Flood-fill a contiguous region with the foreground color.",
        hotkey_action: "fillTool",
        params: &[],
    }
}
