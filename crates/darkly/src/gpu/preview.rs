//! How a previewable registry entry's preview moves, and the primitives every
//! preview is rendered through.
//!
//! A preview is a short sequence of thumbnail frames of one effect, shown in
//! the editor's pickers and written to disk as documentation. An entry declares
//! *that* it has one — a [`PreviewAnim`] on its registration — and *how it
//! moves* as code: `Veil::preview_at` / `Void::preview_at` for the per-instance
//! kinds, a `fn(f32) -> Vec<ParamValue>` on the registration for filters, whose
//! effect object is shared and holds no parameters.
//!
//! **The convention every `preview_at` body follows**: take `t`, set fields,
//! sync the GPU resources those fields feed — in that order, once. Repetition
//! across bodies is extracted into plain helpers here ([`swing`],
//! [`swing_signed`]) that a body calls and stays in control of.
//!
//! **Absolute, not incremental.** `preview_at(0.5)` puts the instance in the
//! same state whether it follows `preview_at(0.4)` or nothing at all. That is
//! what lets a sequence be rebuilt, resumed, or sampled out of order without a
//! replay, and `preview_at_is_absolute` in `tests/picker_preview.rs` is what
//! holds every entry to it.

/// Longest preview-thumbnail edge, in pixels. The source is fit into this box
/// preserving its aspect ratio, so previews aren't distorted regardless of the
/// document's shape.
pub const PREVIEW_MAX_DIM: u32 = 256;
/// Frames captured for an animated effect (≈2s at [`PREVIEW_FPS`]). Static
/// effects render a single frame.
pub const ANIMATED_FRAMES: u32 = 48;
/// Capture / playback rate, in frames per second.
pub const PREVIEW_FPS: u32 = 24;
/// Seconds of an effect's own clock one preview sequence covers. A veil whose
/// motion is temporal rather than parametric maps `t` onto this span.
pub const PREVIEW_SECONDS: f32 = 2.0;

/// Pixel format every preview is rendered and read back in. Matches the
/// compositor's accumulator and the layer-texture atlas, so a preview frame
/// carries the same kind of pixels the canvas does.
pub const PREVIEW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Which of an entry's two previews is wanted.
///
/// A picker shows every card at once, so it asks for [`Still`](Self::Still)
/// — seventeen cards moving at once is noise, and seventeen sequences is
/// forty-eight times the work. [`Animated`](Self::Animated) is what a card asks
/// for when the pointer is over it, and it is the only thing that ever costs a
/// full sequence.
///
/// Both are the same motion sampled differently, which is what keeps the
/// hand-off invisible: `preview_at` is absolute, so a still is literally the
/// animation's frame at [`PreviewAnim::still_at`], rendered without rendering
/// the rest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub enum PreviewVariant {
    /// One frame — the moment of the motion that stands for it.
    Still,
    /// The whole sequence.
    Animated,
}

/// That an entry has a preview, how it plays back, and which moment of it
/// stands for the whole.
///
/// The motion itself is a method, not data — this says only how long it runs,
/// how it ends, and where to freeze it. `loops` is declared rather than derived
/// for exactly that reason: the only thing that knows whether the last frame
/// hands back to the first is the body that wrote the motion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewAnim {
    /// Images in the sequence. `1` is a still at the entry's own parameters.
    pub frames: u32,
    /// Playback rate.
    pub fps: u32,
    /// Whether the last frame hands back to the first without a visible jump.
    pub loops: bool,
    /// Where on the timeline the [`PreviewVariant::Still`] is taken — the one
    /// frame a picker card shows before anyone hovers it, and the poster frame
    /// of the documentation asset.
    ///
    /// The default `0.5` is the peak of [`swing`], which is where a sweep is
    /// furthest from its resting value and so most legible as a single image.
    /// An entry whose motion peaks elsewhere overrides it with
    /// [`with_still_at`](Self::with_still_at) — a sweep resting at `t = 0` would
    /// otherwise pick the frame that looks like no effect at all.
    pub still_at: f32,
}

