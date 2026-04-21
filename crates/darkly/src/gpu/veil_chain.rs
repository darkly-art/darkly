use crate::gpu::effect::{self, EffectCache, EffectPipeline};
use crate::gpu::veil::{budget_scaled_dimensions, ParamValue, Veil, VeilRegistry};

/// Per-veil reduced-resolution resources for CPU rendering.
/// Created when a veil declares a `cpu_pixel_budget` and we're on a
/// software renderer. The veil chain handles downscale/upscale around
/// the veil — the veil itself is resolution-agnostic.
struct VeilScaling {
    /// Kept alive so the GPU textures aren't dropped.
    _textures: [wgpu::Texture; 2],
    views: [wgpu::TextureView; 2],
    /// Bind groups for downscaling: [i] reads native ping-pong[i].
    downscale_bgs: [wgpu::BindGroup; 2],
    /// Bind group for upscaling: reads reduced[1] (veil output).
    upscale_bg: wgpu::BindGroup,
}

/// A veil in the chain, with visibility state and GPU cache.
struct VeilEntry {
    veil: Box<dyn Veil>,
    cache: EffectCache,
    visible: bool,
    /// Present when the veil runs at reduced resolution on CPU.
    scaling: Option<VeilScaling>,
}

pub struct VeilChain {
    registry: VeilRegistry,
    entries: Vec<VeilEntry>,
    /// Ping-pong textures at native viewport resolution.
    /// Created lazily when the first veil is added.
    textures: Option<[wgpu::Texture; 2]>,
    views: Option<[wgpu::TextureView; 2]>,
    /// Blit pipeline for final veil output → surface (surface format).
    blit_pipeline: EffectPipeline,
    /// Bind groups for blitting veil_textures[0] or [1] to surface.
    blit_bind_groups: Option<[wgpu::BindGroup; 2]>,
    /// Blit pipeline for downscale/upscale between native and reduced-res
    /// textures (accum format). Created lazily on first scaled veil.
    scaling_pipeline: Option<EffectPipeline>,
    sampler: wgpu::Sampler,
    /// Current viewport dimensions (updated on resize).
    viewport_width: u32,
    viewport_height: u32,
    accum_format: wgpu::TextureFormat,
    /// True when running on a software renderer (CPU).
    is_software: bool,
    /// Set when structural changes occur (add/remove/visibility/reorder).
    /// Animation-driven re-renders are handled by the compositor's frame scheduler.
    needs_present: bool,
}

