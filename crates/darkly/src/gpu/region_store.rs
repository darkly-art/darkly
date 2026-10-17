//! GPU-side undo snapshot storage.
//!
//! Manages the reusable scratch textures that hold pre-operation snapshots and
//! produces per-action [`UndoRegionEntry`] values that own their own pixel
//! data: no shared ring buffer.
//!
//! # Snapshots own their pixels
//!
//! [`save_region`](RegionScratch::save_region) copies a region into a scratch
//! texture and returns a [`Snapshot`] that holds that texture (through an
//! `Rc`) and the canvas position of its texel `(0, 0)`. Everything that later
//! reads the saved pixels (a commit, the stroke-end diff, a cancel) reads them
//! from the snapshot, never from whatever the scratch slot holds by then.
//! `RegionScratch` reuses a slot's texture only when no snapshot still holds
//! it, and allocates a fresh one otherwise, so a pending commit can never see
//! its pixels overwritten by a later save. Steady state is one texture per
//! format; overlapping operations briefly hold one more each.
//!
//! # Lifetime model
//!
//! Each undo entry owns its pixels via [`EntryPixels`]:
//!
//! - **`Pending { staging }`** - VRAM-resident. The `wgpu::Buffer` that backs
//!   the async readback. Holds until either (a) the readback completes and the
//!   entry transitions to `Ready`, dropping the buffer, or (b) a restore
//!   happens first, in which case the buffer feeds `copy_buffer_to_texture`
//!   directly (GPU-to-GPU, no readback wait).
//! - **`Ready(Vec<u8>)`** - DRAM-resident, in the same layout as the buffer.
//!   The steady state for most actions, since most commits' readbacks finish
//!   before any restore is requested.
//!
//! When the action drops (max_steps overflow, byte-cap overflow, redo cleared,
//! teardown), the staging buffer or `Vec` drops with it. No shared storage, no
//! eviction at the storage layer, no aliasing.
//!
//! # Coordinate frame
//!
//! All public methods take [`CanvasRect`]s (or [`RegionRuns`] of them) and a
//! [`CanvasFrame`] for the source/target texture. Translation to texture-local
//! coordinates happens internally, immediately before each `copy_*` call. The
//! *metadata* (`Snapshot.saved`, `UndoRegionEntry.runs`) is in canvas coords
//! so it remains valid across layer growth (the Storage Frame Rule), and a
//! snapshot translates through its own origin rather than the layer's live
//! frame, so a commit stays correct even after the layer has grown.
//!
//! An entry's buffer holds its runs back to back, each as rows padded to
//! `COPY_BYTES_PER_ROW_ALIGNMENT` ([`RegionRuns::layout`]).

use crate::coord::{CanvasPoint, CanvasRect};
use crate::gpu::atlas::CanvasFrame;
use crate::gpu::readback::ReadbackRequest;
use crate::gpu::region_runs::{bytes_per_pixel, RegionRuns};
use crate::layer::LayerId;
use std::cell::RefCell;
use std::rc::Rc;

/// Pre-operation pixels returned by [`RegionScratch::save_region`]: the saved
/// rect (in canvas coords), the format, and the texture holding the pixels
/// with the canvas position of its texel `(0, 0)`. Required to commit or
/// restore; you can't commit without saving first.
///
/// Holding a snapshot keeps its texture out of reuse, so drop it once its
/// commit is encoded.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub saved: CanvasRect,
    pub format: wgpu::TextureFormat,
    origin: CanvasPoint,
    texture: Rc<wgpu::Texture>,
}

impl Snapshot {
    /// The saved pixels as a frame: canvas position `origin` is texel `(0, 0)`.
    pub fn frame(&self) -> CanvasFrame<'_> {
        CanvasFrame {
            texture: &self.texture,
            canvas_extent: CanvasRect::from_xywh(
                self.origin.x,
                self.origin.y,
                self.texture.width(),
                self.texture.height(),
            ),
        }
    }
}

