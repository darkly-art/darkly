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
//! The R/W hazard is internal; consumers see a single object that handles
//! the WebGPU quirk and call [`Scratch::sync_read_mirror`] with the dab
//! footprint when they need to read the in-flight scratch state.
//!
//! Ownership: owned by `StrokeBuffer`, allocated at stroke start, freed at
//! stroke end.

/// Per-dab read-mirror initial size.  1×1 is the smallest legal wgpu
/// texture; the first dab's footprint will lazy-grow it.  Picking a small
/// initial size avoids paying for layer-sized VRAM up front when most
/// strokes use brushes much smaller than the layer.
const READ_MIRROR_INITIAL_DIM: u32 = 1;

/// WebGPU minimum row alignment for `copy_buffer_to_texture` — buffer
/// rows must be a multiple of this many bytes. 256 is the spec floor.
const COPY_BYTES_PER_ROW_ALIGNMENT: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

#[inline]
fn align_up(value: u32, alignment: u32) -> u32 {
    (value + alignment - 1) & !(alignment - 1)
}

pub struct Scratch {
    // --- Write side (layer-sized) ---
    write_texture: wgpu::Texture,
    write_view: wgpu::TextureView,
    /// Bind group over `write_texture` using the dab BGL — `color_output::commit`
    /// binds this as the composite foreground (the in-flight stroke pixels)
    /// when blitting the stroke onto the layer.
    write_bind_group: wgpu::BindGroup,
    write_w: u32,
    write_h: u32,

    // --- Read mirror (footprint-sized, lazy-grown) ---
    read_mirror_texture: wgpu::Texture,
    read_mirror_view: wgpu::TextureView,
    /// Bind group over `read_mirror_texture` using the canvas-copy BGL —
    /// the per-dab composite shaders (`composite.wgsl`,
    /// `watercolor_composite.wgsl`, `liquify.wgsl`) bind this to sample the
    /// write side without an R/W hazard.
    read_mirror_bind_group: wgpu::BindGroup,
    /// Combined bind group: read_mirror at bindings 0/1 + watercolor pickup
    /// at binding 2.  Watercolor's composite pass needs both in one bind
    /// group (max_bind_groups=4 is tight).  Must be rebuilt whenever the
    /// read mirror is reallocated.
    watercolor_sources_bind_group: wgpu::BindGroup,
    read_w: u32,
    read_h: u32,

    /// Origin (in write-side / layer-local pixels) of the valid region
    /// currently in the read mirror.  Multiple GPU nodes per dab may need
    /// the same canvas region (e.g. watercolor pickup + composite); the
    /// cache lets the second caller skip a redundant copy.  Reset between
    /// dabs (via [`Scratch::reset_read_origin_cache`]) and after any
    /// resize of either side.
    read_origin_cache: Option<[u32; 2]>,

    // --- Bind-group rebuild handles (cheap clones — wgpu types are Arc'd internally) ---
    dab_bgl: wgpu::BindGroupLayout,
    canvas_copy_bgl: wgpu::BindGroupLayout,
    watercolor_sources_bgl: wgpu::BindGroupLayout,
    canvas_copy_sampler: wgpu::Sampler,
    watercolor_pickup_view: wgpu::TextureView,
    /// Linear sampler for the read mirror.  Stored so grow rebuilds can
    /// reuse it instead of allocating per grow.  Nearest would be fine for
    /// composite, but liquify reads at displaced sub-pixel UVs and needs
    /// bilinear interpolation to avoid blocky warp output.
    read_mirror_sampler: wgpu::Sampler,
    /// Sampler for the write-side bind group.  Nearest filter — no sub-
    /// pixel reads in the consumers (color_output's commit blit is
    /// integer-aligned).
    write_sampler: wgpu::Sampler,

    // --- Compute-path scratch (lazy; allocated on first ink_pen_compute use) ---
    //
    // The compute brush terminal cannot read+write the scratch texture in
    // one dispatch (same WebGPU rule that drove the read mirror above), so
    // it operates on a parallel storage buffer instead. Buffer mirrors the
    // write texture's contents, packed `rgba8unorm` as `u32` per pixel.
    // One `copy_buffer_to_texture` per pen event syncs the result back so
    // the existing fragment-path commit (`commit_brush_dab`) keeps working.
    //
    // ALTERNATIVE if storage-buffer perf is insufficient: add
    // `wgpu::TextureUsages::STORAGE_BINDING` to `write_texture` and bind
    // it as `texture_storage_2d<rgba8unorm, read_write>` directly — saves
    // both the copy and the buffer allocation. Requires
    // `Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES` (or its wgpu-28
    // equivalent) at device init. Start with the buffer path; measure
    // before opting into a feature flag.
    compute_buffer: Option<wgpu::Buffer>,
    /// Row pitch in **pixels** (= `copy.bytes_per_row / 4`). Aligned up to
    /// satisfy `COPY_BYTES_PER_ROW_ALIGNMENT` so the buffer can be source
    /// for `copy_buffer_to_texture`.
    compute_aligned_width: u32,
    /// Height in pixels of the allocated buffer (= `write_h` at alloc time).
    compute_buffer_h: u32,
}

