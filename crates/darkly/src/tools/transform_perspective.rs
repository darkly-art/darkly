use crate::tool::ToolRegistration;

/// Perspective sub-mode of the transform tool — surfaced as its own toolbar
/// cluster member so it has a display name + entry point. The transform logic
/// is entirely frontend (the shared gizmo enters perspective on activation);
/// this registration exists only to name the tool across the WASM boundary.
pub fn register() -> ToolRegistration {
    ToolRegistration {
        type_id: "transform_perspective",
        display_name: "Perspective Transform",
        icon: "tabler:perspective",
        description: "Reshape the active layer by dragging its four corners independently.",
        hotkey_action: "transformPerspectiveTool",
        params: &[],
    }
}
