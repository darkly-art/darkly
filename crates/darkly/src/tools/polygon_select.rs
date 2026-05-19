use crate::tool::ToolRegistration;

pub fn register() -> ToolRegistration {
    ToolRegistration {
        type_id: "polygon_select",
        display_name: "Polygon Select",
        params: &[],
    }
}
