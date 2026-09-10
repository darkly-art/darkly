use crate::tool::ToolRegistration;

pub fn register() -> ToolRegistration {
    ToolRegistration {
        type_id: "polygon_select",
        display_name: "Polygon Select",
        icon: "lucide:triangle-dashed",
        description: "Select a region by clicking its corners one at a time.",
        hotkey_action: "polygonSelectTool",
        params: &[],
    }
}
