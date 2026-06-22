//! Video-stream void — live external image (webcam or screenshare) as a layer.
//!
//! The generic machinery shared by the camera and screenshare voids. Both are
//! "browser-owned `MediaStream` → GPU texture → cover-fit shader" at heart; the
//! only differences are metadata (display name, icon), how the browser captures
//! the stream ([`CaptureKind`]), and the seeded initial gizmo transform. Those
//! differences live in a [`VideoStreamConfig`], one `&'static` per kind, and
//! the per-kind files under [`super::voids`] stay tiny.
//!
//! The browser owns a `<video>` element fed by `getUserMedia()` /
//! `getDisplayMedia()`; each render-loop tick the WASM bridge hands the video
//! element to this void via [`Void::upload_external_image`], and we copy the
//! current frame into an aux texture using
//! [`wgpu::Queue::copy_external_image_to_texture`]. The shader then samples that
//! aux texture, applies the user's transform (the generic gizmo affine), and
//! writes to the layer's destination texture.
//!
//! Aspect handling is "cover": at the identity transform the source fills the
//! layer and the short axis is cropped — the active-pixel rect overhangs the
//! canvas on the long axis (see `content_rect`). Out-of-frame samples return
//! transparent. The gizmo wraps that overhanging content rect, not the canvas.
//! Mirroring (the camera's selfie flip) is a negative scale in the gizmo
//! affine, not a shader/uniform concern — the inverse-affine sample handles it.
//!
//! Native: the upload path is unreachable because [`ExternalImageSource`] has no
//! variants on non-wasm targets. The void can still be registered and its layer
//! added, but it will render as the transparent placeholder until a frame is
//! supplied — which only the browser bridge can do.

use crate::gpu::effect::{EffectCache, EffectPipeline};
use crate::gpu::void::{
    CaptureKind, DirtyFlag, ExternalImageSource, ParamDef, ParamValue, Void, VoidRegistration,
};
use std::cell::Cell;
use std::sync::Arc;

/// Static per-variant description of a video-stream void. One of these is
/// declared `&'static` in each variant file (camera / screenshare) and threaded
/// into the shared machinery — the variant files hold nothing else of substance.
#[derive(Debug)]
pub struct VideoStreamConfig {
    pub type_id: &'static str,
    pub display_name: &'static str,
    pub icon: &'static str,
    /// Param schema — looked up by *name* (`"freeze"`, `"frame_divisor"`) by the
    /// shared code, so variants are decoupled from each other's param ordering.
    pub params: &'static [ParamDef],
    /// Browser capture API for this kind (`Camera` → `getUserMedia`,
    /// `Display` → `getDisplayMedia`). Always `Some` for a video-stream void.
    pub capture_kind: CaptureKind,
    /// Initial gizmo transform given the canvas dimensions at creation time.
    /// Camera seeds a horizontal flip (selfie); screenshare seeds identity.
    pub default_transform: fn(u32, u32) -> crate::transform::Transform,
}

/// Build a [`VoidRegistration`] from a static config. Each variant's
/// `register()` is one call: it passes its `&'static CONFIG` plus a `from_params`
/// fn pointer that names that same static (so the constructed void carries its
/// kind). `create_pipeline` is shared verbatim — the shader and layout are
/// identical across kinds, so no per-variant pipeline wrapper is needed.
pub fn registration(
    config: &'static VideoStreamConfig,
    from_params: fn(&[ParamValue], Arc<EffectPipeline>) -> Box<dyn Void>,
) -> VoidRegistration {
    VoidRegistration {
        type_id: config.type_id,
        display_name: config.display_name,
        params: config.params,
        icon: config.icon,
        // The aux texture is a 1×1 placeholder until a frame arrives, so
        // there's nothing meaningful to render at picker-preview time.
        supports_preview: false,
        supports_live_transform: true,
        capture_kind: Some(config.capture_kind),
        default_transform: config.default_transform,
        create_pipeline,
        from_params,
    }
}

/// Construct a [`VideoStreamVoid`] for a given static config — the body behind
/// each variant's `from_params` fn pointer.
pub fn build_void(
    config: &'static VideoStreamConfig,
    params: &[ParamValue],
    shared: Arc<EffectPipeline>,
) -> Box<dyn Void> {
    Box::new(VideoStreamVoid::from_params(config, params, shared))
}

