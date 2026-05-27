//! GPU-side undo snapshot storage.
//!
//! Manages shared scratch textures (pre-operation snapshot, in-flight workspace
//! for save+modify+commit) and produces per-action [`UndoRegionEntry`] values
//! that own their own pixel data — no shared ring buffer.
//!
//! # Lifetime model
//!
//! Each undo entry owns its pixels via [`EntryPixels`]:
//!
//! - **`Pending { staging }`** — VRAM-resident. The `wgpu::Buffer` that backs
//!   the async readback. Holds until either (a) the readback completes and the
//!   entry transitions to `Ready`, dropping the buffer, or (b) a restore
//!   happens first, in which case the buffer feeds `copy_buffer_to_texture`
//!   directly (GPU-to-GPU, no readback wait).
//! - **`Ready(Vec<u8>)`** — DRAM-resident, unpadded row layout. The steady
//!   state for most actions, since most commits' readbacks finish before any
//!   restore is requested.
//!
//! When the action drops (max_steps overflow, byte-cap overflow, redo cleared,
//! teardown), the staging buffer or `Vec` drops with it. No shared storage, no
//! eviction at the storage layer, no aliasing.
//!
//! # Coordinate frame
//!
//! All public methods take [`CanvasRect`] for rect parameters and a
//! [`CanvasFrame`] for the source/target texture. Translation to texture-local
//! coordinates happens internally, immediately before each `copy_texture_*`
//! call. The scratch is texture-aligned (scratch[(x, y)] holds the pre-op
//! snapshot of source[(x, y)]) but the *metadata* — `Snapshot.saved` and
//! `UndoRegionEntry.canvas_rect` — is in canvas coords so it remains valid
//! across mid-stroke layer growth (the Storage Frame Rule).
//!
//! [`save_region`](Self::save_region) returns a [`Snapshot`] token. The token
//! is required by [`commit_region`](Self::commit_region) and
//! [`restore_from_scratch`](Self::restore_from_scratch) — you can't commit
//! without saving first. Commits validate (in debug) that the commit rect is
//! contained in the snapshot's saved rect.

use crate::coord::CanvasRect;
use crate::gpu::atlas::CanvasFrame;
use crate::gpu::readback::ReadbackRequest;
use crate::layer::LayerId;
use std::cell::RefCell;
use std::rc::Rc;

/// Token returned by [`RegionScratch::save_region`]. Carries the saved
/// rect (in canvas coords) and format; required to commit or restore from
/// the scratch.
///
/// `Copy` because it's just a small struct, and several flows hold it as a
/// field across deferred GPU work.
#[derive(Copy, Clone, Debug)]
pub struct Snapshot {
    pub saved: CanvasRect,
    pub format: wgpu::TextureFormat,
}

/// Alignment required by wgpu for bytes_per_row in buffer↔texture copies.
const COPY_ROW_ALIGNMENT: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

/// Per-entry pixel storage, hybrid VRAM/DRAM.
///
/// Single-threaded — the engine drives commits, restores, and readback
/// completion from the same thread, so `Rc<RefCell<…>>` rather than
/// `Arc<Mutex<…>>`. WASM is single-threaded by construction; native tests
/// run with `--test-threads=1`.
pub enum EntryPixels {
    /// Async readback in flight. Holds the committed pixels in two
    /// sibling buffers — WebGPU disallows combining `MAP_READ` and
    /// `COPY_SRC` on a single buffer, so the readback and restore paths
    /// need separate VRAM. Both get filled from the same scratch in the
    /// commit encoder, so their contents are byte-identical.
    Pending {
        /// MAP_READ | COPY_DST — drives the async readback that flips the
        /// entry to `Ready`.
        readback: wgpu::Buffer,
        /// COPY_DST | COPY_SRC — source for the GPU-to-GPU restore path
        /// when a restore arrives before the readback completes. Both
        /// buffers drop together when the entry transitions to `Ready`.
        staging: wgpu::Buffer,
    },
    /// Readback completed, pixels live on the host heap (WASM linear memory
    /// in production, native DRAM in tests). The buffer layout is
    /// `unpadded_row_bytes * height` — restoring re-pads into a temp upload
    /// buffer because `copy_buffer_to_texture` requires
    /// `COPY_BYTES_PER_ROW_ALIGNMENT` rows.
    Ready(Vec<u8>),
}

