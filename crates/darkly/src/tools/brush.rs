use crate::tool::ToolRegistration;

pub fn register() -> ToolRegistration {
    ToolRegistration {
        type_id: "brush",
        display_name: "Brush",
        params: &[],
    }
}
