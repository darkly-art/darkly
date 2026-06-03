//! Engine-owned registry of named GPU textures sampled by brush node graphs.
//!
//! Brush graphs that include an `image` node need a real GPU texture to
//! sample. The texture binding is inlined into the per-brush compiled
//! shader at `@group(3)` (see `crate::brush::wgsl`); at pipeline-build
//! time the registry yields the actual `wgpu::TextureView` for each
//! name the brush requested.
//!
//! The registry owns a single bilinear, wrap-repeat sampler shared by
//! every graph texture (`@group(3) @binding(0)`). Layouts are cached
//! per texture-count so two brushes requesting the same number of
//! graph textures share one `BindGroupLayout`.
//!
//! Built-in textures (paper grains, canvas, etc.) live under
//! `crates/darkly/resources/textures/`. `build.rs` scans the
//! directory and generates [`BUILTIN_TEXTURES`] (an
//! `&[(name, &[u8])]` table) at compile time — drop a `.jpg`,
//! `.png`, or `.webp` in and it shows up in the registry under its
//! file basename. No Rust edit needed.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

// `BUILTIN_TEXTURES: &[(&str, &[u8])]` — auto-generated at build time
// from every image under `crates/darkly/resources/textures/`. See
// `build.rs::generate_texture_registry`.
include!(concat!(env!("OUT_DIR"), "/textures_gen.rs"));

/// One registered texture: GPU resources plus declared dimensions.
pub struct GpuTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

/// Engine-owned registry. Constructed once at brush-pipeline init with
/// the built-in textures pre-loaded; brushes look textures up by name
/// at compile + pipeline-build time.
pub struct TextureRegistry {
    textures: HashMap<String, Arc<GpuTexture>>,
    sampler: wgpu::Sampler,
    /// Bind-group layouts keyed by texture count. Brushes requesting
    /// N graph textures share the layout for N.
    layouts: RefCell<HashMap<usize, Arc<wgpu::BindGroupLayout>>>,
}

impl TextureRegistry {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("brush-graph-texture-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            ..Default::default()
        });
        let mut reg = Self {
            textures: HashMap::new(),
            sampler,
            layouts: RefCell::new(HashMap::new()),
        };
        register_builtin_textures(&mut reg, device, queue);
        reg
    }

    /// Decode an RGBA8 image from raw bytes and register it under `name`.
    /// Overwrites any prior registration with the same name.
    pub fn register_rgba8(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) {
        debug_assert_eq!(
            rgba.len(),
            (width * height * 4) as usize,
            "rgba length must equal width * height * 4"
        );
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&format!("brush-graph-texture/{name}")),
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
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.textures.insert(
            name.to_string(),
            Arc::new(GpuTexture {
                texture,
                view,
                width,
                height,
            }),
        );
    }

    /// Decode a PNG / JPG byte buffer and register it. Errors fall
    /// through `log::warn` so a missing or corrupt asset doesn't take
    /// the engine down — the name just stays unregistered and any
    /// brush referencing it fails to load with a clear message.
    pub fn register_image(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        bytes: &[u8],
    ) {
        match image::load_from_memory(bytes) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                self.register_rgba8(device, queue, name, w, h, &rgba);
            }
            Err(e) => {
                log::warn!("TextureRegistry: failed to decode `{name}`: {e}");
            }
        }
    }

    pub fn contains(&self, name: &str) -> bool {
        self.textures.contains_key(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.textures.keys().map(String::as_str)
    }

    /// The shared bilinear / repeat sampler bound at
    /// `@group(3) @binding(0)`.
    pub fn sampler(&self) -> &wgpu::Sampler {
        &self.sampler
    }

    /// Bind-group layout for a brush requesting `n` graph textures.
    /// `@binding(0)` is the shared sampler, `@binding(1..=n)` is one
    /// `texture_2d<f32>` per requested texture. Cached per `n`.
    pub fn layout_for_count(&self, device: &wgpu::Device, n: usize) -> Arc<wgpu::BindGroupLayout> {
        let mut layouts = self.layouts.borrow_mut();
        layouts
            .entry(n)
            .or_insert_with(|| {
                let mut entries: Vec<wgpu::BindGroupLayoutEntry> = Vec::with_capacity(n + 1);
                entries.push(wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                });
                for i in 0..n {
                    entries.push(wgpu::BindGroupLayoutEntry {
                        binding: 1 + i as u32,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    });
                }
                Arc::new(
                    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                        label: Some(&format!("brush-graph-textures-bgl-{n}")),
                        entries: &entries,
                    }),
                )
            })
            .clone()
    }

    /// Build the `@group(3)` bind group for a compiled brush's
    /// `graph_texture_names`. Missing names — typing in the editor,
    /// a brush referencing a yet-to-be-registered texture — fall
    /// back to the built-in `_fallback` 1×1 white tile so the
    /// brush keeps rendering something coherent and pipelines never
    /// fail to build. A `log::warn` records the miss so authors see
    /// what happened in the console.
    pub fn make_bind_group(
        &self,
        device: &wgpu::Device,
        names: &[String],
    ) -> (Arc<wgpu::BindGroupLayout>, wgpu::BindGroup) {
        let fallback = self
            .textures
            .get(FALLBACK_TEXTURE)
            .expect("TextureRegistry must register _fallback at init");
        let textures: Vec<&Arc<GpuTexture>> = names
            .iter()
            .map(|n| {
                self.textures.get(n).unwrap_or_else(|| {
                    log::warn!("TextureRegistry: no texture `{n}`, substituting `_fallback`",);
                    fallback
                })
            })
            .collect();
        let layout = self.layout_for_count(device, names.len());
        let mut entries: Vec<wgpu::BindGroupEntry> = Vec::with_capacity(names.len() + 1);
        entries.push(wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Sampler(&self.sampler),
        });
        for (i, t) in textures.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: 1 + i as u32,
                resource: wgpu::BindingResource::TextureView(&t.view),
            });
        }
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brush-graph-textures-bg"),
            layout: &layout,
            entries: &entries,
        });
        (layout, bg)
    }
}

/// Name of the built-in 1×1 white texture used when a brush
/// references a texture name that isn't registered. Lets the
/// per-brush pipeline build succeed even while the user is
/// mid-typing a texture name in the node editor.
pub const FALLBACK_TEXTURE: &str = "_fallback";

/// Register every built-in texture shipped with Darkly. Called once at
/// `TextureRegistry::new` time. Failures inside `register_image` are
/// logged and skipped — brushes referencing a missing texture surface
/// a clear error at load time rather than crashing engine init.
fn register_builtin_textures(
    reg: &mut TextureRegistry,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) {
    // Always-available 1×1 white fallback for unresolved texture
    // names — must be registered before any brush asks for it.
    reg.register_rgba8(
        device,
        queue,
        FALLBACK_TEXTURE,
        1,
        1,
        &[255u8, 255, 255, 255],
    );

    // Everything under `crates/darkly/resources/textures/` —
    // discovered + embedded by `build.rs`.
    for (name, bytes) in BUILTIN_TEXTURES {
        reg.register_image(device, queue, name, bytes);
    }
}
