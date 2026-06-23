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

use std::collections::HashSet;
use std::rc::Rc;

use super::host::cell::EngineCell;
use super::DarklyEngine;
use crate::document::layer_kind::{self, PixelBlobSpec};
use crate::document::modifier;
use crate::document::Entity;
use crate::format::manifest::{
    Manifest, ManifestCanvas, ManifestEntry, ManifestRequires, ManifestVeil, ManifestWriter,
    SaveBlob, SaveBundle, CONTAINER_VERSION, FORMAT_TAG,
};
use crate::format::registry_io::InstancePayload;
use crate::gpu::readback::{self, ReadbackFuture};
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

/// One pixel-bearing readback the save task awaits: the zip-relative blob path
/// the bytes land under, plus the awaitable future. Produced for every
/// `PixelBlobSpec` that resolved to a live GPU texture.
struct SaveBlobRead {
    /// Zip-relative blob path matching the entity's
    /// [`crate::format::manifest::ManifestPixelRef::pixels`].
    key: String,
    future: ReadbackFuture,
}

/// Everything the [`run_save`] task needs after its synchronous prelude burst:
/// the manifest snapshot, the awaitable readbacks (per-blob + composite), the
/// pinned source textures, and the dirty-flag policy.
struct SavePrelude {
    /// Manifest built synchronously in the prelude. Captures the document's
    /// tree / modifier / veil / requires state at submit time; any subsequent
    /// doc mutation is invisible to it, so the user can keep editing while the
    /// readbacks complete.
    manifest: Manifest,
    /// Why this save was started — gates whether finishing clears `doc.dirty`.
    purpose: SavePurpose,
    /// One awaitable readback per pixel-bearing entity (raster / mask /
    /// selection all flow through the same path).
    blob_reads: Vec<SaveBlobRead>,
    /// The composite readback: `(width, height, future)`.
    composite: (u32, u32, ReadbackFuture),
    /// Refcounted handles to every source texture this save reads from. wgpu
    /// `Texture` is internally `Arc`-shared, so holding these here keeps the
    /// GPU resource alive even if the user deletes the source layer mid-save
    /// and the compositor drops its handle. Held until the task drops.
    #[allow(dead_code)]
    pinned_textures: Vec<wgpu::Texture>,
}

/// Run a `.darkly` save as a linear task: await each readback kicked by the
/// synchronous [`kick_save_readbacks`](DarklyEngine::kick_save_readbacks)
/// prelude, assemble the [`SaveBundle`], and resolve the originating request.
/// The prelude already built the manifest snapshot and pinned the textures, so
/// edits made while these readbacks land don't affect what's saved.
async fn run_save(cell: Rc<EngineCell>, prelude: SavePrelude, request: u64) {
    let SavePrelude {
        manifest,
        purpose,
        blob_reads,
        composite,
        pinned_textures: _pinned,
    } = prelude;

    // Await every pixel blob, then the composite. A `None` means the handle was
    // disposed mid-save (slot cancelled) — abandon it; teardown already
    // rejected the request. Clear the in-flight guard either way.
    let mut blobs: Vec<SaveBlob> = Vec::with_capacity(blob_reads.len());
    for SaveBlobRead { key, future } in blob_reads {
        let Some(bytes) = future.await else {
            cell.with_async(|e| e.save_in_flight = false).await;
            return;
        };
        blobs.push(SaveBlob { path: key, bytes });
    }

    let (composite_width, composite_height, composite_future) = composite;
    let Some(mut composite_rgba) = composite_future.await else {
        cell.with_async(|e| e.save_in_flight = false).await;
        return;
    };
    composite_rgba.truncate((composite_width * composite_height * 4) as usize);

    // Stable blob ordering for tests + bit-stable output; serialize the
    // manifest off the engine. The final burst only touches engine state.
    blobs.sort_by(|a, b| a.path.cmp(&b.path));
    let manifest_json = serde_json::to_vec_pretty(&manifest);

    cell.with_async(move |e| {
        e.save_in_flight = false;
        let manifest_json = match manifest_json {
            Ok(json) => json,
            Err(_) => {
                e.reject_request(
                    request,
                    crate::engine::protocol::ProtocolError::engine(
                        "save manifest serialize failed",
                    ),
                );
                return;
            }
        };
        // Only a real file save means "disk matches" — an autosave snapshot
        // wrote to OPFS, not the user's file, so it must leave `dirty` set.
        // Edits between submit and here are intentionally invisible: the
        // snapshot the bundle holds is what left the engine.
        if purpose == SavePurpose::File {
            e.doc.dirty = false;
        }
        let bundle = SaveBundle {
            manifest_json,
            composite_width,
            composite_height,
            composite_rgba,
            blobs,
        };
        e.resolve_request(request, pack_save_bundle(bundle));
    })
    .await;
}

impl DarklyEngine {
    /// Kick off a save. Spawns a [`run_save`] task; the originating request
    /// resolves with the packed `SaveBundle` once every readback completes
    /// (typically within a few frames). Errors with [`SaveError::InProgress`]
    /// if a save is already in flight on this engine.
    ///
    /// `purpose` decides whether finishing clears `doc.dirty`:
    /// [`SavePurpose::File`] does (the file matches disk), while an autosave
    /// [`SavePurpose::Snapshot`] leaves it untouched.
    pub fn start_save_document(&mut self, purpose: SavePurpose) -> Result<(), SaveError> {
        if self.save_in_flight {
            return Err(SaveError::InProgress);
        }
        self.save_in_flight = true;
        let request = self.current_request();
        // Build the snapshot + kick the readbacks synchronously *now*, so the
        // save reflects document state at submit time — edits between here and
        // the task draining the readbacks are invisible to it.
        let prelude = self.kick_save_readbacks(purpose);
        let cell = self.self_cell();
        self.spawn(Some(request), run_save(cell, prelude, request));
        Ok(())
    }

