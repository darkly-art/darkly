//! `.darkly` save flow: async readback of every pixel-bearing texture
//! plus the composite, gathered into a [`SaveBundle`] for JS to encode
//! and zip.
//!
//! The save snapshot is the [`Manifest`] itself: built synchronously at
//! `start_save_document` from the live document, it captures the tree,
//! filters, selection metadata, veil chain, and the `requires`
//! inventory at submit time. Pixels are pinned via refcounted
//! [`wgpu::Texture`] handles in the same synchronous prelude, so the
//! user can keep painting / mutating the doc while readbacks complete
//! over the next few frames without affecting (or being affected by)
//! the in-flight save.
//!
//! The build is registry-driven: each entity's `serialize` returns its
//! own opaque body plus a list of [`PixelBlobSpec`]s. Save never branches
//! on layer kind or filter kind: the same loop handles raster, mask,
//! selection, and any future kind that registers itself.

use std::collections::{HashMap, HashSet};

use super::{DarklyEngine, ReadbackContext};
use crate::document::filter;
use crate::document::layer_kind::{self, PixelBlobSpec};
use crate::document::Entity;
use crate::format::manifest::{
    Manifest, ManifestCanvas, ManifestEntry, ManifestFontRef, ManifestRequires, ManifestVeil,
    ManifestWriter, SaveBlob, SaveBundle, CONTAINER_VERSION, FORMAT_TAG,
};
use crate::format::registry_io::InstancePayload;
use crate::gpu::readback;
use crate::layer::LayerId;

/// Errors `start_save_document` can return synchronously.
#[derive(Debug)]
pub enum SaveError {
    /// A save is already in flight on this engine. Wait for
    /// `poll_save_result` to return `Some` before kicking off another.
    /// The UI disables the Save action for that tab during a save.
    InProgress,
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::InProgress => write!(f, "a save is already in flight"),
        }
    }
}

impl std::error::Error for SaveError {}

/// Why a save was kicked off: determines whether draining its result
/// clears the document's [`crate::document::Document::dirty`] flag.
///
/// Autosave reuses the exact same readback pipeline as a real save, so
/// without this distinction every autosave tick would mark the document
/// clean even though nothing reached the user's file, silently
/// suppressing the close-confirmation guard and the `beforeunload`
/// warning that protect genuinely-unsaved work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SavePurpose {
    /// A real save to the user's `.darkly` file. Draining clears `dirty`:
    /// the document on disk now matches the snapshot we sealed.
    File,
    /// An autosave recovery snapshot written to OPFS. Draining must NOT
    /// clear `dirty`: nothing reached the user's file, so unsaved-work
    /// tracking must still see the document as dirty.
    Snapshot,
}

/// Which kind of texture a pending [`ReadbackContext::SaveDocument`]
/// readback is sourced from. Drives how the completed pixels are
/// stitched into the [`SaveJob`]: per-blob bytes for pixel-bearing
/// entities (raster/mask/selection all flow through the same arm), a
/// `(width, height, rgba)` triple for the composite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveReadbackKind {
    /// One pixel-bearing entity's bytes: stored under `key` (the zip-relative
    /// blob path matching the entity's [`crate::format::manifest::ManifestPixelRef::pixels`]).
    BlobBytes { key: String },
    /// The whole composited canvas: stored as the bundle's
    /// `(composite_width, composite_height, composite_rgba)`.
    Composite,
}

/// In-flight save state: created by [`DarklyEngine::start_save_document`]
/// and drained by [`DarklyEngine::poll_save_result`]. The fields capture
/// everything the save needs that's *not* a pixel readback (those land
/// asynchronously into `pending_blobs` / `composite`).
pub struct SaveJob {
    /// Manifest built synchronously at submit time. Captures the
    /// document's tree / filter / veil / requires state at the moment
    /// `start_save_document` ran. Any subsequent doc mutation is
    /// invisible to this manifest.
    manifest: Manifest,
    /// Refcounted handles to every texture this save reads from. wgpu
    /// `Texture` is internally `Arc`-shared, so cloning here keeps the
    /// GPU resource alive even if the user deletes the source layer
    /// mid-save and the compositor drops its handle.
    #[allow(dead_code)] // held purely to keep textures alive across readbacks
    pinned_textures: Vec<wgpu::Texture>,
    /// Zip-relative blob path → completed bytes. `None` while the
    /// readback is in flight; populated by `complete_save_readback` as
    /// each pixel readback lands.
    pending_blobs: HashMap<String, Option<Vec<u8>>>,
    /// Composite readback result. `None` until the `Composite` arm
    /// fires.
    composite: Option<(u32, u32, Vec<u8>)>,
    /// Embedded-font blobs (`fonts/<hash>.ttf`), captured synchronously at
    /// submit time; their bytes are already in CPU RAM, so unlike pixel blobs
    /// they need no async readback. Merged verbatim into the bundle's `blobs`
    /// at drain.
    font_blobs: Vec<SaveBlob>,
    /// Why this save was started: gates whether the drain clears
    /// `doc.dirty` (see [`SavePurpose`]).
    purpose: SavePurpose,
}