/// Metadata + owned pixels for a single undo region. No longer `Clone` —
/// each action owns exactly one entry, and the `Rc<RefCell<…>>` would
/// duplicate the pixel-ownership relationship if cloned.
pub struct UndoRegionEntry {
    pub layer_id: LayerId,
    /// Region in canvas-space pixel coords. Stable across layer growth.
    pub canvas_rect: CanvasRect,
    pub format: wgpu::TextureFormat,
    /// Bytes per row in the buffer (padded to COPY_ROW_ALIGNMENT).
    pub padded_row_bytes: u32,
    /// Bytes per row without padding (`width * bpp`).
    pub unpadded_row_bytes: u32,
    /// VRAM-equivalent byte cost of this entry (padded rows × height). Used
    /// for the [`crate::undo::UndoStack`] memory cap — treated as an upper
    /// bound even when the entry has transitioned to `Ready` (whose unpadded
    /// `Vec` may be slightly smaller). Conservative is correct here.
    pub byte_size: u64,
    /// Shared cell so the readback completion handler can flip
    /// `Pending → Ready` after the action has been pushed onto the undo
    /// stack. Cloned once into the readback request's context at commit
    /// time; the handler holds that clone, the entry holds the other.
    pub pixels: Rc<RefCell<EntryPixels>>,
}

/// Shared scratch + per-entry storage producer for GPU undo regions.
///
/// Holds only the in-flight workspace textures (one RGBA8, one R8) used as
/// pre-op snapshots and commit intermediaries. Each undo entry owns its own
/// pixel data — there is no shared ring buffer. Storage lifetime equals
/// action lifetime, so eviction happens at the policy layer
/// ([`crate::undo::UndoStack`]) rather than down here.
pub struct RegionScratch {
    // --- Scratch textures (one per format) ---
    scratch_rgba: wgpu::Texture,
    scratch_r8: wgpu::Texture,
    scratch_width: u32,
    scratch_height: u32,
}

impl RegionScratch {
    pub fn new(device: &wgpu::Device, canvas_width: u32, canvas_height: u32) -> Self {
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

        RegionScratch {
            scratch_rgba,
            scratch_r8,
            scratch_width: canvas_width,
            scratch_height: canvas_height,
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

    /// Copy a canvas-space rect from a layer/mask texture into the scratch
    /// texture and return a [`Snapshot`] token.
    ///
    /// The scratch is texture-aligned to `source`: the snapshot lands at the
    /// same layer-local `(x, y)` it came from on the source. This lets a
    /// later [`commit_region`](Self::commit_region) commit a sub-rect of the
    /// snapshot at its own canvas position.
    ///
    /// Grows the scratch textures on demand if the translated rect exceeds
    /// the current scratch capacity (e.g. on a paste-extent layer or a
    /// layer that was just grown past canvas). Callers that batch many
    /// saves at known dimensions may still pre-call
    /// [`ensure_scratch_capacity`](Self::ensure_scratch_capacity) to avoid
    /// the per-call branch, but it is no longer required for correctness.
    pub fn save_region(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source: &CanvasFrame<'_>,
        format: wgpu::TextureFormat,
        canvas_rect: CanvasRect,
    ) -> Snapshot {
        let layer_rect = source
            .canvas_to_layer_rect(canvas_rect)
            .expect("save_region rect must overlap the source's canvas extent");
        self.ensure_scratch_capacity(device, layer_rect.x1(), layer_rect.y1());
        let scratch = self.scratch_for(format);

        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: source.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: layer_rect.x0(),
                    y: layer_rect.y0(),
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: scratch,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: layer_rect.x0(),
                    y: layer_rect.y0(),
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: layer_rect.width,
                height: layer_rect.height,
                depth_or_array_layers: 1,
            },
        );

        Snapshot {
            saved: canvas_rect,
            format,
        }
    }

