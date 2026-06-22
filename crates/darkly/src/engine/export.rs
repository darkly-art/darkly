//! Async readback of the composited canvas for image export
//! (PNG/JPEG/WebP). Mirror of the full-canvas branch in `clipboard.rs`,
//! but reads `compositor.composited_texture()` and surfaces raw RGBA8
//! bytes for the JS side to encode via `OffscreenCanvas`.

use super::{DarklyEngine, ReadbackContext};
use crate::engine::protocol::Response;
use crate::gpu::readback;

impl DarklyEngine {
    /// Start an async readback of the full composited canvas. Defers: the
    /// originating request's promise resolves with `{ width, height }` plus the
    /// RGBA8 bytes side-channel once the readback completes (typically the next
    /// frame). The JS side encodes the bytes via `OffscreenCanvas`.
    pub fn start_export(&mut self) {
        let request = self.current_request();

        if self
            .readbacks
            .any(|c| matches!(c, ReadbackContext::ExportImage { .. }))
        {
            // An export is already in flight — nothing new to read back.
            self.resolve_request(request, Response::json(serde_json::Value::Null));
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
            let req = readback::request_readback(
                &self.gpu.device,
                encoder,
                texture,
                wgpu::TextureFormat::Rgba8Unorm,
                crate::coord::LayerRect::from_xywh(0, 0, width, height),
            );
            self.readbacks.submit(
                req,
                ReadbackContext::ExportImage {
                    width,
                    height,
                    request,
                },
            );
        });
    }

    /// Resolve the export request with the completed readback bytes. Called by
    /// `handle_completed_readback`.
    pub(crate) fn complete_export(&mut self, width: u32, height: u32, request: u64, rgba: Vec<u8>) {
        let value = serde_json::json!({ "width": width, "height": height });
        self.resolve_request(request, Response::binary(value, rgba));
    }
}
