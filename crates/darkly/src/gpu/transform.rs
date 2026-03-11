//! Floating content GPU pipeline — transform-blend shader, texture management,
//! and CPU-side affine rasterization for commit.
//!
//! Used by both paste-in-place and the interactive transform tool. The GPU
//! texture provides real-time preview during interaction; the CPU source tiles
//! are used for the final commit (avoiding async GPU readback).

use crate::layer::LayerId;
use crate::tile::{TileGrid, TileStore, AlphaMask, AlphaF32, TILE_SIZE};

// ---------------------------------------------------------------------------
// Affine matrix helpers  ([a, b, tx, c, d, ty])
// ---------------------------------------------------------------------------

/// 2D affine matrix stored as [a, b, tx, c, d, ty].
/// Transforms point (x,y) → (a*x + b*y + tx, c*x + d*y + ty).
pub type Affine2D = [f32; 6];

/// Identity affine: no transformation.
pub const IDENTITY: Affine2D = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];

/// Compute the inverse of a 2D affine matrix.
/// Returns None if the matrix is singular (det ≈ 0).
pub fn affine_inverse(m: &Affine2D) -> Option<Affine2D> {
    let [a, b, tx, c, d, ty] = *m;
    let det = a * d - b * c;
    if det.abs() < 1e-12 {
        return None;
    }
    let inv_det = 1.0 / det;
    Some([
        d * inv_det,
        -b * inv_det,
        (b * ty - d * tx) * inv_det,
        -c * inv_det,
        a * inv_det,
        (c * tx - a * ty) * inv_det,
    ])
}

/// Transform a point by an affine matrix.
pub fn affine_transform(m: &Affine2D, x: f32, y: f32) -> (f32, f32) {
    let [a, b, tx, c, d, ty] = *m;
    (a * x + b * y + tx, c * x + d * y + ty)
}

/// Multiply two affine matrices: result = a ∘ b (apply b first, then a).
pub fn affine_multiply(a: &Affine2D, b: &Affine2D) -> Affine2D {
    [
        a[0] * b[0] + a[1] * b[3],
        a[0] * b[1] + a[1] * b[4],
        a[0] * b[2] + a[1] * b[5] + a[2],
        a[3] * b[0] + a[4] * b[3],
        a[3] * b[1] + a[4] * b[4],
        a[3] * b[2] + a[4] * b[5] + a[5],
    ]
}

/// Build a translation affine.
pub fn affine_translate(tx: f32, ty: f32) -> Affine2D {
    [1.0, 0.0, tx, 0.0, 1.0, ty]
}

/// Build a scale affine.
pub fn affine_scale(sx: f32, sy: f32) -> Affine2D {
    [sx, 0.0, 0.0, 0.0, sy, 0.0]
}

/// Build a rotation affine (angle in radians, CCW).
pub fn affine_rotate(angle: f32) -> Affine2D {
    let (s, c) = angle.sin_cos();
    [c, -s, 0.0, s, c, 0.0]
}

// ---------------------------------------------------------------------------
// FloatingContent — CPU-side data owned by the engine
// ---------------------------------------------------------------------------

/// How the floating content was created — determines commit/cancel behavior.
pub enum FloatingMode {
    /// Clipboard paste — commit composites INTO target. Cancel = no-op.
    Paste,
    /// Extracted from layer — commit writes transformed pixels.
    /// Cancel restores original tiles.
    Transform {
        /// Undo action to restore original tiles on cancel.
        cancel_undo: Box<dyn crate::undo::UndoAction>,
    },
}

/// CPU-side floating content state, owned by the engine.
pub struct FloatingContent {
    /// RGBA source tiles (always RGBA, even for mask sources).
    pub source_tiles: TileGrid,
    /// Pixel offset of the source content in document space.
    pub source_origin: (i32, i32),
    /// Source dimensions in pixels.
    pub source_width: u32,
    pub source_height: u32,
    /// Current affine transform matrix.
    pub matrix: Affine2D,
    /// Target layer.
    pub target_layer: LayerId,
    /// Whether the target is a mask (vs layer tiles).
    pub target_is_mask: bool,
    /// Determines commit/cancel behavior.
    pub mode: FloatingMode,
}

