//! GPU-side undo snapshot storage.
//!
//! Manages a shared scratch texture (pre-operation snapshot) and a ring-buffer
//! undo buffer that stores completed undo entries as raw pixel data.

use crate::layer::LayerId;
use std::collections::VecDeque;

/// Alignment required by wgpu for bytes_per_row in buffer↔texture copies.
const COPY_ROW_ALIGNMENT: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

/// Default undo buffer capacity: 256 MB.
const DEFAULT_CAPACITY: u64 = 256 * 1024 * 1024;

/// GPU-side undo snapshot storage.
///
/// Two components:
/// 1. **Scratch textures** — hold pre-operation snapshots of the affected region.
///    One RGBA8 and one R8, both sized to the canvas. Reused across all operations.
/// 2. **Undo buffer** — ring buffer of raw pixel data for completed undo entries.
pub struct RegionStore {
    // --- Scratch textures (one per format) ---
    scratch_rgba: wgpu::Texture,
    scratch_r8: wgpu::Texture,
    scratch_width: u32,
    scratch_height: u32,

    // --- Undo ring buffer ---
    buffer: wgpu::Buffer,
    capacity: u64,
    head: u64,
    entries: VecDeque<UndoRegionEntry>,
}

/// Metadata for a single undo region stored in the ring buffer.
#[derive(Debug, Clone)]
pub struct UndoRegionEntry {
    pub layer_id: LayerId,
    /// Region in texture space: [x, y, width, height].
    pub rect: [u32; 4],
    pub format: wgpu::TextureFormat,
    /// Byte offset into the undo buffer.
    offset: u64,
    /// Bytes per row in the buffer (padded to COPY_ROW_ALIGNMENT).
    padded_row_bytes: u32,
    /// Total bytes occupied in the buffer.
    byte_size: u64,
}

impl RegionStore {
    pub fn new(device: &wgpu::Device, canvas_width: u32, canvas_height: u32) -> Self {
        Self::with_capacity(device, canvas_width, canvas_height, DEFAULT_CAPACITY)
    }

    pub fn with_capacity(
        device: &wgpu::Device,
        canvas_width: u32,
        canvas_height: u32,
        capacity: u64,
    ) -> Self {
        let scratch_rgba = Self::create_scratch(
            device,
            canvas_width,
            canvas_height,
            wgpu::TextureFormat::Rgba8Unorm,
            "scratch-rgba",
        );
        let scratch_r8 = Self::create_scratch(
            device,
            canvas_width,
            canvas_height,
            wgpu::TextureFormat::R8Unorm,
            "scratch-r8",
        );

        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("undo-ring-buffer"),
            size: capacity,
            usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        RegionStore {
            scratch_rgba,
            scratch_r8,
            scratch_width: canvas_width,
            scratch_height: canvas_height,
            buffer,
            capacity,
            head: 0,
            entries: VecDeque::new(),
        }
    }

    /// Grow scratch textures so they can fit a rect of at least `(w, h)`.
    /// No-op if the current scratch is already large enough. Reallocation
    /// is rare in practice — only happens when a save rect exceeds canvas
    /// bounds (paste-extent layer transform, oversized stroke, etc.).
    ///
    /// Call this once before encoding `save_region` for any rect that
    /// might exceed the current scratch dimensions; routine canvas-bounded
    /// callers can skip it (`save_region` doesn't grow on its own to keep
    /// borrowing simple at call sites that already hold an immutable
    /// borrow into `self`).
    pub fn ensure_scratch_capacity(&mut self, device: &wgpu::Device, w: u32, h: u32) {
        if w <= self.scratch_width && h <= self.scratch_height {
            return;
        }
        let new_w = w.max(self.scratch_width);
        let new_h = h.max(self.scratch_height);
        self.scratch_rgba = Self::create_scratch(
            device,
            new_w,
            new_h,
            wgpu::TextureFormat::Rgba8Unorm,
            "scratch-rgba",
        );
        self.scratch_r8 = Self::create_scratch(
            device,
            new_w,
            new_h,
            wgpu::TextureFormat::R8Unorm,
            "scratch-r8",
        );
        self.scratch_width = new_w;
        self.scratch_height = new_h;
    }

