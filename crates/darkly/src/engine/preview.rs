//! Picker previews: the editor's consumer of [`crate::gpu::preview`].
//!
//! One entry point per verb (enqueue, step, take) over every previewable
//! catalog. Which catalog a request names is looked up in the generated
//! `preview_mechanisms()` table, so a new previewable catalog is reachable here
//! without this file being edited.
//!
//! **Generation is paced, not batched.** `start_preview` only enqueues;
//! [`DarklyEngine::pump_previews`] runs one sequence at a time and encodes at
//! most [`PREVIEW_FRAMES_PER_TICK`] frames per engine tick. Every frame in
//! flight is an unpooled `MAP_READ` staging buffer, so bounding the tick
//! bounds the memory: opening a picker with seventeen animated cards would
//! otherwise put the whole sequence's staging buffers in flight at once.
//!
//! Capture is asynchronous throughout (`CONTRIBUTING.md` §No Blocking GPU
//! Readbacks): each frame's readback is appended to the *same* submission that
//! encoded it,
//! so it captures that frame before the next overwrites the output texture.

use super::DarklyEngine;
use super::PreviewJob;
use super::ReadbackContext;
use crate::catalog::preview_mechanisms;
use crate::coord::LayerRect;
use crate::gpu::preview::{
    close_loop, PreviewMechanism, PreviewSequence, PreviewVariant, PREVIEW_FORMAT,
};

/// What keys a preview job: which catalog, which entry, and which of the entry's
/// two previews. A card's still and its animation are separate generations of
/// the same entry and coexist, so hovering never discards the frame already on
/// screen.
pub(crate) type PreviewKey = (&'static str, &'static str, PreviewVariant);

/// Frames encoded per engine tick, across all pending previews. Bounds both
/// in-flight readback memory (this many `MAP_READ` staging buffers) and the GPU
/// work one tick can add. Ten cards of 48 frames drain in 60 ticks (about a
/// second at 60 Hz), and each card completes in order, so a picker fills
/// top-down rather than everything appearing at once.
pub const PREVIEW_FRAMES_PER_TICK: u32 = 8;

/// The preview being generated right now. The sequence itself is not stored:
/// it borrows a registry off the compositor, so it is re-opened each tick and
/// seeked to `cursor`, free because `preview_at` is absolute and a resumed
/// sequence reaches the state an uninterrupted one would have.
pub(crate) struct ActivePreview {
    pub key: PreviewKey,
    pub cursor: u32,
}

/// Look a catalog id up in the generated table.
fn mechanism(catalog: &str) -> Option<(&'static str, &'static dyn PreviewMechanism)> {
    preview_mechanisms()
        .into_iter()
        .find(|(id, _)| *id == catalog)
}

impl DarklyEngine {
    /// Queue one of the two previews of `catalog`/`type_id`: the effect applied
    /// to the **current canvas** for the kinds that read one, generated from
    /// scratch for the kinds that don't.
    ///
    /// A picker asks for [`PreviewVariant::Still`] per card and for
    /// [`PreviewVariant::Animated`] only when the pointer arrives, so opening a
    /// picker costs one frame per card rather than a full sequence each.
    ///
    /// Enqueues only; frames are produced by [`Self::pump_previews`] and
    /// retrieved with [`Self::poll_preview`]. An unknown catalog or type, or one
    /// declaring no preview, is a silent no-op: the request carries an
    /// arbitrary wire string, and there is nothing to render.
    ///
    /// Fully isolated from the live document: the effect instance is built fresh
    /// against the preview target's own textures, so the user's veil chain,
    /// layer stack and compositor surface are never touched.
    pub fn start_preview(&mut self, catalog: &str, type_id: &str, variant: PreviewVariant) {
        let Some((catalog, mech)) = mechanism(catalog) else {
            return;
        };
        let Some(entry) = mech.resolve(type_id) else {
            return;
        };
        let key = (catalog, entry.type_id, variant);

        // Already done, already running, or already queued. Re-opening the
        // picker after a poll *does* regenerate: the completed job is taken
        // rather than cloned, so the canvas it reflects is always the current
        // one.
        let queued = self.preview_queue.iter().any(|k| *k == key);
        let active = self.preview_active.as_ref().is_some_and(|a| a.key == key);
        if self.previews.contains_key(&key) || queued || active {
            return;
        }
        // A burst of requests (a picker opening) shares one composite. The
        // flag is what makes `render_offscreen` cost once per burst rather than
        // once per card.
        if self.preview_queue.is_empty() && self.preview_active.is_none() {
            self.preview_source_dirty = true;
        }
        self.preview_queue.push_back(key);
    }

    /// Advance preview generation by at most [`PREVIEW_FRAMES_PER_TICK`] frames.
    /// Called once per rendered frame beside the readback drain.
    pub(crate) fn pump_previews(&mut self) {
        let mut budget = PREVIEW_FRAMES_PER_TICK;
        while budget > 0 {
            if self.preview_active.is_none() && !self.open_next() {
                return;
            }
            let Some(active) = self.preview_active.as_ref() else {
                return;
            };
            let (key, cursor) = (active.key, active.cursor);
            let Some((_, mech)) = mechanism(key.0) else {
                self.preview_active = None;
                return;
            };

            match self.encode_frames(mech, key, cursor, budget) {
                // The entry resolved but its mechanism could not open it. Drop
                // the job rather than leave one nothing can ever complete: the
                // frontend polls for 180 frames before giving up, so a job that
                // can never fill is a silent hang.
                None => {
                    self.previews.remove(&key);
                    self.preview_active = None;
                    return;
                }
                Some((encoded, done)) => {
                    budget -= encoded;
                    if let Some(a) = self.preview_active.as_mut() {
                        a.cursor = cursor + encoded;
                    }
                    if done {
                        self.preview_active = None;
                    }
                    if encoded == 0 {
                        return;
                    }
                }
            }
        }
    }

