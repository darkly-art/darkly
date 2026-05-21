//! `.darkly` load flow — atomic install with up-front refusal checks.
//!
//! The contract is **all-or-nothing**: either the file is fully
//! representable in this build and loads completely, or it isn't, and
//! the load refuses with a precise diagnostic while leaving the
//! engine's existing document, compositor, undo stack, and session
//! state untouched.
//!
//! Refusal checks happen *before* any engine mutation:
//!
//! 1. **Zip extraction** — malformed archive → [`LoadError::Zip`].
//! 2. **Manifest existence + container/requires presence** — missing
//!    `manifest.json`, missing `container_version`, or missing
//!    `requires` block → [`LoadError::CorruptManifest`].
//! 3. **Container version** — newer than the binary understands →
//!    [`LoadError::ContainerTooNew`].
//! 4. **`requires` inventory** — diffed against the four registries
//!    (veils, blend modes, layer kinds, modifiers); any miss →
//!    [`LoadError::UnsupportedFeatures`] naming every missing
//!    `"<registry>/<type_id>"`.
//! 5. **Full schema parse + staging-doc construction** — any
//!    per-variant `type_id` the registry doesn't know, despite passing
//!    the inventory diff, means the manifest's `requires` block lied
//!    and the file is corrupt → [`LoadError::CorruptManifest`].
//!
//! Only after every check passes does [`install_staging`] swap the
//! document, replace the compositor, upload pixels, and restore veils.
//! That phase has no fallible operations — by construction the install
//! either completes or panics (a logic bug to fix at the source).
//!
//! The staging-doc construction is registry-driven: each entity's
//! `deserialize` reconstructs its own kind from the opaque manifest
//! body, and a second pass calls each kind's `remap_ids` to rewrite
//! cross-references onto the fresh slotmap. The central code never
//! branches on which kind it got.

use std::collections::HashMap;

use super::DarklyEngine;
use crate::document::layer_kind::{self, IdMap};
use crate::document::modifier;
use crate::document::{Document, Entity};
use crate::format::error::LoadError;
use crate::format::manifest::{Manifest, ManifestPixelRef, ManifestRequires};
use crate::format::unzip::unzip_entries;
use crate::gpu::blend_mode;
use crate::gpu::compositor::Compositor;
use crate::layer::{LayerId, LayerNode};

/// Public marker re-export so the WASM bridge can name the engine-side
/// "loaded document" capability without importing the load module directly.
/// Empty today; reserved for future expansion (e.g. progress callbacks).
pub struct LoadDocument;

impl DarklyEngine {
    /// Load a `.darkly` zip into this engine. **All-or-nothing**: every
    /// refusal path is checked before any engine state changes, so a
    /// returned `Err(_)` guarantees the engine is byte-for-byte the
    /// same as before the call.
    pub fn open_document(&mut self, bytes: &[u8]) -> Result<(), LoadError> {
        // ---- Refusal checks (all happen before any engine mutation) ----

        let entries = unzip_entries(bytes)?;

        let manifest_bytes =
            entries
                .get("manifest.json")
                .ok_or_else(|| LoadError::CorruptManifest {
                    reason: "archive missing manifest.json".to_string(),
                })?;

        // First parse to an untyped JSON value so we can check the
        // container version + `requires` presence BEFORE trying the
        // full typed schema — a too-new container could have schema
        // shapes the typed parse can't make sense of, and we want the
        // precise refusal diagnostic rather than a confused JSON error.
        let raw: serde_json::Value = serde_json::from_slice(manifest_bytes)?;
        pre_check_container_version(&raw)?;
        pre_check_requires_present(&raw)?;

        let manifest: Manifest = serde_json::from_value(raw).map_err(LoadError::from)?;

        // Inventory diff: refuse with precise diagnostics naming every
        // missing `<registry>/<type_id>` so the UI toast can format the
        // exact "needs <thing>, please update Darkly" message.
        pre_check_requires(self, &manifest.requires)?;

        // Staging doc — built off to the side. Per-variant safety net
        // for the cases where `requires` declared what the body uses
        // but the binary's registry is still missing it (would be a
        // bug at the pre-check level), or where `requires` lies short
        // and we hit something undeclared.
        let (staging, id_map) = build_staging_document(&manifest)?;

        // ---- Atomic install. No fallible operations from here on. ----
        install_staging(self, &manifest, staging, id_map, &entries);
        Ok(())
    }
}