/// Per-entry pixel storage, hybrid VRAM/DRAM.
///
/// Single-threaded: the engine drives commits, restores, and readback
/// completion from the same thread, so `Rc<RefCell<…>>` rather than
/// `Arc<Mutex<…>>`. WASM is single-threaded by construction; native tests
/// run with `--test-threads=1`.
///
/// Future improvement: every tab's `Ready` bytes share one 32-bit WASM heap,
/// and the [`crate::undo::UndoStack`] cap is per tab. An inactive tab could
/// hibernate its `Ready` entries to disk through the frontend's storage layer
/// (an on-disk state alongside these two) and reload them when reactivated.
pub enum EntryPixels {
    /// Async readback in flight. Holds the committed pixels in two
    /// sibling buffers: WebGPU disallows combining `MAP_READ` and
    /// `COPY_SRC` on a single buffer, so the readback and restore paths
    /// need separate VRAM. The commit fills `staging` and copies it whole
    /// into `readback`, so their contents are byte-identical.
    Pending {
        /// MAP_READ | COPY_DST, drives the async readback that flips the
        /// entry to `Ready`.
        readback: wgpu::Buffer,
        /// COPY_DST | COPY_SRC, source for the GPU-to-GPU restore path
        /// when a restore arrives before the readback completes. Both
        /// buffers drop together when the entry transitions to `Ready`.
        staging: wgpu::Buffer,
    },
    /// Readback completed, pixels live on the host heap (WASM linear memory
    /// in production, native DRAM in tests), byte for byte the buffer's
    /// layout ([`RegionRuns::layout`]), so a restore uploads it unchanged.
    Ready(Vec<u8>),
}

/// Metadata + owned pixels for a single undo region. Not `Clone`: each
/// action owns exactly one entry, and the `Rc<RefCell<…>>` would duplicate
/// the pixel-ownership relationship if cloned.
pub struct UndoRegionEntry {
    pub layer_id: LayerId,
    /// The region, in canvas-space pixel coords. Stable across layer growth.
    pub runs: RegionRuns,
    pub format: wgpu::TextureFormat,
    /// Shared cell so the readback completion handler can flip
    /// `Pending → Ready` after the action has been pushed onto the undo
    /// stack. Cloned once into the readback request's context at commit
    /// time; the handler holds that clone, the entry holds the other.
    pub pixels: Rc<RefCell<EntryPixels>>,
}

impl UndoRegionEntry {
    /// VRAM-equivalent byte cost of this entry (padded rows of every run),
    /// charged against the [`crate::undo::UndoStack`] memory cap.
    pub fn byte_size(&self) -> u64 {
        self.runs.byte_size(bytes_per_pixel(self.format))
    }
}

/// Reusable scratch textures for pre-operation snapshots, one slot per
/// format (RGBA8 layers, R8 masks), plus the producer of per-entry storage.
///
/// A slot's texture is reused for the next save only when no [`Snapshot`]
/// still holds it; otherwise the save allocates a fresh one into the slot and
/// the held texture lives on with its snapshot. Each undo entry owns its own
/// pixel data, there is no shared ring buffer. Storage lifetime equals action
/// lifetime, so eviction happens at the policy layer
/// ([`crate::undo::UndoStack`]) rather than down here.
///
/// Future improvement: the slots are canvas-sized, allocated at engine
/// construction, and held for the engine's lifetime, so every open tab pays
/// for them on the shared device even while idle (80 MB at 4096²). Once no
/// snapshot is outstanding (no pending undo commit, fill readback, or async
/// selection save), nothing reads their contents again before the next
/// `save_region` overwrites them, so an inactive tab could drop them and
/// reallocate on its next edit, with nothing to preserve.
pub struct RegionScratch {
    rgba: Rc<wgpu::Texture>,
    r8: Rc<wgpu::Texture>,
}

impl RegionScratch {
    pub fn new(device: &wgpu::Device, canvas_width: u32, canvas_height: u32) -> Self {
        RegionScratch {
            rgba: Rc::new(create_scratch(
                device,
                canvas_width,
                canvas_height,
                wgpu::TextureFormat::Rgba8Unorm,
            )),
            r8: Rc::new(create_scratch(
                device,
                canvas_width,
                canvas_height,
                wgpu::TextureFormat::R8Unorm,
            )),
        }
    }

