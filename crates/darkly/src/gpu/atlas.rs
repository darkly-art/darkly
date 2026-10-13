use crate::coord::{CanvasPoint, CanvasRect, LayerPoint, LayerRect};

/// GPU-side texture storage for a single raster layer.
/// One Rgba8Unorm texture per layer, sized to the layer's pixel bounds,
/// which default to canvas dimensions but may be larger when content
/// extends past the canvas (e.g. paste of an oversized image).
///
/// ## Coordinate-space discipline
///
/// All coordinate-bearing fields are private. Callers go through the typed
/// accessors ([`canvas_extent`], [`layer_extent`], [`canvas_to_layer*`],
/// [`layer_to_canvas*`]) so the canvas/layer-local distinction lives in the
/// type system rather than in convention. See module docs of
/// [`crate::coord`] and the project's CONTRIBUTING.md for the rule: every
/// coordinate at every interface names its space; only the texture itself
/// translates between them.
///
/// [`canvas_extent`]: LayerTexture::canvas_extent
/// [`layer_extent`]: LayerTexture::layer_extent
/// [`canvas_to_layer*`]: LayerTexture::canvas_to_layer
/// [`layer_to_canvas*`]: LayerTexture::layer_to_canvas
pub struct LayerTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// Texture dimensions in pixels. Mirrors the size used at allocation
    /// so callers don't need to reach into the wgpu::Texture descriptor.
    /// Exposed via [`layer_extent`](Self::layer_extent).
    width: u32,
    height: u32,
    /// Canvas-space offset of the texture's (0,0) pixel. Default (0,0) for
    /// canvas-aligned layers; non-zero for layers whose bounds extend past
    /// or are placed inside canvas (e.g. paste of an oversized image).
    /// Exposed via [`canvas_extent`](Self::canvas_extent).
    offset_x: i32,
    offset_y: i32,
    /// Texture format. Exposed via [`format`](Self::format) so format-driven
    /// dispatch (R8 vs RGBA paint pipelines, transform pipelines) doesn't
    /// have to reach into `texture.format()`; matches what the document-side
    /// `PixelBuffer` records.
    format: wgpu::TextureFormat,
    /// Usage flags beyond the common set every layer texture carries (e.g.
    /// `STORAGE_BINDING` for a vector layer), and the debug label. Kept so
    /// [`reallocated`](Self::reallocated) can produce the same kind of
    /// texture at another extent.
    extra_usage: wgpu::TextureUsages,
    label: &'static str,
}

