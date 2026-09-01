//! Textured void — a sampled source image drawn through a stored transform.
//!
//! The generic machinery behind every void whose pixels come from an image
//! rather than from a formula: the camera and screenshare streams, the Blender
//! HTTP feed, and placed still images (smart objects). All of them are "source
//! texture → inverse-transform sample → layer texture" at heart; what differs
//! is metadata, where the pixels come from ([`VoidSource`]), how the source is
//! fitted to the canvas ([`ContentFit`]), and the seeded initial gizmo
//! transform. Those differences live in a [`TexturedVoidConfig`], one
//! `&'static` per kind, and the per-kind files under [`super::voids`] stay tiny.
//!
//! For streaming kinds the browser owns a `<video>` element fed by
//! `getUserMedia()` / `getDisplayMedia()`; each render-loop tick the WASM bridge
//! hands the element to this void via [`Void::upload_external_image`]. For
//! one-shot kinds the engine installs the decoded image once via
//! [`Void::set_source_pixels`]. Either way the shader samples that source,
//! applies the inverse of the user's transform, and writes the layer texture.
//!
//! **Scaling.** A one-shot source is the authority and is never resampled in
//! place: the stored transform changes, and each frame re-samples the pristine
//! texels. It carries a full mip chain so minification averages the texels a
//! screen pixel actually covers instead of point-sampling four of them, which
//! is what keeps a shrunk image from shimmering. Streaming sources skip the
//! chain — regenerating it per frame costs more than it buys.
//!
//! **Boundary.** The silhouette is the source rect carried through the stored
//! transform, and the shader antialiases it analytically: alpha is scaled by the
//! fraction of each fragment's footprint that falls inside the source, computed
//! from the screen-space derivatives of the sampled UV. That is a
//! one-destination-pixel edge whatever the scale, and it stays correct at every
//! mip level, which is what a transparent border around the source cannot do —
//! each reduction halves the border's width relative to its own level, so a
//! minified image ends up clamping a nearly opaque edge texel across the canvas.
//!
//! **Alpha convention:** the aux texture stores **premultiplied** texels, so the
//! sampler's linear filter interpolates correctly at alpha edges — filtering
//! straight alpha darkens color toward transparent-black neighbors (dark halos;
//! docs/lessons-learned/compositing-lessons-learned.md #2). Every writer
//! honors it: the live upload converts during the copy
//! (`premultiplied_alpha: true`), and save/load round-trips the raw texels
//! unchanged. The shader un-premultiplies after sampling, so the void still
//! emits the straight alpha the compositor expects. Camera/display frames are
//! opaque, where premultiplication is the identity.
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

use crate::coord::CanvasRect;
use crate::gpu::effect::{EffectCache, EffectPipeline};
use crate::gpu::void::{
    ContentRect, DirtyFlag, ExternalImageSource, ParamDef, ParamValue, Void, VoidRegistration,
    VoidSource,
};
use std::sync::Arc;

/// How a source image is fitted to the canvas at the identity transform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentFit {
    /// Fill the canvas, cropping the short axis — the content rect overhangs
    /// the canvas on the long axis. Anchored to the canvas *window*, so it
    /// keeps filling the frame when the canvas is cropped. What a live camera
    /// or screen capture wants.
    Cover,
    /// Occupy the source's own pixel dimensions, anchored in the document
    /// plane. Does not move when the canvas is cropped — a placed image stays
    /// where the user put it relative to the artwork, not to the window.
    Natural,
}