/// Index of the named param within a config's schema, or `None` if absent.
fn param_index(config: &VideoStreamConfig, name: &str) -> Option<usize> {
    config.params.iter().position(|p| p.name() == name)
}

/// Read the `"freeze"` toggle out of a positional param slice by resolving its
/// name to an index in the config schema first. Defaults to `false`.
fn read_freeze(config: &VideoStreamConfig, params: &[ParamValue]) -> bool {
    match param_index(config, "freeze").and_then(|i| params.get(i)) {
        Some(ParamValue::Bool(v)) => *v,
        _ => false,
    }
}

/// Read the `"frame_divisor"` throttle by name, clamped to `>= 1`. Defaults to
/// 4 (the schema default) when absent.
fn read_frame_divisor(config: &VideoStreamConfig, params: &[ParamValue]) -> u32 {
    match param_index(config, "frame_divisor").and_then(|i| params.get(i)) {
        Some(ParamValue::Int(v)) => (*v).max(1) as u32,
        _ => 4,
    }
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct VideoStreamUniforms {
    /// Inverse of the user transform's affine, row 0: `[a, b, tx, _pad]`.
    /// The shader maps a window-local fragment (relative to the content rect)
    /// through this inverse to find the pre-transform position, then normalizes
    /// to a source UV. Mirrors the `inv_row0/inv_row1` layout of
    /// [`crate::gpu::transform::TransformBlendUniforms`].
    inv_row0: [f32; 4],
    /// Inverse affine row 1: `[c, d, ty, _pad]`.
    inv_row1: [f32; 4],
    /// Window-local origin of the cover-fit content rect (see
    /// [`Void::content_extent`]). Negative on the overhanging axis. Cover-fit
    /// is baked into this rect, so the shader needs no separate canvas dims.
    content_origin: [f32; 2],
    /// Window-local size of the cover-fit content rect.
    content_size: [f32; 2],
}

#[derive(Debug)]
pub struct VideoStreamVoid {
    /// Static per-kind description (display name, icon, capture kind, seed
    /// transform). The trait's `type_id()` reads `config.type_id`.
    config: &'static VideoStreamConfig,
    /// User transform (pan / scale / rotate) edited by the gizmo. The shader
    /// samples through its inverse. NOTE: `canvas_origin` deliberately does not
    /// enter here — the gizmo edits this affine in the void's local frame,
    /// which coincides with window-local (the frame `FragCoord.xy` is in), so
    /// `canvas_origin` cancels in the shader. It matters only at the reporting
    /// boundary (`void_transform_info` reports the bbox origin = canvas_origin
    /// so the gizmo draws in the right plane location).
    transform: crate::transform::Transform,
    freeze: bool,
    /// Rate-limit divisor for source → GPU uploads (1 = every rAF frame,
    /// N = every Nth). Stored here as the source of truth; the JS-side
    /// `MediaStreamSource.tick()` reads it through the layer-tree reconciliation
    /// and gates its uploads accordingly. Never read at render time on the
    /// Rust side.
    frame_divisor: u32,
    /// Current source dimensions (updated on each frame upload). 1×1 until
    /// the first frame arrives — matching the placeholder aux texture.
    src_w: u32,
    src_h: u32,
    /// Canvas dimensions cached from `create_cache`. `Cell` because the trait
    /// gives us `&self` there; `upload_external_image` reads these to rewrite
    /// the uniforms when the source resolution changes.
    canvas_w: Cell<u32>,
    canvas_h: Cell<u32>,
    shared: Arc<EffectPipeline>,
    dirty: DirtyFlag,
}

impl Clone for VideoStreamVoid {
    fn clone(&self) -> Self {
        // `clone_boxed` is called for undo / clone_subtree. The clone gets a
        // fresh `EffectCache` from `ensure_void_layer` with the 1×1
        // placeholder aux texture, so start dirty.
        VideoStreamVoid {
            config: self.config,
            transform: self.transform,
            freeze: self.freeze,
            frame_divisor: self.frame_divisor,
            src_w: self.src_w,
            src_h: self.src_h,
            canvas_w: Cell::new(self.canvas_w.get()),
            canvas_h: Cell::new(self.canvas_h.get()),
            shared: self.shared.clone(),
            dirty: DirtyFlag::new_dirty(),
        }
    }
}

impl VideoStreamVoid {
    fn from_params(
        config: &'static VideoStreamConfig,
        params: &[ParamValue],
        shared: Arc<EffectPipeline>,
    ) -> Self {
        VideoStreamVoid {
            config,
            // Transform is not a param — it lives on the layer and is applied
            // via `set_transform`. New instances start at identity; the
            // compositor pushes the layer's stored transform (the seeded flip
            // for cameras, or any edit) after creation.
            transform: crate::transform::Transform::identity(),
            freeze: read_freeze(config, params),
            frame_divisor: read_frame_divisor(config, params),
            src_w: 1,
            src_h: 1,
            canvas_w: Cell::new(1),
            canvas_h: Cell::new(1),
            shared,
            dirty: DirtyFlag::new_dirty(),
        }
    }

    fn uniforms(&self) -> VideoStreamUniforms {
        // Sample through the inverse of the user transform. A singular matrix
        // (degenerate scale) falls back to identity rather than NaN-ing the UV.
        let fwd = self.transform.to_affine();
        let inv = crate::transform::affine_inverse(&fwd).unwrap_or(crate::transform::IDENTITY);
        let (ox, oy, cw, ch) = self.content_rect(self.canvas_w.get(), self.canvas_h.get());
        VideoStreamUniforms {
            inv_row0: [inv[0], inv[1], inv[2], 0.0],
            inv_row1: [inv[3], inv[4], inv[5], 0.0],
            content_origin: [ox, oy],
            content_size: [cw, ch],
        }
    }

    /// Cover-fit content rect in window-local coords — the source's active
    /// pixels. `(origin_x, origin_y, w, h)`; the origin is negative on the
    /// overhanging (cropped) axis. Until the first frame (the source is the 1×1
    /// placeholder) there's no meaningful aspect, so fall back to canvas-fill.
    /// Backs both the uniform and [`Void::content_extent`] (the gizmo bbox), so
    /// the on-canvas handles wrap exactly the pixels the shader samples.
    fn content_rect(&self, canvas_w: u32, canvas_h: u32) -> (f32, f32, f32, f32) {
        let cw = canvas_w as f32;
        let ch = canvas_h as f32;
        if self.src_w <= 1 || self.src_h <= 1 {
            return (0.0, 0.0, cw, ch);
        }
        let sw = self.src_w as f32;
        let sh = self.src_h as f32;
        let cover = (cw / sw).max(ch / sh);
        let content_w = sw * cover;
        let content_h = sh * cover;
        (
            (cw - content_w) * 0.5,
            (ch - content_h) * 0.5,
            content_w,
            content_h,
        )
    }

    /// CPU mirror of `video_stream.wgsl`'s fragment mapping: a window-local
    /// fragment `(fx, fy)` → source UV. Kept in lockstep with the shader so the
    /// composition (inverse-affine → cover-fit) can be unit-tested without a
    /// live stream. **If you edit the WGSL `fs_main` math, edit this too** (and
    /// vice-versa); the tests pin them together.
    #[cfg(test)]
    fn src_uv(u: &VideoStreamUniforms, frag: (f32, f32)) -> (f32, f32) {
        // Window-local fragment → content-local → inverse affine → normalize.
        let cl = (frag.0 - u.content_origin[0], frag.1 - u.content_origin[1]);
        let lx = u.inv_row0[0] * cl.0 + u.inv_row0[1] * cl.1 + u.inv_row0[2];
        let ly = u.inv_row1[0] * cl.0 + u.inv_row1[1] * cl.1 + u.inv_row1[2];
        let ux = lx / u.content_size[0];
        let uy = ly / u.content_size[1];
        (ux, uy)
    }

    /// Replace the aux frame texture with a fresh `(w, h)` allocation and
    /// rebuild bind group 0 to reference it. Shared by the live-upload
    /// path (`upload_external_image` on a resolution change) and the
    /// save-restore path (`restore_persistent_pixels` at document load).
    fn resize_aux_texture(
        &mut self,
        device: &wgpu::Device,
        cache: &mut EffectCache,
        w: u32,
        h: u32,
    ) {
        let (tex, view) = make_frame_texture(device, w, h);
        if cache.aux_textures.is_empty() {
            cache.aux_textures.push(tex);
            cache.aux_views.push(view);
        } else {
            cache.aux_textures[0] = tex;
            cache.aux_views[0] = view;
        }
        self.src_w = w;
        self.src_h = h;

        // Fresh sampler each rebuild — wgpu reuses internal handles so
        // this is essentially free and avoids threading the compositor's
        // shared sampler through every call site.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("void-video-stream-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        cache.bind_groups[0] = build_bind_groups(
            device,
            &self.shared.bind_group_layout,
            &cache.uniform_bufs[0],
            &cache.aux_views[0],
            &sampler,
        );
    }
}

