//! Scratch: the writable stroke scratch and its read-mirror sibling.
//!
//! WebGPU forbids reading and writing the same texture in a single render
//! pass.  Brush composite shaders need both: they read existing pixels at
//! the dab's footprint (to source-over blend the new dab on top) and write
//! the blended result.  Same texture, both directions, in one pass: illegal.
//!
//! `Scratch` works around this by owning two textures:
//!
//! - **Write side** (`write_texture`): dabs render here.  Sized to the layer
//!   so every layer-local pixel a dab can land on is addressable.  Grows
//!   when the layer grows (via [`Scratch::grow_write`], driven from
//!   `painting.rs::ensure_layer_covers_dab`).  Contents preserved on grow
//!   (in-flight stroke pixels mustn't be lost).
//!
//! - **Read mirror** (`read_mirror_texture`): a per-dab snapshot of the
//!   write side under the dab's footprint.  Sized to the largest dab
//!   footprint seen this stroke; grown lazily inside [`Scratch::sync_read_mirror`]
//!   when a footprint exceeds the current size.  Never preserved across
//!   grow; overwritten by the very next sync.  Per-dab origin tracked so
//!   multiple GPU nodes per dab (color_output + watercolor pickup, etc.)
//!   share one copy.
//!
//! The two sides are managed atomically by this type: there is no public
//! API by which a caller can resize one without going through `Scratch`.
//!
//! There are two ways to read the in-flight scratch, and which one applies
//! is decided by the reader's own render target:
//!
//! - A pass that **also writes** the scratch (its color attachment is
//!   [`Scratch::write_view`]) must go through [`Scratch::sync_read_mirror`].
//!   Sampling the write side from such a pass is the R/W alias WebGPU
//!   forbids; the mirror is what makes it legal.
//! - A pass that **does not** target the scratch may sample the write side
//!   directly via [`Scratch::live_canvas_bind_group`]: no alias, no copy.
//!   Watercolor's pickup atlas pass does this: it renders to the atlas and
//!   reads the scratch to see the wet paint under each dab.
//!
//! The mirror is the more expensive of the two (a `copy_texture_to_texture`
//! per dab), so prefer the direct read whenever the target allows it.
//!
//! A third read path serves a graph that samples the stroke at *other*
//! pixels from inside a dispatch-per-dab pass: the **appearance mirror**
//! ([`Scratch::appearance_view`]), a layer-sized `rgba8unorm` texture the
//! terminal renders the stroke's appearance into under each dab's read
//! region before that dab's dispatch. It is derived per dab and never
//! cleared, checkpointed or restored.
//!
//! Ownership: owned by `StrokeBuffer`, allocated at stroke start, freed at
//! stroke end.

use crate::brush::node::DabPass;
use crate::brush::pipeline::CanvasCopyLayouts;
use crate::coord::LayerRect;

/// Per-dab read-mirror initial size.  1×1 is the smallest legal wgpu
/// texture; the first dab's footprint will lazy-grow it.  Picking a small
/// initial size avoids paying for layer-sized VRAM up front when most
/// strokes use brushes much smaller than the layer.
const READ_MIRROR_INITIAL_DIM: u32 = 1;

pub struct Scratch {
    // --- Write side (layer-sized) ---
    write_texture: wgpu::Texture,
    write_view: wgpu::TextureView,
    /// Bind group over `write_texture` using the canvas-copy BGL:
    /// paint terminals' `commit_brush_dab` bind this as the composite
    /// foreground (the in-flight stroke pixels) when blitting the
    /// stroke onto the layer.
    write_bind_group: wgpu::BindGroup,
    write_w: u32,
    write_h: u32,

    // --- Read mirror (footprint-sized, lazy-grown) ---
    read_mirror_texture: wgpu::Texture,
    read_mirror_view: wgpu::TextureView,
    /// Bind group over `read_mirror_texture` using the canvas-copy BGL:
    /// the per-dab composite shaders (`composite.wgsl`, blur,
    /// liquify) bind this to sample the write side without an
    /// R/W hazard.
    read_mirror_bind_group: wgpu::BindGroup,
    read_w: u32,
    read_h: u32,

    /// Origin (in write-side / layer-local pixels) of the valid region
    /// currently in the read mirror.  Multiple GPU nodes per dab may need
    /// the same canvas region; the cache lets the second caller skip a
    /// redundant copy.  Reset between dabs (via
    /// [`Scratch::reset_read_origin_cache`]) and after any resize of
    /// either side.
    read_origin_cache: Option<[u32; 2]>,

