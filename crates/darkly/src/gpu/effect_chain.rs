//! The screen-space effect chain: effects run on the presented image, at
//! viewport resolution, after the view transform.
//!
//! Each entry owns an [`Effect`] instance, the cache it was built against and
//! its [`ScaledEffect`] scaffolding. Rendering is a ping-pong over two
//! viewport-sized textures: the presented composite lands in slot 0, each
//! visible effect reads one slot and writes the other, and the last one written
//! is blitted to the surface.

use crate::gpu::effect::{self, Effect, EffectCache, EffectPipeline, ParamValue};
use crate::gpu::effect_scaling::{screen_scale, ScaledEffect, ScalingPipelines};

/// Below this a scale change is not worth rebuilding for.
const SCALE_EPSILON: f32 = 1.0e-3;

/// One effect in the chain, with its visibility state and GPU resources.
struct ChainEntry {
    effect: Box<dyn Effect>,
    cache: EffectCache,
    scaled: ScaledEffect,
    visible: bool,
}

pub struct EffectChain {
    entries: Vec<ChainEntry>,
    /// Ping-pong textures at native viewport resolution. Created lazily when
    /// the first effect is added.
    textures: Option<[wgpu::Texture; 2]>,
    views: Option<[wgpu::TextureView; 2]>,
    /// Blit pipeline for final chain output → surface (surface format).
    blit_pipeline: EffectPipeline,
    /// Bind groups for blitting `views[0]` or `views[1]` to the surface.
    blit_bind_groups: Option<[wgpu::BindGroup; 2]>,
    /// Downscale/upscale pipelines for reduced-resolution effects. Created
    /// lazily on the first effect.
    scaling_pipelines: Option<ScalingPipelines>,
    sampler: wgpu::Sampler,
    viewport_width: u32,
    viewport_height: u32,
    accum_format: wgpu::TextureFormat,
    /// The scale the current resources were built for. `sync_resolution_scale`
    /// rebuilds them when it drifts from the config value.
    applied_scale: f32,
    /// Set on structural changes (add/remove/visibility/reorder).
    /// Animation-driven re-renders go through the compositor's frame scheduler.
    needs_present: bool,
}

impl EffectChain {
    pub fn new(
        device: &wgpu::Device,
        sampler: wgpu::Sampler,
        surface_format: wgpu::TextureFormat,
        accum_format: wgpu::TextureFormat,
    ) -> Self {
        EffectChain {
            entries: Vec::new(),
            textures: None,
            views: None,
            blit_pipeline: effect::create_blit_pipeline(device, surface_format, "blit-to-surface"),
            blit_bind_groups: None,
            scaling_pipelines: None,
            sampler,
            viewport_width: 0,
            viewport_height: 0,
            accum_format,
            applied_scale: screen_scale(),
            needs_present: false,
        }
    }

    /// Re-read the configured scale and, if it changed, rebuild per-effect
    /// resources. Called once per frame by the compositor — a no-op when the
    /// value is unchanged or the chain is empty.
    pub fn sync_resolution_scale(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let desired = screen_scale();
        if (self.applied_scale - desired).abs() < SCALE_EPSILON {
            return;
        }
        self.applied_scale = desired;
        if !self.entries.is_empty() {
            self.recreate_resources(device, queue);
            self.needs_present = true;
        }
    }

    // --- Dirty flag ---

    pub fn needs_present(&self) -> bool {
        self.needs_present
    }

    pub fn clear_needs_present(&mut self) {
        self.needs_present = false;
    }

    pub fn accum_format(&self) -> wgpu::TextureFormat {
        self.accum_format
    }

    // --- Chain management ---

    /// Add an effect to the chain, creating its GPU resources immediately.
    pub fn add_effect(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        effect: Box<dyn Effect>,
    ) {
        let Some(entry) = self.realize(device, queue, effect, true) else {
            return;
        };
        self.entries.push(entry);
        self.needs_present = true;
    }