    /// Synchronous save prelude: build the manifest, force a fresh offscreen
    /// composite, pin every source texture, and kick the awaitable readbacks
    /// (one per pixel blob + the composite). Returns the [`SavePrelude`] the
    /// task awaits. Runs in one [`EngineCell::with_async`] burst.
    fn kick_save_readbacks(&mut self, purpose: SavePurpose) -> SavePrelude {
        let (manifest, pixel_blobs) = build_manifest(self);

        // Force an offscreen composite so the composite texture is fresh, even
        // when this engine is headless (no surface present has run since the
        // last doc mutation).
        self.compositor
            .render_offscreen(&self.gpu.device, &self.gpu.queue, &mut self.doc);

        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();

        let mut pinned_textures = Vec::new();

        // Walk the per-entity pixel-blob declarations the registry-driven
        // serializers produced and kick one readback per blob. No kind
        // discrimination: `pixel_data_for` returns the right texture for
        // rasters, masks, AND the selection.
        let blob_reads = pixel_blobs
            .iter()
            .filter_map(|spec| kick_pixel_readback(self, spec, &mut pinned_textures))
            .collect();

        // Composite readback. Pin the composite texture so a later resize /
        // surface change can't pull it out before the readback executes.
        let composite_tex = self.compositor.composited_texture().clone();
        pinned_textures.push(composite_tex.clone());
        let req = self.gpu.encode_ret("save-composite", |encoder| {
            readback::request_readback(
                &self.gpu.device,
                encoder,
                &composite_tex,
                wgpu::TextureFormat::Rgba8Unorm,
                crate::coord::LayerRect::from_xywh(0, 0, canvas_w, canvas_h),
            )
        });
        let composite_future = self.await_readback(req);

        SavePrelude {
            manifest,
            purpose,
            blob_reads,
            composite: (canvas_w, canvas_h, composite_future),
            pinned_textures,
        }
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

/// Kick one awaitable pixel readback, pinning the source texture so it survives
/// concurrent mutation until the readback executes. Returns `None` (silent)
/// when the entity has no GPU texture today — typically a freshly-added layer
/// that hasn't been touched yet, which has nothing to save.
fn kick_pixel_readback(
    engine: &mut DarklyEngine,
    spec: &PixelBlobSpec,
    pinned: &mut Vec<wgpu::Texture>,
) -> Option<SaveBlobRead> {
    let data = engine.compositor.pixel_data_for(spec.source_node_id)?;

    let texture = data.texture.clone();
    let format = data.format;
    let width = data.width;
    let height = data.height;
    let key = spec.blob_key.clone();

    pinned.push(texture.clone());

    let req = engine.gpu.encode_ret("save-pixel-readback", |encoder| {
        readback::request_readback(
            &engine.gpu.device,
            encoder,
            &texture,
            format,
            crate::coord::LayerRect::from_xywh(0, 0, width, height),
        )
    });
    let future = engine.await_readback(req);
    Some(SaveBlobRead { key, future })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::context::GpuContext;
    use crate::gpu::test_utils::test_device;

    use crate::engine::host::EngineHost;

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
        // Under a host: the first save spawns its task and sets the in-flight
        // guard; the second refuses while that task is undriven.
        let host = EngineHost::adopt(engine);
        host.with(|e| e.start_save_document(SavePurpose::File))
            .expect("first save kicks off");
        let err = host
            .with(|e| e.start_save_document(SavePurpose::File))
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

        // Drive the save task to completion through the host; resolving the
        // request clears dirty as part of finishing.
        let host = EngineHost::adopt(engine);
        host.with(|e| e.start_save_document(SavePurpose::File))
            .expect("save kicks off");
        host.pump_until_idle();
        host.with(|e| e.test_take_completed(0))
            .expect("save should complete");
        assert!(
            !host.with(|e| e.is_dirty()),
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

        let host = EngineHost::adopt(engine);
        host.with(|e| e.start_save_document(SavePurpose::Snapshot))
            .expect("snapshot save kicks off");
        host.pump_until_idle();
        host.with(|e| e.test_take_completed(0))
            .expect("snapshot should complete");
        assert!(
            host.with(|e| e.is_dirty()),
            "autosave snapshot must NOT clear dirty — nothing reached the user's file"
        );
    }

    /// A snapshot save completes through the host's task/readback drive — no
    /// composite/present — because a backgrounded tab's rAF loop isn't running.
    /// The `run_save` task awaits its readbacks on the executor; harvesting them
    /// resolves the save request with the packed bundle.
    #[test]
    fn snapshot_completes_without_render() {
        let mut engine = headless_engine(32, 32);
        let _layer = engine.add_raster_layer(None);

        let host = EngineHost::adopt(engine);
        host.with(|e| e.start_save_document(SavePurpose::Snapshot))
            .expect("snapshot save kicks off");
        host.pump_until_idle();
        let resp = host
            .with(|e| e.test_take_completed(0))
            .expect("snapshot must complete via the host's task drive");
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

        let host = EngineHost::adopt(engine);
        host.with(|e| e.start_save_document(SavePurpose::File))
            .expect("save kicks off");

        // Mutate the document mid-save (after the prelude burst pinned the
        // snapshot, before the readbacks finish).
        host.with(|e| e.add_raster_layer(None));

        // Drive readbacks to completion and recover the manifest from the
        // packed bundle (manifest ++ composite ++ blobs in the bytes channel).
        host.pump_until_idle();
        let resp = host
            .with(|e| e.test_take_completed(0))
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