/// Static per-variant description of a video-stream void. One of these is
/// declared `&'static` in each variant file (camera / screenshare) and threaded
/// into the shared machinery — the variant files hold nothing else of substance.
#[derive(Debug)]
pub struct TexturedVoidConfig {
    pub type_id: &'static str,
    pub display_name: &'static str,
    /// One-sentence summary shown as a tooltip in the Add Void picker —
    /// include the terms users would search for.
    pub description: &'static str,
    pub icon: &'static str,
    /// Param schema — looked up by *name* (`"freeze"`, `"frame_divisor"`) by the
    /// shared code, so variants are decoupled from each other's param ordering.
    pub params: &'static [ParamDef],
    /// Where this kind's pixels come from. Drives external-input plumbing, the
    /// animation clock, and mip-chain generation — all three follow from
    /// whether the source streams or is installed once.
    pub source: VoidSource,
    /// How the source is fitted to the canvas at the identity transform, and
    /// therefore which frame its content rect is anchored in.
    pub fit: ContentFit,
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
    config: &'static TexturedVoidConfig,
    from_params: fn(&[ParamValue], Arc<EffectPipeline>) -> Box<dyn Void>,
) -> VoidRegistration {
    VoidRegistration {
        type_id: config.type_id,
        display_name: config.display_name,
        description: config.description,
        params: config.params,
        icon: config.icon,
        // The aux texture is a 1×1 placeholder until a frame arrives, so
        // there is nothing meaningful to render, and nothing to animate.
        preview: None,
        supports_live_transform: true,
        source: config.source,
        default_transform: config.default_transform,
        create_pipeline,
        from_params,
    }
}

/// Construct a [`TexturedVoid`] for a given static config — the body behind
/// each variant's `from_params` fn pointer.
pub fn build_void(
    config: &'static TexturedVoidConfig,
    params: &[ParamValue],
    shared: Arc<EffectPipeline>,
) -> Box<dyn Void> {
    Box::new(TexturedVoid::from_params(config, params, shared))
}

/// Index of the named param within a config's schema, or `None` if absent.
fn param_index(config: &TexturedVoidConfig, name: &str) -> Option<usize> {
    config.params.iter().position(|p| p.name == name)
}

/// Read the `"freeze"` toggle out of a positional param slice by resolving its
/// name to an index in the config schema first. Defaults to `false`.
fn read_freeze(config: &TexturedVoidConfig, params: &[ParamValue]) -> bool {
    match param_index(config, "freeze").and_then(|i| params.get(i)) {
        Some(ParamValue::Bool(v)) => *v,
        _ => false,
    }
}

/// Read the `"frame_divisor"` throttle by name, clamped to `>= 1`. Defaults to
/// 4 (the schema default) when absent.
fn read_frame_divisor(config: &TexturedVoidConfig, params: &[ParamValue]) -> u32 {
    match param_index(config, "frame_divisor").and_then(|i| params.get(i)) {
        Some(ParamValue::Int(v)) => (*v).max(1) as u32,
        _ => 4,
    }
}

