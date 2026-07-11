//! Passive process recording (timelapse) — session-side capture state.
//!
//! The recorder samples [`crate::document::Document::revision`] each frame
//! and, when the document changed, downscales the composited canvas into a
//! fixed-size RGBA target (aspect-fit letterboxed so canvas resizes never
//! change the encoder frame size) and reads it back asynchronously. The
//! frontend drains completed frames via `poll_recording_frame` and feeds
//! them to a WebCodecs `VideoEncoder`; the engine knows nothing about
//! encoding or persistence.
//!
//! All state here is **session** state per the document-authority taxonomy:
//! none of it survives reload, and frames flow strictly downhill
//! (document → compositor → readback → frontend).

use std::collections::VecDeque;

use super::{DarklyEngine, ReadbackContext};
use crate::gpu::effect::{self, EffectPipeline};
use crate::gpu::readback;

/// Ceiling on frames waiting for the frontend to drain. When full, captures
/// are skipped *without* consuming the pending revision — a stalled poller
/// must never eat a burst's final frame; the tick retries next frame.
const MAX_COMPLETED_FRAMES: usize = 4;

/// The fixed-size offscreen render target recording frames are downscaled
/// into before readback. Owned by the recorder; reallocated lazily when the
/// configured frame dimensions change.
struct RecordingTarget {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

/// One captured frame, ready for the frontend to drain. `rgba` is tightly
/// packed `width × height × 4`.
pub struct RecordedFrame {
    pub width: u32,
    pub height: u32,
    pub frame_index: u64,
    pub rgba: Vec<u8>,
}

/// Change-triggered, throttled canvas capture. See the module docs.
pub struct ProcessRecorder {
    enabled: bool,
    min_interval_secs: f32,
    /// Encoder frame dimensions, negotiated by the frontend (even-aligned).
    frame_w: u32,
    frame_h: u32,
    target: Option<RecordingTarget>,
    downscale: Option<EffectPipeline>,
    sampler: Option<wgpu::Sampler>,
    /// Document revision consumed by the most recent capture. Only advanced
    /// when a capture is actually encoded — skipped captures retry.
    last_seen_revision: u64,
    /// `render(time_secs)` clock of the most recent capture.
    last_capture_time: Option<f32>,
    /// Armed when a change lands inside the throttle window: the time at
    /// which the trailing capture fires, guaranteeing a burst's final state
    /// is always recorded.
    trailing_due: Option<f32>,
    frame_index: u64,
    completed: VecDeque<RecordedFrame>,
}

impl ProcessRecorder {
    pub fn new() -> Self {
        ProcessRecorder {
            enabled: false,
            min_interval_secs: 1.5,
            frame_w: 0,
            frame_h: 0,
            target: None,
            downscale: None,
            sampler: None,
            last_seen_revision: 0,
            last_capture_time: None,
            trailing_due: None,
            frame_index: 0,
            completed: VecDeque::new(),
        }
    }

    /// Apply frontend-negotiated parameters. Dimensions are forced even
    /// (encoder requirement); the render target reallocates lazily on the
    /// next capture if they changed.
    pub fn configure(&mut self, enabled: bool, min_interval_secs: f32, width: u32, height: u32) {
        self.enabled = enabled;
        self.min_interval_secs = min_interval_secs.max(0.0);
        self.frame_w = width & !1;
        self.frame_h = height & !1;
        if !enabled {
            self.trailing_due = None;
        }
    }

    /// True while the demand-driven frame loop must keep running for the
    /// recorder's sake: a trailing capture is armed (it fires on a future
    /// frame) or captured frames await draining by the frontend.
    pub fn needs_frames(&self) -> bool {
        self.trailing_due.is_some() || !self.completed.is_empty()
    }

    /// Stash a completed readback. Called by `handle_completed_readback`.
    pub(crate) fn push_completed(&mut self, frame: RecordedFrame) {
        self.completed.push_back(frame);
    }

    /// Drain the oldest completed frame, if any.
    pub(crate) fn pop_completed(&mut self) -> Option<RecordedFrame> {
        self.completed.pop_front()
    }
}

impl Default for ProcessRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl DarklyEngine {
    /// Configure the recorder from frontend-negotiated parameters.
    pub fn set_recording_params(
        &mut self,
        enabled: bool,
        min_interval_secs: f32,
        width: u32,
        height: u32,
    ) {
        self.recorder
            .configure(enabled, min_interval_secs, width, height);
    }

    /// Drain the oldest completed recording frame, if any.
    pub fn poll_recording_frame(&mut self) -> Option<RecordedFrame> {
        self.recorder.pop_completed()
    }