impl LayerTexture {
    /// RGBA layer texture: default fill is 0 (transparent), which is the GPU's init value.
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        Self::with_format(
            device,
            None,
            width,
            height,
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureUsages::empty(),
            "layer-texture",
        )
    }

    /// RGBA layer texture sized + positioned to match the given bounds.
    /// Equivalent to `new` followed by setting `offset_x`/`offset_y`.
    pub fn with_bounds(device: &wgpu::Device, bounds: CanvasRect) -> Self {
        let mut t = Self::new(device, bounds.width, bounds.height);
        t.offset_x = bounds.origin.x;
        t.offset_y = bounds.origin.y;
        t
    }

    /// RGBA layer texture with `STORAGE_BINDING` for renderers that write the
    /// texture as a storage image (Vello's `render_to_texture` requires it).
    /// Only vector layers ask for this; it is not a global flag, so ordinary
    /// raster/void textures stay free of the storage-binding surface.
    pub fn with_bounds_storage(device: &wgpu::Device, bounds: CanvasRect) -> Self {
        let mut t = Self::with_format(
            device,
            None,
            bounds.width,
            bounds.height,
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureUsages::STORAGE_BINDING,
            "vector-layer-texture",
        );
        t.offset_x = bounds.origin.x;
        t.offset_y = bounds.origin.y;
        t
    }

    /// R8Unorm mask texture: default fill is 255 (white = reveal all).
    pub fn new_mask(device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32) -> Self {
        Self::with_format(
            device,
            Some(queue),
            width,
            height,
            wgpu::TextureFormat::R8Unorm,
            wgpu::TextureUsages::empty(),
            "mask-texture",
        )
    }

    /// R8Unorm mask texture sized + positioned to match the given canvas
    /// extent. The mask shares the parent layer's bounds so per-pixel
    /// sampling can use the same layer UV as the layer texture.
    pub fn new_mask_with_extent(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        extent: CanvasRect,
    ) -> Self {
        let mut t = Self::new_mask(device, queue, extent.width, extent.height);
        t.offset_x = extent.origin.x;
        t.offset_y = extent.origin.y;
        t
    }

    /// A texture like this one (format, usage, label, default fill) at
    /// `extent`: the reallocation constructor behind resize, staging and
    /// resample paths, which don't care what kind of texture they move.
    pub fn reallocated(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        extent: CanvasRect,
    ) -> Self {
        let mut t = Self::with_format(
            device,
            Some(queue),
            extent.width,
            extent.height,
            self.format,
            self.extra_usage,
            self.label,
        );
        t.offset_x = extent.origin.x;
        t.offset_y = extent.origin.y;
        t
    }

    fn with_format(
        device: &wgpu::Device,
        queue: Option<&wgpu::Queue>,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        extra_usage: wgpu::TextureUsages,
        label: &'static str,
    ) -> Self {
        let bpp = format.block_copy_size(None).unwrap_or(1);
        let fill_byte = match format {
            wgpu::TextureFormat::R8Unorm => 255u8, // white = reveal all
            _ => 0u8,                              // transparent
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | extra_usage,
            view_formats: &[],
        });

        // Fill with non-zero default if needed (GPU textures init to 0).
        if fill_byte != 0 {
            if let Some(queue) = queue {
                let row_bytes = width * bpp;
                let data = vec![fill_byte; (row_bytes * height) as usize];
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row_bytes),
                        rows_per_image: Some(height),
                    },
                    wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        LayerTexture {
            texture,
            view,
            width,
            height,
            offset_x: 0,
            offset_y: 0,
            format,
            extra_usage,
            label,
        }
    }

    // ----- Typed accessors -----

    /// Borrow the underlying GPU texture. Use this when handing the texture
    /// to a wgpu API; do not reach for coordinate information through it.
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// Borrow the texture's default view.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// Texture-local extent: always at origin `(0, 0)` with the texture's
    /// pixel dimensions. Use this when iterating over the texture in its own
    /// coordinate frame or when handing dimensions to wgpu (which speaks in
    /// texture pixels, not canvas pixels).
    pub fn layer_extent(&self) -> LayerRect {
        LayerRect::from_xywh(0, 0, self.width, self.height)
    }

    /// Canvas-space rect this texture occupies. The origin may be negative
    /// for paste-extent layers or layers grown leftward / upward of canvas.
    pub fn canvas_extent(&self) -> CanvasRect {
        CanvasRect::from_xywh(self.offset_x, self.offset_y, self.width, self.height)
    }

    /// Translate a canvas-space point to the texture's local coordinate frame.
    /// Returns `None` if the point falls outside the texture's extent.
    pub fn canvas_to_layer(&self, p: CanvasPoint) -> Option<LayerPoint> {
        let lx = p.x - self.offset_x;
        let ly = p.y - self.offset_y;
        if lx >= 0 && ly >= 0 && (lx as u32) < self.width && (ly as u32) < self.height {
            Some(LayerPoint::new(lx as u32, ly as u32))
        } else {
            None
        }
    }

    /// The affine taking canvas-space coordinates into this texture's local
    /// frame, for renderers that draw through a transform instead of
    /// addressing texels. The unbounded sibling of
    /// [`canvas_to_layer`](Self::canvas_to_layer): points outside the extent
    /// map outside the texture rather than to `None`.
    pub fn canvas_to_layer_affine(&self) -> kurbo::Affine {
        kurbo::Affine::translate((-self.offset_x as f64, -self.offset_y as f64))
    }

    /// Intersect a canvas-space rect with the texture's extent and translate
    /// the result into texture-local coordinates. Returns `None` if disjoint.
    pub fn canvas_to_layer_rect(&self, r: CanvasRect) -> Option<LayerRect> {
        let clipped = self.canvas_extent().intersect(r)?;
        let lx = (clipped.origin.x - self.offset_x) as u32;
        let ly = (clipped.origin.y - self.offset_y) as u32;
        Some(LayerRect::from_xywh(lx, ly, clipped.width, clipped.height))
    }

    /// Translate a texture-local point back to canvas space.
    pub fn layer_to_canvas(&self, p: LayerPoint) -> CanvasPoint {
        CanvasPoint::new(self.offset_x + p.x as i32, self.offset_y + p.y as i32)
    }

    /// Translate a texture-local rect back to canvas space.
    pub fn layer_to_canvas_rect(&self, r: LayerRect) -> CanvasRect {
        CanvasRect::from_xywh(
            self.offset_x + r.origin.x as i32,
            self.offset_y + r.origin.y as i32,
            r.width,
            r.height,
        )
    }

    /// Intersect a canvas-space rect with this texture's extent. Returns
    /// `None` if disjoint or empty.
    pub fn clamp_canvas_rect(&self, r: CanvasRect) -> Option<CanvasRect> {
        self.canvas_extent().intersect(r)
    }

    /// Borrow this texture as a `CanvasFrame`: a thin (texture, canvas
    /// extent) value passed to the GPU adapter boundary helpers.
    pub fn canvas_frame(&self) -> CanvasFrame<'_> {
        CanvasFrame {
            texture: &self.texture,
            canvas_extent: self.canvas_extent(),
        }
    }
}

