//! WGSL canvas-window helper library.
//!
//! WGSL has no `#include`, so the shared canvas-coordinate helpers in
//! `shaders/lib/canvas.wgsl` are concatenated ahead of every shader that
//! needs them at module-creation time. One definition, many call sites —
//! see the module doc in `shaders/lib/canvas.wgsl`.

/// Source of the shared canvas-window helper functions.
pub const CANVAS_LIB: &str = include_str!("../../../../shaders/lib/canvas.wgsl");

/// Prepend the canvas helper library to a shader source so its functions
/// (`plane_to_selection_uv`, `window_uv_to_plane`) are in scope.
pub fn with_canvas_lib(src: &str) -> String {
    format!("{CANVAS_LIB}\n{src}")
}
