//! `.darkly` save flow — async readback of every pixel-bearing texture
//! plus the composite, gathered into a [`SaveBundle`] for JS to encode
//! and zip.
//!
//! The save snapshot is the [`Manifest`] itself: built synchronously at
//! `start_save_document` from the live document, it captures the tree,
//! modifiers, selection metadata, veil chain, and the `requires`
//! inventory at submit time. Pixels are pinned via refcounted
//! [`wgpu::Texture`] handles in the same synchronous prelude, so the
//! user can keep painting / mutating the doc while readbacks complete
//! over the next few frames without affecting (or being affected by)
//! the in-flight save.
//!
//! The build is registry-driven: each entity's `serialize` returns its
//! own opaque body plus a list of [`PixelBlobSpec`]s. Save never branches
//! on layer kind or modifier kind — the same loop handles raster, mask,
//! selection, and any future kind that registers itself.

use std::collections::{HashMap, HashSet};

use super::{DarklyEngine, ReadbackContext};
use crate::document::layer_kind::{self, PixelBlobSpec};
use crate::document::modifier;
use crate::document::Entity;
use crate::format::manifest::{
    Manifest, ManifestCanvas, ManifestEntry, ManifestRequires, ManifestVeil, ManifestWriter,
    SaveBlob, SaveBundle, CONTAINER_VERSION, FORMAT_TAG,
};
use crate::format::registry_io::InstancePayload;
use crate::gpu::readback;
use crate::layer::LayerId;

/// Errors `start_save_document` can return synchronously.
#[derive(Debug)]
pub enum SaveError {
    /// A save is already in flight on this engine. Wait for the in-flight
    /// `start_save_document` request to resolve before kicking off another.
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

/// Why a save was kicked off — determines whether draining its result
/// clears the document's [`crate::document::Document::dirty`] flag.
///
/// Autosave reuses the exact same readback pipeline as a real save, so
/// without this distinction every autosave tick would mark the document
/// clean even though nothing reached the user's file — silently
/// suppressing the close-confirmation guard and the `beforeunload`
/// warning that protect genuinely-unsaved work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SavePurpose {
    /// A real save to the user's `.darkly` file. Draining clears `dirty`
    /// — the document on disk now matches the snapshot we sealed.
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
    /// One pixel-bearing entity's bytes — stored under `key` (the zip-relative
    /// blob path matching the entity's [`crate::format::manifest::ManifestPixelRef::pixels`]).
    BlobBytes { key: String },
    /// The whole composited canvas — stored as the bundle's
    /// `(composite_width, composite_height, composite_rgba)`.
    Composite,
}

/// In-flight save state — created by [`DarklyEngine::start_save_document`]
/// and finished by `complete_save_readback` once every readback lands. The
/// fields capture everything the save needs that's *not* a pixel readback
/// (those land asynchronously into `pending_blobs` / `composite`).
pub struct SaveJob {
    /// Manifest built synchronously at submit time. Captures the
    /// document's tree / modifier / veil / requires state at the moment
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
    /// Why this save was started — gates whether finishing clears
    /// `doc.dirty` (see [`SavePurpose`]).
    purpose: SavePurpose,
    /// Originating `start_save_document` request id. When the last readback
    /// lands, the assembled [`SaveBundle`] resolves this request's promise.
    request: u64,
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
    /// every source texture, and queues all readbacks. Defers: the
    /// originating request resolves with the packed `SaveBundle` once
    /// every readback completes (typically within a few frames).
    ///
    /// `purpose` decides whether the eventual drain clears `doc.dirty`:
    /// [`SavePurpose::File`] does (the file matches disk), while an
    /// autosave [`SavePurpose::Snapshot`] leaves it untouched.
    pub fn start_save_document(&mut self, purpose: SavePurpose) -> Result<(), SaveError> {
        if self.active_save_job.is_some() {
            return Err(SaveError::InProgress);
        }
        let request = self.current_request();

        let (manifest, pixel_blobs) = build_manifest(self);

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
            purpose,
            request,
        });

        Ok(())
    }

    /// Dispatch from `handle_completed_readback` — populate the matching
    /// blob slot or the composite triple, then finish the save once every
    /// readback has landed. Unknown keys are silently dropped (the save was
    /// cancelled or a stale readback completed after the job finished).
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
        if job.is_complete() {
            self.finish_save_job();
        }
    }

    /// Assemble the [`SaveBundle`] from the completed job and resolve the
    /// originating `start_save_document` request with the packed binary
    /// response. A [`SavePurpose::File`] save also clears [`Document::dirty`]
    /// — the bundle handoff is the moment the document's contents are no
    /// longer "unsaved." Edits queued between `start_save_document` and here
    /// are intentionally invisible to the dirty flag: the snapshot built at
    /// submit time is what's leaving the engine. A [`SavePurpose::Snapshot`]
    /// (autosave) save leaves `dirty` alone.
    fn finish_save_job(&mut self) {
        let Some(job) = self.active_save_job.take() else {
            return;
        };
        let request = job.request;
        let Ok(manifest_json) = serde_json::to_vec_pretty(&job.manifest) else {
            self.reject_request(
                request,
                crate::engine::protocol::ProtocolError::engine("save manifest serialize failed"),
            );
            return;
        };
        let (composite_width, composite_height, composite_rgba) = job.composite.unwrap();
        let mut blobs: Vec<SaveBlob> = job
            .pending_blobs
            .into_iter()
            .map(|(path, bytes)| SaveBlob {
                path,
                bytes: bytes.unwrap_or_default(),
            })
            .collect();
        // Stable ordering for tests + bit-stable output.
        blobs.sort_by(|a, b| a.path.cmp(&b.path));
        // Only a real file save means "disk matches" — an autosave snapshot
        // wrote to OPFS, not the user's file, so it must leave `dirty` set.
        if job.purpose == SavePurpose::File {
            self.doc.dirty = false;
        }
        let bundle = SaveBundle {
            manifest_json,
            composite_width,
            composite_height,
            composite_rgba,
            blobs,
        };
        self.resolve_request(request, pack_save_bundle(bundle));
    }
}