impl PreviewAnim {
    /// The conventional animated preview: [`ANIMATED_FRAMES`] at
    /// [`PREVIEW_FPS`], ending where it began. What a parameter sweep that
    /// returns to its resting value declares.
    pub const LOOPING: Self = Self {
        frames: ANIMATED_FRAMES,
        fps: PREVIEW_FPS,
        loops: true,
        still_at: 0.5,
    };

    /// The same length, for motion that runs one way and does not return — a
    /// clock integrated forward, a counter that only counts up.
    pub const ONE_WAY: Self = Self {
        frames: ANIMATED_FRAMES,
        fps: PREVIEW_FPS,
        loops: false,
        still_at: 0.5,
    };

    /// A single frame at the entry's own parameters, for an entry with nothing
    /// to sweep. Both variants render the same image, so hovering such a card
    /// changes nothing — which is the honest thing for an effect that has one
    /// state.
    pub const STILL: Self = Self {
        frames: 1,
        fps: PREVIEW_FPS,
        loops: true,
        still_at: 0.0,
    };

    /// Take the still somewhere other than the middle. For a sweep that runs
    /// signed — out one way, back, out the other — where the middle is the
    /// resting value and the quarter point is the extreme.
    pub const fn with_still_at(self, still_at: f32) -> Self {
        Self { still_at, ..self }
    }

    /// The frame index the still is taken at, clamped into the sequence.
    pub fn still_frame(&self) -> u32 {
        let frames = self.frames.max(1);
        ((self.still_at * frames as f32) as u32).min(frames - 1)
    }
}

/// Normalized timeline position of frame `i` of `frames`. Frame `frames` itself
/// is `t == 1.0` — the frame *after* the last, which is where a looping
/// sequence hands back to frame 0.
pub fn frame_t(i: u32, frames: u32) -> f32 {
    i as f32 / frames.max(1) as f32
}

/// A smooth out-and-back sweep: `0` at `t = 0`, `1` at `t = 0.5`, back to `0`
/// at `t = 1`, at rest at both ends.
///
/// The shape a control sweep wants — the ends match a render at the resting
/// value, so the sequence closes and the frames either side of the wrap agree.
pub fn swing(t: f32) -> f32 {
    0.5 - 0.5 * (t * std::f32::consts::TAU).cos()
}

/// A signed out-and-back sweep: `0 → 1 → 0 → -1 → 0`, peaking at `t = 0.25` and
/// troughing at `t = 0.75`. For a control whose two directions read differently
/// and are both worth showing in one pass.
pub fn swing_signed(t: f32) -> f32 {
    (t * std::f32::consts::TAU).sin()
}

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

// ---------------------------------------------------------------------------
// The target every preview is rendered into
// ---------------------------------------------------------------------------

/// Preview-sized texture pair. View 0 is what the effect reads — the downscaled
/// source, or a cleared texture for an effect that generates its own content;
/// view 1 is what it writes and what the capture reads back.
struct PreviewTextures {
    width: u32,
    height: u32,
    textures: [wgpu::Texture; 2],
    views: [wgpu::TextureView; 2],
}

/// Two preview-sized textures, a sampler, and the soft downscale that fills the
/// source — everything a preview needs that is not the effect itself.
///
/// One instance is reusable across entries and across consumers: it lazily
/// allocates its sampler and pipeline and reallocates its textures only when
/// the preview dimensions change. Which *subject* it holds is an input, not a
/// fork — the editor loads its own composite, the documentation renderer loads
/// a fixed synthetic field, and nothing downstream can tell.
pub struct PreviewTarget {
    textures: Option<PreviewTextures>,
    sampler: Option<wgpu::Sampler>,
    /// Soft multi-tap downscale used to copy the (often much larger) source
    /// into the preview-sized input texture without hard aliasing.
    downscale: Option<super::effect::EffectPipeline>,
}

impl Default for PreviewTarget {
    fn default() -> Self {
        Self::new()
    }
}

impl PreviewTarget {
    pub fn new() -> Self {
        PreviewTarget {
            textures: None,
            sampler: None,
            downscale: None,
        }
    }

