//! Effect-agnostic picker-preview primitives shared by the veil and void
//! preview renderers.
//!
//! Both renderers produce small, looping thumbnail frames of an effect for the
//! "Add …" pickers, read back asynchronously by the engine. The GPU plumbing
//! differs — veils need a ping-pong texture *pair* (they read one, write the
//! other), voids render straight into a single destination — so each renderer
//! owns its own textures. What's genuinely common lives here: the thumbnail
//! sizing constants and the aspect-fit helper that turns a canvas size into a
//! preview size.

/// Longest preview-thumbnail edge, in pixels. The source is fit into this box
/// preserving its aspect ratio, so previews aren't distorted regardless of the
/// document's shape.
pub const PREVIEW_MAX_DIM: u32 = 256;
/// Frames captured for an animated effect (≈2s at [`PREVIEW_FPS`]). Static
/// effects render a single frame.
pub const ANIMATED_FRAMES: u32 = 48;
/// Capture / playback rate, in frames per second.
pub const PREVIEW_FPS: u32 = 24;
/// Per-frame delta time (seconds) fed to animated effects' `update_time`.
pub const PREVIEW_DT: f32 = 1.0 / PREVIEW_FPS as f32;

/// Fit a `w × h` source into a box of [`PREVIEW_MAX_DIM`] on its longest edge,
/// preserving aspect ratio. Sources already within the box are kept as-is.
pub fn fit_preview_dims(w: u32, h: u32) -> (u32, u32) {
    let w = w.max(1);
    let h = h.max(1);
    let longest = w.max(h);
    if longest <= PREVIEW_MAX_DIM {
        return (w, h);
    }
    let scale = PREVIEW_MAX_DIM as f32 / longest as f32;
    let pw = ((w as f32 * scale).round() as u32).max(1);
    let ph = ((h as f32 * scale).round() as u32).max(1);
    (pw, ph)
}
