use crate::tool::ToolRegistration;

pub fn register() -> ToolRegistration {
    ToolRegistration {
        type_id: "transform",
        display_name: "Transform",
        params: &[],
    }
}
