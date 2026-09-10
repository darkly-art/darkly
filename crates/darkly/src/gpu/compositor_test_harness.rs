//! Test-only present harnesses on [`Compositor`]: run the present pass (and
//! optionally the screen-space run) into a readable offscreen texture, since
//! the production present pass writes to the un-readable surface.

use crate::document::Document;
use crate::gpu::compositor::Compositor;
use crate::gpu::create_texture_with_view;
use crate::gpu::view::ViewTransform;
use crate::layer::LayerId;

impl Compositor {
    /// Run the present pass (`present.wgsl` via the current `view_uniform_buf`)
    /// into a `target_w × target_h` offscreen RGBA8 texture and return its
    /// bytes. Assumes the composite cache and view uniform are already current.
    fn present_into_target(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_w: u32,
        target_h: u32,
    ) -> Vec<u8> {
        let (target, target_view) = create_texture_with_view(
            device,
            target_w,
            target_h,
            wgpu::TextureFormat::Rgba8Unorm,
            "test-present-target",
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("test-present"),
        });
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("test-present-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            rpass.set_pipeline(&self.present_to_effects_pipeline);
            rpass.set_bind_group(0, self.present_cache_bind_group(), &[]);
            rpass.draw(0..3, 0..1);
        }
        queue.submit(std::iter::once(encoder.finish()));

        crate::gpu::test_utils::readback_texture(
            device,
            queue,
            &target,
            wgpu::TextureFormat::Rgba8Unorm,
            target_w,
            target_h,
        )
    }

    /// Run the present pass into a canvas-sized offscreen RGBA8 texture and
    /// return its bytes. For tests: the production present pass writes to the
    /// surface (un-readable), but the present shader is exactly where bugs
    /// like premultiplied-alpha mishandling live, so test coverage of that
    /// stage requires a parallel sink.
    ///
    /// Forces an identity 1:1 view transform so screen pixels map to canvas
    /// pixels and the OOB branch is inactive across the whole target. Use
    /// [`Self::test_present_to_viewport`] instead when the screen↔canvas
    /// mapping itself is under test (resize / squash / offset bugs).
    pub fn test_present_to_canvas(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        doc: &mut Document,
        isolated: Option<LayerId>,
    ) -> Vec<u8> {
        self.render_offscreen(device, queue, doc, isolated);

        let cw = self.canvas_width;
        let ch = self.canvas_height;
        let identity = ViewTransform::from_pan_zoom_rotate(
            0.0, 0.0, 1.0, 0.0, false, cw as f32, ch as f32, cw as f32, ch as f32,
        );
        self.update_view_transform(queue, &identity);

        self.present_into_target(device, queue, cw, ch)
    }

    /// Run the present pass through the **production** cached `view_uniform_buf`
    /// (the matrix `rebuild_view_transform` last uploaded) into a
    /// `viewport_w × viewport_h` target: i.e. exactly what the surface would
    /// show, minus the surface. Unlike [`Self::test_present_to_canvas`] this
    /// does NOT force identity, so it exercises the real screen↔canvas mapping
    /// where view-transform / resize bugs (anisotropic squash, offset) live.
    pub fn test_present_to_viewport(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        doc: &mut Document,
        viewport_w: u32,
        viewport_h: u32,
        isolated: Option<LayerId>,
    ) -> Vec<u8> {
        self.render_offscreen(device, queue, doc, isolated);
        self.present_into_target(device, queue, viewport_w, viewport_h)
    }

    /// Present **through the screen-space run** into a `target_w × target_h`
    /// offscreen texture and return its bytes: what the surface would show,
    /// minus the surface.
    ///
    /// [`Self::test_present_to_viewport`] deliberately stops at the present
    /// pass, so it cannot see the run at all. This one exists because the whole
    /// point of the run is that it happens *after* that pass: nothing below the
    /// surface can observe the difference between the two spaces.
    pub fn test_present_through_screen_run(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        doc: &mut Document,
        target_w: u32,
        target_h: u32,
        isolated: Option<LayerId>,
    ) -> Vec<u8> {
        self.resize_screen_run(target_w, target_h);
        self.render_offscreen(device, queue, doc, isolated);

        // Identity 1:1, so a target texel is a canvas texel and the assertion
        // is about the run rather than about where the view transform put the
        // canvas. `test_present_to_viewport` is the harness for the mapping.
        let identity = ViewTransform::from_pan_zoom_rotate(
            0.0,
            0.0,
            1.0,
            0.0,
            false,
            self.canvas_width as f32,
            self.canvas_height as f32,
            target_w as f32,
            target_h as f32,
        );
        self.update_view_transform(queue, &identity);

        let (target, target_view) = create_texture_with_view(
            device,
            target_w,
            target_h,
            self.screen_run.surface_format(),
            "test-screen-run-target",
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("test-screen-run"),
        });
        self.present_and_screen_run(&mut encoder, device, queue, doc, &target_view, isolated);
        queue.submit(std::iter::once(encoder.finish()));

        let format = self.screen_run.surface_format();
        let mut bytes = crate::gpu::test_utils::readback_texture(
            device, queue, &target, format, target_w, target_h,
        );

        // The surface may be BGRA; hand callers RGBA either way, so a test
        // asserting on a colour never has to know which surface it got.
        if matches!(
            format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        ) {
            for texel in bytes.as_chunks_mut::<4>().0 {
                texel.swap(0, 2);
            }
        }
        bytes
    }
}
