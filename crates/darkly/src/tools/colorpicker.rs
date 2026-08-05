use crate::tool::ToolRegistration;

pub fn register() -> ToolRegistration {
    ToolRegistration {
        type_id: "colorpicker",
        display_name: "Color Picker",
        icon: "fa6-solid:eye-dropper",
        description: "Sample a colour from the canvas into the foreground swatch.",
        hotkey_action: "colorPickerTool",
        params: &[],
    }
}