    // --- Bind-group rebuild handles (cheap clones since wgpu types are Arc'd internally) ---
    /// The canvas-copy layouts; every side binds through the one for its
    /// own format, so a channel of another format than the write side is
    /// readable all the same.  The float layouts carry the linear sampler
    /// the read mirror uses (liquify reads at displaced sub-pixel UVs and
    /// needs bilinear interpolation); the uint layout carries none, since
    /// nothing samples a packed ground.
    layouts: CanvasCopyLayouts,
    /// Sampler for the write-side and channel bind groups.  Nearest
    /// filter: no sub-pixel reads in the consumers (commit blit is
    /// integer-aligned).  Ignored by a layout without a sampler entry.
    write_sampler: wgpu::Sampler,
    /// Texel format of both sides.  Color terminals use `Rgba8Unorm`;
    /// warp terminals store a two-channel displacement field instead of
    /// pixels (see [`crate::brush::warp_field`]) and declare their own
    /// format on [`crate::brush::node::BrushNodeRegistration`]; a compute
    /// dab pass accumulates into a packed uint ground.
    format: wgpu::TextureFormat,
    /// How the terminal's per-dab pass writes the write side; decides its
    /// storage usage, here and on every grow.
    pass: DabPass,

    /// Per-pixel quantities a terminal accumulates alongside the write
    /// side (see [`StrokeChannels`]).  `None` until a terminal asks via
    /// [`Scratch::ensure_channels`]; terminals that declare none pay
    /// nothing.
    channels: Option<StrokeChannels>,

    /// The stroke's appearance under the dabs placed so far, rendered by
    /// the terminal under each dab's read region before that dab runs, for
    /// graphs that sample the live stroke at other pixels. Layer-sized in
    /// the write side's frame so a sampler addresses it through the paint
    /// target's extent alone; written as storage by the terminal's
    /// snapshot dispatch, sampled as an ordinary graph texture by the
    /// dab's dispatch. Derived: never cleared, checkpointed or restored,
    /// because every texel a dab reads was rewritten for that dab. `None`
    /// until a brush asks.
    appearance: Option<(wgpu::Texture, wgpu::TextureView)>,
}

/// Texel format of the appearance mirror: straight RGBA8, write-only
/// storage in core WebGPU and filterable, so the snapshot stores it and a
/// graph texture slot samples it bilinearly.
pub const APPEARANCE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// One extra per-pixel quantity a terminal accumulates over a stroke.
///
/// The write side carries coverage and nothing else: premultiplied
/// source-over saturating at 1.  A terminal that needs to *remember*
/// something per pixel across the stroke declares a channel: it becomes
/// another color attachment on the terminal's existing draw, so the
/// blend unit accumulates it under `blend` for free, with no extra pass.
///
/// The framework has no opinion on what a channel means.  `name` is the
/// terminal's own vocabulary (watercolor's is `"deposit"`) and appears
/// in the generated `FsOut` struct as the field the terminal's body
/// writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StrokeChannel {
    /// WGSL identifier for the `FsOut` field, and the debug label stem.
    pub name: &'static str,
    pub format: wgpu::TextureFormat,
    /// How the terminal writes the channel: as a colour attachment under a
    /// blend law, or as a storage texture from a compute pass.
    pub kind: ChannelUse,
}

/// How a terminal writes a [`StrokeChannel`].
///
/// The framework's bookkeeping (allocate with the write side, clear at
/// stroke start and every rewind boundary, checkpoint, restore, grow) is
/// the same for both; what differs is the texture's usage and whether the
/// channel is a colour target of the terminal's draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelUse {
    /// A colour attachment on the terminal's instanced draw, folded by the
    /// blend unit under `blend`.  Source-over gives `1 - prod(1 - a_i)`,
    /// which is order-invariant and therefore immune to how dabs are
    /// grouped into draws.  Readable from a pass that does not target it
    /// through [`Scratch::channel_bind_group`].
    Attachment { blend: wgpu::BlendState },
    /// A read-write storage texture written by a compute pass.  Not a
    /// colour target of any draw, so it contributes no `FsOut` field; the
    /// terminal binds it for writing through [`Scratch::channel_view`]
    /// and reads it back (at commit) through
    /// [`Scratch::channel_bind_group`] like any channel.
    Storage,
}

impl StrokeChannel {
    /// The blend law when the channel is a colour attachment.
    pub fn attachment_blend(&self) -> Option<wgpu::BlendState> {
        match self.kind {
            ChannelUse::Attachment { blend } => Some(blend),
            ChannelUse::Storage => None,
        }
    }
}