impl SaveJob {
    /// True when every pixel-bearing readback (`pending_blobs` + composite)
    /// has landed.
    fn is_complete(&self) -> bool {
        self.composite.is_some() && self.pending_blobs.values().all(Option::is_some)
    }
}

impl DarklyEngine {
    /// Kick off a save. Synchronously builds the [`Manifest`], pins
    /// every source texture, and queues all readbacks. Returns
    /// immediately; the result lands on [`Self::poll_save_result`] once
    /// every readback completes (typically within a few frames).
    ///
    /// `purpose` decides whether the eventual drain clears `doc.dirty`:
    /// [`SavePurpose::File`] does (the file matches disk), while an
    /// autosave [`SavePurpose::Snapshot`] leaves it untouched.
    pub fn start_save_document(&mut self, purpose: SavePurpose) -> Result<(), SaveError> {
        if self.active_save_job.is_some() {
            return Err(SaveError::InProgress);
        }

        let (manifest, pixel_blobs, font_blobs) = build_manifest(self);

        // Force an offscreen composite so the composite texture is fresh,
        // even when this engine is headless (no surface present has run
        // since the last doc mutation).
        self.compositor
            .render_offscreen(&self.gpu.device, &self.gpu.queue, &mut self.doc);

        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();

        let mut pinned_textures = Vec::new();
        let mut pending_blobs: HashMap<String, Option<Vec<u8>>> = HashMap::new();

        // Walk the per-entity pixel-blob declarations the registry-driven
        // serializers produced and queue one readback per blob. No
        // kind discrimination: `pixel_data_for` returns the right texture
        // for rasters, masks, AND the selection.
        for spec in pixel_blobs {
            queue_pixel_readback(self, &spec, &mut pinned_textures, &mut pending_blobs);
        }

        // Composite readback. We pin the composite texture so a later
        // resize / surface change can't pull it out from under us before
        // the readback completes.
        let composite_tex = self.compositor.composited_texture().clone();
        pinned_textures.push(composite_tex.clone());
        self.gpu.encode("save-composite", |encoder| {
            let request = readback::request_readback(
                &self.gpu.device,
                encoder,
                &composite_tex,
                wgpu::TextureFormat::Rgba8Unorm,
                crate::coord::LayerRect::from_xywh(0, 0, canvas_w, canvas_h),
            );
            self.readbacks.submit(
                request,
                ReadbackContext::SaveDocument {
                    kind: SaveReadbackKind::Composite,
                    width: canvas_w,
                    height: canvas_h,
                },
            );
        });

        self.active_save_job = Some(SaveJob {
            manifest,
            pinned_textures,
            pending_blobs,
            composite: None,
            font_blobs,
            purpose,
        });

        Ok(())
    }

