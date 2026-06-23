//! Async readback of the composited canvas for image export
//! (PNG/JPEG/WebP). Mirror of the full-canvas branch in `clipboard.rs`,
//! but reads `compositor.composited_texture()` and surfaces raw RGBA8
//! bytes for the JS side to encode via `OffscreenCanvas`.

use std::rc::Rc;

use serde_json::json;

use super::host::cell::EngineCell;
use super::DarklyEngine;
use crate::engine::protocol::Response;
use crate::gpu::readback::{self, ReadbackFuture};

/// Run an export as a linear task: force a fresh offscreen composite, kick the
/// canvas readback, await the pixels, and resolve the originating request with
/// `{ width, height }` plus the RGBA8 bytes side-channel.
async fn run_export(cell: Rc<EngineCell>, request: u64) {
    let (width, height, future) = cell.with_async(|e| e.kick_export_readback()).await;
    // `None` means the handle was disposed mid-flight (slot cancelled) — drop
    // the task silently; teardown already rejected the request.
    let Some(pixels) = future.await else {
        return;
    };
    cell.with_async(|e| {
        let value = json!({ "width": width, "height": height });
        e.resolve_request(request, Response::binary(value, pixels));
    })
    .await;
}

impl DarklyEngine {
    /// Start an async readback of the full composited canvas. Spawns a
    /// [`run_export`] task; the originating request's promise resolves with
    /// `{ width, height }` plus the RGBA8 bytes side-channel once the readback
    /// lands (typically the next frame). The JS side encodes the bytes via
    /// `OffscreenCanvas`.
    pub fn start_export(&mut self) {
        let request = self.current_request();
        let cell = self.self_cell();
        self.spawn(Some(request), run_export(cell, request));
    }

    /// Force a fresh offscreen composite, then encode + submit the awaitable
    /// readback of the composited canvas. Returns `(width, height, future)`;
    /// the task awaits the future for the pre-packed RGBA8 pixels.
    ///
    /// The offscreen composite makes the readback see current document state
    /// even when no surface present has happened (test, headless, or a freshly
    /// mutated document that hasn't rendered yet).
    fn kick_export_readback(&mut self) -> (u32, u32, ReadbackFuture) {
        self.compositor
            .render_offscreen(&self.gpu.device, &self.gpu.queue, &mut self.doc);

        let width = self.compositor.canvas_width();
        let height = self.compositor.canvas_height();
        // Clone the refcounted texture handle so the readback encode doesn't
        // hold a compositor borrow across `await_readback`.
        let texture = self.compositor.composited_texture().clone();

        let req = self.gpu.encode_ret("export-readback", |encoder| {
            readback::request_readback(
                &self.gpu.device,
                encoder,
                &texture,
                wgpu::TextureFormat::Rgba8Unorm,
                crate::coord::LayerRect::from_xywh(0, 0, width, height),
            )
        });
        let future = self.await_readback(req);
        (width, height, future)
    }
}
