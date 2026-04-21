//! Pre-allocated pool of GPU dab textures + static textures (brush tips, etc.).
//!
//! Avoids GPU allocation during painting — dab textures are created once and
//! reused across dabs.  Static textures (brush tips, patterns) are uploaded
//! once and live until explicitly cleared.
//!
//! Both dab render targets and static textures are accessed through a unified
//! `TextureHandle`.  Handles with the high bit set (`STATIC_HANDLE_BIT`) refer
//! to static textures; all others index into the dab render target pool.
//! `bind_group(handle)` and `texture_size(handle)` work for either kind.

use super::wire::TextureHandle;

/// Maximum dab texture dimension (width = height).
/// This is the internal rendering resolution — the quality budget.
pub const MAX_DAB_SIZE: u32 = 512;

/// High bit in TextureHandle distinguishes static textures from dab render targets.
const STATIC_HANDLE_BIT: u16 = 0x8000;

/// A single pooled dab texture with its pre-built bind group.
struct DabEntry {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// Bind group for sampling this texture in the composite pass.
    bind_group: wgpu::BindGroup,
    /// Texture dimensions. Most entries are `(max_size, max_size)` from
    /// `acquire`; `acquire_sized` allocates entries with custom dimensions
    /// for previews and other one-shot renders that want texture-self-
    /// describing extents.
    width: u32,
    height: u32,
    in_use: bool,
}

/// A static texture uploaded to the GPU (brush tip, pattern, etc.).
struct StaticEntry {
    _texture: wgpu::Texture,
    _view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    width: u32,
    height: u32,
}

/// Pool of pre-allocated GPU dab textures plus static uploaded textures.
///
/// During a stroke, each dab acquires a texture (procedural/stamp node writes
/// to it), then the composite node samples it.  After `execute_gpu()` finishes
/// for one dab, all dab textures are released back to the pool.
///
/// Static textures are uploaded once (at preset load, etc.) and persist until
/// explicitly cleared.  They are read-only texture sources, while dabs are
/// write-then-read render targets.  Both are accessed through `TextureHandle`
/// (see `STATIC_HANDLE_BIT`).
pub struct DabTexturePool {
    entries: Vec<DabEntry>,
    /// Bind group layout for sampling textures (texture + sampler).
    bgl: wgpu::BindGroupLayout,
    /// Shared sampler for all texture bind groups.
    sampler: wgpu::Sampler,
    max_size: u32,
    /// Static textures (brush tips, patterns, etc.).
    /// `None` entries are tombstones — the GPU resources have been freed
    /// but the index is preserved so existing handles remain valid.
    static_entries: Vec<Option<StaticEntry>>,
}

impl DabTexturePool {
    pub fn new(device: &wgpu::Device) -> Self {
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("dab-texture-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("dab-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Self {
            entries: Vec::new(),
            bgl,
            sampler,
            max_size: MAX_DAB_SIZE,
            static_entries: Vec::new(),
        }
    }

    /// The bind group layout for texture sampling (group(1) in composite).
    pub fn bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.bgl
    }

    /// Maximum dab texture dimension (width = height).
    pub fn max_size(&self) -> u32 {
        self.max_size
    }

    // --- Dab render target pool ---

    /// Acquire a max-size (`max_size × max_size`) dab texture for rendering.
    /// Returns a handle that indexes into the pool. If no free max-size
    /// entry exists, a new texture is allocated.
    pub fn acquire(&mut self, device: &wgpu::Device) -> TextureHandle {
        self.acquire_sized(device, self.max_size, self.max_size)
    }