// ----------------------------------------------------------------------------
// Refusal pre-checks
// ----------------------------------------------------------------------------

/// Refuse files whose `container_version` is newer than this binary
/// supports, or whose value is missing/malformed entirely (we control
/// the writer; absence means the file is corrupt, not "older format" —
/// there's no older format).
fn pre_check_container_version(raw: &serde_json::Value) -> Result<(), LoadError> {
    let found = raw
        .get("container_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| LoadError::CorruptManifest {
            reason: "manifest missing or malformed container_version".to_string(),
        })?;
    if found > crate::format::manifest::CONTAINER_VERSION as u64 {
        return Err(LoadError::ContainerTooNew {
            found: found as u32,
            supported: crate::format::manifest::CONTAINER_VERSION,
        });
    }
    Ok(())
}

/// Refuse files whose manifest is missing the `requires` inventory.
/// We always write one; its absence is malformed, not "older format."
/// The actual diff against the binary's registries happens after the
/// full typed parse in [`pre_check_requires`].
fn pre_check_requires_present(raw: &serde_json::Value) -> Result<(), LoadError> {
    if raw.get("requires").is_none() {
        return Err(LoadError::CorruptManifest {
            reason: "manifest missing required `requires` inventory".to_string(),
        });
    }
    Ok(())
}

/// Cross-reference the manifest's `requires` inventory against the
/// binary's four registries (veils, blend modes, layer kinds,
/// modifiers). Any miss collects a `"<registry>/<type_id>"` entry and
/// the load is refused with [`LoadError::UnsupportedFeatures`].
fn pre_check_requires(engine: &DarklyEngine, requires: &ManifestRequires) -> Result<(), LoadError> {
    let mut missing = Vec::new();

    let blend_registry = blend_mode::registry();
    for id in &requires.blend_mode {
        if blend_registry.get(id).is_none() {
            missing.push(format!("blend_mode/{id}"));
        }
    }

    let layer_kind_registry = layer_kind::registry();
    for id in &requires.layer_kind {
        if layer_kind_registry.get(id).is_none() {
            missing.push(format!("layer_kind/{id}"));
        }
    }

    let modifier_registry = modifier::registry();
    for id in &requires.modifier {
        if modifier_registry.get(id).is_none() {
            missing.push(format!("modifier/{id}"));
        }
    }

    let veil_registry = engine.compositor.veil_chain().registry();
    for id in &requires.veil {
        if !veil_registry.has(id) {
            missing.push(format!("veil/{id}"));
        }
    }

    if !missing.is_empty() {
        // Stable ordering for predictable diagnostics + tests.
        missing.sort();
        return Err(LoadError::UnsupportedFeatures { missing });
    }
    Ok(())
}

// ----------------------------------------------------------------------------
// Staging doc construction (allocation only — no engine mutation)
// ----------------------------------------------------------------------------