fn make_frame_texture(device: &wgpu::Device, w: u32, h: u32) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("void-video-stream-frame"),
        size: wgpu::Extent3d {
            width: w.max(1),
            height: h.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        // RENDER_ATTACHMENT is required by `copy_external_image_to_texture`
        // per the WebGPU spec; TEXTURE_BINDING for shader sampling; COPY_DST
        // for live frame uploads and the load-time restore; COPY_SRC so the
        // save flow can read this persistent frame back (the readback's
        // `copy_texture_to_buffer` is a validation error without it).
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    (tex, view)
}

fn build_bind_groups(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform_buf: &wgpu::Buffer,
    tex_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> [wgpu::BindGroup; 2] {
    let bg = |label: &str| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(tex_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    };
    [bg("void-video-stream-bg-0"), bg("void-video-stream-bg-1")]
}

impl Void for VideoStreamVoid {
    fn type_id(&self) -> &'static str {
        self.config.type_id
    }

    fn clone_boxed(&self) -> Box<dyn Void> {
        Box::new(self.clone())
    }

    fn param_values(&self) -> Vec<ParamValue> {
        // Emit values in the config's declared param order so they round-trip
        // through `from_params`. Each named param maps to a field.
        self.config
            .params
            .iter()
            .map(|def| match def.name() {
                "freeze" => ParamValue::Bool(self.freeze),
                "frame_divisor" => ParamValue::Int(self.frame_divisor as i32),
                // Unknown param: fall back to its declared default so the slice
                // length always matches the schema.
                _ => def.default_value(),
            })
            .collect()
    }

    fn take_dirty(&mut self) -> bool {
        self.dirty.take()
    }

    fn mark_dirty(&mut self) {
        self.dirty.mark();
    }

    fn needs_animation(&self) -> bool {
        // The stream doesn't accumulate time on its own, but the compositor
        // uses `needs_animation()` as the "keep the rAF loop alive" signal.
        // Without it, the void would only re-render on param changes, and live
        // frames would freeze on the first one we uploaded. When frozen, the
        // last frame is held forever — no animation needed, so we stop keeping
        // the rAF loop alive. The visibility half of the gate (don't animate a
        // hidden layer) is the engine's job; this method only knows about
        // kind-specific state.
        !self.freeze
    }

    fn update_params(&mut self, queue: &wgpu::Queue, cache: &EffectCache, params: &[ParamValue]) {
        // In-place: update fields and rewrite the uniform buffer. We do NOT
        // touch `cache.aux_textures` — that's where the live frame lives, and
        // toggling `freeze` (or any other param) must not wipe it. The bind
        // group continues to reference the same texture view, so the next
        // encode samples whatever was last uploaded.
        self.freeze = read_freeze(self.config, params);
        self.frame_divisor = read_frame_divisor(self.config, params);
        if let Some(buf) = cache.uniform_bufs.first() {
            queue.write_buffer(buf, 0, bytemuck::bytes_of(&self.uniforms()));
        }
        self.dirty.mark();
    }

    fn set_transform(
        &mut self,
        queue: &wgpu::Queue,
        cache: &EffectCache,
        transform: &crate::transform::Transform,
    ) {
        // In-place, exactly like `update_params`: store the transform and
        // rewrite the uniform. Never rebuild — that would drop the aux frame
        // texture (the `from_params` rebuild bug documented on `update_params`).
        self.transform = *transform;
        if let Some(buf) = cache.uniform_bufs.first() {
            queue.write_buffer(buf, 0, bytemuck::bytes_of(&self.uniforms()));
        }
        self.dirty.mark();
    }

    fn content_extent(&self, canvas_w: u32, canvas_h: u32) -> (f32, f32, f32, f32) {
        // Use the compositor's LIVE canvas dims (passed in) rather than the
        // cached Cell, so the gizmo bbox is correct immediately after a crop —
        // `set_canvas_rect` updates the compositor's dims but not the void's
        // cached copy.
        self.content_rect(canvas_w, canvas_h)
    }

    fn wants_external_input(&self) -> bool {
        // While frozen, refuse new frames so the displayed image is whatever
        // was in the aux texture at the moment freeze was toggled on. The
        // visibility half of the gate (don't upload to a hidden layer) is
        // the engine's job at the `upload_void_external_image` boundary.
        !self.freeze
    }

    fn persistent_frame_size(&self) -> Option<(u32, u32)> {
        // Only report a size once a real frame has been received. Until the
        // first upload (src_w/h are the placeholder 1×1), the texture has
        // nothing meaningful in it — don't poison saves with a 1×1 black frame.
        if self.src_w > 1 && self.src_h > 1 {
            Some((self.src_w, self.src_h))
        } else {
            None
        }
    }

    fn restore_persistent_pixels(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cache: &mut EffectCache,
        width: u32,
        height: u32,
        bytes: &[u8],
    ) {
        if width == 0 || height == 0 {
            return;
        }
        self.resize_aux_texture(device, cache, width, height);
        // Bytes are already Rgba8Unorm-packed (the format the save flow read
        // back). Direct queue.write_texture is the symmetric load path matching
        // raster's `upload_node_pixels`.
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &cache.aux_textures[0],
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
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
        queue.write_buffer(
            &cache.uniform_bufs[0],
            0,
            bytemuck::bytes_of(&self.uniforms()),
        );
        self.dirty.mark();
    }

    fn upload_external_image(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cache: &mut EffectCache,
        source: ExternalImageSource,
    ) {
        #[cfg(target_arch = "wasm32")]
        {
            let ExternalImageSource::Web(info) = source;
            let (w, h) = (info.source.width(), info.source.height());
            if w == 0 || h == 0 {
                // Video element is not yet ready (no frame, paused, ended).
                // No-op; we'll try again on the next tick.
                return;
            }

            let need_realloc = cache
                .aux_textures
                .first()
                .map(|t| t.width() != w || t.height() != h)
                .unwrap_or(true);

            if need_realloc {
                self.resize_aux_texture(device, cache, w, h);
            }

            // Push the latest uniforms (src_w/h just changed on realloc; params
            // don't change here but rewriting is cheap and avoids a
            // dirty-tracking flag).
            queue.write_buffer(
                &cache.uniform_bufs[0],
                0,
                bytemuck::bytes_of(&self.uniforms()),
            );

            queue.copy_external_image_to_texture(
                &info,
                wgpu::CopyExternalImageDestInfo {
                    texture: &cache.aux_textures[0],
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                    color_space: wgpu::PredefinedColorSpace::Srgb,
                    premultiplied_alpha: false,
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
            self.dirty.mark();
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            // The enum is uninhabited on native — this arm only exists so the
            // method body compiles. The match below is unreachable.
            let _ = (device, queue, cache);
            match source {}
        }
    }

    fn create_cache(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _dst_view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        render_width: u32,
        render_height: u32,
    ) -> EffectCache {
        // Cache the canvas dims; `upload_external_image` needs them to
        // rewrite uniforms when the source resolution changes.
        self.canvas_w.set(render_width.max(1));
        self.canvas_h.set(render_height.max(1));

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("void-video-stream-uniforms"),
            size: std::mem::size_of::<VideoStreamUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&self.uniforms()));

        // 1×1 transparent placeholder until the first frame upload. The
        // placeholder satisfies the bind group's texture binding so the
        // pipeline can run before a frame is available.
        let (placeholder_tex, placeholder_view) = make_frame_texture(device, 1, 1);

        let bind_groups = build_bind_groups(
            device,
            &self.shared.bind_group_layout,
            &uniform_buf,
            &placeholder_view,
            sampler,
        );

        EffectCache {
            uniform_bufs: vec![uniform_buf],
            bind_groups: vec![bind_groups],
            aux_textures: vec![placeholder_tex],
            aux_views: vec![placeholder_view],
            aux_pipelines: Vec::new(),
        }
    }

    fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        cache: &EffectCache,
        dst_view: &wgpu::TextureView,
    ) {
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("void-video-stream-encode"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        rpass.set_pipeline(&self.shared.pipeline);
        rpass.set_bind_group(0, &cache.bind_groups[0][0], &[]);
        rpass.draw(0..3, 0..1);
    }
}