    /// Copy a rect from a layer/mask texture into the scratch texture.
    /// Call this at stroke start to snapshot the region before painting.
    ///
    /// The snapshot always lands at scratch (0, 0); only the rect's
    /// width/height need to fit the scratch. Callers whose rect may
    /// exceed canvas dimensions must call `ensure_scratch_capacity`
    /// first.
    pub fn save_region(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::Texture,
        format: wgpu::TextureFormat,
        rect: [u32; 4],
    ) {
        let [x, y, w, h] = rect;
        debug_assert!(
            w <= self.scratch_width && h <= self.scratch_height,
            "save_region rect ({w}x{h}) exceeds scratch capacity \
             ({}x{}); call ensure_scratch_capacity first",
            self.scratch_width,
            self.scratch_height
        );
        let scratch = self.scratch_for(format);

        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: source,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: scratch,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Copy the saved scratch region into the undo ring buffer.
    /// Call this at stroke end. Returns the entry metadata for the undo stack.
    pub fn commit_region(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        layer_id: LayerId,
        format: wgpu::TextureFormat,
        rect: [u32; 4],
    ) -> UndoRegionEntry {
        let [_x, _y, w, h] = rect;
        let bpp = format.block_copy_size(None).unwrap_or(1);
        let padded_row_bytes = padded_row(w, bpp);
        let byte_size = padded_row_bytes as u64 * h as u64;

        let offset = self.allocate(byte_size);
        let scratch = self.scratch_for(format);

        // Scratch holds the pre-op snapshot at origin (0, 0) (see
        // `save_region`); the rect's xy describe where the snapshot came
        // from on the layer, not where it sits in scratch.
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: scratch,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset,
                    bytes_per_row: Some(padded_row_bytes),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );

        let entry = UndoRegionEntry {
            layer_id,
            rect,
            format,
            offset,
            padded_row_bytes,
            byte_size,
        };
        self.entries.push_back(entry.clone());
        entry
    }

    /// Restore saved pixels from the undo buffer back to the layer texture.
    /// Returns a forward entry (the pre-restore state) for redo.
    ///
    /// Uses the scratch texture as a safe intermediate to avoid buffer overlap.
    pub fn restore_region(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        entry: &UndoRegionEntry,
        texture: &wgpu::Texture,
    ) -> UndoRegionEntry {
        let [x, y, w, h] = entry.rect;
        let layer_origin = wgpu::Origin3d { x, y, z: 0 };
        let scratch_origin = wgpu::Origin3d::ZERO;
        let extent = wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        };

        // 1. Copy current texture rect → scratch[0, 0] (save current state).
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: layer_origin,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: self.scratch_for(entry.format),
                mip_level: 0,
                origin: scratch_origin,
                aspect: wgpu::TextureAspect::All,
            },
            extent,
        );

        // 2. Copy saved buffer → texture (restore old state at the layer's rect).
        encoder.copy_buffer_to_texture(
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: entry.offset,
                    bytes_per_row: Some(entry.padded_row_bytes),
                    rows_per_image: Some(h),
                },
            },
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: layer_origin,
                aspect: wgpu::TextureAspect::All,
            },
            extent,
        );

        // 3. Copy scratch[0, 0] → buffer (save current state as forward entry).
        let forward_offset = self.allocate(entry.byte_size);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: self.scratch_for(entry.format),
                mip_level: 0,
                origin: scratch_origin,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: forward_offset,
                    bytes_per_row: Some(entry.padded_row_bytes),
                    rows_per_image: Some(h),
                },
            },
            extent,
        );

        UndoRegionEntry {
            layer_id: entry.layer_id,
            rect: entry.rect,
            format: entry.format,
            offset: forward_offset,
            padded_row_bytes: entry.padded_row_bytes,
            byte_size: entry.byte_size,
        }
    }

    /// Restore a region directly from the scratch texture to the target.
    ///
    /// Used by `cancel_floating()` to undo the source region clear without
    /// going through the ring buffer. The scratch must still contain the
    /// pre-clear snapshot from a prior `save_region()` call.
    pub fn restore_from_scratch(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        format: wgpu::TextureFormat,
        rect: [u32; 4],
        target: &wgpu::Texture,
    ) {
        let [x, y, w, h] = rect;
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: self.scratch_for(format),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: target,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Reallocate scratch textures when the canvas size changes.
    pub fn resize_scratch(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == self.scratch_width && height == self.scratch_height {
            return;
        }
        self.scratch_rgba = Self::create_scratch(
            device,
            width,
            height,
            wgpu::TextureFormat::Rgba8Unorm,
            "scratch-rgba",
        );
        self.scratch_r8 = Self::create_scratch(
            device,
            width,
            height,
            wgpu::TextureFormat::R8Unorm,
            "scratch-r8",
        );
        self.scratch_width = width;
        self.scratch_height = height;
    }

    /// Create a texture view for the scratch texture of the given format.
    /// Used by `DiffRectPass` to compare pre-stroke state against current canvas.
    pub fn scratch_view(&self, format: wgpu::TextureFormat) -> wgpu::TextureView {
        self.scratch_for(format)
            .create_view(&wgpu::TextureViewDescriptor::default())
    }

    /// Canvas dimensions of the scratch textures.
    pub fn scratch_dimensions(&self) -> (u32, u32) {
        (self.scratch_width, self.scratch_height)
    }

    // --- Internal ---

    fn scratch_for(&self, format: wgpu::TextureFormat) -> &wgpu::Texture {
        match format {
            wgpu::TextureFormat::R8Unorm => &self.scratch_r8,
            _ => &self.scratch_rgba,
        }
    }

    fn create_scratch(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        label: &str,
    ) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
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
            usage: wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    }

    /// Allocate `size` bytes in the ring buffer, evicting oldest entries as needed.
    /// Returns the byte offset of the allocation.
    fn allocate(&mut self, size: u64) -> u64 {
        assert!(size <= self.capacity, "undo entry too large for buffer");

        // If the entry doesn't fit at the current head, wrap to start.
        if self.head + size > self.capacity {
            self.head = 0;
        }

        let alloc_start = self.head;
        let alloc_end = alloc_start + size;

        // Evict entries that overlap with the new allocation.
        while let Some(front) = self.entries.front() {
            let entry_end = front.offset + front.byte_size;
            // Overlap: the new allocation range intersects the front entry's range.
            if front.offset < alloc_end && entry_end > alloc_start {
                self.entries.pop_front();
            } else {
                break;
            }
        }

        self.head = alloc_end;
        alloc_start
    }
}