    /// Per-frame recorder step: sample the document revision and capture the
    /// canvas when it changed, throttled to one capture per
    /// `min_interval_secs` with a trailing capture so a burst's final state
    /// is always recorded. Runs after `poll_pending()` (deferred stroke
    /// undo-commits are then visible) and never mid-stroke.
    pub(crate) fn tick_process_recording(&mut self, time_secs: f32) {
        let rec = &mut self.recorder;
        if !rec.enabled || rec.frame_w == 0 || rec.frame_h == 0 {
            return;
        }
        if self.active_stroke_layer.is_some() {
            return;
        }

        let changed = self.doc.revision != rec.last_seen_revision;
        let interval_elapsed = rec
            .last_capture_time
            .is_none_or(|t| time_secs - t >= rec.min_interval_secs);

        let fire = if changed && interval_elapsed {
            true
        } else if changed {
            // Inside the throttle window — arm (or keep) the trailing
            // capture so the burst's final state lands once the window
            // closes. Idempotent across frames: the due time is fixed by
            // the last capture, not by when the change was observed.
            let due = rec.last_capture_time.unwrap_or(time_secs) + rec.min_interval_secs;
            rec.trailing_due = Some(due);
            false
        } else {
            rec.trailing_due.is_some_and(|d| time_secs >= d)
        };
        if !fire {
            return;
        }

        // Backpressure: at most one readback in flight, and never overwrite
        // a full completed queue. Neither skip consumes the revision or
        // disarms the trailing capture — the tick retries next frame.
        if rec.completed.len() >= MAX_COMPLETED_FRAMES {
            return;
        }
        if self
            .readbacks
            .any(|c| matches!(c, ReadbackContext::RecordingFrame { .. }))
        {
            return;
        }

        self.capture_recording_frame(time_secs);
    }

    /// Encode one capture: refresh the offscreen composite, soft-downscale
    /// it into the recording target (aspect-fit letterboxed on opaque
    /// black), and submit the async readback.
    fn capture_recording_frame(&mut self, time_secs: f32) {
        // Composite cache is rebuilt on demand — same forcing the export
        // readback does, so the capture sees the current document state
        // even when no surface present has happened (headless / tests).
        self.compositor
            .render_offscreen(&self.gpu.device, &self.gpu.queue, &mut self.doc);

        let rec = &mut self.recorder;
        let (fw, fh) = (rec.frame_w, rec.frame_h);
        let format = wgpu::TextureFormat::Rgba8Unorm;

        if rec.sampler.is_none() {
            rec.sampler = Some(self.gpu.device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("process-recording-sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }));
        }
        if rec.downscale.is_none() {
            rec.downscale = Some(effect::create_downscale_pipeline(
                &self.gpu.device,
                format,
                "process-recording-downscale",
            ));
        }
        let realloc = rec
            .target
            .as_ref()
            .is_none_or(|t| t.width != fw || t.height != fh);
        if realloc {
            let texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("process-recording-target"),
                size: wgpu::Extent3d {
                    width: fw,
                    height: fh,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            rec.target = Some(RecordingTarget {
                texture,
                view,
                width: fw,
                height: fh,
            });
        }

        let target = rec.target.as_ref().unwrap();
        let downscale = rec.downscale.as_ref().unwrap();
        let source_view = self
            .compositor
            .composited_texture()
            .create_view(&wgpu::TextureViewDescriptor::default());
        let source_bg = effect::create_blit_bind_group(
            &self.gpu.device,
            &downscale.bind_group_layout,
            &source_view,
            rec.sampler.as_ref().unwrap(),
            "process-recording-source-bg",
        );

        // Aspect-fit the canvas into the fixed frame; the uncovered bars
        // keep the clear color (opaque black).
        let (cw, ch) = (
            self.compositor.canvas_width() as f32,
            self.compositor.canvas_height() as f32,
        );
        let scale = (fw as f32 / cw).min(fh as f32 / ch);
        let vw = cw * scale;
        let vh = ch * scale;
        let vx = (fw as f32 - vw) / 2.0;
        let vy = (fh as f32 - vh) / 2.0;

        let frame_index = rec.frame_index;
        self.gpu.encode("process-recording-capture", |encoder| {
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("process-recording-downscale"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target.view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                pass.set_pipeline(&downscale.pipeline);
                pass.set_bind_group(0, &source_bg, &[]);
                pass.set_viewport(vx, vy, vw, vh, 0.0, 1.0);
                pass.draw(0..3, 0..1);
            }
            let request = readback::request_readback(
                &self.gpu.device,
                encoder,
                &target.texture,
                format,
                crate::coord::LayerRect::from_xywh(0, 0, fw, fh),
            );
            self.readbacks.submit(
                request,
                ReadbackContext::RecordingFrame {
                    width: fw,
                    height: fh,
                    frame_index,
                },
            );
        });

        let rec = &mut self.recorder;
        rec.frame_index += 1;
        rec.last_seen_revision = self.doc.revision;
        rec.last_capture_time = Some(time_secs);
        rec.trailing_due = None;
    }
}