/// The allocated realization of a terminal's declared channels.
///
/// Managed atomically with the write side: allocated together, cleared
/// together, grown together, at the same dimensions and the same
/// canvas-anchored offset.  There is no API by which a caller can resize
/// one without the other.
///
/// A channel is written as a color attachment and read from a pass that
/// does *not* target it: watercolor's per-dab probe reads the deposit
/// while rendering to its atlas. That is an ordinary pass-to-pass
/// dependency, not the read/write alias WebGPU forbids, so a channel needs
/// no mirror.
struct StrokeChannels {
    declared: Vec<StrokeChannel>,
    textures: Vec<wgpu::Texture>,
    /// Attachment views, in declaration order: what the terminal hangs
    /// off its render pass after [`Scratch::write_view`].
    views: Vec<wgpu::TextureView>,
    /// Canvas-copy bind groups, in declaration order, each over the
    /// layout for its channel's format: what a pass that does not target
    /// a channel binds to read it.
    bind_groups: Vec<wgpu::BindGroup>,
}

impl Scratch {
    /// Allocate a new scratch.  Write side starts at `(layer_w, layer_h)`;
    /// read mirror starts at `1×1` and grows lazily on first dab.
    ///
    /// `layouts` are the canvas-copy layouts the brush composite shaders
    /// bind the read mirror, the write side (the composite shader's
    /// foreground at commit time) and each channel through, each by its
    /// own format ([`CanvasCopyLayouts::for_format`]).
    ///
    /// `format` is the terminal's declared scratch format; `pass` is how
    /// the terminal writes the write side per dab, and a compute pass
    /// needs it bound as read-write storage.
    pub fn new(
        device: &wgpu::Device,
        layer_w: u32,
        layer_h: u32,
        layouts: &CanvasCopyLayouts,
        format: wgpu::TextureFormat,
        pass: DabPass,
    ) -> Self {
        let layout = layouts.for_format(format);
        let write_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scratch-write-sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let (write_texture, write_view) =
            create_write_texture(device, layer_w, layer_h, format, pass);
        let write_bind_group = layout.bind(
            device,
            "scratch-write-bg",
            &write_view,
            Some(&write_sampler),
        );

        let (read_mirror_texture, read_mirror_view) = create_read_mirror_texture(
            device,
            READ_MIRROR_INITIAL_DIM,
            READ_MIRROR_INITIAL_DIM,
            format,
        );
        let read_mirror_bind_group =
            layout.bind(device, "scratch-read-mirror-bg", &read_mirror_view, None);

        Self {
            write_texture,
            write_view,
            write_bind_group,
            write_w: layer_w,
            write_h: layer_h,
            read_mirror_texture,
            read_mirror_view,
            read_mirror_bind_group,
            read_w: READ_MIRROR_INITIAL_DIM,
            read_h: READ_MIRROR_INITIAL_DIM,
            read_origin_cache: None,
            layouts: layouts.clone(),
            write_sampler,
            format,
            pass,
            channels: None,
            appearance: None,
        }
    }

    /// Allocate the appearance mirror at the write side's size when
    /// `wanted` and absent; free it when not wanted (a `Scratch` outlives
    /// one brush, the reason [`Scratch::ensure_channels`] frees too).
    /// Idempotent. Its contents need no clear: every texel a dab samples
    /// is rewritten for that dab first.
    pub fn ensure_appearance_mirror(&mut self, device: &wgpu::Device, wanted: bool) {
        if !wanted {
            self.appearance = None;
        } else if self.appearance.is_none() {
            self.appearance = Some(create_appearance_texture(
                device,
                self.write_w,
                self.write_h,
            ));
        }
    }

    /// The appearance mirror's view, when the brush asked for one.
    pub fn appearance_view(&self) -> Option<&wgpu::TextureView> {
        self.appearance.as_ref().map(|(_, view)| view)
    }