impl FloatingContent {
    /// Compute the bounding box of the transformed source in document pixels.
    /// Returns (min_x, min_y, max_x, max_y) inclusive.
    pub fn transformed_bounds(&self) -> (i32, i32, i32, i32) {
        let (ox, oy) = self.source_origin;
        let w = self.source_width as f32;
        let h = self.source_height as f32;

        // Transform the four corners of the source rectangle
        let corners = [
            affine_transform(&self.matrix, 0.0, 0.0),
            affine_transform(&self.matrix, w, 0.0),
            affine_transform(&self.matrix, 0.0, h),
            affine_transform(&self.matrix, w, h),
        ];

        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for (cx, cy) in &corners {
            min_x = min_x.min(*cx);
            min_y = min_y.min(*cy);
            max_x = max_x.max(*cx);
            max_y = max_y.max(*cy);
        }

        (
            (min_x + ox as f32).floor() as i32,
            (min_y + oy as f32).floor() as i32,
            (max_x + ox as f32).ceil() as i32,
            (max_y + oy as f32).ceil() as i32,
        )
    }

    /// Bilinear sample from source_tiles at a fractional source-local position.
    /// Returns [r, g, b, a] or [0,0,0,0] if out of bounds.
    fn sample_bilinear(&self, sx: f32, sy: f32) -> [u8; 4] {
        let (ox, oy) = self.source_origin;
        let w = self.source_width as f32;
        let h = self.source_height as f32;

        if sx < 0.0 || sy < 0.0 || sx >= w || sy >= h {
            return [0, 0, 0, 0];
        }

        let ix = sx.floor() as i32;
        let iy = sy.floor() as i32;
        let fx = sx - ix as f32;
        let fy = sy - iy as f32;

        let get_pixel = |px: i32, py: i32| -> [f32; 4] {
            if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 {
                return [0.0; 4];
            }
            let canvas_x = px + ox;
            let canvas_y = py + oy;
            let (tx, ty) = TileGrid::tile_coords_for_pixel(canvas_x, canvas_y);
            let ts = TILE_SIZE as i32;
            let lx = canvas_x.rem_euclid(ts) as usize;
            let ly = canvas_y.rem_euclid(ts) as usize;
            match self.source_tiles.get(tx, ty) {
                Some(tile) => {
                    let p = tile.data().pixel(lx, ly);
                    [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]
                }
                None => [0.0; 4],
            }
        };

        let p00 = get_pixel(ix, iy);
        let p10 = get_pixel(ix + 1, iy);
        let p01 = get_pixel(ix, iy + 1);
        let p11 = get_pixel(ix + 1, iy + 1);

        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let mut result = [0u8; 4];
        for c in 0..4 {
            let top = lerp(p00[c], p10[c], fx);
            let bot = lerp(p01[c], p11[c], fx);
            let val = lerp(top, bot, fy);
            result[c] = val.round().clamp(0.0, 255.0) as u8;
        }
        result
    }

