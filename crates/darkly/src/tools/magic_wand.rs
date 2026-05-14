use crate::tool::ToolRegistration;

pub fn register() -> ToolRegistration {
    ToolRegistration {
        type_id: "magic_wand",
        display_name: "Magic Wand",
        params: &[],
    }
}
