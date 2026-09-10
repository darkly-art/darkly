use crate::tool::ToolRegistration;

pub fn register() -> ToolRegistration {
    ToolRegistration {
        type_id: "ellipse_select",
        display_name: "Ellipse Select",
        icon: "lucide:circle-dashed",
        description: "Select an elliptical region.",
        hotkey_action: "ellipseSelectTool",
        params: &[],
    }
}