    /// Drain a completed save. Returns `None` while any readback is
    /// still in flight; returns `Some(SaveBundle)` once every pixel
    /// blob and the composite have landed.
    ///
    /// A [`SavePurpose::File`] drain also clears the [`Document::dirty`]
    /// flag: the bundle handoff is the moment the document's contents
    /// are no longer "unsaved." Edits queued between `start_save_document`
    /// and this drain are intentionally lost from the dirty flag's POV:
    /// the snapshot built at submit time is what's leaving the engine, so
    /// the document on disk matches the snapshot we just sealed. A
    /// [`SavePurpose::Snapshot`] (autosave) drain leaves `dirty` alone.
    ///
    /// Drives the readback scheduler itself before checking completion,
    /// so a backgrounded tab (whose `render()` loop isn't running) can
    /// still finish its snapshot when pumped via this method alone.
    pub fn poll_save_result(&mut self) -> Option<SaveBundle> {
        // Advance any in-flight GPU readbacks (non-blocking) so a save can
        // complete without a full render/present. Harmless no-op when the
        // frame loop already drained this frame.
        if self.active_save_job.is_some() {
            self.drain_readbacks();
        }
        let job = self.active_save_job.as_ref()?;
        if !job.is_complete() {
            return None;
        }
        let job = self.active_save_job.take().unwrap();
        let manifest_json = serde_json::to_vec_pretty(&job.manifest).ok()?;
        let (composite_width, composite_height, composite_rgba) = job.composite.unwrap();
        let mut blobs: Vec<SaveBlob> = job
            .pending_blobs
            .into_iter()
            .map(|(path, bytes)| SaveBlob {
                path,
                bytes: bytes.unwrap_or_default(),
            })
            .collect();
        // Embedded fonts carry raw bytes captured at submit time (no readback),
        // so they join the bundle directly alongside the pixel blobs.
        blobs.extend(job.font_blobs);
        // Stable ordering for tests + bit-stable output.
        blobs.sort_by(|a, b| a.path.cmp(&b.path));
        // Only a real file save means "disk matches"; an autosave snapshot
        // wrote to OPFS, not the user's file, so it must leave `dirty` set.
        if job.purpose == SavePurpose::File {
            self.doc.dirty = false;
        }
        Some(SaveBundle {
            manifest_json,
            composite_width,
            composite_height,
            composite_rgba,
            blobs,
        })
    }

    /// Dispatch from `handle_completed_readback`: populate the matching
    /// blob slot or the composite triple. Unknown keys are silently
    /// dropped (the save was cancelled or a stale readback completed
    /// after `poll_save_result` drained the job).
    pub(crate) fn complete_save_readback(
        &mut self,
        kind: SaveReadbackKind,
        width: u32,
        height: u32,
        mut pixels: Vec<u8>,
    ) {
        let Some(job) = self.active_save_job.as_mut() else {
            return;
        };
        match kind {
            SaveReadbackKind::Composite => {
                pixels.truncate((width * height * 4) as usize);
                job.composite = Some((width, height, pixels));
            }
            SaveReadbackKind::BlobBytes { key } => {
                if let Some(slot) = job.pending_blobs.get_mut(&key) {
                    *slot = Some(pixels);
                }
            }
        }
    }
}

/// Walk the live document via the layer-kind / filter registries and
/// produce a [`Manifest`] capturing every piece of state that survives
/// save: tree, filters, selection, veils. Also returns the
/// per-entity pixel-blob declarations the save flow uses to queue
/// readbacks. Synchronous: runs as part of `start_save_document`'s
/// prelude.
fn build_manifest(engine: &DarklyEngine) -> (Manifest, Vec<PixelBlobSpec>, Vec<SaveBlob>) {
    let doc = &engine.doc;
    let mut nodes: Vec<ManifestEntry> = Vec::new();
    let mut filters: Vec<ManifestEntry> = Vec::new();
    let mut blobs: Vec<PixelBlobSpec> = Vec::new();

    let layer_kind_registry = layer_kind::registry();
    let modifier_registry = filter::registry();

    for (_id, entity) in doc.entities.iter() {
        match entity {
            Entity::Node(node) => {
                let reg = layer_kind_registry
                    .get(node.type_id())
                    .expect("layer kind registration missing for type_id from doc");
                let serialized = (reg.serialize)(node);
                nodes.push(ManifestEntry {
                    id: node.id().to_ffi(),
                    type_id: reg.type_id.to_string(),
                    body: serialized.body,
                });
                blobs.extend(serialized.pixel_blobs);
            }
            Entity::Filter(m) => {
                let reg = modifier_registry
                    .get(m.type_id())
                    .expect("filter registration missing for type_id from doc");
                let serialized = (reg.serialize)(m);
                filters.push(ManifestEntry {
                    id: m.id.to_ffi(),
                    type_id: reg.type_id.to_string(),
                    body: serialized.body,
                });
                blobs.extend(serialized.pixel_blobs);
            }
        }
    }

    // Stable order for diffability + reliable id remap during load.
    nodes.sort_by_key(|e| e.id);
    filters.sort_by_key(|e| e.id);

    let veils = build_manifest_veils(engine);
    let requires = requires_from_doc(engine);
    let (fonts, font_blobs) = build_font_blobs(engine);

    let manifest = Manifest {
        format: FORMAT_TAG.to_string(),
        container_version: CONTAINER_VERSION,
        writer: ManifestWriter::current(),
        name: doc.name.clone(),
        canvas: ManifestCanvas {
            width: doc.width,
            height: doc.height,
            origin_x: doc.canvas_origin.x,
            origin_y: doc.canvas_origin.y,
        },
        requires,
        composite: "composite.png".to_string(),
        root: doc.root_id().to_ffi(),
        nodes,
        modifiers: filters,
        selection_id: doc.selection_id().map(LayerId::to_ffi),
        veils,
        fonts,
    };
    (manifest, blobs, font_blobs)
}