impl VeilChain {
    pub fn new(
        device: &wgpu::Device,
        sampler: wgpu::Sampler,
        surface_format: wgpu::TextureFormat,
        accum_format: wgpu::TextureFormat,
        is_software: bool,
    ) -> Self {
        let registry = VeilRegistry::new();
        let blit_pipeline = effect::create_blit_pipeline(device, surface_format, "blit-to-surface");

        VeilChain {
            registry,
            entries: Vec::new(),
            textures: None,
            views: None,
            blit_pipeline,
            blit_bind_groups: None,
            scaling_pipeline: None,
            sampler,
            viewport_width: 0,
            viewport_height: 0,
            accum_format,
            is_software,
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
    pub fn add_veil(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, veil: Box<dyn Veil>) {
        self.ensure_textures(device);
        self.ensure_scaling_pipeline(device);
        let native_views = self.views.as_ref().unwrap();

        let (scaling, cache) = create_veil_resources(
            device,
            queue,
            &*veil,
            native_views,
            &self.sampler,
            self.scaling_pipeline.as_ref(),
            self.accum_format,
            self.viewport_width,
            self.viewport_height,
            self.is_software,
        );
        self.entries.push(VeilEntry {
            veil,
            cache,
            visible: true,
            scaling,
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
        self.ensure_scaling_pipeline(device);
        let native_views = self.views.as_ref().unwrap();

        let (scaling, cache) = create_veil_resources(
            device,
            queue,
            &*new_veil,
            native_views,
            &self.sampler,
            self.scaling_pipeline.as_ref(),
            self.accum_format,
            self.viewport_width,
            self.viewport_height,
            self.is_software,
        );
        let visible = self.entries[index].visible;
        self.entries[index] = VeilEntry {
            veil: new_veil,
            cache,
            visible,
            scaling,
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

    /// Current viewport dimensions.
    pub fn viewport_size(&self) -> (u32, u32) {
        (self.viewport_width, self.viewport_height)
    }

    /// Returns true if any visible veil needs continuous animation frames.
    pub fn needs_animation(&self) -> bool {
        self.entries
            .iter()
            .any(|e| e.visible && e.veil.needs_animation())
    }

    // --- Animation ---

    /// Update all animated veils with the given delta time.
    /// Called by the compositor's frame scheduler on veil-scheduled frames.
    /// No throttle — the frame scheduler handles rate limiting.
    pub fn update_veils(&mut self, queue: &wgpu::Queue, dt: f32) {
        for entry in &mut self.entries {
            if entry.visible && entry.veil.needs_animation() {
                entry.veil.update_time(queue, &entry.cache, dt);
            }
        }
    }

    // --- Viewport ---

    /// Update viewport dimensions. Recreates veil textures and caches if needed.
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

    /// Encode the veil chain: present composite to veil input, run veils,
    /// blit final output to surface.
    pub fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        surface_view: &wgpu::TextureView,
        present_pipeline: &wgpu::RenderPipeline,
        present_bind_group: &wgpu::BindGroup,
        overlay: &crate::gpu::overlay::ToolOverlay,
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
        // Veils with per-veil scaling get downscale → veil → upscale passes.
        let mut current_src = 0usize;
        for entry in &self.entries {
            if !entry.visible {
                continue;
            }
            let dst = 1 - current_src;

            if let Some(ref scaling) = entry.scaling {
                let sp = self.scaling_pipeline.as_ref().unwrap();
                // Downscale: native pp[current_src] → reduced[0]
                blit_pass(
                    encoder,
                    &sp.pipeline,
                    &scaling.downscale_bgs[current_src],
                    &scaling.views[0],
                    "veil-downscale",
                );
                // Run veil at reduced resolution: reads reduced[0], writes reduced[1]
                entry
                    .veil
                    .encode(encoder, &entry.cache, 0, &scaling.views[1]);
                // Upscale: reduced[1] → native pp[dst]
                blit_pass(
                    encoder,
                    &sp.pipeline,
                    &scaling.upscale_bg,
                    &veil_views[dst],
                    "veil-upscale",
                );
            } else {
                entry
                    .veil
                    .encode(encoder, &entry.cache, current_src, &veil_views[dst]);
            }

            current_src = dst;
        }

        // Step 3: Blit final veil output → surface, with solid overlay in
        // the same pass (avoids a separate LoadOp::Load render pass).
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
            // Draw solid overlay primitives in the same pass.
            overlay.draw_solid(&mut rpass);
        }
    }

    // --- Internal helpers ---

    /// Ensure the scaling blit pipeline exists (needed for CPU-scaled veils).
    fn ensure_scaling_pipeline(&mut self, device: &wgpu::Device) {
        if self.is_software && self.scaling_pipeline.is_none() {
            self.scaling_pipeline = Some(effect::create_blit_pipeline(
                device,
                self.accum_format,
                "veil-scaling",
            ));
        }
    }

    /// Ensure native ping-pong textures exist at the current viewport dimensions.
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
        self.ensure_scaling_pipeline(device);

        let native_views = self.views.as_ref().unwrap();
        for entry in &mut self.entries {
            let (scaling, cache) = create_veil_resources(
                device,
                queue,
                &*entry.veil,
                native_views,
                &self.sampler,
                self.scaling_pipeline.as_ref(),
                self.accum_format,
                self.viewport_width,
                self.viewport_height,
                self.is_software,
            );
            entry.cache = cache;
            entry.scaling = scaling;
        }
    }
}

// --- Free functions ---

/// Create the veil's EffectCache and optional VeilScaling.
/// If the veil declares a `cpu_pixel_budget` and we're on CPU,
/// creates reduced-res textures and passes those to the veil.
/// Otherwise passes the native ping-pong views directly.
fn create_veil_resources(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    veil: &dyn Veil,
    native_views: &[wgpu::TextureView; 2],
    sampler: &wgpu::Sampler,
    scaling_pipeline: Option<&EffectPipeline>,
    accum_format: wgpu::TextureFormat,
    viewport_width: u32,
    viewport_height: u32,
    is_software: bool,
) -> (Option<VeilScaling>, EffectCache) {
    let budget = if is_software {
        veil.cpu_pixel_budget()
    } else {
        None
    };

    if let Some(pixel_budget) = budget {
        let (rw, rh) = budget_scaled_dimensions(viewport_width, viewport_height, pixel_budget);
        let sp = scaling_pipeline.expect("scaling pipeline must exist for CPU-scaled veils");
        let scaling = create_veil_scaling(device, sampler, sp, accum_format, rw, rh, native_views);
        let cache = veil.create_cache(device, queue, &scaling.views, sampler, rw, rh);
        (Some(scaling), cache)
    } else {
        let cache = veil.create_cache(
            device,
            queue,
            native_views,
            sampler,
            viewport_width,
            viewport_height,
        );
        (None, cache)
    }
}

/// Create per-veil reduced-resolution textures and scaling bind groups.
fn create_veil_scaling(
    device: &wgpu::Device,
    sampler: &wgpu::Sampler,
    scaling_pipeline: &EffectPipeline,
    format: wgpu::TextureFormat,
    render_width: u32,
    render_height: u32,
    native_views: &[wgpu::TextureView; 2],
) -> VeilScaling {
    let make_tex = |label: &str| -> (wgpu::Texture, wgpu::TextureView) {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: render_width,
                height: render_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        (tex, view)
    };

    let (t0, v0) = make_tex("veil-scaled-0");
    let (t1, v1) = make_tex("veil-scaled-1");

    let layout = &scaling_pipeline.bind_group_layout;

    // Downscale: [i] reads native pp[i], draws to reduced[0].
    let downscale_bgs: [wgpu::BindGroup; 2] = std::array::from_fn(|i| {
        effect::create_blit_bind_group(
            device,
            layout,
            &native_views[i],
            sampler,
            &format!("veil-downscale-{i}"),
        )
    });

    // Upscale: reads reduced[1] (veil output), draws to native pp[dst].
    let upscale_bg = effect::create_blit_bind_group(device, layout, &v1, sampler, "veil-upscale");

    VeilScaling {
        _textures: [t0, t1],
        views: [v0, v1],
        downscale_bgs,
        upscale_bg,
    }
}

/// Execute a fullscreen blit render pass.
fn blit_pass(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
    dst_view: &wgpu::TextureView,
    label: &'static str,
) {
    let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: dst_view,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    });
    rpass.set_pipeline(pipeline);
    rpass.set_bind_group(0, bind_group, &[]);
    rpass.draw(0..3, 0..1);
}
