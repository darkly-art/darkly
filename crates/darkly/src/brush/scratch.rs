//! Scratch — the writable stroke scratch and its read-mirror sibling.
//!
//! WebGPU forbids reading and writing the same texture in a single render
//! pass.  Brush composite shaders need both: they read existing pixels at
//! the dab's footprint (to source-over blend the new dab on top) and write
//! the blended result.  Same texture, both directions, in one pass — illegal.
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
//!   grow — overwritten by the very next sync.  Per-dab origin tracked so
//!   multiple GPU nodes per dab (color_output + watercolor pickup, etc.)
//!   share one copy.
//!
//! The two sides are managed atomically by this type — there is no public
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
//!   directly via [`Scratch::live_canvas_bind_group`] — no alias, no copy.
//!   Watercolor's pickup atlas pass does this: it renders to the atlas and
//!   reads the scratch to see the wet paint under each dab.
//!
//! The mirror is the more expensive of the two (a `copy_texture_to_texture`
//! per dab), so prefer the direct read whenever the target allows it.
//!
//! Ownership: owned by `StrokeBuffer`, allocated at stroke start, freed at
//! stroke end.

/// Per-dab read-mirror initial size.  1×1 is the smallest legal wgpu
/// texture; the first dab's footprint will lazy-grow it.  Picking a small
/// initial size avoids paying for layer-sized VRAM up front when most
/// strokes use brushes much smaller than the layer.
const READ_MIRROR_INITIAL_DIM: u32 = 1;

pub struct Scratch {
    // --- Write side (layer-sized) ---
    write_texture: wgpu::Texture,
    write_view: wgpu::TextureView,
    /// Bind group over `write_texture` using the canvas-copy BGL —
    /// paint terminals' `commit_brush_dab` bind this as the composite
    /// foreground (the in-flight stroke pixels) when blitting the
    /// stroke onto the layer.
    write_bind_group: wgpu::BindGroup,
    write_w: u32,
    write_h: u32,

    // --- Read mirror (footprint-sized, lazy-grown) ---
    read_mirror_texture: wgpu::Texture,
    read_mirror_view: wgpu::TextureView,
    /// Bind group over `read_mirror_texture` using the canvas-copy BGL —
    /// the per-dab composite shaders (`composite.wgsl`, smudge,
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

    // --- Bind-group rebuild handles (cheap clones — wgpu types are Arc'd internally) ---
    canvas_copy_bgl: wgpu::BindGroupLayout,
    /// Linear sampler for the read mirror.  Stored so grow rebuilds can
    /// reuse it instead of allocating per grow.  Liquify reads at
    /// displaced sub-pixel UVs and needs bilinear interpolation.
    read_mirror_sampler: wgpu::Sampler,
    /// Sampler for the write-side bind group.  Nearest filter — no sub-
    /// pixel reads in the consumers (commit blit is integer-aligned).
    write_sampler: wgpu::Sampler,

    /// Per-pixel quantities a terminal accumulates alongside the write
    /// side (see [`StrokeChannels`]).  `None` until a terminal asks via
    /// [`Scratch::ensure_channels`]; terminals that declare none pay
    /// nothing.
    channels: Option<StrokeChannels>,
}

/// One extra per-pixel quantity a terminal accumulates over a stroke.
///
/// The write side carries coverage and nothing else — premultiplied
/// source-over saturating at 1.  A terminal that needs to *remember*
/// something per pixel across the stroke declares a channel: it becomes
/// another colour attachment on the terminal's existing draw, so the
/// blend unit accumulates it under `blend` for free, with no extra pass.
///
/// The framework has no opinion on what a channel means.  `name` is the
/// terminal's own vocabulary — watercolor's is `"deposit"` — and appears
/// in the generated `FsOut` struct as the field the terminal's body
/// writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StrokeChannel {
    /// WGSL identifier for the `FsOut` field, and the debug label stem.
    pub name: &'static str,
    pub format: wgpu::TextureFormat,
    /// How the blend unit folds each dab's contribution into the running
    /// value.  Source-over gives `1 − Π(1−aᵢ)`, which is order-invariant
    /// and therefore immune to how dabs are grouped into draws.
    pub blend: wgpu::BlendState,
}