/// Collect every font a text object references that the registry has runtime
/// bytes for, and produce (a) the `ManifestFontRef` list for the manifest and
/// (b) one `fonts/<hash>.ttf` [`SaveBlob`] per unique hash. Fonts are treated
/// exactly like pixel blobs (content-addressed and deduped) except the bytes
/// are already in CPU RAM, so no GPU readback is queued; they go straight into
/// the bundle. Families with no runtime bytes (the binary-resident fallback,
/// generic `sans-serif`) resolve to `None` and are skipped naturally, so a
/// document whose text uses only the fallback embeds nothing.
///
/// Google-imported fonts have runtime bytes like any upload, so they embed by
/// value here, never by URL reference. A URL-only form would break opening the
/// `.darkly` offline, so this must stay by-value.
fn build_font_blobs(engine: &DarklyEngine) -> (Vec<ManifestFontRef>, Vec<SaveBlob>) {
    // Every distinct family referenced by a text object across the document.
    let mut families: Vec<String> = Vec::new();
    for v in engine.doc.all_vector_layers() {
        for obj in &v.objects {
            if let crate::layer::ObjectSource::Text(t) = &obj.source {
                if !families.contains(&t.font_family) {
                    families.push(t.font_family.clone());
                }
            }
        }
    }
    families.sort();

    let mut fonts: Vec<ManifestFontRef> = Vec::new();
    let mut seen_hashes: HashSet<String> = HashSet::new();
    let mut blobs: Vec<SaveBlob> = Vec::new();
    for family in families {
        let Some((hash, bytes)) = engine.fonts.font_blob(&family) else {
            continue;
        };
        let path = format!("fonts/{hash}.ttf");
        // One blob per unique hash: several families can share one file.
        if seen_hashes.insert(hash.to_string()) {
            blobs.push(SaveBlob {
                path: path.clone(),
                bytes: bytes.to_vec(),
            });
        }
        fonts.push(ManifestFontRef {
            family,
            hash: hash.to_string(),
            path,
        });
    }
    (fonts, blobs)
}

fn build_manifest_veils(engine: &DarklyEngine) -> Vec<ManifestVeil> {
    let chain = engine.compositor.veil_chain();
    let count = chain.count();
    let mut veils = Vec::with_capacity(count);
    // Chain order on the wire matches apply order (bottom of stack to
    // top). `chain.info(i)` is in chain order; no need to reverse.
    for i in 0..count {
        let Some((type_id, visible)) = chain.info(i) else {
            continue;
        };
        let params = chain.param_values(i).unwrap_or_default();
        veils.push(ManifestVeil {
            instance: InstancePayload::new(type_id.to_string(), params),
            visible,
        });
    }
    veils
}

/// Walk the live document + veil chain and collect every modular
/// `type_id` in use. Registry-driven: no hand-maintained list to keep
/// in sync when a new module is added. The load path diffs this against
/// the binary's registries before parsing the body.
pub fn requires_from_doc(engine: &DarklyEngine) -> ManifestRequires {
    let mut layer_kinds = HashSet::new();
    let mut blend_modes = HashSet::new();
    let mut filter_kinds = HashSet::new();
    let mut veil_types = HashSet::new();

    for entity in engine.doc.entities.values() {
        match entity {
            Entity::Node(node) => {
                layer_kinds.insert(node.type_id().to_string());
                blend_modes.insert(node.blend().blend_mode.type_id.to_string());
            }
            Entity::Filter(m) => {
                filter_kinds.insert(m.type_id().to_string());
            }
        }
    }

    let chain = engine.compositor.veil_chain();
    for i in 0..chain.count() {
        if let Some(id) = chain.type_id(i) {
            veil_types.insert(id.to_string());
        }
    }

    let mut layer_kind: Vec<String> = layer_kinds.into_iter().collect();
    let mut blend_mode: Vec<String> = blend_modes.into_iter().collect();
    let mut filter: Vec<String> = filter_kinds.into_iter().collect();
    let mut veil: Vec<String> = veil_types.into_iter().collect();
    layer_kind.sort();
    blend_mode.sort();
    filter.sort();
    veil.sort();

    ManifestRequires {
        veil,
        blend_mode,
        layer_kind,
        modifier: filter,
    }
}