    /// Aspect-fit `src_w × src_h` into the preview box and downscale `source`
    /// into the source texture, reallocating if the dimensions changed. For
    /// mechanisms that read a source.
    pub fn load_source(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: &wgpu::TextureView,
        src_w: u32,
        src_h: u32,
    ) {
        self.ensure(device, src_w, src_h);
        let textures = self.textures.as_ref().expect("ensure allocates");
        let downscale = self.downscale.as_ref().expect("ensure allocates");
        let source_bg = super::effect::create_blit_bind_group(
            device,
            &downscale.bind_group_layout,
            source,
            self.sampler.as_ref().expect("ensure allocates"),
            "preview-source-bg",
        );

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("preview-load-source"),
        });
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("preview-downscale"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &textures.views[0],
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            rpass.set_pipeline(&downscale.pipeline);
            rpass.set_bind_group(0, &source_bg, &[]);
            rpass.draw(0..3, 0..1);
        }
        queue.submit([encoder.finish()]);
    }

    /// Aspect-fit `src_w × src_h` and clear the source texture. For mechanisms
    /// that generate their own content and never sample view 0 — the clear is
    /// what keeps it a defined value rather than whatever the previous entry
    /// left there.
    pub fn clear_source(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        src_w: u32,
        src_h: u32,
    ) {
        self.ensure(device, src_w, src_h);
        let textures = self.textures.as_ref().expect("ensure allocates");
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("preview-clear-source"),
        });
        super::clear_view_transparent(&mut encoder, &textures.views[0], "preview-clear-source");
        queue.submit([encoder.finish()]);
    }

    /// The preview dimensions of the currently loaded target, or `(0, 0)` if
    /// nothing is loaded.
    pub fn size(&self) -> (u32, u32) {
        self.textures
            .as_ref()
            .map(|t| (t.width, t.height))
            .unwrap_or((0, 0))
    }

    /// Both views, in the ping-pong order a [`Veil`](super::veil::Veil)'s
    /// `create_cache` expects: it reads index 0 and is encoded against index 1.
    pub fn views(&self) -> &[wgpu::TextureView; 2] {
        &self.textures.as_ref().expect("load or clear first").views
    }

    pub fn source_view(&self) -> &wgpu::TextureView {
        &self.views()[0]
    }

    pub fn output_view(&self) -> &wgpu::TextureView {
        &self.views()[1]
    }

    /// The texture holding the most recently encoded frame — readback source.
    pub fn output_texture(&self) -> &wgpu::Texture {
        &self
            .textures
            .as_ref()
            .expect("load or clear first")
            .textures[1]
    }

    /// The downscaled source a mechanism reads. Test-only: nothing in the
    /// engine or the binary reads the source back, and `AGENTS.md`
    /// §No Blocking GPU Readbacks keeps readback surface behind the gate.
    #[cfg(any(test, feature = "testing"))]
    pub fn source_texture(&self) -> &wgpu::Texture {
        &self
            .textures
            .as_ref()
            .expect("load or clear first")
            .textures[0]
    }

    pub fn sampler(&self) -> &wgpu::Sampler {
        self.sampler.as_ref().expect("load or clear first")
    }

    fn ensure(&mut self, device: &wgpu::Device, src_w: u32, src_h: u32) {
        if self.sampler.is_none() {
            self.sampler = Some(device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("preview-sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }));
        }
        if self.downscale.is_none() {
            self.downscale = Some(super::effect::create_downscale_pipeline(
                device,
                PREVIEW_FORMAT,
                "preview-downscale",
            ));
        }
        let (pw, ph) = fit_preview_dims(src_w, src_h);
        let realloc = match &self.textures {
            Some(t) => t.width != pw || t.height != ph,
            None => true,
        };
        if realloc {
            self.textures = Some(make_textures(device, pw, ph));
        }
    }
}

fn make_textures(device: &wgpu::Device, width: u32, height: u32) -> PreviewTextures {
    let usage = wgpu::TextureUsages::RENDER_ATTACHMENT
        | wgpu::TextureUsages::TEXTURE_BINDING
        | wgpu::TextureUsages::COPY_SRC
        | wgpu::TextureUsages::COPY_DST;
    let (t0, v0) = super::create_texture_with_view(
        device,
        width,
        height,
        PREVIEW_FORMAT,
        "preview-source",
        usage,
    );
    let (t1, v1) = super::create_texture_with_view(
        device,
        width,
        height,
        PREVIEW_FORMAT,
        "preview-output",
        usage,
    );
    PreviewTextures {
        width,
        height,
        textures: [t0, t1],
        views: [v0, v1],
    }
}

// ---------------------------------------------------------------------------
// How a catalog answers "is this previewable, and how do I run it"
// ---------------------------------------------------------------------------

/// The registries a mechanism may need to open an entry, borrowed from whoever
/// owns them: the compositor in the engine, `docs_render::Gpu` in the binary.
/// One field per previewable catalog.
///
/// This is the one hand-written per-catalog list in the design, and it cannot
/// be generated: a session must be opened against a *concretely typed*
/// registry, so the alternatives are a downcast through `Any` — which
/// `AGENTS.md` §Type-owned dispatch forbids — or a named field. **Growth rule:**
/// a new previewable catalog costs one field here and one line each in
/// `Compositor::preview_registries` and `docs_render::Gpu`; a new
/// non-previewable catalog costs nothing.
pub struct PreviewRegistries<'a> {
    pub veils: &'a mut super::veil::VeilRegistry,
    pub voids: &'a mut super::void::VoidRegistry,
    pub filters: &'a mut super::filter::FilterPipelineRegistry,
}

