//! Pre-allocated pool of GPU dab textures.
//!
//! Avoids GPU allocation during painting — textures are created once and
//! reused across dabs.  Each pool entry holds an RGBA8 texture at the max
//! dab size, a texture view, and a pre-built bind group for sampling in
//! the composite pass.

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

/// Pool of pre-allocated RGBA8 dab textures.
///
/// During a stroke, each dab acquires a texture (procedural node writes to
/// it), then the composite node samples it.  After `execute_gpu()` finishes
/// for one dab, all textures are released back to the pool.
///
/// The pool starts empty and grows on demand (lazy allocation).  Textures
/// are never freed — the pool holds them for the program's lifetime.
pub struct DabTexturePool {
    entries: Vec<DabEntry>,
    /// Bind group layout for sampling dab textures (texture + sampler).
    bgl: wgpu::BindGroupLayout,
    /// Shared sampler for all dab texture bind groups.
    sampler: wgpu::Sampler,
    max_size: u32,
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
}
