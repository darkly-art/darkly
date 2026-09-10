use crate::action::{ActionCategory, ActionDef};

/// Selecting a tool is not declared here: each tool names the action that
/// selects it on its own `ToolRegistration` (`hotkey_action`), so the `tools`
/// catalog already documents those twelve. What is left is the tool state a
/// hotkey can flip without being a tool of its own.
const ACTIONS: &[ActionDef] = &[ActionDef {
    id: "toggleEraseMode",
    display_name: "Toggle Erase Mode",
    description: "Toggle erase mode on the brush tool. Switches to the brush tool first if another tool is active.",
    icon: "fa6-solid:eraser",
}];

pub fn register() -> ActionCategory {
    ActionCategory {
        id: "tools",
        actions: ACTIONS,
    }
}
