//! Camera void — live webcam (or other browser MediaStream) as a layer.
//!
//! The first void to consume external input. The browser owns a `<video>`
//! element fed by `getUserMedia()`; each render-loop tick, the WASM bridge
//! hands the video element to this void via [`Void::upload_external_image`],
//! and we copy the current frame into an aux texture using
//! [`wgpu::Queue::copy_external_image_to_texture`]. The shader then samples
//! that aux texture, applies the user's scale / rotation / pan transforms,
//! and writes to the layer's destination texture.
//!
//! Aspect handling is "cover": at scale=1, pan=(0,0) the webcam fills the
//! layer and the short axis is cropped. Out-of-frame samples (e.g. after a
//! large pan) return transparent.
//!
//! Native: the upload path is unreachable because [`crate::gpu::void::ExternalImageSource`]
//! has no variants on non-wasm targets. The void can still be registered and
//! its layer added, but it will render as the transparent placeholder until a
//! frame is supplied — which only the browser bridge can do.

use crate::gpu::effect::{EffectCache, EffectPipeline};
use crate::gpu::void::{ExternalImageSource, ParamDef, ParamValue, Void, VoidRegistration};
use std::cell::Cell;
use std::sync::Arc;

pub const TYPE_ID: &str = "camera";

const PARAMS: &[ParamDef] = &[
    // Zoom about the layer center. 1.0 = "cover" fit (default). <1 zooms
    // out (lets letterbox / surrounding-pan show as transparent); >1 zooms in.
    ParamDef::Float {
        name: "scale",
        min: 0.1,
        max: 4.0,
        default: 1.0,
    },
    // CCW rotation in degrees. UI presents degrees because 0–360 is more
    // intuitive than -π–π on a slider; the shader converts to radians.
    ParamDef::Float {
        name: "rotation",
        min: 0.0,
        max: 360.0,
        default: 0.0,
    },
    // Pan in fractions of canvas width / height. ±1 shifts the source by a
    // full canvas dimension, which combined with scale<1 lets users surface
    // the cropped-out regions of the cover-fit frame.
    ParamDef::Float {
        name: "pan_x",
        min: -1.0,
        max: 1.0,
        default: 0.0,
    },
    ParamDef::Float {
        name: "pan_y",
        min: -1.0,
        max: 1.0,
        default: 0.0,
    },
    // Horizontal mirror (selfie mode). Defaults to ON because that's what
    // every video-call app shows the user — the webcam comes in pointed
    // at them, and they expect their reflection, not a backwards view.
    ParamDef::Bool {
        name: "mirror_h",
        default: true,
    },
    // Vertical mirror — niche but useful for some kinds of mount /
    // teleprompter setups. Default off.
    ParamDef::Bool {
        name: "mirror_v",
        default: false,
    },
    // Freeze the layer on its last received frame. When on, the void stops
    // accepting external image uploads (`wants_external_input` returns
    // false) and the JS-side `CameraSource` is torn down so the OS camera
    // indicator turns off. Toggle back off to resume the live feed.
    ParamDef::Bool {
        name: "freeze",
        default: false,
    },
    // How many rAF frames to skip between webcam → GPU uploads. 1 = upload
    // every frame (live 60fps), 4 = upload every 4th frame (~15fps at 60Hz
    // rAF, the default). Higher values trade smoothness for GPU/CPU savings
    // — each skipped tick avoids a JS `drawImage` blit, a
    // `copy_external_image_to_texture`, the void's canvas-resolution
    // fragment shader, and the full compositor re-encode that an upload
    // would trigger. The JS-side `CameraSource.tick()` reads this value
    // from the layer params and gates its own upload accordingly; nothing
    // here reads the field at render time.
    ParamDef::Int {
        name: "frame_divisor",
        min: 1,
        max: 60,
        default: 4,
    },
];