/// Compute the row byte count padded to wgpu's copy alignment.
fn padded_row(width: u32, bytes_per_pixel: u32) -> u32 {
    let unpadded = width * bytes_per_pixel;
    unpadded.div_ceil(COPY_ROW_ALIGNMENT) * COPY_ROW_ALIGNMENT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padded_row_alignment() {
        // 128 pixels × 4 bpp = 512 bytes, already aligned to 256.
        assert_eq!(padded_row(128, 4), 512);
        // 100 pixels × 4 bpp = 400 bytes → next multiple of 256 = 512.
        assert_eq!(padded_row(100, 4), 512);
        // 64 pixels × 1 bpp = 64 bytes → 256.
        assert_eq!(padded_row(64, 1), 256);
        // 256 pixels × 1 bpp = 256 bytes, already aligned.
        assert_eq!(padded_row(256, 1), 256);
        // 1 pixel × 4 bpp = 4 bytes → 256.
        assert_eq!(padded_row(1, 4), 256);
    }

    /// Simulates the ring buffer allocation logic without GPU resources.
    struct MockRing {
        capacity: u64,
        head: u64,
        entries: VecDeque<(u64, u64)>,
    }

    impl MockRing {
        fn new(capacity: u64) -> Self {
            MockRing {
                capacity,
                head: 0,
                entries: VecDeque::new(),
            }
        }

        fn alloc(&mut self, size: u64) -> u64 {
            if self.head + size > self.capacity {
                self.head = 0;
            }
            let start = self.head;
            let end = start + size;
            while let Some(&(offset, byte_size)) = self.entries.front() {
                if offset < end && offset + byte_size > start {
                    self.entries.pop_front();
                } else {
                    break;
                }
            }
            self.head = end;
            self.entries.push_back((start, size));
            start
        }
    }

    #[test]
    fn ring_buffer_allocation_basic() {
        let mut ring = MockRing::new(1024);

        // Fill buffer with 4 × 256-byte entries.
        assert_eq!(ring.alloc(256), 0);
        assert_eq!(ring.alloc(256), 256);
        assert_eq!(ring.alloc(256), 512);
        assert_eq!(ring.alloc(256), 768);
        assert_eq!(ring.entries.len(), 4);

        // Next allocation wraps and evicts the oldest.
        assert_eq!(ring.alloc(256), 0);
        assert_eq!(ring.entries.len(), 4); // oldest evicted, new one added
        assert_eq!(ring.entries.front().unwrap().0, 256); // second entry is now front
    }
}
