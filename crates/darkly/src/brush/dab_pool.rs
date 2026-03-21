//! Pre-allocated pool of GPU dab textures + cached brush tip textures.
//!
//! Avoids GPU allocation during painting — dab textures are created once and
//! reused across dabs.  Brush tip textures are uploaded once on brush load
//! and cached by name.

use std::collections::HashMap;

use super::wire::TextureHandle;

/// Maximum dab texture dimension (width = height).
pub const MAX_DAB_SIZE: u32 = 512;

/// A single pooled dab texture with its pre-built bind group.
struct DabEntry {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// Bind group for sampling this texture in the composite pass.
    bind_group: wgpu::BindGroup,
    in_use: bool,
}

/// A cached brush tip texture uploaded to the GPU.
struct BrushTipEntry {
    _texture: wgpu::Texture,
    _view: wgpu::TextureView,
    /// Bind group for sampling this tip in the stamp pass (same BGL as dab).
    bind_group: wgpu::BindGroup,
    width: u32,
    height: u32,
}

/// Pool of pre-allocated GPU dab textures plus cached brush tip textures.
///
/// During a stroke, each dab acquires a texture (procedural/stamp node writes
/// to it), then the composite node samples it.  After `execute_gpu()` finishes
/// for one dab, all textures are released back to the pool.
///
/// Brush tip textures are uploaded once on brush load and cached by name.
/// They are separate from dab render targets — tips are read-only texture
/// sources, dabs are write-then-read render targets.
pub struct DabTexturePool {
    entries: Vec<DabEntry>,
    /// Bind group layout for sampling dab textures (texture + sampler).
    bgl: wgpu::BindGroupLayout,
    /// Shared sampler for all dab texture bind groups.
    sampler: wgpu::Sampler,
    max_size: u32,
    /// Cached brush tip textures, keyed by resource name.
    tip_cache: HashMap<String, BrushTipEntry>,
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
            tip_cache: HashMap::new(),
        }
    }

    /// The bind group layout for dab texture sampling (group(1) in composite).
    pub fn bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.bgl
    }

    /// Maximum texture dimension (width = height).
    pub fn max_size(&self) -> u32 {
        self.max_size
    }

    /// Acquire a dab texture for rendering.  Returns a handle that indexes
    /// into the pool.  If no free entries exist, a new texture is allocated.
    pub fn acquire(&mut self, device: &wgpu::Device) -> TextureHandle {
        // Reuse a free entry if available.
        for (i, entry) in self.entries.iter_mut().enumerate() {
            if !entry.in_use {
                entry.in_use = true;
                return TextureHandle(i as u16);
            }
        }

        // Allocate a new entry.
        let idx = self.entries.len();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&format!("dab-texture-{idx}")),
            size: wgpu::Extent3d {
                width: self.max_size,
                height: self.max_size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING,
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
            in_use: true,
        });
        TextureHandle(idx as u16)
    }

    /// Release all in-use textures back to the pool.
    ///
    /// Called after each dab's GPU passes are recorded (not submitted —
    /// the textures are still referenced by the command buffer, but wgpu
    /// handles that via internal reference counting).
    pub fn release_all(&mut self) {
        for entry in &mut self.entries {
            entry.in_use = false;
        }
    }

    /// Get the texture view for a handle (for use as a render target).
    pub fn view(&self, handle: TextureHandle) -> &wgpu::TextureView {
        &self.entries[handle.0 as usize].view
    }

    /// Get the raw texture for a handle.
    pub fn texture(&self, handle: TextureHandle) -> &wgpu::Texture {
        &self.entries[handle.0 as usize].texture
    }

    /// Get the pre-built bind group for sampling a dab texture in the
    /// composite pass.
    pub fn bind_group(&self, handle: TextureHandle) -> &wgpu::BindGroup {
        &self.entries[handle.0 as usize].bind_group
    }

    // --- Brush tip texture cache ---

    /// Upload a brush tip image (RGBA8 bytes) and cache it by name.
    ///
    /// Called once when a brush preset is loaded.  The resulting bind group
    /// is used by the stamp node to sample the tip texture during dab
    /// generation.  If a tip with the same name already exists, it is
    /// replaced.
    pub fn upload_tip(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&format!("brush-tip-{name}")),
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
            label: Some(&format!("brush-tip-bg-{name}")),
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

        self.tip_cache.insert(
            name.to_string(),
            BrushTipEntry {
                _texture: texture,
                _view: view,
                bind_group,
                width,
                height,
            },
        );
    }

    /// Get the bind group for a cached brush tip texture.
    ///
    /// Returns `None` if the tip hasn't been uploaded.
    pub fn tip_bind_group(&self, name: &str) -> Option<&wgpu::BindGroup> {
        self.tip_cache.get(name).map(|e| &e.bind_group)
    }

    /// Get the dimensions of a cached brush tip texture.
    pub fn tip_size(&self, name: &str) -> Option<(u32, u32)> {
        self.tip_cache.get(name).map(|e| (e.width, e.height))
    }

    /// Check if a brush tip is cached.
    pub fn has_tip(&self, name: &str) -> bool {
        self.tip_cache.contains_key(name)
    }

    /// Clear all cached brush tips (e.g. when switching brush presets).
    pub fn clear_tips(&mut self) {
        self.tip_cache.clear();
    }
}
