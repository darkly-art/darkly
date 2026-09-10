use crate::tool::ToolRegistration;

pub fn register() -> ToolRegistration {
    ToolRegistration {
        type_id: "text",
        display_name: "Text",
        icon: "at-icons:text",
        description: "Place and edit editable text on a vector layer.",
        hotkey_action: "textTool",
        // Font size / color / alignment are driven from the frontend options
        // panel and passed per-request, so the tool itself declares no params.
        params: &[],
    }
}