    /// Allocate the terminal's declared channels if they aren't already,
    /// clearing each to zero at the moment of allocation.
    ///
    /// Idempotent: safe to call every flush; only a first call, or one
    /// whose declaration differs from what is allocated, does work.
    ///
    /// Clearing here rather than relying on the stroke prologue matters:
    /// [`Lifecycle::ClearScratchToTransparent`] runs in `begin_stroke`,
    /// before any flush, so on a stroke's first flush there is nothing
    /// allocated for it to have cleared.  A rewind, by contrast, clears
    /// channels that already exist.  Clearing at allocation makes the two
    /// paths agree.
    ///
    /// [`Lifecycle::ClearScratchToTransparent`]: crate::brush::node::Lifecycle::ClearScratchToTransparent
    pub fn ensure_channels(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        declared: &[StrokeChannel],
    ) {
        if self
            .channels
            .as_ref()
            .is_some_and(|c| c.declared == declared)
        {
            return;
        }
        if declared.is_empty() {
            // A terminal that declares none frees what the last one left.
            // Scratches outlive a single brush (the preview renderer keeps
            // one across brushes), so a surviving channel would be bound by
            // whatever ran next, at the wrong format and holding the wrong
            // brush's accumulation.
            self.channels = None;
            return;
        }
        let channels = build_channels(
            device,
            &self.layouts,
            &self.write_sampler,
            self.write_w,
            self.write_h,
            declared,
        );
        clear_channel_views(encoder, &channels.views);
        self.channels = Some(channels);
    }

    /// Read bind group for the channel a terminal declared under `name`,
    /// over the canvas-copy layout for the channel's format.
    ///
    /// By name, never by position: a terminal asks for the accumulation it
    /// declared, and gets `None` only if it never declared it.
    pub fn channel_bind_group(&self, name: &str) -> Option<&wgpu::BindGroup> {
        let channels = self.channels.as_ref()?;
        let i = channels.declared.iter().position(|c| c.name == name)?;
        channels.bind_groups.get(i)
    }

    /// The view of the channel a terminal declared under `name`, for a
    /// terminal that binds it itself (a storage channel written from a
    /// compute pass).  By name, like [`Scratch::channel_bind_group`].
    pub fn channel_view(&self, name: &str) -> Option<&wgpu::TextureView> {
        let channels = self.channels.as_ref()?;
        let i = channels.declared.iter().position(|c| c.name == name)?;
        channels.views.get(i)
    }