/// Pack a [`SaveBundle`] into the binary protocol response the JS save flow
/// unpacks: every byte buffer (manifest ++ composite ++ each blob) concatenated
/// into the single `bytes` side-channel, with the lengths carried in the JSON
/// value so the JS edge can slice them back out in order.
fn pack_save_bundle(bundle: SaveBundle) -> crate::engine::protocol::Response {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&bundle.manifest_json);
    bytes.extend_from_slice(&bundle.composite_rgba);
    let blobs: Vec<serde_json::Value> = bundle
        .blobs
        .iter()
        .map(|b| {
            bytes.extend_from_slice(&b.bytes);
            serde_json::json!({ "path": b.path, "len": b.bytes.len() })
        })
        .collect();
    let value = serde_json::json!({
        "manifestLen": bundle.manifest_json.len(),
        "compositeWidth": bundle.composite_width,
        "compositeHeight": bundle.composite_height,
        "compositeLen": bundle.composite_rgba.len(),
        "blobs": blobs,
    });
    crate::engine::protocol::Response::binary(value, bytes)
}

/// Walk the live document via the layer-kind / modifier registries and
/// produce a [`Manifest`] capturing every piece of state that survives
/// save: tree, modifiers, selection, veils. Also returns the
/// per-entity pixel-blob declarations the save flow uses to queue
/// readbacks. Synchronous — runs as part of `start_save_document`'s
/// prelude.
fn build_manifest(engine: &DarklyEngine) -> (Manifest, Vec<PixelBlobSpec>) {
    let doc = &engine.doc;
    let mut nodes: Vec<ManifestEntry> = Vec::new();
    let mut modifiers: Vec<ManifestEntry> = Vec::new();
    let mut blobs: Vec<PixelBlobSpec> = Vec::new();

    let layer_kind_registry = layer_kind::registry();
    let modifier_registry = modifier::registry();

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
            Entity::Modifier(m) => {
                let reg = modifier_registry
                    .get(m.type_id())
                    .expect("modifier registration missing for type_id from doc");
                let serialized = (reg.serialize)(m);
                modifiers.push(ManifestEntry {
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
    modifiers.sort_by_key(|e| e.id);

    let veils = build_manifest_veils(engine);
    let requires = requires_from_doc(engine);

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
        modifiers,
        selection_id: doc.selection_id().map(LayerId::to_ffi),
        veils,
    };
    (manifest, blobs)
}

fn build_manifest_veils(engine: &DarklyEngine) -> Vec<ManifestVeil> {
    let chain = engine.compositor.veil_chain();
    let count = chain.count();
    let mut veils = Vec::with_capacity(count);
    // Chain order on the wire matches apply order (bottom of stack to
    // top). `chain.info(i)` is in chain order — no need to reverse.
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
/// `type_id` in use. Registry-driven — no hand-maintained list to keep
/// in sync when a new module is added. The load path diffs this against
/// the binary's registries before parsing the body.
pub fn requires_from_doc(engine: &DarklyEngine) -> ManifestRequires {
    let mut layer_kinds = HashSet::new();
    let mut blend_modes = HashSet::new();
    let mut modifier_kinds = HashSet::new();
    let mut veil_types = HashSet::new();

    for entity in engine.doc.entities.values() {
        match entity {
            Entity::Node(node) => {
                layer_kinds.insert(node.type_id().to_string());
                blend_modes.insert(node.blend().blend_mode.type_id.to_string());
            }
            Entity::Modifier(m) => {
                modifier_kinds.insert(m.type_id().to_string());
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
    let mut modifier: Vec<String> = modifier_kinds.into_iter().collect();
    let mut veil: Vec<String> = veil_types.into_iter().collect();
    layer_kind.sort();
    blend_mode.sort();
    modifier.sort();
    veil.sort();

    ManifestRequires {
        veil,
        blend_mode,
        layer_kind,
        modifier,
    }
}

/// Queue one pixel readback and reserve its blob slot. Pins the source
/// texture so the readback survives concurrent mutation. Returns
/// without queueing (silent) when the entity has no GPU texture today
/// — typically a freshly-added layer that hasn't been touched yet, which
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
    let width = data.width;
    let height = data.height;
    let key = spec.blob_key.clone();

    pinned.push(texture.clone());
    blobs.insert(key.clone(), None);

    engine.gpu.encode("save-pixel-readback", |encoder| {
        let request = readback::request_readback(
            &engine.gpu.device,
            encoder,
            &texture,
            format,
            crate::coord::LayerRect::from_xywh(0, 0, width, height),
        );
        engine.readbacks.submit(
            request,
            ReadbackContext::SaveDocument {
                kind: SaveReadbackKind::BlobBytes { key },
                width,
                height,
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
        // adding a veil — otherwise `ensure_textures` no-ops on a 0×0
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
        engine.add_veil("grain", &defaults);

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
    /// bit. This is the "file matches disk now" handoff — anything the user
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
        // Drive readbacks to completion; the save request resolves with the
        // packed bundle and clears dirty as part of finishing the job.
        engine
            .test_drive_completed(0)
            .expect("save should complete");
        assert!(
            !engine.is_dirty(),
            "successful save must clear dirty — bundle handoff matches disk"
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
        engine
            .test_drive_completed(0)
            .expect("snapshot should complete");
        assert!(
            engine.is_dirty(),
            "autosave snapshot must NOT clear dirty — nothing reached the user's file"
        );
    }

    /// A snapshot save must be drivable to completion by flushing readbacks
    /// alone — no `render()`/present — because a backgrounded tab's render
    /// loop isn't running. Completing the readbacks resolves the save request
    /// with the packed bundle.
    #[test]
    fn snapshot_completes_without_render() {
        let mut engine = headless_engine(32, 32);
        let _layer = engine.add_raster_layer(None);

        engine
            .start_save_document(SavePurpose::Snapshot)
            .expect("snapshot save kicks off");
        // `test_drive_completed` drives the readback scheduler (no render()).
        let resp = engine
            .test_drive_completed(0)
            .expect("snapshot must complete via flush alone, without render()");
        assert_eq!(
            resp.value["compositeWidth"].as_u64(),
            Some(32),
            "composite width should match the canvas"
        );
        assert_eq!(
            resp.value["compositeHeight"].as_u64(),
            Some(32),
            "composite height should match the canvas"
        );
    }

    /// The save snapshot must survive concurrent edits — the manifest
    /// is built at submit time, GPU textures are refcount-pinned, and
    /// readbacks see GPU command-buffer state at submit time. Adding a
    /// layer between start_save and the save's completion must *not* end up
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

        // Drive readbacks to completion and recover the manifest from the
        // packed bundle (manifest ++ composite ++ blobs in the bytes channel).
        let resp = engine
            .test_drive_completed(0)
            .expect("save should complete");
        let manifest_len = resp.value["manifestLen"].as_u64().unwrap() as usize;
        let bytes = resp.bytes.expect("save resolves with packed bytes");
        let manifest: Manifest = serde_json::from_slice(&bytes[..manifest_len]).unwrap();

        let raster_count = manifest
            .nodes
            .iter()
            .filter(|e| e.type_id == crate::document::layer_kinds::raster::TYPE_ID)
            .count();
        assert_eq!(
            raster_count, 1,
            "snapshot must reflect doc state at start_save_document time, \
             not the post-mutation state — found {raster_count} rasters"
        );
    }
}
