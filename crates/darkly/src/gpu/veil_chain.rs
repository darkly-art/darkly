use crate::gpu::effect::{self, EffectCache, EffectPipeline};
use crate::gpu::veil::{ParamValue, Veil, VeilRegistry};

/// A veil in the chain, with visibility state and GPU cache.
struct VeilEntry {
    veil: Box<dyn Veil>,
    cache: EffectCache,
    visible: bool,
}

pub struct VeilChain {
    registry: VeilRegistry,
    entries: Vec<VeilEntry>,
    /// Screen-sized ping-pong textures for veil chain.
    /// Created lazily when the first veil is added.
    textures: Option<[wgpu::Texture; 2]>,
    views: Option<[wgpu::TextureView; 2]>,
    /// Blit pipeline for final veil output → surface.
    blit_pipeline: EffectPipeline,
    /// Bind groups for blitting veil_textures[0] or [1] to surface.
    blit_bind_groups: Option<[wgpu::BindGroup; 2]>,
    sampler: wgpu::Sampler,
    /// Current viewport dimensions (updated on resize).
    viewport_width: u32,
    viewport_height: u32,
    /// Last wall-clock time (seconds) passed to `update_time`.
    last_time: f32,
    /// Time accumulated since the last animation render. Used to throttle
    /// animated veils to a cinematic framerate instead of 60fps.
    anim_accum: f32,
    accum_format: wgpu::TextureFormat,
    needs_present: bool,
}

impl VeilChain {
    pub fn new(
        device: &wgpu::Device,
        sampler: wgpu::Sampler,
        surface_format: wgpu::TextureFormat,
        accum_format: wgpu::TextureFormat,
    ) -> Self {
        let registry = VeilRegistry::new();
        let blit_pipeline =
            effect::create_blit_pipeline(device, surface_format, "blit-to-surface");

        VeilChain {
            registry,
            entries: Vec::new(),
            textures: None,
            views: None,
            blit_pipeline,
            blit_bind_groups: None,
            sampler,
            viewport_width: 0,
            viewport_height: 0,
            last_time: 0.0,
            anim_accum: 0.0,
            accum_format,
            needs_present: false,
        }
    }

    // --- Dirty flag ---

    pub fn needs_present(&self) -> bool {
        self.needs_present
    }

    pub fn clear_needs_present(&mut self) {
        self.needs_present = false;
    }

    // --- Registry access ---

    pub fn registry(&self) -> &VeilRegistry {
        &self.registry
    }

    pub fn registry_mut(&mut self) -> &mut VeilRegistry {
        &mut self.registry
    }

    pub fn accum_format(&self) -> wgpu::TextureFormat {
        self.accum_format
    }

    // --- Veil management ---

    /// Add a veil to the chain. Creates GPU resources immediately.
    pub fn add_veil(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        veil: Box<dyn Veil>,
    ) {
        self.ensure_textures(device);
        let views = self.views.as_ref().unwrap();
        let cache = veil.create_cache(
            device,
            queue,
            views,
            &self.sampler,
            self.viewport_width,
            self.viewport_height,
        );
        self.entries.push(VeilEntry {
            veil,
            cache,
            visible: true,
        });
        self.needs_present = true;
    }

    /// Remove a veil by index.
    pub fn remove_veil(&mut self, index: usize) {
        if index < self.entries.len() {
            self.entries.remove(index);
            if self.entries.is_empty() {
                self.drop_textures();
            }
            self.needs_present = true;
        }
    }

    /// Remove all veils.
    pub fn clear_veils(&mut self) {
        self.entries.clear();
        self.drop_textures();
        self.needs_present = true;
    }

    /// Toggle veil visibility.
    pub fn set_veil_visible(&mut self, index: usize, visible: bool) {
        if let Some(entry) = self.entries.get_mut(index) {
            entry.visible = visible;
            self.needs_present = true;
        }
    }

    /// Move a veil from one position to another.
    pub fn move_veil(&mut self, from: usize, to: usize) {
        if from >= self.entries.len() || to >= self.entries.len() {
            return;
        }
        let entry = self.entries.remove(from);
        self.entries.insert(to, entry);
        self.needs_present = true;
    }

    /// Replace the veil at `index` with a new instance, preserving visibility.
    /// Used when parameters change — veil params affect GPU resources,
    /// so recreation is required.
    pub fn update_veil(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        index: usize,
        new_veil: Box<dyn Veil>,
    ) {
        if index >= self.entries.len() {
            return;
        }
        self.ensure_textures(device);
        let views = self.views.as_ref().unwrap();
        let cache = new_veil.create_cache(
            device,
            queue,
            views,
            &self.sampler,
            self.viewport_width,
            self.viewport_height,
        );
        let visible = self.entries[index].visible;
        self.entries[index] = VeilEntry {
            veil: new_veil,
            cache,
            visible,
        };
        self.needs_present = true;
    }

    // --- Queries ---