    /// Copy a sub-rect of the saved scratch region into a freshly-allocated
    /// per-entry staging buffer, returning the new undo entry plus a
    /// [`ReadbackRequest`] for the async `Pending → Ready` transition.
    ///
    /// The caller is responsible for submitting the request to its
    /// [`crate::gpu::readback::ReadbackScheduler`] paired with a context
    /// that, on completion, assigns the extracted pixels into
    /// `entry.pixels`'s `RefCell` — flipping the entry to `Ready` and
    /// dropping the staging buffer.
    ///
    /// # Lifetime contract for the staging buffers
    ///
    /// Two VRAM buffers are allocated:
    /// - **`readback`** (`MAP_READ | COPY_DST`) — fed straight from scratch
    ///   in this encoder, then handed to the scheduler. When `map_async`
    ///   resolves, the readback completion handler flips
    ///   `entry.pixels` to `Ready(vec)`, dropping both buffers.
    /// - **`staging`** (`COPY_DST | COPY_SRC`) — also fed from scratch in
    ///   this encoder, kept alive in `entry.pixels` so a restore-while-
    ///   pending can feed `copy_buffer_to_texture` GPU-to-GPU.
    ///
    /// WebGPU forbids combining `MAP_READ` and `COPY_SRC` on a single
    /// buffer, which is why the split exists. The cost is one extra
    /// `copy_texture_to_buffer` per commit and one extra `byte_size` of
    /// VRAM per *pending* entry — both transient (the readback resolves
    /// in 1-3 frames in production).
    ///
    /// `wgpu::Buffer` is Arc-backed; the readback request holds its own
    /// ref to `readback`, so dropping the original handle on transition
    /// to `Ready` is safe even if the request's `map_async` callback
    /// hasn't fully unwound.
    ///
    /// `canvas_rect` must be contained in `snapshot.saved`. In debug builds
    /// this is asserted at runtime; release builds will silently read whatever
    /// scratch contents lie at the rect (likely uninitialised junk from a
    /// prior op), so don't rely on the assert being inert.
    pub fn commit_region(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        layer_id: LayerId,
        source: &CanvasFrame<'_>,
        snapshot: &Snapshot,
        canvas_rect: CanvasRect,
    ) -> (UndoRegionEntry, ReadbackRequest) {
        debug_assert!(
            snapshot.saved.contains(canvas_rect),
            "commit_region rect {:?} not contained in snapshot.saved {:?}",
            canvas_rect,
            snapshot.saved,
        );
        let layer_rect = source
            .canvas_to_layer_rect(canvas_rect)
            .expect("commit_region rect must overlap the source's canvas extent");
        let bpp = snapshot.format.block_copy_size(None).unwrap_or(1);
        let unpadded_row_bytes = layer_rect.width * bpp;
        let padded_row_bytes = padded_row(layer_rect.width, bpp);
        let byte_size = padded_row_bytes as u64 * layer_rect.height as u64;

        let (readback, staging) = allocate_pending_buffers(device, byte_size);
        let scratch = self.scratch_for(snapshot.format);
        let texel_layout = wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(padded_row_bytes),
            rows_per_image: Some(layer_rect.height),
        };
        let extent = wgpu::Extent3d {
            width: layer_rect.width,
            height: layer_rect.height,
            depth_or_array_layers: 1,
        };
        let origin = wgpu::Origin3d {
            x: layer_rect.x0(),
            y: layer_rect.y0(),
            z: 0,
        };