/// Build a fresh [`Document`] from a [`Manifest`], producing an
/// `old_id → new_id` map so the loader can rewrite cross-references
/// (children, modifiers, host) into the new slotmap.
///
/// Three passes:
///
/// 1. **Allocate every entity.** Each entity's manifest body is handed
///    to its registered `deserialize` to produce a fresh [`LayerNode`] /
///    [`crate::document::Modifier`] with cross-refs still pointing at
///    manifest-old ids.
/// 2. **Remap cross-refs.** Each entity's registered `remap_ids` rewrites
///    its cross-references using the `id_map`. Each kind owns the
///    knowledge of which fields hold ids — a future kind that adds a
///    private id field is forced to implement this hook by signature.
/// 3. **Rebuild parent map.** Walk every group's `children` and every
///    host's `modifiers`; the body's child/modifier list is the single
///    source of truth, and the `parent` SecondaryMap is derived from it.
fn build_staging_document(manifest: &Manifest) -> Result<(Document, IdMap), LoadError> {
    let mut doc = Document::new(manifest.canvas.width, manifest.canvas.height);
    doc.name = manifest.name.clone();

    let mut id_map: IdMap = HashMap::with_capacity(manifest.nodes.len() + manifest.modifiers.len());

    // The manifest's `root` maps to the new doc's auto-allocated root —
    // we don't re-create the root entity, we reuse the one Document::new
    // built. Every other node + modifier is allocated fresh and
    // recorded.
    let new_root = doc.root_id();
    id_map.insert(manifest.root, new_root);

    let layer_kind_registry = layer_kind::registry();
    let modifier_registry = modifier::registry();

    // Pass 1a: allocate every non-root node entity.
    for entry in &manifest.nodes {
        if entry.id == manifest.root {
            // Patch the auto-allocated root group with the body's
            // fields (children/modifiers lists still carry manifest-old
            // ids; pass 2 / 3 will rewrite them).
            let reg = layer_kind_registry.get(&entry.type_id).ok_or_else(|| {
                LoadError::CorruptManifest {
                    reason: format!(
                        "manifest root {} declares layer_kind/{} but registry is missing it \
                         — `requires` block lies",
                        entry.id, entry.type_id
                    ),
                }
            })?;
            let new_root_node = (reg.deserialize)(&entry.body, new_root)?;
            if let Some(Entity::Node(slot)) = doc.entities.get_mut(new_root) {
                *slot = new_root_node;
            }
            continue;
        }
        let reg =
            layer_kind_registry
                .get(&entry.type_id)
                .ok_or_else(|| LoadError::CorruptManifest {
                    reason: format!(
                        "node {} declares layer_kind/{} but registry is missing it \
                     — `requires` block lies",
                        entry.id, entry.type_id
                    ),
                })?;
        // `insert_with_key` runs the constructor with the freshly-allocated
        // key. `deserialize` is fallible — if it errors, we want to bubble
        // up rather than leave a half-formed slotmap entry, so we use a
        // two-step allocate-then-fill pattern via a Result-bearing temporary.
        let mut new_id_opt: Option<LayerId> = None;
        let mut deserialize_err: Option<LoadError> = None;
        let new_id = doc.entities.insert_with_key(|key| {
            new_id_opt = Some(key);
            match (reg.deserialize)(&entry.body, key) {
                Ok(node) => Entity::Node(node),
                Err(e) => {
                    deserialize_err = Some(e);
                    // Insert a placeholder; the caller will pull this
                    // entity right back out once we propagate the error.
                    Entity::Node(LayerNode::Group(crate::layer::LayerGroup::new(
                        key,
                        String::new(),
                    )))
                }
            }
        });
        if let Some(e) = deserialize_err {
            doc.entities.remove(new_id);
            return Err(e);
        }
        id_map.insert(
            entry.id,
            new_id_opt.expect("insert_with_key always invokes closure"),
        );
    }

    // Pass 1b: allocate modifiers.
    for entry in &manifest.modifiers {
        let reg =
            modifier_registry
                .get(&entry.type_id)
                .ok_or_else(|| LoadError::CorruptManifest {
                    reason: format!(
                        "modifier {} declares modifier/{} but registry is missing it \
                     — `requires` block lies",
                        entry.id, entry.type_id
                    ),
                })?;
        let mut new_id_opt: Option<LayerId> = None;
        let mut deserialize_err: Option<LoadError> = None;
        let new_id = doc.entities.insert_with_key(|key| {
            new_id_opt = Some(key);
            match (reg.deserialize)(&entry.body, key) {
                Ok(m) => Entity::Modifier(m),
                Err(e) => {
                    deserialize_err = Some(e);
                    Entity::Modifier(crate::document::Modifier {
                        id: key,
                        common: crate::layer::NodeCommon::new(String::new()),
                        kind: crate::document::ModifierKind::mask_with_bounds(
                            crate::coord::CanvasRect::from_xywh(0, 0, 0, 0),
                        ),
                    })
                }
            }
        });
        if let Some(e) = deserialize_err {
            doc.entities.remove(new_id);
            return Err(e);
        }
        id_map.insert(
            entry.id,
            new_id_opt.expect("insert_with_key always invokes closure"),
        );
    }

    // Pass 2: rewrite cross-refs. Each kind owns this via remap_ids,
    // so no central knowledge of which fields hold ids is required.
    let entity_ids: Vec<LayerId> = doc.entities.keys().collect();
    for eid in entity_ids {
        // Each closure runs on a separate borrow of the entity slot so
        // we can call the registry dispatch (which is `&'static`)
        // alongside.
        let type_id = match doc.entities.get(eid) {
            Some(Entity::Node(n)) => Some(("node", n.type_id())),
            Some(Entity::Modifier(m)) => Some(("modifier", m.type_id())),
            None => None,
        };
        match type_id {
            Some(("node", tid)) => {
                let reg = layer_kind_registry
                    .get(tid)
                    .expect("layer kind type_id passed pass 1");
                if let Some(Entity::Node(n)) = doc.entities.get_mut(eid) {
                    (reg.remap_ids)(n, &id_map);
                }
            }
            Some(("modifier", tid)) => {
                let reg = modifier_registry
                    .get(tid)
                    .expect("modifier type_id passed pass 1");
                if let Some(Entity::Modifier(m)) = doc.entities.get_mut(eid) {
                    (reg.remap_ids)(m, &id_map);
                }
            }
            _ => {}
        }
    }

    // Pass 3: rebuild the parent SecondaryMap from groups' children and
    // hosts' modifiers. The body's lists are the single source of truth.
    rebuild_parent_map(&mut doc);

    // Selection sentinel — point `Document::selection` at the
    // freshly-allocated id (if the manifest declared one).
    if let Some(old_sel_id) = manifest.selection_id {
        if let Some(new_id) = id_map.get(&old_sel_id) {
            doc.selection = Some(*new_id);
        }
    }

    Ok((doc, id_map))
}