    /// Acquire a dab texture sized exactly `width × height`. Used by the
    /// preview path so the brush terminal can publish a texture whose own
    /// dimensions encode its canvas-pixel extent — `texture_size(handle)`
    /// returns `(width, height)`, no separate size wire needed.
    ///
    /// Reuses any free entry of the requested dimensions; otherwise
    /// allocates a fresh one. Per-hover-frame allocation amortises across
    /// the dab pool's free-list — a stable preview size will not allocate
    /// after the first hover frame.
    pub fn acquire_sized(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> TextureHandle {
        // Reuse a free entry of matching dimensions if available.
        for (i, entry) in self.entries.iter_mut().enumerate() {
            if !entry.in_use && entry.width == width && entry.height == height {
                entry.in_use = true;
                return TextureHandle(i as u16);
            }
        }

        // Allocate a new entry sized to (width, height).
        let idx = self.entries.len();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&format!("dab-texture-{idx}")),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("dab-texture-bg-{idx}")),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        self.entries.push(DabEntry {
            texture,
            view,
            bind_group,
            width,
            height,
            in_use: true,
        });
        TextureHandle(idx as u16)
    }

    /// Release all in-use dab textures back to the pool.
    ///
    /// Called after each dab's GPU passes are recorded (not submitted —
    /// the textures are still referenced by the command buffer, but wgpu
    /// handles that via internal reference counting).
    pub fn release_all(&mut self) {
        for entry in &mut self.entries {
            entry.in_use = false;
        }
    }

    /// Get the texture view for a dab handle (for use as a render target).
    pub fn view(&self, handle: TextureHandle) -> &wgpu::TextureView {
        &self.entries[handle.0 as usize].view
    }

    /// Get the raw texture for a dab handle.
    pub fn texture(&self, handle: TextureHandle) -> &wgpu::Texture {
        &self.entries[handle.0 as usize].texture
    }

    // --- Unified handle access (dab + static) ---

    /// Get the pre-built bind group for sampling any texture by handle.
    ///
    /// Panics if the handle refers to a released static texture.
    pub fn bind_group(&self, handle: TextureHandle) -> &wgpu::BindGroup {
        if handle.0 & STATIC_HANDLE_BIT != 0 {
            let idx = (handle.0 & !STATIC_HANDLE_BIT) as usize;
            &self.static_entries[idx]
                .as_ref()
                .expect("static texture released")
                .bind_group
        } else {
            &self.entries[handle.0 as usize].bind_group
        }
    }

    /// Get the pixel dimensions of any texture by handle.
    ///
    /// Dab render targets carry their per-allocation dimensions (set by
    /// `acquire` or `acquire_sized`); static textures have their natural
    /// upload dimensions.
    pub fn texture_size(&self, handle: TextureHandle) -> (u32, u32) {
        if handle.0 & STATIC_HANDLE_BIT != 0 {
            let idx = (handle.0 & !STATIC_HANDLE_BIT) as usize;
            let e = self.static_entries[idx]
                .as_ref()
                .expect("static texture released");
            (e.width, e.height)
        } else {
            let e = &self.entries[handle.0 as usize];
            (e.width, e.height)
        }
    }

    /// Check whether a static texture handle is still valid (not released).
    pub fn is_static_valid(&self, handle: TextureHandle) -> bool {
        if handle.0 & STATIC_HANDLE_BIT == 0 {
            return false;
        }
        let idx = (handle.0 & !STATIC_HANDLE_BIT) as usize;
        idx < self.static_entries.len() && self.static_entries[idx].is_some()
    }

    // --- Static texture uploads ---

    /// Upload an RGBA8 image to the GPU and return a static `TextureHandle`.
    ///
    /// The texture persists until `clear_static()` is called.  The returned
    /// handle works with `bind_group()` and `texture_size()`.
    pub fn upload_image(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &str,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> TextureHandle {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("{label}-bg")),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        // Reuse a tombstone slot if available.
        let entry = Some(StaticEntry {
            _texture: texture,
            _view: view,
            bind_group,
            width,
            height,
        });
        for (i, slot) in self.static_entries.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = entry;
                return TextureHandle(STATIC_HANDLE_BIT | i as u16);
            }
        }
        let idx = self.static_entries.len() as u16;
        self.static_entries.push(entry);
        TextureHandle(STATIC_HANDLE_BIT | idx)
    }

    /// Release a single static texture, freeing its GPU resources.
    ///
    /// The handle becomes invalid — subsequent `bind_group()` or
    /// `texture_size()` calls on it will panic.  The slot is tombstoned
    /// and may be reused by a future `upload_image()`.
    pub fn release_static(&mut self, handle: TextureHandle) {
        if handle.0 & STATIC_HANDLE_BIT == 0 {
            return;
        }
        let idx = (handle.0 & !STATIC_HANDLE_BIT) as usize;
        if idx < self.static_entries.len() {
            self.static_entries[idx] = None; // drops GPU resources
        }
    }

    /// Clear all static textures (e.g. when switching brush presets).
    pub fn clear_static(&mut self) {
        self.static_entries.clear();
    }
}