    /// Colour attachments for a per-dab pass: the write side first, then
    /// each declared channel in order, all under `load`.
    ///
    /// The order is the one the generated `FsOut` declares and the one a
    /// terminal's pipeline targets, so the three stay in step by
    /// construction rather than by three hand-written lists agreeing.
    pub fn color_attachments(
        &self,
        load: wgpu::LoadOp<wgpu::Color>,
    ) -> Vec<Option<wgpu::RenderPassColorAttachment<'_>>> {
        std::iter::once(&self.write_view)
            .chain(self.channel_views())
            .map(|view| {
                Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load,
                        store: wgpu::StoreOp::Store,
                    },
                })
            })
            .collect()
    }

    /// Attachment views for the declared channels, in declaration order:
    /// what a terminal hangs off its render pass after
    /// [`Scratch::write_view`].  Empty when none are declared.
    pub fn channel_views(&self) -> &[wgpu::TextureView] {
        self.channels.as_ref().map_or(&[], |c| &c.views)
    }

    /// The channel textures, in declaration order.
    ///
    /// The checkpoint ring snapshots these alongside the write side: a
    /// rewind that restores the scratch but not the channels replays the
    /// post-checkpoint dabs onto a channel that already counted them.
    pub fn channel_textures(&self) -> &[wgpu::Texture] {
        self.channels.as_ref().map_or(&[], |c| &c.textures)
    }

    /// Formats of [`Scratch::channel_textures`], in the same order.
    pub fn channel_formats(&self) -> Vec<wgpu::TextureFormat> {
        self.channels
            .as_ref()
            .map_or_else(Vec::new, |c| c.declared.iter().map(|d| d.format).collect())
    }

    pub fn write_texture(&self) -> &wgpu::Texture {
        &self.write_texture
    }

    /// Texel format of both sides.
    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// Usage the write side was allocated with: attachment, copy and
    /// sampling always, plus storage under a compute dab pass.
    pub fn write_usage(&self) -> wgpu::TextureUsages {
        self.write_texture.usage()
    }

    /// Stroke-prologue helper: clear the write side to fully transparent
    /// in a single attachment-clear render pass. Used by terminals whose
    /// composite accumulates from zero (paint, watercolor); see
    /// [`crate::brush::node::Lifecycle::ClearScratchToTransparent`]. The
    /// framework calls this during `BrushGraphRunner::begin_stroke` based
    /// on the terminal's declared lifecycle, so the four terminals no
    /// longer carry a copy-pasted prologue each. A partial rewind clears a
    /// region instead: [`Scratch::clear_region`].
    pub fn clear_to_transparent(&self, encoder: &mut wgpu::CommandEncoder) {
        // Channels clear alongside the write side. A channel surviving a
        // stroke start or a rewind boundary would let dabs that no longer
        // exist keep contributing to what the next dab reads.
        let attachments = self.color_attachments(wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT));
        let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scratch-clear-transparent"),
            color_attachments: &attachments,
            ..Default::default()
        });
    }

    /// Stroke-prologue helper: copy a full-canvas pre-stroke snapshot
    /// into the write side so the eventual scratch→layer commit
    /// reproduces unchanged pixels verbatim. Used by terminals whose
    /// commit blits the entire scratch (blur, liquify); see
    /// [`crate::brush::node::Lifecycle::SeedScratchFromPreStroke`].
    ///
    /// Caller is responsible for confirming the source matches the
    /// scratch's dimensions; the copy uses the write side's own size.
    pub fn seed_from_pre_stroke(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pre_stroke: &wgpu::Texture,
    ) {
        crate::gpu::blit_region(
            encoder,
            pre_stroke,
            (0, 0),
            &self.write_texture,
            (0, 0),
            self.write_w,
            self.write_h,
        );
    }

    /// Rewind-boundary counterpart of [`Scratch::seed_from_pre_stroke`]:
    /// re-seed only `rect` (write-side local) from the pre-stroke snapshot,
    /// for a partial rewind whose every other pixel already holds the
    /// state being restored. The snapshot is layer sized like the write
    /// side, so the same rect addresses both.
    pub fn seed_region_from_pre_stroke(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pre_stroke: &wgpu::Texture,
        rect: LayerRect,
    ) {
        if rect.is_empty() {
            return;
        }
        crate::gpu::blit_region(
            encoder,
            pre_stroke,
            (rect.x0(), rect.y0()),
            &self.write_texture,
            (rect.x0(), rect.y0()),
            rect.width,
            rect.height,
        );
    }

    /// Rewind-boundary counterpart of [`Scratch::clear_to_transparent`]:
    /// zero only `rect` (write-side local) of the write side and every
    /// channel, for a partial rewind whose every other pixel already holds
    /// the state being restored. A buffer copy from `zero` rather than an
    /// attachment clear, which has no sub-rect form; see
    /// [`crate::gpu::zero_fill`].
    pub fn clear_region(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        zero: &wgpu::Buffer,
        rect: LayerRect,
    ) {
        for texture in std::iter::once(&self.write_texture).chain(self.channel_textures()) {
            crate::gpu::zero_fill::zero_fill_rect(encoder, zero, texture, rect);
        }
    }
    pub fn write_view(&self) -> &wgpu::TextureView {
        &self.write_view
    }
    pub fn write_bind_group(&self) -> &wgpu::BindGroup {
        &self.write_bind_group
    }
    pub fn read_mirror_bind_group(&self) -> &wgpu::BindGroup {
        &self.read_mirror_bind_group
    }
    /// The write side bound for sampling, for passes that read the
    /// in-flight stroke pixels **without** targeting the scratch; see the
    /// module docs for which of the two read paths applies. Callers whose
    /// render target *is* the scratch must use
    /// [`Scratch::sync_read_mirror`] instead; sampling here from such a
    /// pass is the read/write alias WebGPU forbids.
    pub fn live_canvas_bind_group(&self) -> &wgpu::BindGroup {
        &self.write_bind_group
    }
    pub fn read_mirror_texture(&self) -> &wgpu::Texture {
        &self.read_mirror_texture
    }
    pub fn write_dimensions(&self) -> (u32, u32) {
        (self.write_w, self.write_h)
    }

    /// Reset the per-dab read-origin cache.  Called by the stroke engine
    /// before each dab so the first node that needs the read mirror this
    /// dab actually issues a fresh `copy_texture_to_texture` (subsequent
    /// nodes within the same dab can reuse the same copy as long as their
    /// origin matches).
    pub fn reset_read_origin_cache(&mut self) {
        self.read_origin_cache = None;
    }

    /// Snapshot the write side under `(origin_x, origin_y, w, h)` into the
    /// read mirror at `(0, 0)`.  Lazy-grows the read mirror first if its
    /// current size doesn't fit the requested footprint.
    ///
    /// Idempotent within a dab: the first caller issues the copy;
    /// subsequent callers with matching origin are no-ops.  Mismatched
    /// origins force a fresh copy.  A grow always invalidates the cache
    /// (the new texture has no contents to reuse).
    ///
    /// `origin_x`/`origin_y` are layer-local pixels into the write side.
    pub fn sync_read_mirror(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        origin_x: u32,
        origin_y: u32,
        w: u32,
        h: u32,
    ) {
        if w == 0 || h == 0 {
            return;
        }
        // Lazy-grow before the cache check: if the texture had to grow,
        // the cache is stale anyway (a fresh allocation has no contents).
        if w > self.read_w || h > self.read_h {
            self.grow_read_mirror(device, w.max(self.read_w), h.max(self.read_h));
        }
        if self.read_origin_cache == Some([origin_x, origin_y]) {
            return;
        }
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.write_texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: origin_x,
                    y: origin_y,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.read_mirror_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.read_origin_cache = Some([origin_x, origin_y]);
    }

    /// Reallocate the write side at `(new_w, new_h)`, copying existing
    /// contents into the new texture at `(dst_offset_x, dst_offset_y)` so
    /// in-flight stroke pixels survive a layer auto-grow.  Rebuilds the
    /// write bind group.  Resets the read-origin cache because the layer-
    /// local coordinate frame has shifted.
    ///
    /// The read mirror is **not** touched: its size is footprint-driven,
    /// not layer-driven, and the layer growth doesn't change what footprint
    /// the next dab will request.  The next `sync_read_mirror` call will
    /// re-copy in the new write-side coordinate frame anyway.
    pub fn grow_write(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        new_w: u32,
        new_h: u32,
        dst_offset_x: u32,
        dst_offset_y: u32,
    ) {
        if new_w == self.write_w && new_h == self.write_h && dst_offset_x == 0 && dst_offset_y == 0
        {
            return;
        }
        let target_w = new_w.max(self.write_w);
        let target_h = new_h.max(self.write_h);

        let (new_texture, new_view) =
            create_write_texture(device, target_w, target_h, self.format, self.pass);

        // Copy existing scratch contents into the new texture at the
        // canvas-anchored offset.  Old regions outside the source rect
        // start as transparent (texture default), which is exactly the
        // pre-stroke state of pixels that didn't exist before growth.
        let had_pixels = self.write_w > 0 && self.write_h > 0;
        if had_pixels {
            crate::gpu::blit_region(
                encoder,
                &self.write_texture,
                (0, 0),
                &new_texture,
                (dst_offset_x, dst_offset_y),
                self.write_w,
                self.write_h,
            );
        }

        let new_bind_group = self.layouts.for_format(self.format).bind(
            device,
            "scratch-write-bg",
            &new_view,
            Some(&self.write_sampler),
        );

        // Channels rebase identically: same target size, same canvas-
        // anchored offset, so they stay addressable at the write side's
        // layer-local coordinates. An in-flight stroke's accumulated
        // quantities are as unrecoverable as its pixels, so contents are
        // preserved rather than recreated.
        if let Some(old) = self.channels.take() {
            let grown = build_channels(
                device,
                &self.layouts,
                &self.write_sampler,
                target_w,
                target_h,
                &old.declared,
            );
            if had_pixels {
                for (src, dst) in old.textures.iter().zip(&grown.textures) {
                    crate::gpu::blit_region(
                        encoder,
                        src,
                        (0, 0),
                        dst,
                        (dst_offset_x, dst_offset_y),
                        self.write_w,
                        self.write_h,
                    );
                }
            }
            self.channels = Some(grown);
        }

        // The appearance mirror follows the write side's frame with no
        // copy: every texel a dab reads is rewritten for that dab.
        if self.appearance.is_some() {
            self.appearance = Some(create_appearance_texture(device, target_w, target_h));
        }

        self.write_texture = new_texture;
        self.write_view = new_view;
        self.write_bind_group = new_bind_group;
        self.write_w = target_w;
        self.write_h = target_h;
        // The cache origin was in the OLD write-side frame.  After the
        // rebase, the same origin value points at different pixels; drop it.
        self.read_origin_cache = None;
    }

    /// Reallocate the read mirror at `(new_w, new_h)` and rebuild every
    /// bind group that references it.  Contents are not preserved; the
    /// next `sync_read_mirror` call re-populates from the write side.
    fn grow_read_mirror(&mut self, device: &wgpu::Device, new_w: u32, new_h: u32) {
        let (new_texture, new_view) = create_read_mirror_texture(device, new_w, new_h, self.format);

        let new_read_bg = self.layouts.for_format(self.format).bind(
            device,
            "scratch-read-mirror-bg",
            &new_view,
            None,
        );

        self.read_mirror_texture = new_texture;
        self.read_mirror_view = new_view;
        self.read_mirror_bind_group = new_read_bg;
        self.read_w = new_w;
        self.read_h = new_h;
        self.read_origin_cache = None;
    }
}