/// Walk every group node's `children` and every host node's `modifiers`,
/// populating [`Document::parent`]. Called after pass 2 has rewritten
/// every cross-ref to its fresh slotmap id.
fn rebuild_parent_map(doc: &mut Document) {
    doc.parent.clear();
    // Snapshot all node-entity ids so we can mutate `doc.parent`
    // without holding a borrow on `doc.entities`.
    let node_ids: Vec<LayerId> = doc
        .entities
        .iter()
        .filter_map(|(id, e)| matches!(e, Entity::Node(_)).then_some(id))
        .collect();
    for nid in node_ids {
        let (children, modifiers) = match doc.entities.get(nid) {
            Some(Entity::Node(LayerNode::Group(g))) => (g.children.clone(), g.modifiers.clone()),
            Some(Entity::Node(n)) => (Vec::new(), n.modifiers().to_vec()),
            _ => continue,
        };
        for c in children {
            doc.parent.insert(c, nid);
        }
        for m in modifiers {
            doc.parent.insert(m, nid);
        }
    }
}

// ----------------------------------------------------------------------------
// Atomic install — no fallible operations from here on.
// ----------------------------------------------------------------------------

/// Swap the staging doc into the engine and rebuild every derived
/// piece (compositor, GPU textures, veil chain) keyed off the new
/// slotmap. Called once every refusal check has passed.
fn install_staging(
    engine: &mut DarklyEngine,
    manifest: &Manifest,
    staging: Document,
    id_map: IdMap,
    entries: &HashMap<String, Vec<u8>>,
) {
    // Cancel any in-flight readbacks that referenced the previous
    // document — their context ids point at the about-to-be-dropped
    // slotmap entries. Done before the doc swap so the cancel sees the
    // old context types correctly.
    engine.readbacks.cancel(|_| true);

    // The compositor caches a lot keyed off the old document's
    // slotmap. Building a fresh one is the simplest correct route —
    // every old texture / bind group / passthrough state is dropped.
    engine.doc = staging;
    engine.compositor = Compositor::new(
        &engine.gpu.device,
        &engine.gpu.queue,
        engine.gpu.surface_format(),
        engine.doc.width,
        engine.doc.height,
        engine.doc.root_id(),
    );

    upload_loaded_pixels(engine, manifest, &id_map, entries);

    // The veil chain sizes to the surface in production (via
    // `resize()`); on a freshly-loaded compositor it's still 0×0,
    // and `add_veil`'s `ensure_textures` would no-op silently and
    // then unwrap on `views`. Seed to canvas dimensions so the
    // restore path always sees a sized viewport — the next real
    // resize cascades to the right surface size automatically.
    engine.compositor.veil_chain_mut().resize(
        &engine.gpu.device,
        &engine.gpu.queue,
        engine.doc.width,
        engine.doc.height,
    );
    restore_veils(engine, manifest);
    ensure_selection_state(engine);
    engine.sync_compositor_layers();
    // Restore void persistent pixels (camera void's last frame, etc.)
    // AFTER `sync_compositor_layers` — that's where the void's GPU cache
    // is first allocated (with a placeholder texture), and our restore
    // method resizes + writes bytes into that cache.
    upload_loaded_void_pixels(engine, entries);

    // Reset session-level state so the loaded doc starts clean.
    engine.active_stroke_layer = None;
    engine.isolated_node = None;
    engine.floating = None;
    engine.selection_overlay.clear();
    engine.tool_overlay.clear();
    engine.undo_stack = crate::undo::UndoStack::new(50);
    engine.thumbnail_cache = super::ThumbnailCache::new();
    engine.thumbnail_version = engine.thumbnail_version.wrapping_add(1);

    engine.compositor.mark_dirty();
    engine.compositor.mark_needs_present();
}