    /// Copy a canvas-space rect from a layer/mask texture into a scratch
    /// texture and return the [`Snapshot`] that holds it.
    ///
    /// The scratch is texture-aligned to `source`: the snapshot lands at the
    /// same texture-local `(x, y)` it came from, so the snapshot's origin is
    /// `source`'s canvas origin and a diff can compare the two texel for
    /// texel. The slot's texture is reused when no snapshot holds it and it
    /// is large enough; otherwise a fresh one replaces it in the slot.
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
        let slot = self.slot_mut(format);
        if Rc::strong_count(slot) > 1
            || slot.width() < layer_rect.x1()
            || slot.height() < layer_rect.y1()
        {
            let w = slot.width().max(layer_rect.x1());
            let h = slot.height().max(layer_rect.y1());
            *slot = Rc::new(create_scratch(device, w, h, format));
        }
        let snapshot = Snapshot {
            saved: canvas_rect,
            format,
            origin: source.canvas_extent.origin,
            texture: Rc::clone(slot),
        };
        source.copy_rect_to(encoder, &snapshot.frame(), canvas_rect);
        snapshot
    }

    /// Re-anchor `snapshot` to a layer that grew from the snapshot's extent
    /// to `new_extent` mid-operation, upholding this post-condition:
    ///
    /// > After the call, the snapshot holds valid pre-operation state over
    /// > the **entire new extent**: its saved pixels at their canvas
    /// > position, and the format default (transparent for RGBA, **white**
    /// > for an R8 mask, see [`scratch_default_clear`]) everywhere else, and
    /// > its texture is texel-aligned to the grown layer.
    ///
    /// The newly-grown pixels didn't exist before the grow, so the default IS
    /// their pre-operation value, and `saved` widens to `new_extent` so a diff
    /// that spills into the grown area is still covered at commit time.
    ///
    /// Every growth allocates a texture sized to `new_extent`, cleared to the
    /// default by a render-pass `LoadOp::Clear` (`clear_texture` zeroes,
    /// wrong for R8's white default), then the saved rect is copied in. When
    /// the slot still pointed at the snapshot's old texture, the slot takes
    /// the new one so the next save reuses it.
    pub fn rebase(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        snapshot: &mut Snapshot,
        new_extent: CanvasRect,
    ) {
        debug_assert!(
            new_extent.contains(snapshot.saved),
            "rebase: new extent {new_extent:?} must contain the saved rect {:?}",
            snapshot.saved,
        );
        let format = snapshot.format;
        let texture = Rc::new(create_scratch(
            device,
            new_extent.width,
            new_extent.height,
            format,
        ));
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scratch-rebase-clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(scratch_default_clear(format)),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        let rebased = Snapshot {
            saved: new_extent,
            format,
            origin: new_extent.origin,
            texture,
        };
        snapshot
            .frame()
            .copy_rect_to(encoder, &rebased.frame(), snapshot.saved);
        let slot = self.slot_mut(format);
        if Rc::ptr_eq(slot, &snapshot.texture) {
            *slot = Rc::clone(&rebased.texture);
        }
        *snapshot = rebased;
    }

    /// Copy `runs` of the snapshot's saved pixels into a freshly-allocated
    /// per-entry buffer, returning the new undo entry plus a
    /// [`ReadbackRequest`] for the async `Pending → Ready` transition.
    ///
    /// The caller is responsible for submitting the request to its
    /// [`crate::gpu::readback::ReadbackScheduler`] paired with a context
    /// that, on completion, assigns the extracted bytes into
    /// `entry.pixels`'s `RefCell`, flipping the entry to `Ready` and
    /// dropping the buffers.
    ///
    /// Every run must be contained in `snapshot.saved`. In debug builds this
    /// is asserted at runtime; release builds will silently read whatever the
    /// snapshot's texture holds there, so don't rely on the assert being
    /// inert.
    pub fn commit_region(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        layer_id: LayerId,
        snapshot: &Snapshot,
        runs: impl Into<RegionRuns>,
    ) -> (UndoRegionEntry, ReadbackRequest) {
        let runs = runs.into();
        debug_assert!(
            runs.rects().iter().all(|r| snapshot.saved.contains(*r)),
            "commit_region runs {:?} not contained in snapshot.saved {:?}",
            runs.rects(),
            snapshot.saved,
        );
        snapshot_runs(
            encoder,
            device,
            &snapshot.frame(),
            snapshot.format,
            layer_id,
            runs,
        )
    }

    /// Restore `entry`'s saved pixels into `target`, producing the forward
    /// entry (the pre-restore state) + its async-readback request for the
    /// inverse op. The constant-extent combinator of
    /// [`capture_region`](Self::capture_region) +
    /// [`upload_region`](Self::upload_region): capture the entry's runs, then
    /// upload, in one encoder (capture sequences before the overwrite). Use the
    /// two halves directly when the target texture is realloc'd between them
    /// (image rescale's extent change).
    pub fn restore_region(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        entry: &UndoRegionEntry,
        target: &CanvasFrame<'_>,
    ) -> (UndoRegionEntry, ReadbackRequest) {
        let (forward, request) = self.capture_region(
            encoder,
            device,
            entry.layer_id,
            entry.format,
            target,
            entry.runs.clone(),
        );
        self.upload_region(encoder, device, entry, target);
        (forward, request)
    }

    /// Capture `capture` of `target`'s **current** pixels into a fresh undo
    /// entry (the forward / opposite-direction state) plus its async readback
    /// request.
    ///
    /// This is the first half of an undo restore: snapshot what's there now so
    /// the inverse operation can put it back. Pairs with
    /// [`upload_region`](Self::upload_region); a caller that needs to realloc
    /// the target texture (image rescale changes a node's extent) can do so
    /// *between* the two calls: capture reads the old extent, upload writes
    /// the new. For constant-extent restores pass the entry's own runs, which
    /// is what [`restore_region`](Self::restore_region) does.
    ///
    /// The capture commands must be encoded (and, across an extent change,
    /// submitted) before the upload overwrites / the realloc replaces the
    /// target, same encoder preserves order for the constant-extent case.
    pub fn capture_region(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        layer_id: LayerId,
        format: wgpu::TextureFormat,
        target: &CanvasFrame<'_>,
        capture: impl Into<RegionRuns>,
    ) -> (UndoRegionEntry, ReadbackRequest) {
        snapshot_runs(encoder, device, target, format, layer_id, capture.into())
    }

    /// Upload `entry`'s saved pixels back into `target` at each of its runs,
    /// the second half of an undo restore. The `Ready` branch allocates a
    /// one-shot mapped-at-creation upload buffer so the copy stays in the
    /// encoder's command stream (`queue.write_texture` would re-order to the
    /// start of the next submit and stomp a same-encoder forward capture).
    ///
    /// `target` must already be sized so every run maps fully into it (the
    /// caller realloc's the node texture for an extent-changing restore
    /// before calling this).
    pub fn upload_region(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        entry: &UndoRegionEntry,
        target: &CanvasFrame<'_>,
    ) {
        let pixels = entry.pixels.borrow();
        // The `Ready` bytes are already in the buffer's layout; upload them
        // into a short-lived buffer. The encoder's submit holds the
        // Arc-backed ref until the GPU is done with it.
        let upload;
        let buffer = match &*pixels {
            EntryPixels::Pending { staging, .. } => staging,
            EntryPixels::Ready(bytes) => {
                upload = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("undo-region-upload"),
                    size: bytes.len() as u64,
                    usage: wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: true,
                });
                upload
                    .slice(..)
                    .get_mapped_range_mut()
                    .copy_from_slice(bytes);
                upload.unmap();
                &upload
            }
        };
        for run in entry.runs.layout(bytes_per_pixel(entry.format)) {
            let local = target
                .canvas_to_layer_rect(run.rect)
                .expect("upload_region run must overlap the target's canvas extent");
            debug_assert_eq!(
                (local.width, local.height),
                (run.rect.width, run.rect.height),
                "upload_region run {:?} must lie inside the target {:?}",
                run.rect,
                target.canvas_extent,
            );
            encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo {
                    buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: run.offset,
                        bytes_per_row: Some(run.bytes_per_row),
                        rows_per_image: Some(local.height),
                    },
                },
                wgpu::TexelCopyTextureInfo {
                    texture: target.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: local.x0(),
                        y: local.y0(),
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: local.width,
                    height: local.height,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    /// Restore a region directly from a snapshot's texture to the target,
    /// without going through a per-entry buffer.
    ///
    /// `canvas_rect` must be contained in `snapshot.saved`: the snapshot only
    /// holds saved pixels at that footprint.
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
        snapshot.frame().copy_rect_to(encoder, target, canvas_rect);
    }

    fn slot_mut(&mut self, format: wgpu::TextureFormat) -> &mut Rc<wgpu::Texture> {
        match format {
            wgpu::TextureFormat::R8Unorm => &mut self.r8,
            _ => &mut self.rgba,
        }
    }
}