    /// Number of veils in the chain.
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// Get veil type_id and visibility at index.
    pub fn info(&self, index: usize) -> Option<(&str, bool)> {
        self.entries
            .get(index)
            .map(|e| (e.veil.type_id(), e.visible))
    }

    /// Get the type_id of the veil at index.
    pub fn type_id(&self, index: usize) -> Option<&'static str> {
        self.entries.get(index).map(|e| e.veil.type_id())
    }

    /// Get the current parameter values of the veil at index.
    pub fn param_values(&self, index: usize) -> Option<Vec<ParamValue>> {
        self.entries.get(index).map(|e| e.veil.param_values())
    }

    /// Returns true if any veil is visible.
    pub fn has_visible(&self) -> bool {
        self.entries.iter().any(|e| e.visible)
    }

    // --- Animation ---

    /// Advance veil animation time. Computes delta from the previous call,
    /// updates each animated veil's internal time, and conditionally sets
    /// `needs_present` only when enough time has elapsed for a new frame.
    /// Animated veils run at a configurable FPS to reduce GPU/CPU overhead.
    pub fn update_time(&mut self, queue: &wgpu::Queue, wall_time: f32) {
        let anim_frame_interval = 1.0 / crate::config::get_f64("animation.fps") as f32;

        let dt = if self.last_time > 0.0 {
            (wall_time - self.last_time).max(0.0)
        } else {
            0.0
        };
        self.last_time = wall_time;

        if dt == 0.0 {
            return;
        }

        let has_animating = self
            .entries
            .iter()
            .any(|e| e.visible && e.veil.needs_animation());
        if !has_animating {
            return;
        }

        self.anim_accum += dt;
        if self.anim_accum < anim_frame_interval {
            return;
        }

        // Consume the accumulated time and update veils with the full delta.
        let anim_dt = self.anim_accum;
        self.anim_accum = 0.0;

        for entry in &mut self.entries {
            if entry.visible && entry.veil.needs_animation() {
                entry.veil.update_time(queue, &entry.cache, anim_dt);
            }
        }
        self.needs_present = true;
    }

    // --- Viewport ---

    /// Update viewport dimensions. Recreates veil textures and caches if needed.
    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
    ) {
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

    /// Encode the veil chain: present composite to veil input, run veils,
    /// blit final output to surface.
    pub fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        surface_view: &wgpu::TextureView,
        present_pipeline: &wgpu::RenderPipeline,
        present_bind_group: &wgpu::BindGroup,
    ) {
        let veil_views = self.views.as_ref().unwrap();

        // Step 1: Present composite_cache → veil_textures[0] (with view transform).
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("present-to-veil"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &veil_views[0],
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

        // Step 2: Run visible veils with ping-pong.
        let mut current_src = 0usize;
        for entry in &self.entries {
            if !entry.visible {
                continue;
            }
            let dst = 1 - current_src;
            entry
                .veil
                .encode(encoder, &entry.cache, current_src, &veil_views[dst]);
            current_src = dst;
        }

        // Step 3: Blit final veil output → surface.
        let blit_bgs = self.blit_bind_groups.as_ref().unwrap();
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("veil-blit-to-surface"),
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
            rpass.set_bind_group(0, &blit_bgs[current_src], &[]);
            rpass.draw(0..3, 0..1);
        }
    }

    // --- Internal helpers ---

    /// Ensure screen-sized veil textures exist at the current viewport dimensions.
    fn ensure_textures(&mut self, device: &wgpu::Device) {
        let w = self.viewport_width;
        let h = self.viewport_height;
        if w == 0 || h == 0 {
            return;
        }

        if self.textures.is_some() {
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

        let (t0, v0) = make_tex("veil-0");
        let (t1, v1) = make_tex("veil-1");

        let blit_bg: [wgpu::BindGroup; 2] = [
            effect::create_blit_bind_group(
                device,
                &self.blit_pipeline.bind_group_layout,
                &v0,
                &self.sampler,
                "veil-blit-0",
            ),
            effect::create_blit_bind_group(
                device,
                &self.blit_pipeline.bind_group_layout,
                &v1,
                &self.sampler,
                "veil-blit-1",
            ),
        ];

        self.textures = Some([t0, t1]);
        self.views = Some([v0, v1]);
        self.blit_bind_groups = Some(blit_bg);
    }

    /// Drop veil textures and associated bind groups.
    fn drop_textures(&mut self) {
        self.textures = None;
        self.views = None;
        self.blit_bind_groups = None;
    }

    /// Recreate veil textures, blit bind groups, and all veil caches.
    /// Called when viewport dimensions change while veils are active.
    fn recreate_resources(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.drop_textures();
        self.ensure_textures(device);

        let views = self.views.as_ref().unwrap();
        for entry in &mut self.entries {
            entry.cache = entry.veil.create_cache(
                device,
                queue,
                views,
                &self.sampler,
                self.viewport_width,
                self.viewport_height,
            );
        }
    }
}
