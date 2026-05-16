use crate::tool::ToolRegistration;

pub fn register() -> ToolRegistration {
    ToolRegistration {
        type_id: "colorpicker",
        display_name: "Color Picker",
        params: &[],
    }
}