/// A texture paired with the canvas-space rect it occupies. Used as the
/// argument type at the GPU adapter boundary so callers don't have to know
/// whether the underlying texture is a `LayerTexture` (layer-aligned) or a
/// canvas-aligned texture like the selection mask. The frame owns nothing;
/// borrows are elided at call sites.
#[derive(Copy, Clone)]
pub struct CanvasFrame<'a> {
    pub texture: &'a wgpu::Texture,
    pub canvas_extent: CanvasRect,
}

impl<'a> CanvasFrame<'a> {
    /// Intersect a canvas-space rect with this frame's extent and translate
    /// the result into texture-local coordinates. Returns `None` if disjoint.
    pub fn canvas_to_layer_rect(&self, r: CanvasRect) -> Option<LayerRect> {
        let clipped = self.canvas_extent.intersect(r)?;
        let lx = (clipped.origin.x - self.canvas_extent.origin.x) as u32;
        let ly = (clipped.origin.y - self.canvas_extent.origin.y) as u32;
        Some(LayerRect::from_xywh(lx, ly, clipped.width, clipped.height))
    }

    /// Copy the canvas rect `rect` from this frame into `dst`: the rect is
    /// clipped to both extents and each side is addressed in its own
    /// texture-local coordinates, so two frames at different canvas
    /// origins exchange a region by its canvas position alone. A rect
    /// disjoint from either frame copies nothing.
    pub fn copy_rect_to(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        dst: &CanvasFrame<'_>,
        rect: CanvasRect,
    ) {
        let Some(clipped) = rect
            .intersect(self.canvas_extent)
            .and_then(|r| r.intersect(dst.canvas_extent))
        else {
            return;
        };
        let (Some(src), Some(dst_local)) = (
            self.canvas_to_layer_rect(clipped),
            dst.canvas_to_layer_rect(clipped),
        ) else {
            return;
        };
        crate::gpu::blit_region(
            encoder,
            self.texture,
            (src.x0(), src.y0()),
            dst.texture,
            (dst_local.x0(), dst_local.y0()),
            clipped.width,
            clipped.height,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::test_utils::test_device;

    fn make_layer(off_x: i32, off_y: i32, w: u32, h: u32) -> LayerTexture {
        let (device, _queue) = test_device();
        LayerTexture::with_bounds(&device, CanvasRect::from_xywh(off_x, off_y, w, h))
    }

    /// Two frames at different negative canvas origins exchange a rect by
    /// its canvas position: the copy lands at each side's own local
    /// offset and is clipped to both extents and the rect.
    #[test]
    fn copy_rect_to_addresses_each_frame_by_its_own_origin() {
        use crate::gpu::test_utils::readback_texture;
        let (device, queue) = test_device();
        let (w, h) = (64u32, 64u32);
        let make = |label| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let (src_tex, dst_tex) = (make("src"), make("dst"));
        // Source pixel = its own local coordinates, so a readback of the
        // destination says exactly which source texel landed where.
        let pattern: Vec<u8> = (0..h)
            .flat_map(|y| (0..w).flat_map(move |x| [x as u8, y as u8, 7, 255]))
            .collect();
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &src_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pattern,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        let src = CanvasFrame {
            texture: &src_tex,
            canvas_extent: CanvasRect::from_xywh(-100, 50, w, h),
        };
        let dst = CanvasFrame {
            texture: &dst_tex,
            canvas_extent: CanvasRect::from_xywh(-80, 40, w, h),
        };
        // Partly outside both frames, so the clip to each extent matters.
        let rect = CanvasRect::from_xywh(-200, 0, 150, 70);
        let mut encoder = device.create_command_encoder(&Default::default());
        src.copy_rect_to(&mut encoder, &dst, rect);
        queue.submit([encoder.finish()]);
        let out = readback_texture(
            &device,
            &queue,
            &dst_tex,
            wgpu::TextureFormat::Rgba8Unorm,
            w,
            h,
        );
        let landed = rect
            .intersect(src.canvas_extent)
            .and_then(|r| r.intersect(dst.canvas_extent))
            .unwrap();
        assert_eq!(landed, CanvasRect::from_xywh(-80, 50, 30, 20));
        for dy in 0..h {
            for dx in 0..w {
                let (cx, cy) = (
                    dst.canvas_extent.x0() + dx as i32,
                    dst.canvas_extent.y0() + dy as i32,
                );
                let i = ((dy * w + dx) * 4) as usize;
                let got = [out[i], out[i + 1], out[i + 2], out[i + 3]];
                let expected = if landed.contains(CanvasRect::from_xywh(cx, cy, 1, 1)) {
                    let sx = (cx - src.canvas_extent.x0()) as u8;
                    let sy = (cy - src.canvas_extent.y0()) as u8;
                    [sx, sy, 7, 255]
                } else {
                    [0, 0, 0, 0]
                };
                assert_eq!(
                    got, expected,
                    "destination local ({dx}, {dy}), canvas ({cx}, {cy})"
                );
            }
        }
    }

    #[test]
    fn canvas_extent_reflects_offset_and_size() {
        let l = make_layer(-100, 50, 200, 300);
        assert_eq!(l.canvas_extent(), CanvasRect::from_xywh(-100, 50, 200, 300));
    }

    #[test]
    fn canvas_to_layer_round_trip() {
        let l = make_layer(-100, 50, 200, 300);
        let p = CanvasPoint::new(-50, 100);
        let lp = l.canvas_to_layer(p).unwrap();
        assert_eq!(lp, LayerPoint::new(50, 50));
        assert_eq!(l.layer_to_canvas(lp), p);
    }

    #[test]
    fn canvas_to_layer_outside_returns_none() {
        let l = make_layer(0, 0, 100, 100);
        assert_eq!(l.canvas_to_layer(CanvasPoint::new(-1, 50)), None);
        assert_eq!(l.canvas_to_layer(CanvasPoint::new(100, 50)), None);
        assert_eq!(l.canvas_to_layer(CanvasPoint::new(50, 100)), None);
    }

    #[test]
    fn canvas_to_layer_rect_clips_to_extent() {
        let l = make_layer(0, 0, 100, 100);
        let r = CanvasRect::from_xywh(50, 50, 200, 200);
        let lr = l.canvas_to_layer_rect(r).unwrap();
        assert_eq!(lr, LayerRect::from_xywh(50, 50, 50, 50));
    }

    #[test]
    fn canvas_to_layer_rect_disjoint_is_none() {
        let l = make_layer(0, 0, 100, 100);
        assert_eq!(
            l.canvas_to_layer_rect(CanvasRect::from_xywh(200, 200, 50, 50)),
            None,
        );
    }

    #[test]
    fn clamp_canvas_rect_inside_is_unchanged() {
        let l = make_layer(-50, -50, 200, 200);
        let r = CanvasRect::from_xywh(0, 0, 100, 100);
        assert_eq!(l.clamp_canvas_rect(r), Some(r));
    }

    #[test]
    fn layer_to_canvas_rect_offsets_origin() {
        let l = make_layer(-100, 50, 200, 300);
        let lr = LayerRect::from_xywh(10, 20, 30, 40);
        assert_eq!(
            l.layer_to_canvas_rect(lr),
            CanvasRect::from_xywh(-90, 70, 30, 40),
        );
    }

    /// A dab footprint that crosses the edges of an offset (paste-extent) layer
    /// clamps to the layer's canvas extent, then translates into texture-local
    /// pixels. The layer spans canvas x[-100,100] y[50,350]; the dab x[60,120]
    /// y[30,90] pokes past the right and top edges.
    #[test]
    fn clamp_f32_then_layer_rect_off_extent_dab() {
        let l = make_layer(-100, 50, 200, 300);
        let extent = l.canvas_extent();

        let canvas_bbox = extent.clamp_f32(60.0, 30.0, 120.0, 90.0).unwrap();
        // Clipped to the extent's right (100) and top (50) edges.
        assert_eq!(canvas_bbox, CanvasRect::from_xywh(60, 50, 40, 40));

        // Translated into the texture's local frame (subtract the -100/50 offset).
        let local = l.canvas_to_layer_rect(canvas_bbox).unwrap();
        assert_eq!(local, LayerRect::from_xywh(160, 0, 40, 40));
    }
}