        // Scratch is texture-aligned to the source (see `save_region`): the
        // snapshot of pixels at layer-space `(x, y)` lives at scratch `(x, y)`.
        // Two writes — one per buffer. WebGPU doesn't let a single buffer be
        // both MAP_READ and COPY_SRC, so the readback and the in-flight
        // GPU-to-GPU restore each need their own.
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: scratch,
                mip_level: 0,
                origin,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: texel_layout,
            },
            extent,
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: scratch,
                mip_level: 0,
                origin,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: texel_layout,
            },
            extent,
        );

        let pixels = Rc::new(RefCell::new(EntryPixels::Pending {
            readback: readback.clone(),
            staging,
        }));
        let request = ReadbackRequest::from_buffer(
            readback,
            layer_rect.height,
            padded_row_bytes,
            unpadded_row_bytes,
        );

        let entry = UndoRegionEntry {
            layer_id,
            canvas_rect,
            format: snapshot.format,
            padded_row_bytes,
            unpadded_row_bytes,
            byte_size,
            pixels,
        };
        (entry, request)
    }

    /// Restore saved pixels back to the layer texture, producing a forward
    /// entry (the pre-restore state) plus its async-readback request for
    /// redo.
    ///
    /// Both branches encode their commands into the supplied encoder, so the
    /// forward capture sequences strictly before the restore. The
    /// `Ready` branch allocates a one-shot upload buffer (mapped at
    /// creation) so the restore stays in the encoder's command stream —
    /// `queue.write_texture` would re-order to the start of the next submit
    /// and stomp the forward capture.
    pub fn restore_region(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        entry: &UndoRegionEntry,
        target: &CanvasFrame<'_>,
    ) -> (UndoRegionEntry, ReadbackRequest) {
        let layer_rect = target
            .canvas_to_layer_rect(entry.canvas_rect)
            .expect("restore_region entry must overlap the target's canvas extent");
        let origin = wgpu::Origin3d {
            x: layer_rect.x0(),
            y: layer_rect.y0(),
            z: 0,
        };
        let extent = wgpu::Extent3d {
            width: layer_rect.width,
            height: layer_rect.height,
            depth_or_array_layers: 1,
        };
        let padded_row_bytes = entry.padded_row_bytes;
        let unpadded_row_bytes = entry.unpadded_row_bytes;
        let byte_size = entry.byte_size;
        let height = layer_rect.height;
        let texel_layout = wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(padded_row_bytes),
            rows_per_image: Some(height),
        };

        // 1. Allocate the forward entry's pair of staging buffers + capture
        //    the pre-restore target into BOTH. The commands must precede the
        //    restore-into-target command below so the forward entry contains
        //    the redo pixels rather than the undone pixels.
        let (forward_readback, forward_staging) = allocate_pending_buffers(device, byte_size);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: target.texture,
                mip_level: 0,
                origin,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &forward_readback,
                layout: texel_layout,
            },
            extent,
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: target.texture,
                mip_level: 0,
                origin,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &forward_staging,
                layout: texel_layout,
            },
            extent,
        );

        // 2. Restore old pixels into target — same encoder, so this runs
        //    after the forward capture.
        let pixels_borrow = entry.pixels.borrow();
        match &*pixels_borrow {
            EntryPixels::Pending { staging, .. } => {
                encoder.copy_buffer_to_texture(
                    wgpu::TexelCopyBufferInfo {
                        buffer: staging,
                        layout: texel_layout,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: target.texture,
                        mip_level: 0,
                        origin,
                        aspect: wgpu::TextureAspect::All,
                    },
                    extent,
                );
            }
            EntryPixels::Ready(vec) => {
                // Re-pad the unpadded `Vec` into a fresh mapped-at-creation
                // upload buffer. `copy_buffer_to_texture` requires
                // bytes_per_row to be COPY_BYTES_PER_ROW_ALIGNMENT-aligned;
                // the `Ready` storage drops the padding for efficiency, so
                // we add it back here. The buffer is short-lived — the
                // encoder's submit takes the Arc-backed ref until the GPU
                // is done with it; the Rust handle drops at the end of
                // this scope.
                let upload = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("undo-region-upload"),
                    size: byte_size,
                    usage: wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: true,
                });
                {
                    let mut mapped = upload.slice(..).get_mapped_range_mut();
                    let unpadded_row = unpadded_row_bytes as usize;
                    let padded_row = padded_row_bytes as usize;
                    if unpadded_row == padded_row {
                        let n = vec.len().min(mapped.len());
                        mapped[..n].copy_from_slice(&vec[..n]);
                    } else {
                        for row in 0..height as usize {
                            let src_off = row * unpadded_row;
                            let dst_off = row * padded_row;
                            mapped[dst_off..dst_off + unpadded_row]
                                .copy_from_slice(&vec[src_off..src_off + unpadded_row]);
                        }
                    }
                }
                upload.unmap();
                encoder.copy_buffer_to_texture(
                    wgpu::TexelCopyBufferInfo {
                        buffer: &upload,
                        layout: texel_layout,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: target.texture,
                        mip_level: 0,
                        origin,
                        aspect: wgpu::TextureAspect::All,
                    },
                    extent,
                );
            }
        }
        drop(pixels_borrow);

        let forward_pixels = Rc::new(RefCell::new(EntryPixels::Pending {
            readback: forward_readback.clone(),
            staging: forward_staging,
        }));
        let request = ReadbackRequest::from_buffer(
            forward_readback,
            height,
            padded_row_bytes,
            unpadded_row_bytes,
        );

        let forward = UndoRegionEntry {
            layer_id: entry.layer_id,
            canvas_rect: entry.canvas_rect,
            format: entry.format,
            padded_row_bytes,
            unpadded_row_bytes,
            byte_size,
            pixels: forward_pixels,
        };
        (forward, request)
    }

    /// Restore a region directly from the scratch texture to the target,
    /// without going through the per-entry buffer. Used by `cancel_floating()`
    /// to undo the source region clear.
    ///
    /// `canvas_rect` must be contained in `snapshot.saved` — the scratch only
    /// holds the snapshot at that footprint; reading outside it would pull in
    /// uninitialised pixels from a prior op.
    pub fn restore_from_scratch(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        snapshot: &Snapshot,
        target: &CanvasFrame<'_>,
        canvas_rect: CanvasRect,
    ) {
        debug_assert!(
            snapshot.saved.contains(canvas_rect),
            "restore_from_scratch rect {:?} not contained in snapshot.saved {:?}",
            canvas_rect,
            snapshot.saved,
        );
        let layer_rect = target
            .canvas_to_layer_rect(canvas_rect)
            .expect("restore_from_scratch rect must overlap the target's canvas extent");
        let origin = wgpu::Origin3d {
            x: layer_rect.x0(),
            y: layer_rect.y0(),
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
                texture: target.texture,
                mip_level: 0,
                origin,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: layer_rect.width,
                height: layer_rect.height,
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
}

/// Compute the row byte count padded to wgpu's copy alignment.
fn padded_row(width: u32, bytes_per_pixel: u32) -> u32 {
    let unpadded = width * bytes_per_pixel;
    unpadded.div_ceil(COPY_ROW_ALIGNMENT) * COPY_ROW_ALIGNMENT
}

/// Allocate the readback + staging buffer pair for a `Pending` entry.
/// WebGPU disallows mixing `MAP_READ` and `COPY_SRC` on one buffer, so the
/// readback (async DRAM transition) and the staging (GPU-to-GPU restore
/// fallback while the readback is in flight) live in separate VRAM
/// allocations.
fn allocate_pending_buffers(device: &wgpu::Device, byte_size: u64) -> (wgpu::Buffer, wgpu::Buffer) {
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("undo-region-readback"),
        size: byte_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("undo-region-staging"),
        size: byte_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    (readback, staging)
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
}