/// Normalize an incoming param slice to exactly the config's schema length,
/// filling missing/short entries with each def's declared default. Backs
/// `param_snapshot`, so passthrough params (e.g. `url`) always have a value to
/// echo back even if a caller supplied a shorter slice.
fn normalize_params(config: &TexturedVoidConfig, params: &[ParamValue]) -> Vec<ParamValue> {
    config
        .params
        .iter()
        .enumerate()
        .map(|(i, def)| {
            params
                .get(i)
                .cloned()
                .unwrap_or_else(|| def.default_value())
        })
        .collect()
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct TexturedVoidUniforms {
    /// Inverse of the user transform's homography, row 0: `[m00, m01, m02, _]`.
    /// The shader maps a window-local fragment (relative to the content rect)
    /// through this inverse (with the perspective divide via the shared
    /// `proj_local`) to find the pre-transform position, then normalizes to a
    /// source UV. Same packing as
    /// [`crate::gpu::transform::TransformBlendUniforms`] (via `pack_inv_rows`).
    inv_row0: [f32; 4],
    /// Inverse homography row 1: `[m10, m11, m12, _]`.
    inv_row1: [f32; 4],
    /// Inverse homography row 2: `[m20, m21, m22, _]`. Affine is the special
    /// case `[0, 0, 1, _]` (perspective divide collapses to `w ≡ 1`).
    inv_row2: [f32; 4],
    /// Window-local origin of the content rect (see [`Void::content_extent`]),
    /// converted from plane space by [`ContentRect::to_window`]. Negative on a
    /// `Cover` void's overhanging axis. The fit is baked into this rect, so the
    /// shader needs no separate canvas dims.
    content_origin: [f32; 2],
    /// Window-local size of the content rect.
    content_size: [f32; 2],
}

#[derive(Debug)]
pub struct TexturedVoid {
    /// Static per-kind description (display name, icon, capture kind, seed
    /// transform). The trait's `type_id()` reads `config.type_id`.
    config: &'static TexturedVoidConfig,
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
    /// Snapshot of every param value in the config's schema order, kept in sync
    /// with the document by `from_params` / `update_params`. `freeze` and
    /// `frame_divisor` are modeled by the typed fields above and read from them;
    /// any *other* param a config declares (e.g. the Blender void's `url`) is
    /// opaque passthrough — never interpreted here, but stored so `param_values`
    /// echoes the user's edits back for save/load and the frontend reconciler.
    /// This keeps the shared machinery generic: a config can add a
    /// frontend-only param without a bespoke field.
    param_snapshot: Vec<ParamValue>,
    /// Current source dimensions. 1×1 until a
    /// source arrives — matching the placeholder aux texture.
    src_w: u32,
    src_h: u32,
    /// Whether a real source has been installed. Tracked explicitly rather
    /// than inferred from `src_w`/`src_h` exceeding the 1×1 placeholder, so a
    /// legitimately 1×N or N×1 image still saves.
    has_source: bool,
    /// The canvas rect as of the last `create_cache` / `set_canvas_rect`. The
    /// uniform is written in window-local coordinates, so it has to be rebuilt
    /// whenever the canvas is resized or cropped as well as whenever the source
    /// resolution changes.
    canvas: CanvasRect,
    shared: Arc<EffectPipeline>,
    dirty: DirtyFlag,
}

impl Clone for TexturedVoid {
    fn clone(&self) -> Self {
        // `clone_boxed` is called for undo / clone_subtree. The clone gets a
        // fresh `EffectCache` from `ensure_void_layer` with the 1×1
        // placeholder aux texture, so start dirty.
        TexturedVoid {
            config: self.config,
            transform: self.transform,
            freeze: self.freeze,
            frame_divisor: self.frame_divisor,
            param_snapshot: self.param_snapshot.clone(),
            src_w: self.src_w,
            src_h: self.src_h,
            has_source: self.has_source,
            canvas: self.canvas,
            shared: self.shared.clone(),
            dirty: DirtyFlag::new_dirty(),
        }
    }
}

impl TexturedVoid {
    fn from_params(
        config: &'static TexturedVoidConfig,
        params: &[ParamValue],
        shared: Arc<EffectPipeline>,
    ) -> Self {
        TexturedVoid {
            config,
            // Transform is not a param — it lives on the layer and is applied
            // via `set_transform`. New instances start at identity; the
            // compositor pushes the layer's stored transform (the seeded flip
            // for cameras, or any edit) after creation.
            transform: crate::transform::Transform::identity(),
            freeze: read_freeze(config, params),
            frame_divisor: read_frame_divisor(config, params),
            param_snapshot: normalize_params(config, params),
            src_w: 1,
            src_h: 1,
            has_source: false,
            canvas: CanvasRect::from_xywh(0, 0, 1, 1),
            shared,
            dirty: DirtyFlag::new_dirty(),
        }
    }

    fn uniforms(&self) -> TexturedVoidUniforms {
        // Sample through the inverse of the user transform's homography (shared
        // packing; singular matrices fall back to identity rather than NaN-ing
        // the UV). Affine transforms carry a `[0,0,1]` bottom row, so the
        // shader's perspective divide is a no-op for them.
        let [inv_row0, inv_row1, inv_row2] =
            crate::gpu::transform::pack_inv_rows(&self.transform.to_projective());
        // `content_rect` answers in plane space; the shader indexes
        // window-local fragments, so convert once, here.
        let local = self.content_rect(self.canvas).to_window(self.canvas);
        TexturedVoidUniforms {
            inv_row0,
            inv_row1,
            inv_row2,
            content_origin: [local.x, local.y],
            content_size: [local.width, local.height],
        }
    }

    /// The source's active pixels, **in plane space**. Backs both the uniform
    /// and [`Void::content_extent`] (the gizmo bbox), so the on-canvas handles
    /// wrap exactly the pixels the shader samples.
    ///
    /// Before a source arrives there is no meaningful aspect to fit, so every
    /// fit falls back to the canvas window — which is what keeps a
    /// freshly-added camera layer reporting the canvas as its bbox.
    fn content_rect(&self, canvas: CanvasRect) -> ContentRect {
        if !self.has_source {
            return ContentRect::covering(canvas);
        }
        let (sw, sh) = (self.src_w as f32, self.src_h as f32);
        match self.config.fit {
            ContentFit::Cover => {
                // Fill the window, cropping the short axis; the rect overhangs
                // on the long one. Anchored to the window, so it follows a crop.
                let (cw, ch) = (canvas.width as f32, canvas.height as f32);
                let cover = (cw / sw).max(ch / sh);
                let (content_w, content_h) = (sw * cover, sh * cover);
                ContentRect::new(
                    canvas.origin.x as f32 + (cw - content_w) * 0.5,
                    canvas.origin.y as f32 + (ch - content_h) * 0.5,
                    content_w,
                    content_h,
                )
            }
            // Natural size at the plane origin. Deliberately independent of
            // `canvas`: cropping the canvas must not move placed content.
            ContentFit::Natural => ContentRect::new(0.0, 0.0, sw, sh),
        }
    }

    /// CPU mirror of `textured.wgsl`'s fragment mapping: a window-local
    /// fragment `(fx, fy)` → source UV. Kept in lockstep with the shader so the
    /// composition (inverse-affine → cover-fit) can be unit-tested without a
    /// live stream. **If you edit the WGSL `fs_main` math, edit this too** (and
    /// vice-versa); the tests pin them together.
    #[cfg(test)]
    fn src_uv(u: &TexturedVoidUniforms, frag: (f32, f32)) -> (f32, f32) {
        // Window-local fragment → content-local → inverse homography (with the
        // perspective divide, mirroring `proj_local`) → normalize.
        let cl = (frag.0 - u.content_origin[0], frag.1 - u.content_origin[1]);
        let hx = u.inv_row0[0] * cl.0 + u.inv_row0[1] * cl.1 + u.inv_row0[2];
        let hy = u.inv_row1[0] * cl.0 + u.inv_row1[1] * cl.1 + u.inv_row1[2];
        let hw = u.inv_row2[0] * cl.0 + u.inv_row2[1] * cl.1 + u.inv_row2[2];
        let ux = (hx / hw) / u.content_size[0];
        let uy = (hy / hw) / u.content_size[1];
        (ux, uy)
    }

    /// Whether this kind's source is installed once and then sampled
    /// repeatedly, which is what makes a mip chain worth building.
    fn is_one_shot(&self) -> bool {
        !self.config.source.is_streaming()
    }

    /// Replace the aux source texture with a fresh `w × h` allocation and
    /// rebuild bind group 0 to reference it. Shared by the live-upload path
    /// (`upload_external_image` on a resolution change) and `set_source_pixels`.
    fn resize_aux_texture(
        &mut self,
        device: &wgpu::Device,
        cache: &mut EffectCache,
        w: u32,
        h: u32,
    ) {
        let (tex, view) = make_frame_texture(device, w, h, self.is_one_shot());
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
        // ClampToEdge keeps a sample just past the image reading the edge texel
        // rather than wrapping; the shader's coverage term is what decides how
        // much of it survives, so the address mode never shows on its own.
        //
        // Trilinear + anisotropy only mean anything with a chain behind them,
        // so a streaming source (allocated with a single level) asks for
        // neither: `anisotropy_clamp > 1` is a validation error unless all
        // three filters are linear, and a mipmap filter over one level is
        // wasted state.
        let one_shot = self.is_one_shot();
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("textured-void-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: if one_shot {
                wgpu::MipmapFilterMode::Linear
            } else {
                wgpu::MipmapFilterMode::Nearest
            },
            anisotropy_clamp: if one_shot { 16 } else { 1 },
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

/// Allocate a source texture for a `w × h` image. `mipped` requests the full
/// chain, for sources installed once and sampled at many scales.
fn make_frame_texture(
    device: &wgpu::Device,
    w: u32,
    h: u32,
    mipped: bool,
) -> (wgpu::Texture, wgpu::TextureView) {
    let (w, h) = (w.max(1), h.max(1));
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("textured-void-source"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: if mipped {
            crate::gpu::rescale::levels_for(w, h)
        } else {
            1
        },
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        // RENDER_ATTACHMENT is required by `copy_external_image_to_texture`
        // per the WebGPU spec; TEXTURE_BINDING for shader sampling; COPY_DST
        // for live frame uploads and the load-time restore; COPY_SRC so the
        // save flow can read this persistent frame back (the readback's
        // `copy_texture_to_buffer` is a validation error without it).
        // RENDER_ATTACHMENT doubles as the mip-chain generator's target.
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
    [bg("textured-void-bg-0"), bg("textured-void-bg-1")]
}

impl Void for TexturedVoid {
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
            .enumerate()
            .map(|(i, def)| match def.name {
                "freeze" => ParamValue::Bool(self.freeze),
                "frame_divisor" => ParamValue::Int(self.frame_divisor as i32),
                // Passthrough param (e.g. `url`): echo the stored value so the
                // user's edits round-trip through save/load and reach the
                // frontend reconciler. Falls back to the schema default if the
                // snapshot is somehow short.
                _ => self
                    .param_snapshot
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| def.default_value()),
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
        self.param_snapshot = normalize_params(self.config, params);
        cache.write_uniform(queue, 0, bytemuck::bytes_of(&self.uniforms()));
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
        cache.write_uniform(queue, 0, bytemuck::bytes_of(&self.uniforms()));
        self.dirty.mark();
    }

    fn content_extent(&self, canvas: CanvasRect) -> ContentRect {
        // Answer from the caller's live canvas rect, not the cached copy, so
        // the gizmo bbox is right on the same frame a crop lands.
        self.content_rect(canvas)
    }

    fn set_canvas_rect(&mut self, queue: &wgpu::Queue, cache: &EffectCache, canvas: CanvasRect) {
        if self.canvas == canvas {
            return;
        }
        // The uniform is expressed in window-local coordinates and, for a
        // `Cover` fit, derived from the canvas size — both change here, so
        // rewrite it in place. Without this the sampler keeps mapping the
        // source across the *old* window while the gizmo reports the new one.
        self.canvas = canvas;
        cache.write_uniform(queue, 0, bytemuck::bytes_of(&self.uniforms()));
        self.dirty.mark();
    }

    fn wants_external_input(&self) -> bool {
        // Only streaming sources take frames at all; a one-shot image is
        // installed once through `set_source_pixels`. While frozen, refuse new
        // frames so the displayed image is whatever was in the source texture
        // at the moment freeze was toggled on — the stream stays open on the JS
        // side, so unfreezing resumes immediately. The visibility half of the
        // gate (don't upload to a hidden layer) is the engine's job at the
        // `upload_void_external_image` boundary.
        self.config.source.is_streaming() && !self.freeze
    }

    fn persistent_frame_size(&self) -> Option<(u32, u32)> {
        // Only report a size once a real source has been installed; until then
        // the texture holds the placeholder and there is nothing worth saving.
        // Keyed on the explicit flag, not on the dimensions, so a legitimately
        // 1-pixel-wide image still round-trips.
        self.has_source.then_some((self.src_w, self.src_h))
    }

    fn allocate_source(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cache: &mut EffectCache,
        width: u32,
        height: u32,
    ) {
        if width == 0 || height == 0 {
            return;
        }
        self.resize_aux_texture(device, cache, width, height);
        // `has_source` is what switches `content_rect` off its canvas-covering
        // fallback and onto the source's natural size, and what makes
        // `persistent_frame_size` report.
        self.has_source = true;
        // Both of those feed the sampling uniform, so it is republished here
        // rather than left to whatever writes the pixels. A blit-based install
        // never touches the uniform, and the layer would sample as if the
        // source still covered the canvas until the next transform.
        cache.write_uniform(queue, 0, bytemuck::bytes_of(&self.uniforms()));
        self.dirty.mark();
    }

    fn set_source_pixels(
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
        self.allocate_source(device, queue, cache, width, height);
        // Bytes are Rgba8Unorm in the source texture's premultiplied-alpha
        // convention — the save flow read back exactly these texels, and the
        // placement path premultiplies before calling.
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
                    // The aux texture stores PREMULTIPLIED texels (see the
                    // module docs); the browser converts the source bitmap
                    // during the copy whatever its decode state.
                    premultiplied_alpha: true,
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
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _dst_view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        render_width: u32,
        render_height: u32,
    ) -> EffectCache {
        // Cache the canvas geometry; the uniform is window-local, so it has to
        // be rebuilt whenever the source resolution *or* the canvas changes.
        // `create_cache` only learns the size — `set_canvas_rect` supplies the
        // origin as soon as the compositor knows it.
        self.canvas = CanvasRect::from_xywh(
            self.canvas.origin.x,
            self.canvas.origin.y,
            render_width.max(1),
            render_height.max(1),
        );

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("textured-void-uniforms"),
            size: std::mem::size_of::<TexturedVoidUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&self.uniforms()));

        // 1×1 transparent placeholder until the first frame upload. The
        // placeholder satisfies the bind group's texture binding so the
        // pipeline can run before a frame is available.
        let (placeholder_tex, placeholder_view) =
            make_frame_texture(device, 1, 1, self.is_one_shot());

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

    // Prepend the shared inverse-homography sampler (lib/projective.wgsl) —
    // the same `proj_local` the floating commit path uses, so voids get the
    // full perspective divide without a divergent affine-only copy.
    let src = concat!(
        include_str!("../../shaders/lib/projective.wgsl"),
        "\n",
        include_str!("../../shaders/voids/textured.wgsl"),
    );
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
    use crate::gpu::void::CaptureKind;

    // A test-only config standing in for a real variant. Identical param schema
    // to camera / screenshare; identity seed transform keeps the affine math
    // easy to reason about.
    const TEST_PARAMS: &[ParamDef] = &[
        ParamDef::boolean("freeze", false),
        ParamDef::int("frame_divisor", 1, 60, 4),
    ];

    static TEST_CONFIG: TexturedVoidConfig = TexturedVoidConfig {
        type_id: "test_textured",
        display_name: "Test",
        description: "Test fixture.",
        icon: "tabler:test",
        params: TEST_PARAMS,
        source: VoidSource::Capture {
            capture: CaptureKind::Camera,
        },
        fit: ContentFit::Cover,
        default_transform: |_, _| crate::transform::Transform::identity(),
    };

    fn default_params() -> Vec<ParamValue> {
        TEST_PARAMS.iter().map(ParamDef::default_value).collect()
    }

    fn fake_pipeline() -> Arc<EffectPipeline> {
        let (device, _queue) = crate::gpu::test_utils::test_device();
        Arc::new(create_pipeline(&device, wgpu::TextureFormat::Rgba8Unorm))
    }

    fn make_void() -> TexturedVoid {
        TexturedVoid::from_params(&TEST_CONFIG, &default_params(), fake_pipeline())
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

    // A config with a passthrough `url` param on top of the shared two, standing
    // in for the Blender void — exercises that params the machinery doesn't model
    // still round-trip.
    const URL_PARAMS: &[ParamDef] = &[
        ParamDef::boolean("freeze", false),
        ParamDef::int("frame_divisor", 1, 60, 4),
        ParamDef::string("url", "http://localhost:8765/stream"),
    ];

    static URL_CONFIG: TexturedVoidConfig = TexturedVoidConfig {
        type_id: "test_url_stream",
        display_name: "Test URL",
        description: "Test fixture.",
        icon: "tabler:test",
        params: URL_PARAMS,
        source: VoidSource::Capture {
            capture: CaptureKind::Stream,
        },
        fit: ContentFit::Cover,
        default_transform: |_, _| crate::transform::Transform::identity(),
    };

    /// A passthrough param (`url`) the machinery never interprets must still
    /// round-trip: its user-edited value has to survive `from_params` →
    /// `param_values` (save/load) and `update_params` (the properties panel),
    /// or the frontend would always reconnect to the default endpoint. Before
    /// `param_snapshot`, `param_values` regenerated non-modeled params from
    /// their default and silently discarded edits.
    #[test]
    fn passthrough_param_round_trips() {
        let (_device, queue) = crate::gpu::test_utils::test_device();
        let mut v = TexturedVoid::from_params(
            &URL_CONFIG,
            &[
                ParamValue::Bool(false),
                ParamValue::Int(4),
                ParamValue::String("http://example.test/a".into()),
            ],
            fake_pipeline(),
        );
        assert_eq!(
            v.param_values()[2],
            ParamValue::String("http://example.test/a".into()),
            "the created url must echo back, not the schema default",
        );

        let uniform_buf = _device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: std::mem::size_of::<TexturedVoidUniforms>() as u64,
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

        v.update_params(
            &queue,
            &cache,
            &[
                ParamValue::Bool(false),
                ParamValue::Int(4),
                ParamValue::String("http://example.test/b".into()),
            ],
        );
        assert_eq!(
            v.param_values()[2],
            ParamValue::String("http://example.test/b".into()),
            "editing the url must persist through update_params",
        );
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
            size: std::mem::size_of::<TexturedVoidUniforms>() as u64,
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
        let v = TexturedVoid::from_params(&TEST_CONFIG, &params, fake_pipeline());
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
            size: std::mem::size_of::<TexturedVoidUniforms>() as u64,
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
            size: std::mem::size_of::<TexturedVoidUniforms>() as u64,
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
        // The WGSL `Params` struct in textured.wgsl is inv_row0[4] +
        // inv_row1[4] + inv_row2[4] + content_origin[2] + content_size[2]
        // = 16 f32s = 64 bytes. 64 is a clean multiple of 16 so no extra
        // std140 padding. Catches layout drift.
        assert_eq!(std::mem::size_of::<TexturedVoidUniforms>(), 64);
        assert_eq!(std::mem::size_of::<TexturedVoidUniforms>() % 16, 0);
        assert_eq!(std::mem::align_of::<TexturedVoidUniforms>(), 4);
    }

    /// CPU-side proof of the shader composition (inverse-affine → cover-fit),
    /// pinned to `TexturedVoid::src_uv` which mirrors
    /// `textured.wgsl::fs_main`. Square 100×100 source on a 100×100 canvas
    /// keeps cover-fit = 1, so the math is easy to reason about by hand.
    #[test]
    fn src_uv_composition() {
        let v = {
            let mut x = make_void();
            x.src_w = 100;
            x.src_h = 100;
            x.has_source = true;
            x.canvas = CanvasRect::from_xywh(0, 0, 100, 100);
            x
        };

        // Identity transform: canvas center maps to source center (0.5, 0.5).
        let u = v.uniforms();
        let (ux, uy) = TexturedVoid::src_uv(&u, (50.0, 50.0));
        approx(ux, 0.5);
        approx(uy, 0.5);

        // Body-translate the content +20px in x (gizmo drag). The output pixel
        // at canvas x=70 now shows what was at the center → uv.x back to 0.5.
        let mut shifted = v;
        shifted.transform =
            crate::transform::Transform::from_affine([1.0, 0.0, 20.0, 0.0, 1.0, 0.0]);
        let u2 = shifted.uniforms();
        let (sx, sy) = TexturedVoid::src_uv(&u2, (70.0, 50.0));
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
        v.canvas = CanvasRect::from_xywh(0, 0, 100, 100);
        v.src_w = 200;
        v.src_h = 100;
        v.has_source = true;
        let r = v.content_extent(CanvasRect::from_xywh(0, 0, 100, 100));
        approx(r.width, 200.0);
        approx(r.height, 100.0);
        approx(r.x, -50.0); // overhangs left/right
        approx(r.y, 0.0); // fits vertically
    }

    /// A `Cover` void is anchored to the canvas *window*, so its plane-space
    /// content rect shifts with `canvas_origin` after a crop — that is what
    /// "keeps filling the frame" means. Its window-local uniform, by contrast,
    /// is unchanged, because the origin cancels.
    #[test]
    fn cover_content_extent_follows_canvas_origin() {
        let mut v = make_void();
        v.src_w = 200;
        v.src_h = 100;
        v.has_source = true;
        let cropped = CanvasRect::from_xywh(10, 20, 100, 100);
        v.canvas = cropped;

        let plane = v.content_extent(cropped);
        approx(plane.x, 10.0 - 50.0);
        approx(plane.y, 20.0);

        let local = plane.to_window(cropped);
        approx(local.x, -50.0);
        approx(local.y, 0.0);
    }

    /// A `Natural` void is anchored in the document plane: its content rect is
    /// the source's own pixel rect and does NOT move when the canvas is
    /// cropped. Cropping must not slide a placed image across the artwork.
    #[test]
    fn natural_content_extent_ignores_canvas_origin() {
        static NATURAL_CONFIG: TexturedVoidConfig = TexturedVoidConfig {
            type_id: "test_natural",
            display_name: "Test Natural",
            description: "Test fixture.",
            icon: "tabler:test",
            params: &[],
            source: VoidSource::Image,
            fit: ContentFit::Natural,
            default_transform: |_, _| crate::transform::Transform::identity(),
        };
        let mut v = TexturedVoid::from_params(&NATURAL_CONFIG, &[], fake_pipeline());
        v.src_w = 300;
        v.src_h = 200;
        v.has_source = true;

        let before = v.content_extent(CanvasRect::from_xywh(0, 0, 500, 500));
        let after = v.content_extent(CanvasRect::from_xywh(10, 20, 40, 50));
        assert_eq!(before, ContentRect::new(0.0, 0.0, 300.0, 200.0));
        assert_eq!(
            after, before,
            "a placed image's plane rect must not depend on the canvas window",
        );

        // The window-local uniform absorbs the crop, so the shader still finds
        // the image at the same place in the document.
        v.canvas = CanvasRect::from_xywh(10, 20, 40, 50);
        let u = v.uniforms();
        approx(u.content_origin[0], -10.0);
        approx(u.content_origin[1], -20.0);
    }

    /// Until the first frame arrives (1×1 placeholder), there's no aspect to
    /// cover-fit, so the content rect falls back to canvas-fill.
    #[test]
    fn content_extent_falls_back_to_canvas_without_frame() {
        let mut v = make_void();
        v.canvas = CanvasRect::from_xywh(0, 0, 80, 60);
        let r = v.content_extent(CanvasRect::from_xywh(0, 0, 80, 60));
        assert_eq!(r, ContentRect::new(0.0, 0.0, 80.0, 60.0));
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
    /// `allocate_source` is the allocation half of `set_source_pixels`, used by
    /// callers that blit from the GPU. By the time it returns, the void must
    /// already describe the new source — natural-size content rect and a
    /// reported persistent frame — or the blit lands against a placeholder.
    #[test]
    fn allocate_source_adopts_the_new_dimensions() {
        static NATURAL: TexturedVoidConfig = TexturedVoidConfig {
            type_id: "test_allocate",
            display_name: "Test Allocate",
            description: "Test fixture.",
            icon: "tabler:test",
            params: &[],
            source: VoidSource::Image,
            fit: ContentFit::Natural,
            default_transform: |_, _| crate::transform::Transform::identity(),
        };
        // One device for the pipeline and the cache — `test_device()` mints a
        // fresh one per call, and a bind group layout cannot cross devices.
        let (device, queue) = crate::gpu::test_utils::test_device();
        let shared = Arc::new(create_pipeline(&device, wgpu::TextureFormat::Rgba8Unorm));
        let mut v = TexturedVoid::from_params(&NATURAL, &[], shared);

        // Before: no source, so the content rect falls back to the canvas and
        // there is nothing worth saving.
        assert_eq!(v.persistent_frame_size(), None);

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
        let (dst, dst_view) = make_frame_texture(&device, 8, 8, true);
        let mut cache = v.create_cache(&device, &queue, &dst_view, &sampler, 8, 8);
        drop(dst);

        v.allocate_source(&device, &queue, &mut cache, 300, 200);

        assert_eq!(v.persistent_frame_size(), Some((300, 200)));
        assert_eq!(
            v.content_extent(CanvasRect::from_xywh(0, 0, 8, 8)),
            ContentRect::new(0.0, 0.0, 300.0, 200.0),
            "a Natural void sits at its own pixel size at the plane origin",
        );
        assert_eq!(
            cache.aux_textures[0].width(),
            300,
            "the aux texture was reallocated, so a blit has somewhere to land",
        );
        assert!(
            cache.aux_textures[0].mip_level_count() > 1,
            "a one-shot source keeps its mip chain across reallocation",
        );
    }
}