pub fn register() -> VoidRegistration {
    VoidRegistration {
        type_id: TYPE_ID,
        display_name: "Camera",
        params: PARAMS,
        create_pipeline,
        from_params: |params, shared| Box::new(Camera::from_params(params, shared)),
    }
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniforms {
    scale: f32,
    rotation_rad: f32,
    pan_x: f32,
    pan_y: f32,
    webcam_w: f32,
    webcam_h: f32,
    canvas_w: f32,
    canvas_h: f32,
    /// 0.0 or 1.0 — used in the shader as a sign-flip on the source-x
    /// offset. f32 (not bool/u32) so the shader can do
    /// `1.0 - 2.0 * mirror_h` without casts.
    mirror_h: f32,
    mirror_v: f32,
    _pad0: f32,
    _pad1: f32,
}

#[derive(Clone, Debug)]
pub struct Camera {
    scale: f32,
    rotation_deg: f32,
    pan_x: f32,
    pan_y: f32,
    mirror_h: bool,
    mirror_v: bool,
    freeze: bool,
    /// Rate-limit divisor for webcam → GPU uploads (1 = every rAF frame,
    /// N = every Nth). Stored here as the source of truth; the JS-side
    /// `CameraSource.tick()` reads it through the layer-tree reconciliation
    /// and gates its uploads accordingly. Never read at render time on
    /// the Rust side.
    frame_divisor: u32,
    /// Current source dimensions (updated on each frame upload). 1×1 until
    /// the first frame arrives — matching the placeholder aux texture.
    webcam_w: u32,
    webcam_h: u32,
    /// Canvas dimensions cached from `create_cache`. `Cell` because the trait
    /// gives us `&self` there; `upload_external_image` reads these to rewrite
    /// the uniforms when the webcam resolution changes.
    canvas_w: Cell<u32>,
    canvas_h: Cell<u32>,
    shared: Arc<EffectPipeline>,
}

impl Camera {
    fn from_params(params: &[ParamValue], shared: Arc<EffectPipeline>) -> Self {
        let scale = match params.first() {
            Some(ParamValue::Float(v)) => *v,
            _ => 1.0,
        };
        let rotation_deg = match params.get(1) {
            Some(ParamValue::Float(v)) => *v,
            _ => 0.0,
        };
        let pan_x = match params.get(2) {
            Some(ParamValue::Float(v)) => *v,
            _ => 0.0,
        };
        let pan_y = match params.get(3) {
            Some(ParamValue::Float(v)) => *v,
            _ => 0.0,
        };
        let mirror_h = match params.get(4) {
            Some(ParamValue::Bool(v)) => *v,
            _ => true,
        };
        let mirror_v = match params.get(5) {
            Some(ParamValue::Bool(v)) => *v,
            _ => false,
        };
        let freeze = match params.get(6) {
            Some(ParamValue::Bool(v)) => *v,
            _ => false,
        };
        let frame_divisor = match params.get(7) {
            Some(ParamValue::Int(v)) => (*v).max(1) as u32,
            _ => 4,
        };
        Camera {
            scale,
            rotation_deg,
            pan_x,
            pan_y,
            mirror_h,
            mirror_v,
            freeze,
            frame_divisor,
            webcam_w: 1,
            webcam_h: 1,
            canvas_w: Cell::new(1),
            canvas_h: Cell::new(1),
            shared,
        }
    }

    fn uniforms(&self) -> CameraUniforms {
        CameraUniforms {
            scale: self.scale,
            rotation_rad: self.rotation_deg.to_radians(),
            pan_x: self.pan_x,
            pan_y: self.pan_y,
            webcam_w: self.webcam_w as f32,
            webcam_h: self.webcam_h as f32,
            canvas_w: self.canvas_w.get() as f32,
            canvas_h: self.canvas_h.get() as f32,
            mirror_h: if self.mirror_h { 1.0 } else { 0.0 },
            mirror_v: if self.mirror_v { 1.0 } else { 0.0 },
            _pad0: 0.0,
            _pad1: 0.0,
        }
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
        self.webcam_w = w;
        self.webcam_h = h;

        // Fresh sampler each rebuild — wgpu reuses internal handles so
        // this is essentially free and avoids threading the compositor's
        // shared sampler through every call site.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("void-camera-sampler"),
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
        label: Some("void-camera-frame"),
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
        // per the WebGPU spec; TEXTURE_BINDING for shader sampling;
        // COPY_DST for the texel copy fallback path.
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
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
    [bg("void-camera-bg-0"), bg("void-camera-bg-1")]
}

impl Void for Camera {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn clone_boxed(&self) -> Box<dyn Void> {
        Box::new(self.clone())
    }

    fn param_values(&self) -> Vec<ParamValue> {
        vec![
            ParamValue::Float(self.scale),
            ParamValue::Float(self.rotation_deg),
            ParamValue::Float(self.pan_x),
            ParamValue::Float(self.pan_y),
            ParamValue::Bool(self.mirror_h),
            ParamValue::Bool(self.mirror_v),
            ParamValue::Bool(self.freeze),
            ParamValue::Int(self.frame_divisor as i32),
        ]
    }

    fn needs_animation(&self) -> bool {
        // The camera doesn't accumulate time on its own, but the compositor
        // uses `needs_animation()` as the "keep the rAF loop alive" signal.
        // Without it, the void would only re-render on param changes, and
        // live webcam frames would freeze on the first one we uploaded.
        // When frozen, the last frame is held forever — no animation
        // needed, so we stop keeping the rAF loop alive. The visibility
        // half of the gate (don't animate a hidden layer) is the engine's
        // job; this method only knows about kind-specific state.
        !self.freeze
    }

    fn update_params(&mut self, queue: &wgpu::Queue, cache: &EffectCache, params: &[ParamValue]) {
        // In-place: update fields and rewrite the uniform buffer. We do
        // NOT touch `cache.aux_textures` — that's where the live webcam
        // frame lives, and toggling `freeze` (or any other param) must
        // not wipe it. The bind group continues to reference the same
        // texture view, so the next encode samples whatever was last
        // uploaded.
        self.scale = match params.first() {
            Some(ParamValue::Float(v)) => *v,
            _ => self.scale,
        };
        self.rotation_deg = match params.get(1) {
            Some(ParamValue::Float(v)) => *v,
            _ => self.rotation_deg,
        };
        self.pan_x = match params.get(2) {
            Some(ParamValue::Float(v)) => *v,
            _ => self.pan_x,
        };
        self.pan_y = match params.get(3) {
            Some(ParamValue::Float(v)) => *v,
            _ => self.pan_y,
        };
        self.mirror_h = match params.get(4) {
            Some(ParamValue::Bool(v)) => *v,
            _ => self.mirror_h,
        };
        self.mirror_v = match params.get(5) {
            Some(ParamValue::Bool(v)) => *v,
            _ => self.mirror_v,
        };
        self.freeze = match params.get(6) {
            Some(ParamValue::Bool(v)) => *v,
            _ => self.freeze,
        };
        self.frame_divisor = match params.get(7) {
            Some(ParamValue::Int(v)) => (*v).max(1) as u32,
            _ => self.frame_divisor,
        };
        if let Some(buf) = cache.uniform_bufs.first() {
            queue.write_buffer(buf, 0, bytemuck::bytes_of(&self.uniforms()));
        }
    }

    fn wants_external_input(&self) -> bool {
        // While frozen, refuse new frames so the displayed image is whatever
        // was in the aux texture at the moment freeze was toggled on. The
        // visibility half of the gate (don't upload to a hidden layer) is
        // the engine's job at the `upload_void_external_image` boundary.
        !self.freeze
    }

    fn persistent_frame_size(&self) -> Option<(u32, u32)> {
        // Only report a size once a real frame has been received. Until
        // the first upload (webcam_w/h are the placeholder 1×1), the
        // texture has nothing meaningful in it — don't poison saves with
        // a 1×1 black frame.
        if self.webcam_w > 1 && self.webcam_h > 1 {
            Some((self.webcam_w, self.webcam_h))
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
        // Bytes are already Rgba8Unorm-packed (the format the save flow
        // read back). Direct queue.write_texture is the symmetric load
        // path matching raster's `upload_node_pixels`.
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

            // Push the latest uniforms (webcam_w/h just changed on realloc;
            // params don't change here but rewriting is cheap and avoids a
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
        // rewrite uniforms when the webcam resolution changes.
        self.canvas_w.set(render_width.max(1));
        self.canvas_h.set(render_height.max(1));

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("void-camera-uniforms"),
            size: std::mem::size_of::<CameraUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&self.uniforms()));

        // 1×1 transparent placeholder until the first frame upload. The
        // placeholder satisfies the bind group's texture binding so the
        // pipeline can run before a webcam frame is available.
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
            label: Some("void-camera-encode"),
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

fn create_pipeline(device: &wgpu::Device, format: wgpu::TextureFormat) -> EffectPipeline {
    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("void-camera-bgl"),
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
        label: Some("void-camera-pipeline-layout"),
        bind_group_layouts: &[&bind_group_layout],
        immediate_size: 0,
    });

    let src = include_str!("../../../../../shaders/voids/camera.wgsl");
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("void-camera-shader"),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("void-camera-pipeline"),
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

    fn default_params() -> Vec<ParamValue> {
        PARAMS.iter().map(ParamDef::default_value).collect()
    }

    fn fake_pipeline() -> Arc<EffectPipeline> {
        let (device, _queue) = crate::gpu::test_utils::test_device();
        Arc::new(create_pipeline(&device, wgpu::TextureFormat::Rgba8Unorm))
    }

    #[test]
    fn param_round_trip() {
        // Default params round-trip through from_params → param_values.
        // Order matches PARAMS: scale, rotation, pan_x, pan_y,
        // mirror_h, mirror_v, freeze, frame_divisor. mirror_h defaults to
        // ON (selfie mode); everything else off / zero / identity except
        // frame_divisor which defaults to 4 (~15fps at 60Hz rAF).
        let cam = Camera::from_params(&default_params(), fake_pipeline());
        let out = cam.param_values();
        assert_eq!(out.len(), 8);
        assert_eq!(out[0], ParamValue::Float(1.0));
        assert_eq!(out[1], ParamValue::Float(0.0));
        assert_eq!(out[2], ParamValue::Float(0.0));
        assert_eq!(out[3], ParamValue::Float(0.0));
        assert_eq!(out[4], ParamValue::Bool(true), "mirror_h defaults on");
        assert_eq!(out[5], ParamValue::Bool(false), "mirror_v defaults off");
        assert_eq!(out[6], ParamValue::Bool(false), "freeze defaults off");
        assert_eq!(out[7], ParamValue::Int(4), "frame_divisor defaults to 4");
    }

    #[test]
    fn frame_divisor_round_trip() {
        // The JS side reads `frame_divisor` from the layer-tree params via
        // `param_values` to throttle its `tick()` uploads. Verify update_params
        // mutates the field in place and the new value flows back out.
        let (_device, queue) = crate::gpu::test_utils::test_device();
        let pipeline = fake_pipeline();
        let mut cam = Camera::from_params(&default_params(), pipeline);
        assert_eq!(cam.frame_divisor, 4);

        let uniform_buf = _device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: std::mem::size_of::<CameraUniforms>() as u64,
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
        new_params[7] = ParamValue::Int(8);
        cam.update_params(&queue, &cache, &new_params);
        assert_eq!(cam.frame_divisor, 8);
        assert_eq!(cam.param_values()[7], ParamValue::Int(8));

        // Out-of-range values are clamped to >= 1 — divisor 0 would mean
        // "upload every 0th frame" which is undefined; the JS gate uses
        // `counter % divisor` so a zero divisor would panic on modulo.
        new_params[7] = ParamValue::Int(0);
        cam.update_params(&queue, &cache, &new_params);
        assert_eq!(cam.frame_divisor, 1, "divisor clamps up to 1");
    }

    #[test]
    fn freeze_stops_external_input() {
        // wants_external_input is the gate the compositor uses to drop
        // uploads from the JS side; once `freeze` flips on, that gate
        // should close so subsequent webcam frames are ignored. freeze
        // is the 7th param (index 6) — mirror_h and mirror_v come
        // between the transforms and freeze.
        let mut params = default_params();
        params[6] = ParamValue::Bool(true);
        let cam = Camera::from_params(&params, fake_pipeline());
        assert!(!cam.wants_external_input());
        assert!(!cam.needs_animation());
    }

    /// Regression: toggling any param (notably `freeze`) must not wipe the
    /// camera's accumulated GPU state — earlier the compositor's
    /// `update_void_layer_params` rebuilt the void from `from_params` and
    /// re-allocated `EffectCache`, dropping the aux texture that holds the
    /// live webcam frame. The user reported "clicking freeze disappears
    /// the whole layer" because the rebuild reset `webcam_w/h` to the 1×1
    /// placeholder. `update_params` must mutate fields in place.
    #[test]
    fn update_params_preserves_webcam_dimensions() {
        let (device, queue) = crate::gpu::test_utils::test_device();
        let pipeline = Arc::new(create_pipeline(&device, wgpu::TextureFormat::Rgba8Unorm));
        let mut cam = Camera::from_params(&default_params(), pipeline);

        // Pretend an upload arrived and set the live dimensions.
        cam.webcam_w = 640;
        cam.webcam_h = 480;

        // Minimal cache — just a uniform buffer, which is all
        // `update_params` writes to. (We deliberately don't include any
        // aux textures here; the test is asserting state on Camera, not
        // on cache shape.)
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("test-uniforms"),
            size: std::mem::size_of::<CameraUniforms>() as u64,
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

        // Toggle freeze on via update_params. freeze is the 7th param
        // (index 6) — mirror_h/v sit between the transforms and freeze.
        let mut new_params = default_params();
        new_params[6] = ParamValue::Bool(true);
        cam.update_params(&queue, &cache, &new_params);

        // Webcam dimensions survive — the saved frame texture would too.
        assert_eq!(cam.webcam_w, 640);
        assert_eq!(cam.webcam_h, 480);
        // The freeze toggle did take effect.
        assert!(cam.freeze);
        assert!(!cam.wants_external_input());
    }

    #[test]
    fn uniforms_layout_matches_wgsl() {
        // The WGSL `Params` struct in camera.wgsl has 12 f32s — 48 bytes
        // (multiple of 16 as uniform-buffer layout requires). If we ever
        // add/reorder a field, this catches the drift.
        assert_eq!(std::mem::size_of::<CameraUniforms>(), 48);
        assert_eq!(std::mem::size_of::<CameraUniforms>() % 16, 0);
        assert_eq!(std::mem::align_of::<CameraUniforms>(), 4);
    }

    #[test]
    fn cover_fit_math_landscape_webcam_square_canvas() {
        // 16:9 camera, 1:1 canvas, scale=1, no rotation, no pan.
        // Shader maps dest-x ∈ [-0.5, +0.5] → src-x-centered ∈ [-0.5·f, +0.5·f]
        // with f = ca / wa. The visible source-x range therefore has width f.
        // For cover we want f < 1 (the long axis is cropped); the y axis
        // should be untouched, so its visible range stays exactly 1.
        let webcam_aspect = 16.0_f32 / 9.0;
        let canvas_aspect = 1.0_f32;
        let factor = canvas_aspect / webcam_aspect;
        assert!(factor < 1.0);
        assert!((factor - 9.0 / 16.0).abs() < 1e-5);
        let visible_width_in_source = factor;
        let visible_height_in_source = 1.0_f32;
        assert!(visible_width_in_source < visible_height_in_source);
    }

    #[test]
    fn cover_fit_math_portrait_webcam_square_canvas() {
        // 9:16 camera, 1:1 canvas → y axis shrinks instead. Symmetric to the
        // landscape case.
        let webcam_aspect = 9.0_f32 / 16.0;
        let canvas_aspect = 1.0_f32;
        let factor = webcam_aspect / canvas_aspect;
        assert!(factor < 1.0);
        assert!((factor - 9.0 / 16.0).abs() < 1e-5);
    }

    #[test]
    fn cover_fit_math_matching_aspects_is_identity() {
        // Square camera on square canvas: no crop, no letterbox. Either
        // branch of the shader's if/else collapses to a multiplication by 1.
        let webcam_aspect = 1.0_f32;
        let canvas_aspect = 1.0_f32;
        let factor = canvas_aspect / webcam_aspect;
        assert!((factor - 1.0).abs() < 1e-6);
    }
}