/// Allocate every declared channel plus its mirror at `(width, height)`.
///
/// Both sides are layer-sized: the accumulation texture because it must
/// stay addressable at the write side's layer-local coordinates, the
/// mirror so a dab can sample it without an origin translation.
fn build_channels(
    device: &wgpu::Device,
    layouts: &CanvasCopyLayouts,
    sampler: &wgpu::Sampler,
    width: u32,
    height: u32,
    declared: &[StrokeChannel],
) -> StrokeChannels {
    let mut textures = Vec::with_capacity(declared.len());
    let mut views = Vec::with_capacity(declared.len());
    let mut bind_groups = Vec::with_capacity(declared.len());

    for channel in declared {
        // Every channel keeps `RENDER_ATTACHMENT` so the framework's clear
        // pass can clear it as an attachment, whatever writes it later.
        let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::TEXTURE_BINDING;
        if channel.kind == ChannelUse::Storage {
            usage |= wgpu::TextureUsages::STORAGE_BINDING;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&format!("scratch-channel-{}", channel.name)),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: channel.format,
            usage,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        bind_groups.push(layouts.for_format(channel.format).bind(
            device,
            &format!("scratch-channel-{}-bg", channel.name),
            &view,
            Some(sampler),
        ));
        views.push(view);
        textures.push(texture);
    }

    StrokeChannels {
        declared: declared.to_vec(),
        textures,
        views,
        bind_groups,
    }
}

