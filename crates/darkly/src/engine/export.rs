//! Async readback of the composited canvas for image export
//! (PNG/JPEG/WebP). Mirror of the full-canvas branch in `clipboard.rs`,
//! but reads `compositor.composited_texture()` and surfaces raw RGBA8
//! bytes for the JS side to encode via `OffscreenCanvas`.

use super::{DarklyEngine, ReadbackContext};
use crate::gpu::readback;

/// Completed export readback — drained by `poll_export_result`.
pub struct ExportImageResult {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl DarklyEngine {
    /// Start an async readback of the full composited canvas. Returns
    /// immediately; the result lands on `pending_export_result` once the
    /// readback completes (typically the next frame).
    pub fn start_export(&mut self) {
        if self
            .readbacks
            .any(|c| matches!(c, ReadbackContext::ExportImage { .. }))
        {
            return;
        }

        // Composite cache is rebuilt on demand by the offscreen render — same
        // mechanism `test_readback_canvas` uses headlessly, and the production
        // present path keeps it fresh. Force an offscreen composite first so
        // the readback sees the current document state even when no surface
        // present has happened (e.g. test, headless, or a freshly mutated
        // document that hasn't had a `render()` yet).
        self.compositor
            .render_offscreen(&self.gpu.device, &self.gpu.queue, &mut self.doc);

        let width = self.compositor.canvas_width();
        let height = self.compositor.canvas_height();
        let texture = self.compositor.composited_texture();

        self.gpu.encode("export-readback", |encoder| {
            let request = readback::request_readback(
                &self.gpu.device,
                encoder,
                texture,
                wgpu::TextureFormat::Rgba8Unorm,
                crate::coord::LayerRect::from_xywh(0, 0, width, height),
            );
            self.readbacks
                .submit(request, ReadbackContext::ExportImage { width, height });
        });
    }

    /// Drain the most recent export result. Returns `None` until the
    /// async readback completes (next frame after `start_export`).
    pub fn poll_export_result(&mut self) -> Option<ExportImageResult> {
        self.pending_export_result.take()
    }

    /// Stash a completed export readback. Called by `handle_completed_readback`.
    pub(crate) fn complete_export(&mut self, width: u32, height: u32, rgba: Vec<u8>) {
        self.pending_export_result = Some(ExportImageResult {
            width,
            height,
            rgba,
        });
    }
}