    /// Remove the effect at `index`.
    pub fn remove_effect(&mut self, index: usize) {
        if index < self.entries.len() {
            self.entries.remove(index);
            if self.entries.is_empty() {
                self.drop_textures();
            }
            self.needs_present = true;
        }
    }

    pub fn clear_effects(&mut self) {
        self.entries.clear();
        self.drop_textures();
        self.needs_present = true;
    }

    pub fn set_effect_visible(&mut self, index: usize, visible: bool) {
        if let Some(entry) = self.entries.get_mut(index) {
            entry.visible = visible;
            self.needs_present = true;
        }
    }

    pub fn move_effect(&mut self, from: usize, to: usize) {
        if from >= self.entries.len() || to >= self.entries.len() {
            return;
        }
        let entry = self.entries.remove(from);
        self.entries.insert(to, entry);
        self.needs_present = true;
    }

    /// Adopt new parameter values for the effect at `index`, rebuilding only if
    /// the effect says its cache no longer describes it. Most effects rewrite
    /// their uniform in place, so a slider drag costs one buffer write.
    pub fn set_effect_params(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        index: usize,
        params: &[ParamValue],
    ) {
        let Some(entry) = self.entries.get_mut(index) else {
            return;
        };
        if entry.effect.set_params(queue, &entry.cache, params) {
            self.needs_present = true;
            return;
        }
        // The cache no longer fits; rebuild this entry against the same views.
        let effect = entry.effect.clone_boxed();
        let visible = entry.visible;
        if let Some(rebuilt) = self.realize(device, queue, effect, visible) {
            self.entries[index] = rebuilt;
        }
        self.needs_present = true;
    }

    // --- Queries ---

    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// Type id and visibility of the effect at `index`.
    pub fn info(&self, index: usize) -> Option<(&str, bool)> {
        self.entries
            .get(index)
            .map(|e| (e.effect.type_id(), e.visible))
    }

