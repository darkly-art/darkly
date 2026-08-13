use crate::tool::ToolRegistration;

pub fn register() -> ToolRegistration {
    ToolRegistration {
        type_id: "gradient",
        display_name: "Gradient",
        icon: "boxicons:gradient",
        description: "Drag out a smooth ramp between two or more colours.",
        hotkey_action: "gradientTool",
        params: &[],
    }
}