    /// CPU-side rasterization: write transformed source pixels into a target
    /// TileGrid (layer tiles) using Normal blend.
    pub fn rasterize_to_tiles(
        &self,
        tiles: &mut TileGrid,
        selection: Option<&AlphaMask>,
    ) {
        let inv = match affine_inverse(&self.matrix) {
            Some(inv) => inv,
            None => return, // singular matrix — nothing to draw
        };

        let (min_x, min_y, max_x, max_y) = self.transformed_bounds();

        for py in min_y..=max_y {
            for px in min_x..=max_x {
                // Apply selection mask
                if let Some(sel) = selection {
                    let (stx, sty) = TileGrid::tile_coords_for_pixel(px, py);
                    let ts = TILE_SIZE as i32;
                    match sel.get(stx, sty) {
                        Some(st) => {
                            let slx = px.rem_euclid(ts) as usize;
                            let sly = py.rem_euclid(ts) as usize;
                            if st.data().get(slx, sly) <= 0.0 {
                                continue;
                            }
                        }
                        None => continue, // unselected region
                    }
                }

                // Transform to source-local coords
                let local_x = px as f32 - self.source_origin.0 as f32;
                let local_y = py as f32 - self.source_origin.1 as f32;
                let (src_x, src_y) = affine_transform(&inv, local_x, local_y);

                let fg = self.sample_bilinear(src_x, src_y);
                if fg[3] == 0 {
                    continue;
                }

                let (tx, ty) = TileGrid::tile_coords_for_pixel(px, py);
                let ts = TILE_SIZE as i32;
                let lx = px.rem_euclid(ts) as usize;
                let ly = py.rem_euclid(ts) as usize;

                let dst_tile = tiles.get_or_create(tx, ty);
                let dst = dst_tile.write().pixel_mut(lx, ly);

                if matches!(self.mode, FloatingMode::Transform { .. }) {
                    // Transform mode: direct write (target was cleared)
                    dst.copy_from_slice(&fg);
                } else {
                    // Paste mode: Normal blend onto existing
                    let fa = fg[3] as f32 / 255.0;
                    let ba = dst[3] as f32 / 255.0;
                    let out_a = fa + ba * (1.0 - fa);
                    if out_a > 0.0 {
                        for c in 0..3 {
                            let fg_pre = fg[c] as f32 * fa;
                            let bg_pre = dst[c] as f32 * ba;
                            let blended = fg_pre + bg_pre * (1.0 - fa);
                            dst[c] = (blended / out_a).round().clamp(0.0, 255.0) as u8;
                        }
                        dst[3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
                    }
                }
            }
        }
    }

    /// CPU-side rasterization: write transformed source pixels into a target
    /// AlphaMask. Extracts luminance from RGBA source.
    pub fn rasterize_to_mask(
        &self,
        mask: &mut AlphaMask,
        selection: Option<&AlphaMask>,
    ) {
        let inv = match affine_inverse(&self.matrix) {
            Some(inv) => inv,
            None => return,
        };

        let (min_x, min_y, max_x, max_y) = self.transformed_bounds();

        for py in min_y..=max_y {
            for px in min_x..=max_x {
                // Apply selection mask
                if let Some(sel) = selection {
                    let (stx, sty) = TileGrid::tile_coords_for_pixel(px, py);
                    let ts = TILE_SIZE as i32;
                    match sel.get(stx, sty) {
                        Some(st) => {
                            let slx = px.rem_euclid(ts) as usize;
                            let sly = py.rem_euclid(ts) as usize;
                            if st.data().get(slx, sly) <= 0.0 {
                                continue;
                            }
                        }
                        None => continue,
                    }
                }

                let local_x = px as f32 - self.source_origin.0 as f32;
                let local_y = py as f32 - self.source_origin.1 as f32;
                let (src_x, src_y) = affine_transform(&inv, local_x, local_y);

                let fg = self.sample_bilinear(src_x, src_y);
                if fg[3] == 0 {
                    continue;
                }

                // Convert RGBA to alpha via luminance (un-premultiply first)
                let a = fg[3] as f32 / 255.0;
                let (r, g, b) = if a > 0.0 {
                    (
                        (fg[0] as f32 / a).min(255.0),
                        (fg[1] as f32 / a).min(255.0),
                        (fg[2] as f32 / a).min(255.0),
                    )
                } else {
                    (0.0, 0.0, 0.0)
                };
                let lum = (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0;
                let alpha_val = lum * a;

                if alpha_val <= 0.0 {
                    continue;
                }

                let (tx, ty) = TileStore::<AlphaF32>::tile_coords_for_pixel(px, py);
                let ts = TILE_SIZE as i32;
                let lx = px.rem_euclid(ts) as usize;
                let ly = py.rem_euclid(ts) as usize;

                let dst_tile = mask.get_or_create(tx, ty);
                let dst = dst_tile.write();

                if matches!(self.mode, FloatingMode::Transform { .. }) {
                    dst.set(lx, ly, alpha_val);
                } else {
                    // Paste mode: add (clamped) onto existing
                    let existing = dst.get(lx, ly);
                    dst.set(lx, ly, (existing + alpha_val).min(1.0));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TransformPass — GPU pipeline and active state, owned by compositor
// ---------------------------------------------------------------------------

/// Uniforms for the transform-blend shader (64 bytes, std140-aligned).
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TransformBlendUniforms {
    /// Inverse affine row 0: [a, b, tx, _pad]
    pub inv_row0: [f32; 4],
    /// Inverse affine row 1: [c, d, ty, _pad]
    pub inv_row1: [f32; 4],
    /// Source origin in canvas pixel coords.
    pub source_origin: [f32; 2],
    /// Source texture dimensions in pixels.
    pub source_size: [f32; 2],
    /// Full canvas dimensions in pixels.
    pub canvas_size: [f32; 2],
    /// Opacity (0.0–1.0).
    pub opacity: f32,
    pub _pad: f32,
}

/// GPU resources for an active floating content.
pub struct TransformState {
    pub source_texture: wgpu::Texture,
    pub source_view: wgpu::TextureView,
    pub uniform_buf: wgpu::Buffer,
    /// bind_groups[src_accum_index] — two for ping-pong.
    pub bind_groups: [wgpu::BindGroup; 2],
    /// Bind group reading from composite cache as background.
    pub cache_source_bind_group: wgpu::BindGroup,
    pub target_layer: LayerId,
    pub target_is_mask: bool,
}

/// GPU pipeline + optional active floating content.
pub struct TransformPass {
    pub pipeline: wgpu::RenderPipeline,
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub active: Option<TransformState>,
}

impl TransformPass {
    pub fn new(device: &wgpu::Device, accum_format: wgpu::TextureFormat) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("transform-bgl"),
            entries: &[
                // binding 0: background accumulator
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
                // binding 1: source texture
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                // binding 2: sampler
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // binding 3: uniforms
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("transform-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            immediate_size: 0,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("transform-shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../shaders/transform.wgsl").into(),
            ),
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("transform-blend-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: accum_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        TransformPass {
            pipeline,
            bind_group_layout,
            active: None,
        }
    }

    /// Upload source tiles as an RGBA texture and create bind groups for
    /// compositing against both ping-pong accumulators.
    pub fn set_floating_content(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sampler: &wgpu::Sampler,
        accum_views: &[wgpu::TextureView; 2],
        cache_view: &wgpu::TextureView,
        source_tiles: &TileGrid,
        source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
        canvas_width: u32,
        canvas_height: u32,
        target_layer: LayerId,
        target_is_mask: bool,
    ) {
        // Create the source texture and upload tile data
        let source_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("transform-source"),
            size: wgpu::Extent3d {
                width: source_width.max(1),
                height: source_height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Rasterize source tiles into a contiguous RGBA buffer for upload
        let w = source_width as usize;
        let h = source_height as usize;
        let mut rgba = vec![0u8; w * h * 4];
        let ts = TILE_SIZE as i32;
        let (ox, oy) = source_origin;

        for ((tx, ty), tile) in source_tiles.iter() {
            let tile_px = tx * ts;
            let tile_py = ty * ts;
            let data = tile.data();
            for ly in 0..TILE_SIZE {
                for lx in 0..TILE_SIZE {
                    let canvas_x = tile_px + lx as i32;
                    let canvas_y = tile_py + ly as i32;
                    let img_x = canvas_x - ox;
                    let img_y = canvas_y - oy;
                    if img_x < 0 || img_y < 0 || img_x >= w as i32 || img_y >= h as i32 {
                        continue;
                    }
                    let pixel = data.pixel(lx, ly);
                    if pixel[3] == 0 {
                        continue;
                    }
                    let offset = (img_y as usize * w + img_x as usize) * 4;
                    rgba[offset..offset + 4].copy_from_slice(pixel);
                }
            }
        }

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &source_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w as u32 * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: source_width.max(1),
                height: source_height.max(1),
                depth_or_array_layers: 1,
            },
        );

        let source_view = source_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Uniform buffer (identity matrix initially)
        let uniforms = TransformBlendUniforms {
            inv_row0: [1.0, 0.0, 0.0, 0.0],
            inv_row1: [0.0, 1.0, 0.0, 0.0],
            source_origin: [source_origin.0 as f32, source_origin.1 as f32],
            source_size: [source_width as f32, source_height as f32],
            canvas_size: [canvas_width as f32, canvas_height as f32],
            opacity: 1.0,
            _pad: 0.0,
        };

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("transform-uniforms"),
            size: std::mem::size_of::<TransformBlendUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        // Create bind groups for both ping-pong directions
        let make_bind_group = |bg_view: &wgpu::TextureView, label: &str| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(bg_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&source_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: uniform_buf.as_entire_binding(),
                    },
                ],
            })
        };

        let bg0 = make_bind_group(&accum_views[0], "transform-bg0");
        let bg1 = make_bind_group(&accum_views[1], "transform-bg1");
        let cache_bg = make_bind_group(cache_view, "transform-bg-cache");

        self.active = Some(TransformState {
            source_texture,
            source_view,
            uniform_buf,
            bind_groups: [bg0, bg1],
            cache_source_bind_group: cache_bg,
            target_layer,
            target_is_mask,
        });
    }

    /// Update the affine matrix uniform for real-time preview.
    pub fn update_matrix(
        &self,
        queue: &wgpu::Queue,
        matrix: &Affine2D,
        source_origin: (i32, i32),
        source_width: u32,
        source_height: u32,
        canvas_width: u32,
        canvas_height: u32,
    ) {
        let state = match &self.active {
            Some(s) => s,
            None => return,
        };

        let inv = affine_inverse(matrix).unwrap_or(IDENTITY);

        let uniforms = TransformBlendUniforms {
            inv_row0: [inv[0], inv[1], inv[2], 0.0],
            inv_row1: [inv[3], inv[4], inv[5], 0.0],
            source_origin: [source_origin.0 as f32, source_origin.1 as f32],
            source_size: [source_width as f32, source_height as f32],
            canvas_size: [canvas_width as f32, canvas_height as f32],
            opacity: 1.0,
            _pad: 0.0,
        };

        queue.write_buffer(&state.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
    }

    /// Remove floating content GPU state.
    pub fn clear(&mut self) {
        self.active = None;
    }

    /// Check if floating content is active and targets the given layer.
    pub fn targets_layer(&self, layer_id: LayerId) -> bool {
        self.active.as_ref().map_or(false, |s| s.target_layer == layer_id)
    }
}

// ---------------------------------------------------------------------------
// Utility: build source tiles + dimensions from an ImageClip
// ---------------------------------------------------------------------------

/// Extract source tiles, origin, and dimensions from an ImageClip for
/// creating a FloatingContent.
pub fn source_from_clip(
    clip: &crate::clipboard::ImageClip,
) -> (TileGrid, (i32, i32), u32, u32) {
    // Clone the clip's tiles for the FloatingContent's CPU source
    let mut tiles = TileGrid::new();
    for ((tx, ty), src_tile) in clip.tiles.iter() {
        let dst = tiles.get_or_create(tx, ty);
        *dst.write() = src_tile.data().clone();
    }
    let (x, y, w, h) = clip.bounds;
    (tiles, (x, y), w, h)
}
