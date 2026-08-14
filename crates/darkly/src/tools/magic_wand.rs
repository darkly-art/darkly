use crate::tool::ToolRegistration;

pub fn register() -> ToolRegistration {
    ToolRegistration {
        type_id: "magic_wand",
        display_name: "Magic Wand",
        icon: "fa6-solid:wand-magic-sparkles",
        description: "Select a contiguous region of similar color.",
        hotkey_action: "magicWandTool",
        params: &[],
    }
}
