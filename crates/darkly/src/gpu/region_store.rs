//! GPU-side undo snapshot storage.
//!
//! Manages a shared scratch texture (pre-operation snapshot) and a ring-buffer
//! undo buffer that stores completed undo entries as raw pixel data.
//!
//! # Coordinate frame
//!
//! All public methods take [`LayerRect`] for the rect parameter — texture-local
//! pixel coords. The scratch is texture-aligned: scratch[(x, y)] holds the
//! pre-op snapshot of source[(x, y)]. Layer textures use layer-local coords;
//! the selection texture is canvas-sized at offset 0, so its texture-local
//! frame coincides with canvas (and `LayerRect` is the right type for it too,
//! despite the name).
//!
//! [`save_region`](Self::save_region) returns a [`Snapshot`] token. The token
//! is required by [`commit_region`](Self::commit_region) and
//! [`restore_from_scratch`](Self::restore_from_scratch) — you can't commit
//! without saving first. Commits validate (in debug) that the commit rect is
//! contained in the snapshot's saved rect.

use crate::coord::LayerRect;
use crate::layer::LayerId;
use std::collections::VecDeque;

/// Token returned by [`RegionStore::save_region`]. Carries the saved
/// rect and format; required to commit or restore from the scratch.
///
/// `Copy` because it's just a pair of u32s and a small enum tag, and several
/// flows hold it as a struct field across deferred GPU work.
///
/// When the underlying scratch is rebased mid-stroke (see
/// [`RegionStore::grow_scratch_preserving`]), holders of a live `Snapshot`
/// are responsible for updating `saved` to reflect the new layer frame —
/// see `engine::painting::ensure_layer_covers_dab` for the canonical update.
#[derive(Copy, Clone, Debug)]
pub struct Snapshot {
    pub saved: LayerRect,
    pub format: wgpu::TextureFormat,
}

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
    /// Region in texture space (layer-local).
    pub rect: LayerRect,
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

    /// Reallocate the scratch textures to `(new_w, new_h)` and copy the
    /// existing scratch contents into the new textures at
    /// `(dst_offset_x, dst_offset_y)`. Used during mid-stroke layer
    /// growth: the scratch holds the pre-stroke snapshot, which must
    /// remain anchored to the same canvas-space pixels even though the
    /// layer's local-coord origin has shifted.
    ///
    /// The newly-allocated regions outside the copied rect start at the
    /// GPU default (0 = transparent for RGBA, 0 = full transparency for R8).
    pub fn grow_scratch_preserving(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        new_w: u32,
        new_h: u32,
        dst_offset_x: u32,
        dst_offset_y: u32,
    ) {
        if new_w <= self.scratch_width
            && new_h <= self.scratch_height
            && dst_offset_x == 0
            && dst_offset_y == 0
        {
            return;
        }
        let copy_w = self.scratch_width;
        let copy_h = self.scratch_height;
        let new_rgba = Self::create_scratch(
            device,
            new_w.max(self.scratch_width),
            new_h.max(self.scratch_height),
            wgpu::TextureFormat::Rgba8Unorm,
            "scratch-rgba",
        );
        let new_r8 = Self::create_scratch(
            device,
            new_w.max(self.scratch_width),
            new_h.max(self.scratch_height),
            wgpu::TextureFormat::R8Unorm,
            "scratch-r8",
        );

        if copy_w > 0 && copy_h > 0 {
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.scratch_rgba,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &new_rgba,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: dst_offset_x,
                        y: dst_offset_y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: copy_w,
                    height: copy_h,
                    depth_or_array_layers: 1,
                },
            );
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.scratch_r8,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &new_r8,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: dst_offset_x,
                        y: dst_offset_y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: copy_w,
                    height: copy_h,
                    depth_or_array_layers: 1,
                },
            );
        }

        self.scratch_rgba = new_rgba;
        self.scratch_r8 = new_r8;
        self.scratch_width = new_w.max(self.scratch_width);
        self.scratch_height = new_h.max(self.scratch_height);
    }

    /// Copy a rect from a layer/mask texture into the scratch texture and
    /// return a [`Snapshot`] token.
    ///
    /// The scratch is texture-aligned: the snapshot lands at the same
    /// `(rect.x0, rect.y0)` it came from on the source. This lets a later
    /// [`commit_region`](Self::commit_region) commit a sub-rect of the
    /// snapshot at its own `(x, y)` (the brush flow saves the full layer
    /// here and commits just the diff_rect at stroke end).
    ///
    /// Callers whose rect may exceed the current scratch dimensions must
    /// call [`ensure_scratch_capacity`](Self::ensure_scratch_capacity) first.
    pub fn save_region(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::Texture,
        format: wgpu::TextureFormat,
        rect: LayerRect,
    ) -> Snapshot {
        debug_assert!(
            rect.x1() <= self.scratch_width && rect.y1() <= self.scratch_height,
            "save_region rect {:?} exceeds scratch capacity ({}x{}); call ensure_scratch_capacity first",
            rect,
            self.scratch_width,
            self.scratch_height
        );
        let scratch = self.scratch_for(format);

        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: source,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: rect.x0(),
                    y: rect.y0(),
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: scratch,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: rect.x0(),
                    y: rect.y0(),
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: rect.width,
                height: rect.height,
                depth_or_array_layers: 1,
            },
        );

        Snapshot {
            saved: rect,
            format,
        }
    }

    /// Copy a sub-rect of the saved scratch region into the undo ring buffer.
    /// Call this at stroke end. Returns the entry metadata for the undo stack.
    ///
    /// `rect` must be contained in `snapshot.saved`. In debug builds this is
    /// asserted at runtime; release builds will silently read whatever
    /// scratch contents lie at the rect (likely uninitialised junk from a
    /// prior op), so don't rely on the assert being inert.
    pub fn commit_region(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        layer_id: LayerId,
        snapshot: &Snapshot,
        rect: LayerRect,
    ) -> UndoRegionEntry {
        debug_assert!(
            snapshot.saved.contains(rect),
            "commit_region rect {:?} not contained in snapshot.saved {:?}",
            rect,
            snapshot.saved,
        );
        let bpp = snapshot.format.block_copy_size(None).unwrap_or(1);
        let padded_row_bytes = padded_row(rect.width, bpp);
        let byte_size = padded_row_bytes as u64 * rect.height as u64;

        let offset = self.allocate(byte_size);
        let scratch = self.scratch_for(snapshot.format);

        // Scratch is texture-aligned (see `save_region`): the snapshot of
        // pixels at layer-space `(x, y)` lives at scratch `(x, y)`.
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: scratch,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: rect.x0(),
                    y: rect.y0(),
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset,
                    bytes_per_row: Some(padded_row_bytes),
                    rows_per_image: Some(rect.height),
                },
            },
            wgpu::Extent3d {
                width: rect.width,
                height: rect.height,
                depth_or_array_layers: 1,
            },
        );

        let entry = UndoRegionEntry {
            layer_id,
            rect,
            format: snapshot.format,
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
        let origin = wgpu::Origin3d {
            x: entry.rect.x0(),
            y: entry.rect.y0(),
            z: 0,
        };
        let extent = wgpu::Extent3d {
            width: entry.rect.width,
            height: entry.rect.height,
            depth_or_array_layers: 1,
        };

        // 1. Copy current texture rect → scratch at the same (x, y).
        //    Scratch is texture-aligned, so steps 2/3 read it back from the
        //    same origin without an extra translation.
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: self.scratch_for(entry.format),
                mip_level: 0,
                origin,
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
                    rows_per_image: Some(entry.rect.height),
                },
            },
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin,
                aspect: wgpu::TextureAspect::All,
            },
            extent,
        );

        // 3. Copy scratch (now holding the pre-restore state) → buffer for redo.
        let forward_offset = self.allocate(entry.byte_size);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: self.scratch_for(entry.format),
                mip_level: 0,
                origin,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: forward_offset,
                    bytes_per_row: Some(entry.padded_row_bytes),
                    rows_per_image: Some(entry.rect.height),
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

    /// Restore a region directly from the scratch texture to the target,
    /// without going through the ring buffer. Used by `cancel_floating()`
    /// to undo the source region clear.
    ///
    /// `rect` must be contained in `snapshot.saved` — the scratch only holds
    /// the snapshot at that footprint; reading outside it would pull in
    /// uninitialised pixels from a prior op.
    pub fn restore_from_scratch(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        snapshot: &Snapshot,
        rect: LayerRect,
        target: &wgpu::Texture,
    ) {
        debug_assert!(
            snapshot.saved.contains(rect),
            "restore_from_scratch rect {:?} not contained in snapshot.saved {:?}",
            rect,
            snapshot.saved,
        );
        let origin = wgpu::Origin3d {
            x: rect.x0(),
            y: rect.y0(),
            z: 0,
        };
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: self.scratch_for(snapshot.format),
                mip_level: 0,
                origin,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: target,
                mip_level: 0,
                origin,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: rect.width,
                height: rect.height,
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
