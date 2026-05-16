use crate::tool::ToolRegistration;

pub fn register() -> ToolRegistration {
    ToolRegistration {
        type_id: "fill",
        display_name: "Fill",
        params: &[],
    }
}
