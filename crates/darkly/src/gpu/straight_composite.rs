//! Shader-side straight-alpha compositing utilities.
//!
//! Hardware alpha blending (`SrcAlpha`/`OneMinusSrcAlpha`) produces premultiplied
//! output, which corrupts straight-alpha layer textures. Any render pass that
//! composites onto a straight-alpha target must:
//!
//! 1. Copy the destination region to a temp texture
//! 2. Have the shader read both source and dest, compute Porter-Duff manually
//!    using the `source_over()` function from `shaders/source_over.wgsl`
//! 3. Output with REPLACE blend (`blend: None`)
//!
//! The WGSL `source_over()` function is included via `concat!` at shader load:
//! ```ignore
//! concat!(include_str!("source_over.wgsl"), "\n", include_str!("my_shader.wgsl"))
//! ```
//!
//! This module provides the Rust-side utilities for step 1.
//! See `compositing-lessons-learned.md` #4 for the full rationale.

/// Create a bind group layout with a single `texture_2d<f32>` at binding 0.
///
/// Used for destination copy textures (read via `textureLoad` in the shader)
/// and for the premultiply conversion pass. Shared across systems to avoid
/// duplicating the same single-texture BGL definition.
pub fn single_texture_bind_group_layout(
    device: &wgpu::Device,
    label: &str,
) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    })
}

/// Copy a texture and create a bind group for shader-side compositing.
///
/// Creates a temporary texture matching the source's size and format,
/// copies the full texture into it, and returns a bind group with the copy
/// at binding 0. The shader reads this via `textureLoad` to get the
/// destination values for manual Porter-Duff blending.
pub fn copy_for_compositing(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    layout: &wgpu::BindGroupLayout,
    texture: &wgpu::Texture,
    format: wgpu::TextureFormat,
) -> wgpu::BindGroup {
    let size = texture.size();
    let copy = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("dest-copy"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyTextureInfo {
            texture: &copy,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        size,
    );
    let view = copy.create_view(&wgpu::TextureViewDescriptor::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("dest-copy-bg"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&view),
        }],
    })
}