/// Build the shared render pipeline for a video-stream void. Identical across
/// kinds (same shader, same layout) — each registered kind caches its own
/// `Arc<EffectPipeline>`, which is two identical pipelines in VRAM when both
/// camera and screenshare are present. Expected, not a leak.
fn create_pipeline(device: &wgpu::Device, format: wgpu::TextureFormat) -> EffectPipeline {
    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("void-video-stream-bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
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
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("void-video-stream-pipeline-layout"),
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });

    let src = include_str!("../../../../shaders/voids/video_stream.wgsl");
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("void-video-stream-shader"),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("void-video-stream-pipeline"),
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
                format,
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

    EffectPipeline {
        pipeline,
        bind_group_layout,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A test-only config standing in for a real variant. Identical param schema
    // to camera / screenshare; identity seed transform keeps the affine math
    // easy to reason about.
    const TEST_PARAMS: &[ParamDef] = &[
        ParamDef::Bool {
            name: "freeze",
            default: false,
        },
        ParamDef::Int {
            name: "frame_divisor",
            min: 1,
            max: 60,
            default: 4,
        },
    ];

    static TEST_CONFIG: VideoStreamConfig = VideoStreamConfig {
        type_id: "test_video_stream",
        display_name: "Test",
        icon: "tabler:test",
        params: TEST_PARAMS,
        capture_kind: CaptureKind::Camera,
        default_transform: |_, _| crate::transform::Transform::identity(),
    };

    fn default_params() -> Vec<ParamValue> {
        TEST_PARAMS.iter().map(ParamDef::default_value).collect()
    }

    fn fake_pipeline() -> Arc<EffectPipeline> {
        let (device, _queue) = crate::gpu::test_utils::test_device();
        Arc::new(create_pipeline(&device, wgpu::TextureFormat::Rgba8Unorm))
    }

    fn make_void() -> VideoStreamVoid {
        VideoStreamVoid::from_params(&TEST_CONFIG, &default_params(), fake_pipeline())
    }

    #[test]
    fn param_round_trip() {
        // Default params round-trip through from_params → param_values. Order
        // matches the schema: freeze, frame_divisor (mirror is gone — it's a
        // gizmo negative scale now).
        let v = make_void();
        let out = v.param_values();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], ParamValue::Bool(false), "freeze defaults off");
        assert_eq!(out[1], ParamValue::Int(4), "frame_divisor defaults to 4");
    }

    #[test]
    fn frame_divisor_round_trip() {
        // The JS side reads `frame_divisor` from the layer-tree params via
        // `param_values` to throttle its `tick()` uploads. Verify update_params
        // mutates the field in place and the new value flows back out.
        let (_device, queue) = crate::gpu::test_utils::test_device();
        let mut v = make_void();
        assert_eq!(v.frame_divisor, 4);

        let uniform_buf = _device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: std::mem::size_of::<VideoStreamUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cache = EffectCache {
            uniform_bufs: vec![uniform_buf],
            bind_groups: Vec::new(),
            aux_textures: Vec::new(),
            aux_views: Vec::new(),
            aux_pipelines: Vec::new(),
        };

        let mut new_params = default_params();
        new_params[1] = ParamValue::Int(8);
        v.update_params(&queue, &cache, &new_params);
        assert_eq!(v.frame_divisor, 8);
        assert_eq!(v.param_values()[1], ParamValue::Int(8));

        // Out-of-range values are clamped to >= 1 — divisor 0 would mean
        // "upload every 0th frame" which is undefined; the JS gate uses
        // `counter % divisor` so a zero divisor would panic on modulo.
        new_params[1] = ParamValue::Int(0);
        v.update_params(&queue, &cache, &new_params);
        assert_eq!(v.frame_divisor, 1, "divisor clamps up to 1");
    }

    #[test]
    fn freeze_stops_external_input() {
        // wants_external_input is the gate the compositor uses to drop uploads
        // from the JS side; once `freeze` flips on, that gate should close so
        // subsequent frames are ignored. freeze is the 1st param (index 0).
        let mut params = default_params();
        params[0] = ParamValue::Bool(true);
        let v = VideoStreamVoid::from_params(&TEST_CONFIG, &params, fake_pipeline());
        assert!(!v.wants_external_input());
        assert!(!v.needs_animation());
    }

    /// Regression: toggling any param (notably `freeze`) must not wipe the
    /// void's accumulated GPU state — earlier the compositor's
    /// `update_void_layer_params` rebuilt the void from `from_params` and
    /// re-allocated `EffectCache`, dropping the aux texture that holds the live
    /// frame. The user reported "clicking freeze disappears the whole layer"
    /// because the rebuild reset `src_w/h` to the 1×1 placeholder.
    /// `update_params` must mutate fields in place.
    #[test]
    fn update_params_preserves_source_dimensions() {
        let (device, queue) = crate::gpu::test_utils::test_device();
        let mut v = make_void();

        // Pretend an upload arrived and set the live dimensions.
        v.src_w = 640;
        v.src_h = 480;

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("test-uniforms"),
            size: std::mem::size_of::<VideoStreamUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cache = EffectCache {
            uniform_bufs: vec![uniform_buf],
            bind_groups: Vec::new(),
            aux_textures: Vec::new(),
            aux_views: Vec::new(),
            aux_pipelines: Vec::new(),
        };

        let mut new_params = default_params();
        new_params[0] = ParamValue::Bool(true); // freeze on
        v.update_params(&queue, &cache, &new_params);

        assert_eq!(v.src_w, 640);
        assert_eq!(v.src_h, 480);
        assert!(v.freeze);
        assert!(!v.wants_external_input());
    }

    /// Regression (sibling of `update_params_preserves_source_dimensions`):
    /// `set_transform` is the gizmo's live-update path and must also mutate in
    /// place — never rebuild from `from_params`, which would drop the aux frame
    /// texture and blank the layer mid-drag.
    #[test]
    fn set_transform_preserves_source_dimensions() {
        let (device, queue) = crate::gpu::test_utils::test_device();
        let mut v = make_void();
        v.src_w = 1280;
        v.src_h = 720;

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("test-uniforms"),
            size: std::mem::size_of::<VideoStreamUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cache = EffectCache {
            uniform_bufs: vec![uniform_buf],
            bind_groups: Vec::new(),
            aux_textures: Vec::new(),
            aux_views: Vec::new(),
            aux_pipelines: Vec::new(),
        };

        let t = crate::transform::Transform::from_affine([2.0, 0.0, 5.0, 0.0, 2.0, 9.0]);
        v.set_transform(&queue, &cache, &t);

        assert_eq!(v.src_w, 1280);
        assert_eq!(v.src_h, 720);
        assert_eq!(v.transform, t, "transform stored");
    }

    #[test]
    fn uniforms_layout_matches_wgsl() {
        // The WGSL `Params` struct in video_stream.wgsl is inv_row0[4] +
        // inv_row1[4] + content_origin[2] + content_size[2] = 12 f32s = 48
        // bytes. 48 is a clean multiple of 16 so no extra std140 padding.
        // Catches layout drift.
        assert_eq!(std::mem::size_of::<VideoStreamUniforms>(), 48);
        assert_eq!(std::mem::size_of::<VideoStreamUniforms>() % 16, 0);
        assert_eq!(std::mem::align_of::<VideoStreamUniforms>(), 4);
    }

    /// CPU-side proof of the shader composition (inverse-affine → cover-fit),
    /// pinned to `VideoStreamVoid::src_uv` which mirrors
    /// `video_stream.wgsl::fs_main`. Square 100×100 source on a 100×100 canvas
    /// keeps cover-fit = 1, so the math is easy to reason about by hand.
    #[test]
    fn src_uv_composition() {
        let v = {
            let mut x = make_void();
            x.src_w = 100;
            x.src_h = 100;
            x.canvas_w.set(100);
            x.canvas_h.set(100);
            x
        };

        // Identity transform: canvas center maps to source center (0.5, 0.5).
        let u = v.uniforms();
        let (ux, uy) = VideoStreamVoid::src_uv(&u, (50.0, 50.0));
        approx(ux, 0.5);
        approx(uy, 0.5);

        // Body-translate the content +20px in x (gizmo drag). The output pixel
        // at canvas x=70 now shows what was at the center → uv.x back to 0.5.
        let mut shifted = v;
        shifted.transform =
            crate::transform::Transform::from_affine([1.0, 0.0, 20.0, 0.0, 1.0, 0.0]);
        let u2 = shifted.uniforms();
        let (sx, sy) = VideoStreamVoid::src_uv(&u2, (70.0, 50.0));
        approx(sx, 0.5);
        approx(sy, 0.5);
    }

    fn approx(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-4, "expected {b}, got {a}");
    }

    /// The gizmo bbox = cover-fit content rect, which OVERHANGS the canvas on
    /// the cropped axis (it should reflect the real source bounds, not the
    /// canvas). A 200×100 source on a 100×100 canvas covers by scaling ×1, so
    /// the content is 200 wide — extending 50px past each side — and 100 tall.
    #[test]
    fn content_extent_overhangs_canvas() {
        let mut v = make_void();
        v.canvas_w.set(100);
        v.canvas_h.set(100);
        v.src_w = 200;
        v.src_h = 100;
        let (ox, oy, w, h) = v.content_extent(100, 100);
        approx(w, 200.0);
        approx(h, 100.0);
        approx(ox, -50.0); // overhangs left/right
        approx(oy, 0.0); // fits vertically
    }

    /// Until the first frame arrives (1×1 placeholder), there's no aspect to
    /// cover-fit, so the content rect falls back to canvas-fill.
    #[test]
    fn content_extent_falls_back_to_canvas_without_frame() {
        let v = make_void();
        v.canvas_w.set(80);
        v.canvas_h.set(60);
        let (ox, oy, w, h) = v.content_extent(80, 60);
        assert_eq!((ox, oy, w, h), (0.0, 0.0, 80.0, 60.0));
    }

    #[test]
    fn cover_fit_math_landscape_source_square_canvas() {
        // 16:9 source, 1:1 canvas, scale=1, no rotation, no pan.
        // Shader maps dest-x ∈ [-0.5, +0.5] → src-x-centered ∈ [-0.5·f, +0.5·f]
        // with f = ca / sa. The visible source-x range therefore has width f.
        // For cover we want f < 1 (the long axis is cropped); the y axis
        // should be untouched, so its visible range stays exactly 1.
        let source_aspect = 16.0_f32 / 9.0;
        let canvas_aspect = 1.0_f32;
        let factor = canvas_aspect / source_aspect;
        assert!(factor < 1.0);
        assert!((factor - 9.0 / 16.0).abs() < 1e-5);
        let visible_width_in_source = factor;
        let visible_height_in_source = 1.0_f32;
        assert!(visible_width_in_source < visible_height_in_source);
    }

    #[test]
    fn cover_fit_math_portrait_source_square_canvas() {
        // 9:16 source, 1:1 canvas → y axis shrinks instead. Symmetric to the
        // landscape case.
        let source_aspect = 9.0_f32 / 16.0;
        let canvas_aspect = 1.0_f32;
        let factor = source_aspect / canvas_aspect;
        assert!(factor < 1.0);
        assert!((factor - 9.0 / 16.0).abs() < 1e-5);
    }

    #[test]
    fn cover_fit_math_matching_aspects_is_identity() {
        // Square source on square canvas: no crop, no letterbox. Either branch
        // of the shader's if/else collapses to a multiplication by 1.
        let source_aspect = 1.0_f32;
        let canvas_aspect = 1.0_f32;
        let factor = canvas_aspect / source_aspect;
        assert!((factor - 1.0).abs() < 1e-6);
    }
}