/// Allocate GPU textures for every loaded entity and upload the
/// matching pixel bytes from the zip. Walks each entity, reads the
/// `pixels` ref out of the body, and routes through the appropriate
/// allocator. Infallible — pixel-buffer size mismatches log and
/// continue (the surrounding load is already past every refusal check
/// and we'd rather show a half-loaded layer than abort with a fresh
/// compositor sitting around).
fn upload_loaded_pixels(
    engine: &mut DarklyEngine,
    manifest: &Manifest,
    id_map: &IdMap,
    entries: &HashMap<String, Vec<u8>>,
) {
    use crate::format::manifest::texture_format_from_str;

    fn extract_pixel_ref(body: &serde_json::Value) -> Option<ManifestPixelRef> {
        body.get("pixels")
            .and_then(|v| serde_json::from_value::<ManifestPixelRef>(v.clone()).ok())
    }

    for entry in &manifest.nodes {
        // Voids carry their pixels in a separate aux texture (the void's
        // own EffectCache), not in `node_textures`. They get restored in
        // a dedicated pass after `sync_compositor_layers`; skip them
        // here so we don't accidentally allocate a raster cache for a
        // void's id.
        if entry.type_id == crate::document::layer_kinds::void::TYPE_ID {
            continue;
        }
        let Some(new_id) = id_map.get(&entry.id).copied() else {
            continue;
        };
        let Some(pixels) = extract_pixel_ref(&entry.body) else {
            continue;
        };
        let Some(format) = texture_format_from_str(&pixels.format) else {
            continue;
        };
        match format {
            wgpu::TextureFormat::Rgba8Unorm => {
                engine.compositor.ensure_raster_layer(
                    &engine.gpu.device,
                    &engine.gpu.queue,
                    new_id,
                    pixels.bounds,
                );
            }
            _ => {
                engine.compositor.ensure_node_texture(
                    &engine.gpu.device,
                    &engine.gpu.queue,
                    new_id,
                    format,
                    pixels.bounds,
                );
            }
        }
        if let Some(bytes) = entries.get(&pixels.pixels) {
            upload_to_node(engine, new_id, bytes);
        }
    }

    // Modifiers with bodies that include a `pixels` ref go through the
    // unified `ensure_node_texture`. The selection modifier carries
    // a `pixels` body field too, but its R8 texture is allocated by
    // `ensure_selection_state` below; skip the unified allocator for
    // the selection id specifically.
    let selection_old_id = manifest.selection_id;
    for entry in &manifest.modifiers {
        if Some(entry.id) == selection_old_id {
            continue;
        }
        let Some(new_id) = id_map.get(&entry.id).copied() else {
            continue;
        };
        let Some(pixels) = extract_pixel_ref(&entry.body) else {
            continue;
        };
        let Some(format) = texture_format_from_str(&pixels.format) else {
            continue;
        };
        engine.compositor.ensure_node_texture(
            &engine.gpu.device,
            &engine.gpu.queue,
            new_id,
            format,
            pixels.bounds,
        );
        if let Some(bytes) = entries.get(&pixels.pixels) {
            upload_to_node(engine, new_id, bytes);
        }
    }
}