/// Everything a catalog knows statically about one previewable entry.
pub struct PreviewEntry {
    /// The registry's own `'static` id, which keys long-lived state without
    /// leaking a `String` per preview.
    pub type_id: &'static str,
    pub anim: PreviewAnim,
}

/// How one catalog answers "is this previewable, and how do I open it". One
/// implementation per previewable catalog, in that catalog's own module.
pub trait PreviewMechanism {
    /// `None` for an id this catalog does not know or one that declares no
    /// preview — the single question both consumers ask before doing any work.
    /// Answerable without a device.
    fn resolve(&self, type_id: &str) -> Option<PreviewEntry>;

    /// Whether this kind reads the target's source texture. Voids generate
    /// their own content and answer `false`, which is what tells the caller to
    /// clear the source rather than load one.
    fn reads_source(&self) -> bool;

    /// Open a session for `type_id` against the registries it needs. The
    /// session owns the concrete effect instance for the rest of the sequence,
    /// which is what lets a mechanism drive its own instance without anyone
    /// recovering a concrete type from a trait object. `None` on an unknown
    /// `type_id` — an unknown entry is a no-op, never a panic.
    fn open<'a>(
        &self,
        regs: PreviewRegistries<'a>,
        type_id: &str,
    ) -> Option<Box<dyn PreviewSession + 'a>>;
}

/// One open preview, mid-sequence. Holds its catalog's concrete instance and
/// whatever registry access another frame needs.
pub trait PreviewSession {
    /// Bring the instance to the state its preview shows at `t`, building it on
    /// the first call and rebuilding whatever the new state invalidated.
    ///
    /// Absolute, like the `preview_at` it forwards to: the frame this produces
    /// depends on `t` and nothing else, which is why a sequence can be dropped
    /// and re-opened mid-run without replaying anything.
    fn set_t(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, target: &PreviewTarget, t: f32);

    /// Encode this frame: read `target.source_view()`, write
    /// `target.output_view()`.
    fn encode(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &PreviewTarget,
    );
}

// ---------------------------------------------------------------------------
// The sequence, and the driver over it
// ---------------------------------------------------------------------------

/// One preview being generated, frame by frame. Owns the session, so it also
/// owns the effect instance and the registry borrow for its whole life.
///
/// A *steppable* object rather than a closed loop, because the two consumers
/// want the same frames at different rates: the documentation binary wants all
/// of them now, the browser a bounded number per tick.
pub struct PreviewSequence<'a> {
    session: Box<dyn PreviewSession + 'a>,
    anim: PreviewAnim,
    variant: PreviewVariant,
    cursor: u32,
}