impl Scratch {
    /// Allocate a new scratch.  Write side starts at `(layer_w, layer_h)`;
    /// read mirror starts at `1×1` and grows lazily on first dab.
    ///
    /// `dab_bgl` is the dab-pool's "texture+sampler" BGL; the write bind
    /// group uses it so the composite shader can sample the write side as
    /// the foreground at commit time.
    ///
    /// `canvas_copy_bgl` is the per-dab read BGL the brush composite
    /// shaders bind on group 2 or 3 (depending on terminal).
    ///
    /// `watercolor_sources_bgl` is the combined "read mirror + pickup" BGL
    /// the watercolor composite shader binds on group 3.
    ///
    /// `canvas_copy_sampler` is shared across the canvas-copy BGL bind
    /// groups.  Linear filter (liquify needs sub-pixel sampling).
    ///
    /// `watercolor_pickup_view` is the 1×1 pickup texture's view, owned by
    /// `BrushPipelines`.  Embedded in the watercolor sources bind group at
    /// binding 2.  Static across strokes — only the read-mirror side of
    /// the bind group needs rebuilding when the mirror grows.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        layer_w: u32,
        layer_h: u32,
        dab_bgl: &wgpu::BindGroupLayout,
        canvas_copy_bgl: &wgpu::BindGroupLayout,
        watercolor_sources_bgl: &wgpu::BindGroupLayout,
        canvas_copy_sampler: &wgpu::Sampler,
        watercolor_pickup_view: &wgpu::TextureView,
    ) -> Self {
        let write_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scratch-write-sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let read_mirror_sampler = canvas_copy_sampler.clone();

        let (write_texture, write_view) = create_write_texture(device, layer_w, layer_h);
        let write_bind_group = build_write_bind_group(device, dab_bgl, &write_view, &write_sampler);

        let (read_mirror_texture, read_mirror_view) =
            create_read_mirror_texture(device, READ_MIRROR_INITIAL_DIM, READ_MIRROR_INITIAL_DIM);
        let read_mirror_bind_group = build_read_mirror_bind_group(
            device,
            canvas_copy_bgl,
            &read_mirror_view,
            canvas_copy_sampler,
        );
        let watercolor_sources_bind_group = build_watercolor_sources_bind_group(
            device,
            watercolor_sources_bgl,
            &read_mirror_view,
            canvas_copy_sampler,
            watercolor_pickup_view,
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
            watercolor_sources_bind_group,
            read_w: READ_MIRROR_INITIAL_DIM,
            read_h: READ_MIRROR_INITIAL_DIM,
            read_origin_cache: None,
            dab_bgl: dab_bgl.clone(),
            canvas_copy_bgl: canvas_copy_bgl.clone(),
            watercolor_sources_bgl: watercolor_sources_bgl.clone(),
            canvas_copy_sampler: canvas_copy_sampler.clone(),
            watercolor_pickup_view: watercolor_pickup_view.clone(),
            read_mirror_sampler,
            write_sampler,
            compute_buffer: None,
            compute_aligned_width: 0,
            compute_buffer_h: 0,
        }
    }

    pub fn write_texture(&self) -> &wgpu::Texture {
        &self.write_texture
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
    pub fn read_mirror_texture(&self) -> &wgpu::Texture {
        &self.read_mirror_texture
    }
    pub fn watercolor_sources_bind_group(&self) -> &wgpu::BindGroup {
        &self.watercolor_sources_bind_group
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

        let new_bind_group =
            build_write_bind_group(device, &self.dab_bgl, &new_view, &self.write_sampler);

        self.write_texture = new_texture;
        self.write_view = new_view;
        self.write_bind_group = new_bind_group;
        self.write_w = target_w;
        self.write_h = target_h;
        // The cache origin was in the OLD write-side frame.  After the
        // rebase, the same origin value points at different pixels — drop it.
        self.read_origin_cache = None;
        // Compute buffer (if allocated) was sized to the old write dims;
        // drop it so the next ink_pen_compute use lazy-reallocates against
        // the new layer extent. Its contents were authoritative for the
        // in-flight stroke, but a stroke-mid layer grow is a rewind boundary
        // anyway — begin_stroke will re-fire and zero the buffer.
        self.compute_buffer = None;
        self.compute_aligned_width = 0;
        self.compute_buffer_h = 0;
    }

    // --- Compute-path scratch buffer ---

    /// Lazily allocate (or re-allocate after a layer grow) the
    /// compute-path storage buffer to match the current write-side
    /// dimensions. Idempotent when the buffer already covers the current
    /// extent. Compute terminals call this once per stroke (in
    /// `begin_stroke`); per-event commits assume it's already in place.
    pub fn ensure_compute_buffer(&mut self, device: &wgpu::Device) {
        let bytes_per_row = align_up(self.write_w * 4, COPY_BYTES_PER_ROW_ALIGNMENT);
        let aligned_width = bytes_per_row / 4;
        if self.compute_buffer.is_some()
            && self.compute_aligned_width == aligned_width
            && self.compute_buffer_h == self.write_h
        {
            return;
        }
        let total_bytes = (bytes_per_row as u64) * (self.write_h as u64);
        self.compute_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scratch-compute-buffer"),
            size: total_bytes,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        self.compute_aligned_width = aligned_width;
        self.compute_buffer_h = self.write_h;
    }

    /// The compute-path storage buffer. `None` until `ensure_compute_buffer`
    /// is called (typically by the ink-pen-compute terminal's `begin_stroke`).
    pub fn compute_buffer(&self) -> Option<&wgpu::Buffer> {
        self.compute_buffer.as_ref()
    }

    /// Row pitch of the compute buffer in **pixels** — the value to pass
    /// as `aligned_width` to the compute shader for `y * aligned_width + x`
    /// indexing.
    pub fn compute_aligned_width(&self) -> u32 {
        self.compute_aligned_width
    }

    /// Zero the compute buffer. Called at stroke start / rewind boundary
    /// alongside clearing the write texture, so the buffer's authoritative
    /// state matches the texture's cleared state.
    pub fn clear_compute_buffer(&self, encoder: &mut wgpu::CommandEncoder) {
        if let Some(buf) = &self.compute_buffer {
            encoder.clear_buffer(buf, 0, None);
        }
    }

    /// Copy a row-range of the compute buffer back into the write
    /// texture. Called once per pen event after the compute dispatch so
    /// the existing fragment-path commit (`commit_brush_dab`) can still
    /// sample the scratch as a texture. Copies whole rows (x=0..write_w)
    /// because partial-x sub-rects need `offset % 256 == 0`, which a
    /// dab-aligned bbox can't guarantee.
    pub fn sync_compute_buffer_to_texture(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        first_row: u32,
        row_count: u32,
    ) {
        let Some(buf) = &self.compute_buffer else {
            return;
        };
        if row_count == 0 || first_row >= self.write_h {
            return;
        }
        let row_count = row_count.min(self.write_h - first_row);
        let bytes_per_row = self.compute_aligned_width * 4;
        let offset = (first_row as u64) * (bytes_per_row as u64);
        encoder.copy_buffer_to_texture(
            wgpu::TexelCopyBufferInfo {
                buffer: buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(row_count),
                },
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.write_texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: first_row,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: self.write_w,
                height: row_count,
                depth_or_array_layers: 1,
            },
        );
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
        let new_watercolor_bg = build_watercolor_sources_bind_group(
            device,
            &self.watercolor_sources_bgl,
            &new_view,
            &self.canvas_copy_sampler,
            &self.watercolor_pickup_view,
        );

        self.read_mirror_texture = new_texture;
        self.read_mirror_view = new_view;
        self.read_mirror_bind_group = new_read_bg;
        self.watercolor_sources_bind_group = new_watercolor_bg;
        self.read_w = new_w;
        self.read_h = new_h;
        self.read_origin_cache = None;
    }
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
    dab_bgl: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("scratch-write-bg"),
        layout: dab_bgl,
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

fn build_watercolor_sources_bind_group(
    device: &wgpu::Device,
    watercolor_sources_bgl: &wgpu::BindGroupLayout,
    read_mirror_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    watercolor_pickup_view: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("scratch-watercolor-sources-bg"),
        layout: watercolor_sources_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(read_mirror_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(watercolor_pickup_view),
            },
        ],
    })
}