/// Restore persistent void textures (camera void's last received frame,
/// future screenshare, …) into their freshly-allocated GPU caches. Runs
/// after `sync_compositor_layers` so every void already has an
/// `EffectCache` to overwrite. Walks the doc's void layers — the
/// `frame` field on each is the authoritative source of "this void
/// has a persisted texture under blob_key X at dims W×H".
fn upload_loaded_void_pixels(engine: &mut DarklyEngine, entries: &HashMap<String, Vec<u8>>) {
    let restores: Vec<(LayerId, u32, u32, String)> = engine
        .doc
        .all_void_layers()
        .into_iter()
        .filter_map(|v| {
            let frame = v.frame.as_ref()?;
            let bounds = frame.bounds;
            Some((v.id, bounds.width, bounds.height, frame.pixels.clone()))
        })
        .collect();
    for (id, w, h, blob_key) in restores {
        let Some(bytes) = entries.get(&blob_key) else {
            continue;
        };
        engine.compositor.restore_void_pixels(
            &engine.gpu.device,
            &engine.gpu.queue,
            id,
            w,
            h,
            bytes,
        );
    }
}

/// Thin wrapper that routes through `Compositor::upload_node_pixels`
/// (which atomically writes + dirty-marks). A `false` return means the
/// node has no texture or the buffer is short — log and continue
/// (the load is past every refusal gate; half a layer beats aborting
/// with a fresh compositor sitting around).
fn upload_to_node(engine: &mut DarklyEngine, node_id: LayerId, bytes: &[u8]) {
    let ok = engine
        .compositor
        .upload_node_pixels(&engine.gpu.queue, node_id, bytes);
    if !ok {
        log::error!("load: pixel upload failed for node {:?}", node_id.to_ffi());
    }
}

/// Rebuild the veil chain from `manifest.veils`. The `requires`
/// pre-check has already refused any veil the binary doesn't know
/// about, so any miss here is a logic bug — the registry must have
/// changed between pre-check and restore (impossible without a
/// concurrent mutation we don't allow).
fn restore_veils(engine: &mut DarklyEngine, manifest: &Manifest) {
    engine.compositor.veil_chain_mut().clear_veils();
    for veil in &manifest.veils {
        let type_id = veil.instance.type_id.clone();
        let params = veil.instance.params.clone();
        if !engine.compositor.veil_chain().registry().has(&type_id) {
            log::error!(
                "load: veil '{type_id}' missing despite requires pre-check — \
                 registry drift?"
            );
            continue;
        }
        engine.add_veil(&type_id, &params);
        if !veil.visible {
            let last = engine.compositor.veil_chain().count().saturating_sub(1);
            engine.set_veil_visible(last, false);
        }
    }
}