impl<'a> PreviewSequence<'a> {
    /// `None` when `mech` does not know `type_id` or it declares no preview —
    /// how an unknown entry becomes a no-op rather than a panic. Does not touch
    /// the target: loading or clearing the source is the caller's, because only
    /// the caller knows what the source *is*.
    pub fn open(
        mech: &dyn PreviewMechanism,
        regs: PreviewRegistries<'a>,
        type_id: &str,
        variant: PreviewVariant,
    ) -> Option<Self> {
        let entry = mech.resolve(type_id)?;
        Some(PreviewSequence {
            session: mech.open(regs, type_id)?,
            anim: entry.anim,
            variant,
            cursor: 0,
        })
    }

    pub fn anim(&self) -> PreviewAnim {
        self.anim
    }

    /// Frames this sequence will produce: the whole animation, or the one frame
    /// that stands for it.
    pub fn total(&self) -> u32 {
        match self.variant {
            PreviewVariant::Still => 1,
            PreviewVariant::Animated => self.anim.frames.max(1),
        }
    }

    /// Where on the timeline frame `i` of this sequence sits. The only place the
    /// two variants differ, and the reason they cannot drift apart: a still is
    /// the animation sampled at one point, not a separate rendering of it.
    fn t_at(&self, i: u32) -> f32 {
        match self.variant {
            PreviewVariant::Still => self.anim.still_at,
            PreviewVariant::Animated => frame_t(i, self.total()),
        }
    }

    pub fn is_done(&self) -> bool {
        self.cursor >= self.total()
    }

    /// Resume at frame `cursor`. Free, and that is the point: `set_t` is
    /// absolute, so a sequence re-opened mid-run reaches the same state the
    /// uninterrupted one would have without replaying a single frame.
    pub fn seek(&mut self, cursor: u32) {
        self.cursor = cursor;
    }

    /// Encode exactly one frame and hand the still-open encoder to `capture`,
    /// which owns finishing and submitting it. That is what lets the engine
    /// append a readback request into the *same* submission, so a frame's
    /// readback captures it before the next frame overwrites the texture.
    /// Answers `false` when the sequence was already complete.
    pub fn step(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &PreviewTarget,
        capture: impl FnOnce(wgpu::CommandEncoder, &wgpu::Texture, u32, u32),
    ) -> bool {
        let (i, total) = (self.cursor, self.total());
        if i >= total {
            return false;
        }
        self.session.set_t(device, queue, target, self.t_at(i));
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("preview-frame"),
        });
        self.session.encode(device, &mut encoder, target);
        capture(encoder, target.output_texture(), i, total);
        self.cursor += 1;
        true
    }
}

/// Run a sequence to completion. The blocking consumer's whole loop.
pub fn drive(
    seq: &mut PreviewSequence,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    target: &PreviewTarget,
    mut capture: impl FnMut(wgpu::CommandEncoder, &wgpu::Texture, u32, u32),
) {
    while seq.step(device, queue, target, &mut capture) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both sweeps rest at their ends and reach their extremes where the
    /// bodies that call them expect — the property every `preview_at` written
    /// against them relies on for its sequence to close.
    #[test]
    fn the_sweeps_rest_at_their_ends_and_peak_where_they_say() {
        let near = |a: f32, b: f32| (a - b).abs() < 1e-6;
        assert!(near(swing(0.0), 0.0));
        assert!(near(swing(0.5), 1.0));
        assert!(near(swing(1.0), 0.0));
        assert!(near(swing_signed(0.0), 0.0));
        assert!(near(swing_signed(0.25), 1.0));
        assert!(near(swing_signed(0.75), -1.0));
        assert!(near(swing_signed(1.0), 0.0));

        // Monotone across the rising half — without this a sweep could step
        // and still satisfy every "the frames differ" assertion downstream.
        for i in 0..(ANIMATED_FRAMES / 2) {
            let (a, b) = (
                swing(frame_t(i, ANIMATED_FRAMES)),
                swing(frame_t(i + 1, ANIMATED_FRAMES)),
            );
            assert!(a < b, "the sweep fell on its rising half at frame {i}");
        }
    }
}