    pub fn type_id(&self, index: usize) -> Option<&'static str> {
        self.entries.get(index).map(|e| e.effect.type_id())
    }

    pub fn param_values(&self, index: usize) -> Option<Vec<ParamValue>> {
        self.entries.get(index).map(|e| e.effect.param_values())
    }

    pub fn has_visible(&self) -> bool {
        self.entries.iter().any(|e| e.visible)
    }

    pub fn viewport_size(&self) -> (u32, u32) {
        (self.viewport_width, self.viewport_height)
    }

    /// True when any visible effect needs continuous animation frames.
    pub fn needs_animation(&self) -> bool {
        self.entries
            .iter()
            .any(|e| e.visible && e.effect.needs_animation())
    }

    // --- Animation ---

    /// Advance every animated effect by `dt`. Called by the compositor's frame
    /// scheduler on effect-scheduled frames; rate limiting is the scheduler's.
    pub fn update_effects(&mut self, queue: &wgpu::Queue, dt: f32) {
        for entry in &mut self.entries {
            if entry.visible && entry.effect.needs_animation() {
                entry.effect.update_time(queue, &entry.cache, dt);
            }
        }
    }

    // --- Viewport ---

    /// Update viewport dimensions, recreating textures and caches if they moved.
    pub fn resize(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32) {
        if self.viewport_width == width && self.viewport_height == height {
            return;
        }
        self.viewport_width = width;
        self.viewport_height = height;
        if !self.entries.is_empty() {
            self.recreate_resources(device, queue);
        }
    }

    // --- Rendering ---

    /// Present the composite into the chain's input, run the visible effects,
    /// and blit the result to the surface.
    pub fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        surface_view: &wgpu::TextureView,
        present_pipeline: &wgpu::RenderPipeline,
        present_bind_group: &wgpu::BindGroup,
        overlay: &crate::gpu::overlay::ToolOverlay,
    ) {
        let (Some(views), Some(blit_bgs), Some(pipelines)) = (
            self.views.as_ref(),
            self.blit_bind_groups.as_ref(),
            self.scaling_pipelines.as_ref(),
        ) else {
            return;
        };

        // The presented composite (with the view transform applied) becomes
        // slot 0, the chain's input.
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("present-to-effects"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &views[0],
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            rpass.set_pipeline(present_pipeline);
            rpass.set_bind_group(0, present_bind_group, &[]);
            rpass.draw(0..3, 0..1);
        }

        let mut src = 0usize;
        for entry in &self.entries {
            if !entry.visible {
                continue;
            }
            let dst = 1 - src;
            entry.scaled.encode(
                encoder,
                &*entry.effect,
                &entry.cache,
                pipelines,
                src,
                &views[dst],
            );
            src = dst;
        }

        // Final blit to the surface, with the solid overlay drawn in the same
        // pass rather than a second `LoadOp::Load` one.
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("effects-blit-to-surface"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: surface_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            rpass.set_pipeline(&self.blit_pipeline.pipeline);
            rpass.set_bind_group(0, &blit_bgs[src], &[]);
            rpass.draw(0..3, 0..1);
            overlay.draw_solid(&mut rpass);
        }
    }

    // --- Internal helpers ---

    /// Build one entry's GPU resources against the current native views. The
    /// single place an entry is realized, so adding, rebuilding after a
    /// parameter change, and recreating after a resize cannot disagree.
    /// `None` when the viewport has no size yet.
    fn realize(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mut effect: Box<dyn Effect>,
        visible: bool,
    ) -> Option<ChainEntry> {
        self.ensure_textures(device);
        self.ensure_scaling_pipelines(device);
        let views = self.views.as_ref()?;
        let pipelines = self.scaling_pipelines.as_ref()?;
        let (scaled, cache) = ScaledEffect::prepare(
            device,
            queue,
            &mut *effect,
            views,
            &self.sampler,
            pipelines,
            self.accum_format,
            self.viewport_width,
            self.viewport_height,
            self.applied_scale,
        );
        Some(ChainEntry {
            effect,
            cache,
            scaled,
            visible,
        })
    }

    fn ensure_scaling_pipelines(&mut self, device: &wgpu::Device) {
        if self.scaling_pipelines.is_none() {
            self.scaling_pipelines = Some(ScalingPipelines::new(
                device,
                self.accum_format,
                "screen-effect",
            ));
        }
    }

    /// Ensure the native ping-pong textures exist at the current viewport size.
    fn ensure_textures(&mut self, device: &wgpu::Device) {
        let (w, h) = (self.viewport_width, self.viewport_height);
        if w == 0 || h == 0 || self.textures.is_some() {
            return;
        }

        let format = self.accum_format;
        let make_tex = |label: &str| -> (wgpu::Texture, wgpu::TextureView) {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            (tex, view)
        };

        let (t0, v0) = make_tex("screen-effect-0");
        let (t1, v1) = make_tex("screen-effect-1");

        let blit_bg: [wgpu::BindGroup; 2] = [
            effect::create_blit_bind_group(
                device,
                &self.blit_pipeline.bind_group_layout,
                &v0,
                &self.sampler,
                "screen-effect-blit-0",
            ),
            effect::create_blit_bind_group(
                device,
                &self.blit_pipeline.bind_group_layout,
                &v1,
                &self.sampler,
                "screen-effect-blit-1",
            ),
        ];

        self.textures = Some([t0, t1]);
        self.views = Some([v0, v1]);
        self.blit_bind_groups = Some(blit_bg);
    }

    fn drop_textures(&mut self) {
        self.textures = None;
        self.views = None;
        self.blit_bind_groups = None;
    }

    /// Recreate the chain's textures and every entry's resources — what a
    /// viewport resize or a scale change needs.
    fn recreate_resources(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.drop_textures();
        let taken: Vec<(Box<dyn Effect>, bool)> = self
            .entries
            .drain(..)
            .map(|e| (e.effect, e.visible))
            .collect();
        for (effect, visible) in taken {
            if let Some(entry) = self.realize(device, queue, effect, visible) {
                self.entries.push(entry);
            }
        }
    }
}