/// Allocate the selection-modifier GPU state — mirrors the engine
/// constructor's eager allocation.
fn ensure_selection_state(engine: &mut DarklyEngine) {
    let id = engine.doc.ensure_selection_modifier();
    engine.compositor.ensure_selection_state(
        &engine.gpu.device,
        id,
        engine.brush_pipelines.selection_bind_group_layout(),
        &engine.paint_pipelines.selection_bind_group_layout,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::manifest::ManifestEntry;
    #[allow(unused_imports)]
    use serde_json::json;

    /// `rebuild_parent_map` derives [`Document::parent`] entirely from
    /// nodes' `children` and `modifiers` lists. A handwritten staging doc
    /// with empty parent map must end up consistent after the rebuild.
    #[test]
    fn rebuild_parent_map_derives_from_children_lists() {
        let mut doc = Document::new(8, 8);
        let g = doc.add_group(None);
        let l = doc.add_raster_layer(Some(g));
        let m = doc.add_mask_modifier(l).expect("mask added");

        // Wipe parent map and force a rebuild — the derived state must
        // match the original.
        doc.parent.clear();
        rebuild_parent_map(&mut doc);

        assert_eq!(doc.parent_of(l), Some(g));
        assert_eq!(doc.parent_of(m), Some(l));
        assert_eq!(doc.parent_of(g), Some(doc.root));
    }

    /// `build_staging_document` builds a Document via the layer-kind /
    /// modifier registries — no central match on type_id. A manifest with
    /// one of every kind round-trips through `serialize` + `deserialize`
    /// and arrives with consistent tree + parent state.
    #[test]
    fn staging_document_round_trips_one_of_each_kind() {
        // We don't go through save here — we just hand-build a manifest
        // shape that mirrors what a real save would produce, then assert
        // the staging doc's structure.
        let root_id: u64 = 1;
        let group_id: u64 = 2;
        let raster_id: u64 = 3;
        let mask_id: u64 = 4;

        let manifest = Manifest {
            format: crate::format::manifest::FORMAT_TAG.to_string(),
            container_version: crate::format::manifest::CONTAINER_VERSION,
            writer: crate::format::manifest::ManifestWriter::current(),
            name: "Test".to_string(),
            canvas: crate::format::manifest::ManifestCanvas {
                width: 8,
                height: 8,
            },
            requires: ManifestRequires {
                layer_kind: vec!["group".to_string(), "raster".to_string()],
                modifier: vec!["mask".to_string()],
                blend_mode: vec!["normal".to_string()],
                ..Default::default()
            },
            composite: "composite.png".to_string(),
            root: root_id,
            nodes: vec![
                ManifestEntry {
                    id: root_id,
                    type_id: "group".to_string(),
                    body: json!({
                        "name": "Root",
                        "visible": true,
                        "locked": false,
                        "opacity": 1.0,
                        "blend_mode": "normal",
                        "passthrough": true,
                        "collapsed": false,
                        "children": [group_id],
                        "modifiers": []
                    }),
                },
                ManifestEntry {
                    id: group_id,
                    type_id: "group".to_string(),
                    body: json!({
                        "name": "Group",
                        "visible": true,
                        "locked": false,
                        "opacity": 1.0,
                        "blend_mode": "normal",
                        "passthrough": true,
                        "collapsed": false,
                        "children": [raster_id],
                        "modifiers": []
                    }),
                },
                ManifestEntry {
                    id: raster_id,
                    type_id: "raster".to_string(),
                    body: json!({
                        "name": "Layer",
                        "visible": true,
                        "locked": false,
                        "opacity": 1.0,
                        "blend_mode": "normal",
                        "pixels": {
                            "format": "rgba8unorm",
                            "pixels": "layers/3.pixels",
                            "bounds": { "origin": { "x": 0, "y": 0 }, "width": 8, "height": 8 }
                        },
                        "modifiers": [mask_id]
                    }),
                },
            ],
            modifiers: vec![ManifestEntry {
                id: mask_id,
                type_id: "mask".to_string(),
                body: json!({
                    "name": "Mask",
                    "visible": true,
                    "locked": false,
                    "pixels": {
                        "format": "r8unorm",
                        "pixels": "layers/4.mask.pixels",
                        "bounds": { "origin": { "x": 0, "y": 0 }, "width": 8, "height": 8 }
                    }
                }),
            }],
            selection_id: None,
            veils: Vec::new(),
        };

        let (doc, id_map) = build_staging_document(&manifest).expect("build staging doc");
        assert_eq!(doc.width, 8);
        assert_eq!(doc.height, 8);
        let new_group = *id_map.get(&group_id).expect("group remap");
        let new_raster = *id_map.get(&raster_id).expect("raster remap");
        let new_mask = *id_map.get(&mask_id).expect("mask remap");
        assert_eq!(doc.parent_of(new_group), Some(doc.root_id()));
        assert_eq!(doc.parent_of(new_raster), Some(new_group));
        assert_eq!(doc.parent_of(new_mask), Some(new_raster));
    }
}