/// The allocated realization of a terminal's declared channels.
///
/// Managed atomically with the write side: allocated together, cleared
/// together, grown together, at the same dimensions and the same
/// canvas-anchored offset.  There is no API by which a caller can resize
/// one without the other.
///
/// Each channel carries a mirror as well as its accumulation texture,
/// because a fragment cannot sample the attachment it is blending into.
/// The mirror is layer-sized so a dab reads it at plain layer-local
/// coordinates with no origin translation; only the flush's bbox is
/// copied into it, so the per-flush cost stays proportional to what the
/// flush actually touched.
struct StrokeChannels {
    declared: Vec<StrokeChannel>,
    textures: Vec<wgpu::Texture>,
    /// Attachment views, in declaration order — what the terminal hangs
    /// off its render pass after [`Scratch::write_view`].
    views: Vec<wgpu::TextureView>,
    mirrors: Vec<wgpu::Texture>,
    mirror_views: Vec<wgpu::TextureView>,
}

impl Scratch {
    /// Allocate a new scratch.  Write side starts at `(layer_w, layer_h)`;
    /// read mirror starts at `1×1` and grows lazily on first dab.
    ///
    /// `canvas_copy_bgl` is the per-dab read BGL the brush composite
    /// shaders bind for the read mirror; the same BGL also holds the
    /// write-side bind group (the composite shader's foreground at
    /// commit time).
    ///
    /// `canvas_copy_sampler` is shared across the canvas-copy BGL bind
    /// groups.  Linear filter (liquify needs sub-pixel sampling).
    pub fn new(
        device: &wgpu::Device,
        layer_w: u32,
        layer_h: u32,
        canvas_copy_bgl: &wgpu::BindGroupLayout,
        canvas_copy_sampler: &wgpu::Sampler,
    ) -> Self {
        let write_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scratch-write-sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let read_mirror_sampler = canvas_copy_sampler.clone();

        let (write_texture, write_view) = create_write_texture(device, layer_w, layer_h);
        let write_bind_group =
            build_write_bind_group(device, canvas_copy_bgl, &write_view, &write_sampler);

        let (read_mirror_texture, read_mirror_view) =
            create_read_mirror_texture(device, READ_MIRROR_INITIAL_DIM, READ_MIRROR_INITIAL_DIM);
        let read_mirror_bind_group = build_read_mirror_bind_group(
            device,
            canvas_copy_bgl,
            &read_mirror_view,
            canvas_copy_sampler,
        );

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
            canvas_copy_bgl: canvas_copy_bgl.clone(),
            read_mirror_sampler,
            write_sampler,
            channels: None,
        }
    }

    /// Allocate the terminal's declared channels if they aren't already,
    /// clearing each to zero at the moment of allocation.
    ///
    /// Idempotent — safe to call every flush; only a first call, or one
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
        if declared.is_empty() {
            return;
        }
        if self
            .channels
            .as_ref()
            .is_some_and(|c| c.declared == declared)
        {
            return;
        }
        let channels = build_channels(device, self.write_w, self.write_h, declared);
        clear_channel_views(encoder, &channels.views);
        self.channels = Some(channels);
    }

    /// Attachment views for the declared channels, in declaration order —
    /// what a terminal hangs off its render pass after
    /// [`Scratch::write_view`].  Empty when none are declared.
    pub fn channel_views(&self) -> &[wgpu::TextureView] {
        self.channels.as_ref().map_or(&[], |c| &c.views)
    }

    /// Mirror views, in declaration order — what a terminal binds to
    /// *read* a channel while writing its attachment.  Layer-sized, so a
    /// dab samples at layer-local coordinates directly.
    pub fn channel_mirror_views(&self) -> &[wgpu::TextureView] {
        self.channels.as_ref().map_or(&[], |c| &c.mirror_views)
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

    /// Refresh each channel's mirror over `(origin_x, origin_y, w, h)` in
    /// layer-local pixels — the union bbox of the flush about to draw.
    ///
    /// Deliberately **not** origin-cached, unlike [`Scratch::sync_read_mirror`].
    /// That cache is sound only because the stroke engine resets it before
    /// every dab; a channel is written between flushes by the terminal's
    /// draw *and*, out of band, by a checkpoint restore that copies into it
    /// from outside `Scratch` entirely.  A repeated bbox origin — what a
    /// dwelling stroke produces — would then be served a stale mirror.
    pub fn sync_channel_mirrors(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        origin_x: u32,
        origin_y: u32,
        w: u32,
        h: u32,
    ) {
        let Some(channels) = self.channels.as_ref() else {
            return;
        };
        let w = w.min(self.write_w.saturating_sub(origin_x));
        let h = h.min(self.write_h.saturating_sub(origin_y));
        if w == 0 || h == 0 {
            return;
        }
        for (texture, mirror) in channels.textures.iter().zip(&channels.mirrors) {
            copy_region(encoder, texture, mirror, origin_x, origin_y, w, h);
        }
    }

    pub fn write_texture(&self) -> &wgpu::Texture {
        &self.write_texture
    }

    /// Stroke-prologue helper: clear the write side to fully transparent
    /// in a single attachment-clear render pass. Used by terminals whose
    /// composite accumulates from zero (paint, watercolor) — see
    /// [`crate::brush::node::Lifecycle::ClearScratchToTransparent`]. The
    /// framework calls this during `BrushGraphRunner::begin_stroke` based
    /// on the terminal's declared lifecycle, so the four terminals no
    /// longer carry a copy-pasted prologue each.
    pub fn clear_to_transparent(&self, encoder: &mut wgpu::CommandEncoder) {
        // Channels clear alongside the write side. A channel surviving a
        // stroke start or a rewind boundary would let dabs that no longer
        // exist keep contributing to what the next dab reads.
        let mut attachments: Vec<Option<wgpu::RenderPassColorAttachment>> =
            Vec::with_capacity(1 + self.channel_views().len());
        for view in std::iter::once(&self.write_view).chain(self.channel_views()) {
            attachments.push(Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            }));
        }
        let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scratch-clear-transparent"),
            color_attachments: &attachments,
            ..Default::default()
        });
    }

    /// Stroke-prologue helper: copy a full-canvas pre-stroke snapshot
    /// into the write side so the eventual scratch→layer commit
    /// reproduces unchanged pixels verbatim. Used by terminals whose
    /// commit blits the entire scratch (smudge, liquify) — see
    /// [`crate::brush::node::Lifecycle::SeedScratchFromPreStroke`].
    ///
    /// Caller is responsible for confirming the source matches the
    /// scratch's dimensions; the copy uses the write side's own size.
    pub fn seed_from_pre_stroke(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pre_stroke: &wgpu::Texture,
    ) {
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: pre_stroke,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.write_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: self.write_w,
                height: self.write_h,
                depth_or_array_layers: 1,
            },
        );
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
    /// in-flight stroke pixels **without** targeting the scratch — see the
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

        let (new_texture, new_view) = create_write_texture(device, target_w, target_h);

        // Copy existing scratch contents into the new texture at the
        // canvas-anchored offset.  Old regions outside the source rect
        // start as transparent (texture default), which is exactly the
        // pre-stroke state of pixels that didn't exist before growth.
        if self.write_w > 0 && self.write_h > 0 {
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.write_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &new_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: dst_offset_x,
                        y: dst_offset_y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: self.write_w,
                    height: self.write_h,
                    depth_or_array_layers: 1,
                },
            );
        }

        let new_bind_group = build_write_bind_group(
            device,
            &self.canvas_copy_bgl,
            &new_view,
            &self.write_sampler,
        );

        // Channels rebase identically — same target size, same canvas-
        // anchored offset — so they stay addressable at the write side's
        // layer-local coordinates. An in-flight stroke's accumulated
        // quantities are as unrecoverable as its pixels, so contents are
        // preserved rather than recreated.
        if let Some(old) = self.channels.take() {
            let grown = build_channels(device, target_w, target_h, &old.declared);
            for (src, dst) in old.textures.iter().zip(&grown.textures) {
                copy_region_offset(
                    encoder,
                    src,
                    dst,
                    self.write_w,
                    self.write_h,
                    dst_offset_x,
                    dst_offset_y,
                );
            }
            self.channels = Some(grown);
        }

        self.write_texture = new_texture;
        self.write_view = new_view;
        self.write_bind_group = new_bind_group;
        self.write_w = target_w;
        self.write_h = target_h;
        // The cache origin was in the OLD write-side frame.  After the
        // rebase, the same origin value points at different pixels — drop it.
        self.read_origin_cache = None;
    }

    /// Reallocate the read mirror at `(new_w, new_h)` and rebuild every
    /// bind group that references it.  Contents are not preserved; the
    /// next `sync_read_mirror` call re-populates from the write side.
    fn grow_read_mirror(&mut self, device: &wgpu::Device, new_w: u32, new_h: u32) {
        let (new_texture, new_view) = create_read_mirror_texture(device, new_w, new_h);

        let new_read_bg = build_read_mirror_bind_group(
            device,
            &self.canvas_copy_bgl,
            &new_view,
            &self.read_mirror_sampler,
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
    width: u32,
    height: u32,
    declared: &[StrokeChannel],
) -> StrokeChannels {
    let mut textures = Vec::with_capacity(declared.len());
    let mut views = Vec::with_capacity(declared.len());
    let mut mirrors = Vec::with_capacity(declared.len());
    let mut mirror_views = Vec::with_capacity(declared.len());

    for channel in declared {
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
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let mirror = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&format!("scratch-channel-{}-mirror", channel.name)),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: channel.format,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        views.push(texture.create_view(&wgpu::TextureViewDescriptor::default()));
        mirror_views.push(mirror.create_view(&wgpu::TextureViewDescriptor::default()));
        textures.push(texture);
        mirrors.push(mirror);
    }

    StrokeChannels {
        declared: declared.to_vec(),
        textures,
        views,
        mirrors,
        mirror_views,
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

/// Copy `(origin, w, h)` from `src` to the same coordinates in `dst`.
fn copy_region(
    encoder: &mut wgpu::CommandEncoder,
    src: &wgpu::Texture,
    dst: &wgpu::Texture,
    origin_x: u32,
    origin_y: u32,
    w: u32,
    h: u32,
) {
    let origin = wgpu::Origin3d {
        x: origin_x,
        y: origin_y,
        z: 0,
    };
    encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: src,
            mip_level: 0,
            origin,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyTextureInfo {
            texture: dst,
            mip_level: 0,
            origin,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
}

/// Copy all of `src` into `dst` at a canvas-anchored destination offset —
/// the growth rebase, matching [`Scratch::grow_write`]'s own blit.
fn copy_region_offset(
    encoder: &mut wgpu::CommandEncoder,
    src: &wgpu::Texture,
    dst: &wgpu::Texture,
    w: u32,
    h: u32,
    dst_offset_x: u32,
    dst_offset_y: u32,
) {
    if w == 0 || h == 0 {
        return;
    }
    encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: src,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyTextureInfo {
            texture: dst,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: dst_offset_x,
                y: dst_offset_y,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
}

fn create_write_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
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
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn create_read_mirror_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
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
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn build_write_bind_group(
    device: &wgpu::Device,
    canvas_copy_bgl: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("scratch-write-bg"),
        layout: canvas_copy_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

fn build_read_mirror_bind_group(
    device: &wgpu::Device,
    canvas_copy_bgl: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("scratch-read-mirror-bg"),
        layout: canvas_copy_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}
