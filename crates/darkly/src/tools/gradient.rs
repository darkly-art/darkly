use crate::tool::ToolRegistration;

pub fn register() -> ToolRegistration {
    ToolRegistration {
        type_id: "gradient",
        display_name: "Gradient",
        params: &[],
    }
}