/// Copy `runs` of `src` into a fresh pair of per-entry buffers, packed per
/// [`RegionRuns::layout`]: one `copy_texture_to_buffer` per run into
/// `staging`, then one `copy_buffer_to_buffer` of the whole thing into
/// `readback`. The one body behind both a commit (from a snapshot) and a
/// capture (from a live layer).
///
/// WebGPU forbids combining `MAP_READ` and `COPY_SRC` on a single buffer,
/// which is why there are two. The cost is one extra `byte_size` of VRAM per
/// *pending* entry, transient (the readback resolves in 1-3 frames in
/// production). `wgpu::Buffer` is Arc-backed; the readback request holds its
/// own ref to `readback`, so dropping the original handle on transition to
/// `Ready` is safe even if the request's `map_async` callback hasn't fully
/// unwound.
fn snapshot_runs(
    encoder: &mut wgpu::CommandEncoder,
    device: &wgpu::Device,
    src: &CanvasFrame<'_>,
    format: wgpu::TextureFormat,
    layer_id: LayerId,
    runs: RegionRuns,
) -> (UndoRegionEntry, ReadbackRequest) {
    let byte_size = runs.byte_size(bytes_per_pixel(format));
    let (readback, staging) = allocate_pending_buffers(device, byte_size);
    for run in runs.layout(bytes_per_pixel(format)) {
        let local = src
            .canvas_to_layer_rect(run.rect)
            .expect("snapshot run must overlap the source's canvas extent");
        debug_assert_eq!(
            (local.width, local.height),
            (run.rect.width, run.rect.height),
            "snapshot run {:?} must lie inside the source {:?}",
            run.rect,
            src.canvas_extent,
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: src.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: local.x0(),
                    y: local.y0(),
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: run.offset,
                    bytes_per_row: Some(run.bytes_per_row),
                    rows_per_image: Some(local.height),
                },
            },
            wgpu::Extent3d {
                width: local.width,
                height: local.height,
                depth_or_array_layers: 1,
            },
        );
    }
    encoder.copy_buffer_to_buffer(&staging, 0, &readback, 0, byte_size);

    let pixels = Rc::new(RefCell::new(EntryPixels::Pending {
        readback: readback.clone(),
        staging,
    }));
    let entry = UndoRegionEntry {
        layer_id,
        runs,
        format,
        pixels,
    };
    (entry, ReadbackRequest::from_buffer(readback))
}

fn create_scratch(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    // Encode the generation's dimensions into the label so that if two
    // scratch generations collide in a future wgpu validation error, the
    // message names the right one (the AAR lesson: shared labels lie).
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(&format!("scratch-{format:?}-{width}x{height}")),
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
            | wgpu::TextureUsages::TEXTURE_BINDING
            // RENDER_ATTACHMENT so a rebase can clear the grown region to the
            // format's default before the saved content is copied in.
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    })
}

/// The pre-operation value a freshly-grown region of a scratch texture holds,
/// matching the live texture's default fill: R8 masks default to white
/// (255 = reveal), everything else to transparent/zero. Getting this wrong
/// for an R8 mask corrupts undo: the grown region would restore to black.
fn scratch_default_clear(format: wgpu::TextureFormat) -> wgpu::Color {
    match format {
        wgpu::TextureFormat::R8Unorm => wgpu::Color::WHITE,
        _ => wgpu::Color::TRANSPARENT,
    }
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