/// Zero every channel attachment in one clear pass.
fn clear_channel_views(encoder: &mut wgpu::CommandEncoder, views: &[wgpu::TextureView]) {
    if views.is_empty() {
        return;
    }
    let attachments: Vec<Option<wgpu::RenderPassColorAttachment>> = views
        .iter()
        .map(|view| {
            Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })
        })
        .collect();
    let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("scratch-channel-clear"),
        color_attachments: &attachments,
        ..Default::default()
    });
}

/// The write side: an attachment the framework clears, a copy source and
/// destination for the checkpoint ring and the grow, a sampled foreground
/// at commit, and under a compute dab pass a read-write storage texture.
fn create_write_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    pass: DabPass,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("scratch-write"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::TEXTURE_BINDING
            | pass.write_side_usage(),
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn create_appearance_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("scratch-appearance"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: APPEARANCE_FORMAT,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn create_read_mirror_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("scratch-read-mirror"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::pipeline::BrushPipelines;
    use crate::gpu::test_utils::test_device;

    fn make_scratch(
        format: wgpu::TextureFormat,
        pass: DabPass,
    ) -> (wgpu::Device, wgpu::Queue, Scratch) {
        let (device, queue) = test_device();
        let pipelines = BrushPipelines::new(
            &device,
            &queue,
            &crate::gpu::selection::selection_mask_bgl(&device),
        );
        let scratch = Scratch::new(
            &device,
            32,
            16,
            pipelines.canvas_copy_layouts(),
            format,
            pass,
        );
        (device, queue, scratch)
    }

    /// A packed ground under a dispatch per dab is read-write storage, and
    /// stays so through the framework's clear and grow; an instanced
    /// scratch never is.
    #[test]
    fn dispatch_per_dab_write_side_has_storage_usage_and_a_uint_bind_group() {
        let (device, queue, mut scratch) = make_scratch(
            crate::brush::node::PACKED_GROUND_FORMAT,
            DabPass::DispatchPerDab,
        );
        assert!(scratch
            .write_usage()
            .contains(wgpu::TextureUsages::STORAGE_BINDING));
        assert!(scratch
            .write_usage()
            .contains(wgpu::TextureUsages::RENDER_ATTACHMENT));

        let mut encoder = device.create_command_encoder(&Default::default());
        scratch.clear_to_transparent(&mut encoder);
        scratch.grow_write(&device, &mut encoder, 48, 40, 16, 24);
        queue.submit([encoder.finish()]);
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("the clear and the grow validate on a uint ground");
        assert_eq!(scratch.write_dimensions(), (48, 40));
        assert!(
            scratch
                .write_usage()
                .contains(wgpu::TextureUsages::STORAGE_BINDING),
            "the usage survives the grow"
        );
        // The write bind group is over the uint layout: binding it where a
        // float layout is expected is a validation error, so the packed
        // commit pipeline is what it pairs with.
        let _ = scratch.write_bind_group();

        let (_, _, instanced) = make_scratch(
            crate::brush::node::COLOR_SCRATCH_FORMAT,
            DabPass::InstancedDraw,
        );
        assert!(!instanced
            .write_usage()
            .contains(wgpu::TextureUsages::STORAGE_BINDING));
    }

    /// A region clear zeroes the rect on the write side and on every
    /// channel, and nothing outside it, on a packed `r32uint` ground with
    /// a storage channel (the dial's two grounds).
    #[test]
    fn clear_region_zeroes_the_rect_on_both_grounds() {
        use crate::gpu::test_utils::readback_texture;
        let (device, queue, mut scratch) = make_scratch(
            crate::brush::node::PACKED_GROUND_FORMAT,
            DabPass::DispatchPerDab,
        );
        let mut encoder = device.create_command_encoder(&Default::default());
        scratch.ensure_channels(
            &device,
            &mut encoder,
            &[StrokeChannel {
                name: "build",
                format: crate::brush::node::PACKED_GROUND_FORMAT,
                kind: ChannelUse::Storage,
            }],
        );
        queue.submit([encoder.finish()]);
        let (w, h) = scratch.write_dimensions();
        let pattern: Vec<u8> = (0..(w * h * 4)).map(|i| (i % 253) as u8 + 1).collect();
        let textures: Vec<&wgpu::Texture> = std::iter::once(scratch.write_texture())
            .chain(scratch.channel_textures())
            .collect();
        assert_eq!(textures.len(), 2);
        for texture in &textures {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &pattern,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 4),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
        }
        let rect = LayerRect::from_xywh(5, 3, 20, 10);
        let zero = crate::gpu::zero_fill::create_zero_buffer(&device);
        let mut encoder = device.create_command_encoder(&Default::default());
        scratch.clear_region(&mut encoder, &zero, rect);
        queue.submit([encoder.finish()]);
        for (name, texture) in ["write side", "build channel"].iter().zip(&textures) {
            let out = readback_texture(
                &device,
                &queue,
                texture,
                crate::brush::node::PACKED_GROUND_FORMAT,
                w,
                h,
            );
            for y in 0..h {
                for x in 0..w {
                    let inside = x >= rect.x0() && x < rect.x1() && y >= rect.y0() && y < rect.y1();
                    let i = ((y * w + x) * 4) as usize;
                    let expected = if inside {
                        [0; 4]
                    } else {
                        pattern[i..i + 4].try_into().unwrap()
                    };
                    assert_eq!(
                        &out[i..i + 4],
                        &expected,
                        "{name} at ({x}, {y}), inside: {inside}"
                    );
                }
            }
        }
    }

    /// The appearance mirror is allocated layer-sized for storage writes
    /// and filtered sampling only when asked, follows the write side
    /// through a grow, and is freed when the next brush does not ask.
    #[test]
    fn appearance_mirror_is_layer_sized_and_follows_the_write_side() {
        let (device, queue, mut scratch) = make_scratch(
            crate::brush::node::PACKED_GROUND_FORMAT,
            DabPass::DispatchPerDab,
        );
        assert!(scratch.appearance_view().is_none());
        scratch.ensure_appearance_mirror(&device, true);
        let size = |s: &Scratch| {
            let (t, _) = s.appearance.as_ref().expect("allocated");
            assert_eq!(t.format(), APPEARANCE_FORMAT);
            assert_eq!(
                t.usage(),
                wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING
            );
            (t.width(), t.height())
        };
        assert_eq!(size(&scratch), scratch.write_dimensions());

        let mut encoder = device.create_command_encoder(&Default::default());
        scratch.grow_write(&device, &mut encoder, 48, 40, 16, 24);
        queue.submit([encoder.finish()]);
        assert_eq!(size(&scratch), (48, 40));

        scratch.ensure_appearance_mirror(&device, false);
        assert!(scratch.appearance_view().is_none());
    }

    /// Every declared channel is readable through the canvas-copy layout
    /// for its own format, a storage one included: the commit reads the
    /// build ground back the same way it reads the scratch.
    #[test]
    fn storage_channels_get_a_read_bind_group() {
        let (device, _queue, mut scratch) = make_scratch(
            crate::brush::node::PACKED_GROUND_FORMAT,
            DabPass::DispatchPerDab,
        );
        let mut encoder = device.create_command_encoder(&Default::default());
        scratch.ensure_channels(
            &device,
            &mut encoder,
            &[StrokeChannel {
                name: "build",
                format: crate::brush::node::PACKED_GROUND_FORMAT,
                kind: ChannelUse::Storage,
            }],
        );
        assert!(scratch.channel_view("build").is_some());
        assert!(scratch.channel_bind_group("build").is_some());
        assert!(scratch.channel_bind_group("deposit").is_none());
    }
}
