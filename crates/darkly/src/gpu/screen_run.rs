//! The screen-space run: the effect layers the user has placed above the
//! divider, realized on the presented image at viewport resolution, after the
//! view transform.
//!
//! This owns only *resources* — the ping-pong pair the run reads and writes, a
//! scratch target for the in-place apply pass, the scaling pipelines and the
//! final blit. Membership, order and visibility come from the document
//! ([`Document::screen_space_run`]), and the effect instances themselves live in
//! the compositor's `effect_instances` map alongside the canvas-space ones, so
//! there is exactly one place an effect is realized regardless of which space it
//! ends up in.
//!
//! [`Document::screen_space_run`]: crate::document::Document::screen_space_run

use crate::gpu::effect::{self, EffectPipeline};
use crate::gpu::effect_scaling::ScalingPipelines;

pub struct ScreenRun {
    /// Ping-pong textures at native viewport resolution, plus the scratch the
    /// in-place apply pass writes an effect's output into before blending it
    /// back. Created lazily once the viewport has a size.
    textures: Option<[wgpu::Texture; 2]>,
    views: Option<[wgpu::TextureView; 2]>,
    scratch: Option<(wgpu::Texture, wgpu::TextureView)>,
    /// Blit pipeline for final run output → surface (surface format).
    blit_pipeline: EffectPipeline,
    /// Bind groups for blitting `views[0]` or `views[1]` to the surface.
    blit_bind_groups: Option<[wgpu::BindGroup; 2]>,
    /// Downscale/upscale pipelines for reduced-resolution effects.
    scaling_pipelines: Option<ScalingPipelines>,
    sampler: wgpu::Sampler,
    viewport_width: u32,
    viewport_height: u32,
    accum_format: wgpu::TextureFormat,
    surface_format: wgpu::TextureFormat,
}

impl ScreenRun {
    pub fn new(
        device: &wgpu::Device,
        sampler: wgpu::Sampler,
        surface_format: wgpu::TextureFormat,
        accum_format: wgpu::TextureFormat,
    ) -> Self {
        ScreenRun {
            textures: None,
            views: None,
            scratch: None,
            blit_pipeline: effect::create_blit_pipeline(device, surface_format, "blit-to-surface"),
            blit_bind_groups: None,
            scaling_pipelines: None,
            sampler,
            viewport_width: 0,
            viewport_height: 0,
            accum_format,
            surface_format,
        }
    }

    // --- Queries ---

    pub fn accum_format(&self) -> wgpu::TextureFormat {
        self.accum_format
    }

    /// The format the final blit writes — the surface's. A test sink standing
    /// in for the surface has to match it or the pipeline is incompatible with
    /// the pass.
    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.surface_format
    }

    pub fn viewport_size(&self) -> (u32, u32) {
        (self.viewport_width, self.viewport_height)
    }

    /// The ping-pong pair an instance in this space is prepared against.
    pub fn views(&self) -> Option<&[wgpu::TextureView; 2]> {
        self.views.as_ref()
    }

    /// Where an effect writes its output before the apply pass blends it back
    /// into the pair — the screen-space counterpart of the canvas apply scratch.
    pub fn scratch_view(&self) -> Option<&wgpu::TextureView> {
        self.scratch.as_ref().map(|(_, v)| v)
    }

    pub fn scaling_pipelines(&self) -> Option<&ScalingPipelines> {
        self.scaling_pipelines.as_ref()
    }

    // --- Resources ---

    /// Update viewport dimensions. Returns whether the textures were replaced,
    /// which invalidates every bind group pointing at them — the caller bumps
    /// the revisions that fact implies.
    pub fn resize(&mut self, width: u32, height: u32) -> bool {
        if self.viewport_width == width && self.viewport_height == height {
            return false;
        }
        self.viewport_width = width;
        self.viewport_height = height;
        self.drop_textures();
        true
    }

    /// Ensure the ping-pong pair, the scratch and the scaling pipelines exist
    /// at the current viewport size. A no-op once they do; called before the
    /// instances that bind them are prepared.
    pub fn ensure_resources(&mut self, device: &wgpu::Device) {
        if self.scaling_pipelines.is_none() {
            self.scaling_pipelines = Some(ScalingPipelines::new(
                device,
                self.accum_format,
                "screen-effect",
            ));
        }

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

        self.scratch = Some(make_tex("screen-effect-apply-scratch"));
        self.textures = Some([t0, t1]);
        self.views = Some([v0, v1]);
        self.blit_bind_groups = Some(blit_bg);
    }

    fn drop_textures(&mut self) {
        self.textures = None;
        self.views = None;
        self.scratch = None;
        self.blit_bind_groups = None;
    }

    // --- Rendering ---

    /// Present the composite into the run's input slot, so the first effect
    /// reads the view-transformed image the user is looking at.
    pub fn encode_present_into_run(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        present_pipeline: &wgpu::RenderPipeline,
        present_bind_group: &wgpu::BindGroup,
    ) -> bool {
        let Some(views) = self.views.as_ref() else {
            return false;
        };
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("present-to-screen-run"),
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
        true
    }

    /// Blit the slot the run last wrote to the surface, with the solid overlay
    /// drawn in the same pass rather than a second `LoadOp::Load` one.
    pub fn blit_to_surface(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        surface_view: &wgpu::TextureView,
        src: usize,
        overlay: &crate::gpu::overlay::ToolOverlay,
    ) {
        let Some(blit_bgs) = self.blit_bind_groups.as_ref() else {
            return;
        };
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("screen-run-blit-to-surface"),
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