/// Queue one pixel readback and reserve its blob slot. Pins the source
/// texture so the readback survives concurrent mutation. Returns
/// without queueing (silent) when the entity has no GPU texture today,
/// typically a freshly-added layer that hasn't been touched yet, which
/// has nothing to save.
fn queue_pixel_readback(
    engine: &mut DarklyEngine,
    spec: &PixelBlobSpec,
    pinned: &mut Vec<wgpu::Texture>,
    blobs: &mut HashMap<String, Option<Vec<u8>>>,
) {
    let Some(data) = engine.compositor.pixel_data_for(spec.source_node_id) else {
        return;
    };

    let texture = data.texture.clone();
    let format = data.format;
    let rect = data.rect();
    let key = spec.blob_key.clone();

    pinned.push(texture.clone());
    blobs.insert(key.clone(), None);

    engine.gpu.encode("save-pixel-readback", |encoder| {
        let request =
            readback::request_readback(&engine.gpu.device, encoder, &texture, format, rect);
        engine.readbacks.submit(
            request,
            ReadbackContext::SaveDocument {
                kind: SaveReadbackKind::BlobBytes { key },
                width: rect.width,
                height: rect.height,
            },
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::context::GpuContext;
    use crate::gpu::test_utils::test_device;

    fn headless_engine(w: u32, h: u32) -> DarklyEngine {
        let (device, queue) = test_device();
        let gpu = GpuContext::new_headless(device, queue);
        DarklyEngine::new(gpu, w, h)
    }

    /// A second `start_save_document` while one is in flight must error
    /// rather than spawning a parallel save. The UI disables the Save
    /// action for that tab while a save is active; if the error ever
    /// reaches it, it's a logic bug worth surfacing loudly.
    #[test]
    fn save_in_progress_returns_err() {
        let mut engine = headless_engine(32, 32);
        let _layer = engine.add_raster_layer(None);
        engine
            .start_save_document(SavePurpose::File)
            .expect("first save kicks off");
        let err = engine
            .start_save_document(SavePurpose::File)
            .expect_err("second save must refuse");
        assert!(matches!(err, SaveError::InProgress));
    }

    /// `requires_from_doc` walks the live document + veil chain and
    /// collects every modular `type_id` actually in use. Adding the
    /// `noise` veil must show up under `requires.veil`; the existing
    /// raster + group layer kinds and `normal` blend mode must show up
    /// in their respective buckets.
    #[test]
    fn requires_inventory_collects_used_modules() {
        let mut engine = headless_engine(32, 32);
        let _layer = engine.add_raster_layer(None);

        // The veil chain's GPU textures size with the viewport; tests
        // run headless (no surface), so seed the size manually before
        // adding a veil; otherwise `ensure_textures` no-ops on a 0×0
        // viewport and `add_veil` panics on the `views.unwrap()`.
        engine
            .compositor
            .veil_chain_mut()
            .resize(&engine.gpu.device, &engine.gpu.queue, 32, 32);

        let defaults: Vec<crate::gpu::params::ParamValue> = engine
            .veil_param_defs("grain")
            .iter()
            .map(crate::gpu::params::ParamDef::default_value)
            .collect();
        engine.add_veil_layer("grain", &defaults);

        let requires = requires_from_doc(&engine);
        assert!(
            requires.veil.iter().any(|v| v == "grain"),
            "requires.veil should list grain (got {:?})",
            requires.veil
        );
        assert!(
            requires.layer_kind.iter().any(|k| k == "raster"),
            "requires.layer_kind should list raster (got {:?})",
            requires.layer_kind
        );
        // Root group is always present.
        assert!(
            requires.layer_kind.iter().any(|k| k == "group"),
            "requires.layer_kind should list group (got {:?})",
            requires.layer_kind
        );
        assert!(
            requires.blend_mode.iter().any(|m| m == "normal"),
            "requires.blend_mode should list normal (got {:?})",
            requires.blend_mode
        );
    }

    /// Successful save clears the sticky [`crate::document::Document::dirty`]
    /// bit. This is the "file matches disk now" handoff: anything the user
    /// did between `start_save_document` and the drain is intentionally not
    /// re-dirty: the snapshot the bundle holds *is* the file we just wrote.
    #[test]
    fn dirty_flag_cleared_by_save() {
        let mut engine = headless_engine(32, 32);
        // add_raster_layer pushes to undo, which flips dirty.
        let _layer = engine.add_raster_layer(None);
        assert!(engine.is_dirty(), "add_raster_layer must flip dirty");

        engine
            .start_save_document(SavePurpose::File)
            .expect("save kicks off");
        // Drive readbacks to completion.
        let mut bundle = None;
        for _ in 0..16 {
            engine.test_flush_readbacks();
            engine.render(0.0);
            if let Some(b) = engine.poll_save_result() {
                bundle = Some(b);
                break;
            }
        }
        bundle.expect("save should complete within 16 frames");
        assert!(
            !engine.is_dirty(),
            "successful save must clear dirty: bundle handoff matches disk"
        );
    }

    /// Regression: an autosave [`SavePurpose::Snapshot`] writes to OPFS,
    /// not the user's file, so draining it must leave `dirty` set.
    /// Otherwise the close-confirmation guard + `beforeunload` warning
    /// would silently treat genuinely-unsaved work as saved.
    #[test]
    fn snapshot_save_does_not_clear_dirty() {
        let mut engine = headless_engine(32, 32);
        let _layer = engine.add_raster_layer(None);
        assert!(engine.is_dirty(), "add_raster_layer must flip dirty");

        engine
            .start_save_document(SavePurpose::Snapshot)
            .expect("snapshot save kicks off");
        let mut bundle = None;
        for _ in 0..16 {
            engine.test_flush_readbacks();
            engine.render(0.0);
            if let Some(b) = engine.poll_save_result() {
                bundle = Some(b);
                break;
            }
        }
        bundle.expect("snapshot should complete within 16 frames");
        assert!(
            engine.is_dirty(),
            "autosave snapshot must NOT clear dirty: nothing reached the user's file"
        );
    }

    /// A snapshot save must be drivable to completion via
    /// `poll_save_result` alone (no `render()`/present) because a
    /// backgrounded tab's render loop isn't running. `poll_save_result`
    /// drains the readback scheduler itself.
    #[test]
    fn snapshot_completes_without_render() {
        let mut engine = headless_engine(32, 32);
        let _layer = engine.add_raster_layer(None);

        engine
            .start_save_document(SavePurpose::Snapshot)
            .expect("snapshot save kicks off");
        let mut bundle = None;
        for _ in 0..32 {
            // Native stand-in for the browser event loop: fire the GPU map
            // callbacks but DON'T poll/dispatch the scheduler here; leave
            // that to poll_save_result. NO engine.render(): the whole point.
            engine.test_wait_gpu();
            if let Some(b) = engine.poll_save_result() {
                bundle = Some(b);
                break;
            }
        }
        let bundle = bundle.expect("snapshot must complete via poll alone, without render()");
        assert!(
            bundle.composite_width == 32 && bundle.composite_height == 32,
            "composite dimensions should match the canvas"
        );
    }

    /// The save snapshot must survive concurrent edits: the manifest
    /// is built at submit time, GPU textures are refcount-pinned, and
    /// readbacks see GPU command-buffer state at submit time. Adding a
    /// layer between start_save and poll_save_result must *not* end up
    /// in the saved manifest.
    #[test]
    fn save_concurrent_edit_does_not_corrupt() {
        let mut engine = headless_engine(32, 32);
        let _baseline = engine.add_raster_layer(None);
        engine
            .start_save_document(SavePurpose::File)
            .expect("save kicks off");

        // Mutate the document mid-save.
        let _added_mid_save = engine.add_raster_layer(None);

        // Drive readbacks to completion.
        let mut bundle = None;
        for _ in 0..16 {
            engine.test_flush_readbacks();
            engine.render(0.0);
            if let Some(b) = engine.poll_save_result() {
                bundle = Some(b);
                break;
            }
        }
        let bundle = bundle.expect("save should complete within 16 frames");
        let manifest: Manifest = serde_json::from_slice(&bundle.manifest_json).unwrap();

        let raster_count = manifest
            .nodes
            .iter()
            .filter(|e| e.type_id == crate::document::layer_kinds::raster::TYPE_ID)
            .count();
        assert_eq!(
            raster_count, 1,
            "snapshot must reflect doc state at start_save_document time, \
             not the post-mutation state: found {raster_count} rasters"
        );
    }
}
