use crate::tool::ToolRegistration;

pub fn register() -> ToolRegistration {
    ToolRegistration {
        type_id: "lasso_select",
        display_name: "Lasso Select",
        icon: "tabler:lasso",
        description: "Select a region by drawing its outline freehand.",
        hotkey_action: "lassoSelectTool",
        params: &[],
    }
}