    /// Encode up to `budget` frames of `type_id` starting at `cursor`, each
    /// with its readback appended to the encoding submission. Answers the frames
    /// encoded and whether the sequence finished, or `None` if it could not be
    /// opened.
    fn encode_frames(
        &mut self,
        mech: &'static dyn PreviewMechanism,
        key: PreviewKey,
        cursor: u32,
        budget: u32,
    ) -> Option<(u32, bool)> {
        let (catalog, type_id, variant) = key;
        // Disjoint fields: the sequence borrows the compositor's registries
        // while the capture closure borrows the GPU context and the readback
        // scheduler.
        let Self {
            compositor,
            gpu,
            readbacks,
            preview_target,
            ..
        } = self;
        let mut seq =
            PreviewSequence::open(mech, compositor.preview_registries(), type_id, variant)?;
        seq.seek(cursor);

        let mut encoded = 0;
        while encoded < budget {
            let stepped = seq.step(
                &gpu.device,
                &gpu.queue,
                preview_target,
                |mut encoder, output, frame_idx, total| {
                    let rect = LayerRect::from_xywh(0, 0, output.width(), output.height());
                    let request = crate::gpu::readback::request_readback(
                        &gpu.device,
                        &mut encoder,
                        output,
                        PREVIEW_FORMAT,
                        rect,
                    );
                    gpu.queue.submit([encoder.finish()]);
                    readbacks.submit(
                        request,
                        ReadbackContext::PreviewFrame {
                            catalog,
                            type_id,
                            variant,
                            frame_idx,
                            total,
                        },
                    );
                },
            );
            if !stepped {
                break;
            }
            encoded += 1;
        }
        Some((encoded, seq.is_done()))
    }

    /// Open the next queued preview, refreshing the target's subject if this is
    /// the first of a burst. `false` when the queue is empty or the entry
    /// evaporated.
    ///
    /// The composite does not change between the cards of one picker batch, so
    /// one full-canvas `render_offscreen` serves all of them, which matters
    /// most for the stills, where the batch is every card at once.
    fn open_next(&mut self) -> bool {
        let Some(key) = self.preview_queue.pop_front() else {
            return false;
        };
        let (catalog, type_id, variant) = key;
        let Some((_, mech)) = mechanism(catalog) else {
            return false;
        };
        let Some(entry) = mech.resolve(type_id) else {
            return false;
        };

        self.load_subject(mech.reads_source());

        let (pw, ph) = self.preview_target.size();
        let frames = match variant {
            PreviewVariant::Still => 1,
            PreviewVariant::Animated => entry.anim.frames.max(1),
        };
        self.previews.insert(
            key,
            PreviewJob {
                width: pw,
                height: ph,
                fps: entry.anim.fps,
                frames: vec![None; frames as usize],
                anim: entry.anim,
            },
        );
        self.preview_active = Some(ActivePreview { key, cursor: 0 });
        true
    }

    /// Put the subject the next entry needs into the target: the live composite
    /// for a mechanism that reads one, a cleared texture for one that generates
    /// its own content.
    ///
    /// Reloads only when the target does not already hold what is wanted,
    /// which for a burst of same-kind requests is once, and for a burst that
    /// mixes kinds is once per switch.
    fn load_subject(&mut self, reads_source: bool) {
        if !self.preview_source_dirty && self.preview_source_is_composite == reads_source {
            return;
        }
        if reads_source {
            // Refresh the composite so the preview reflects the current
            // document, even with no surface present yet (mirrors
            // `start_export`).
            self.compositor
                .render_offscreen(&self.gpu.device, &self.gpu.queue, &mut self.doc);
        }
        let (w, h) = (
            self.compositor.canvas_width(),
            self.compositor.canvas_height(),
        );
        let Self {
            compositor,
            gpu,
            preview_target,
            ..
        } = self;
        if reads_source {
            let source = compositor.composited_texture();
            let view = source.create_view(&wgpu::TextureViewDescriptor::default());
            preview_target.load_source(&gpu.device, &gpu.queue, &view, w, h);
        } else {
            preview_target.clear_source(&gpu.device, &gpu.queue, w, h);
        }
        self.preview_source_is_composite = reads_source;
        self.preview_source_dirty = false;
    }

    /// Take the completed preview for `catalog`/`type_id`/`variant` as
    /// `(width, height, fps, frames)` once every frame has landed, else `None`.
    /// Each frame is `width × height` tightly-packed RGBA8.
    ///
    /// Takes rather than clones: the frontend copies each frame into an
    /// `ImageData` of its own, so retaining them here would keep every picker
    /// session's frames alive for the life of the engine. Re-opening the picker
    /// regenerates, which is correct anyway: the canvas has moved on.
    pub fn poll_preview(
        &mut self,
        catalog: &str,
        type_id: &str,
        variant: PreviewVariant,
    ) -> Option<(u32, u32, u32, Vec<Vec<u8>>)> {
        let key = *self
            .previews
            .keys()
            .find(|(c, t, v)| *c == catalog && *t == type_id && *v == variant)?;
        if self.previews[&key].frames.iter().any(Option::is_none) {
            return None;
        }
        let job = self.previews.remove(&key)?;
        let frames = job
            .frames
            .into_iter()
            .map(|f| f.expect("all filled"))
            .collect();
        // The picker wraps its cursor modulo the frame count, so a one-way
        // sequence jumped on every repeat. Closed here rather than there (and
        // by the same function the documentation render goes through) because
        // this is where the whole sequence first exists at once.
        Some((job.width, job.height, job.fps, close_loop(job.anim, frames)))
    }
}
