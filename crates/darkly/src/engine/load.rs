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

use std::collections::HashMap;

use super::DarklyEngine;
use crate::coord::CanvasRect;
use crate::document::{layer_kind, modifier};
use crate::document::{Document, Entity, ModifierKind};
use crate::document::{Modifier, SelectionCpuCache, SelectionModifier};
use crate::format::error::LoadError;
use crate::format::manifest::{
    texture_format_from_str, Manifest, ManifestModifier, ManifestNode, ManifestRequires,
    CONTAINER_VERSION,
};
use crate::format::unzip::unzip_entries;
use crate::gpu::blend_mode;
use crate::gpu::compositor::Compositor;
use crate::layer::{
    BlendProps, Layer, LayerGroup, LayerId, LayerNode, NodeCommon, PixelBuffer, RasterLayer,
};

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
    if found > CONTAINER_VERSION as u64 {
        return Err(LoadError::ContainerTooNew {
            found: found as u32,
            supported: CONTAINER_VERSION,
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
/// Returns [`LoadError::CorruptManifest`] when a per-variant `type_id`
/// references something the registry doesn't know. The `requires`
/// pre-check should have caught this earlier — reaching it here means
/// the file's `requires` block lied (declared less than it actually
/// uses, or was inconsistent with the body), so we treat it as
/// corruption rather than mere absence.
fn build_staging_document(
    manifest: &Manifest,
) -> Result<(Document, HashMap<u64, LayerId>), LoadError> {
    let mut doc = Document::new(manifest.canvas.width, manifest.canvas.height);
    doc.name = manifest.name.clone();

    // Map manifest ids → fresh slotmap keys. The manifest's `root` maps
    // to the new doc's auto-allocated root — we don't re-create the
    // root entity, we reuse the one Document::new built. Every other
    // node + modifier is allocated fresh and recorded.
    let mut id_map: HashMap<u64, LayerId> = HashMap::with_capacity(manifest.tree.nodes.len());
    let new_root = doc.root_id();
    id_map.insert(manifest.tree.root, new_root);

    let blend_registry = blend_mode::registry();

    // First pass: insert raw entities into the slotmap (no parent links
    // yet — the rewrite pass below patches children/modifiers/parent
    // once the full id map is known).
    for node in &manifest.tree.nodes {
        if node.id() == manifest.tree.root {
            continue;
        }
        let new_id = match node {
            ManifestNode::Raster(r) => {
                let blend_reg = blend_registry.get(&r.blend_mode).ok_or_else(|| {
                    LoadError::CorruptManifest {
                        reason: format!(
                            "node {} references undeclared blend_mode/{} \
                                 — `requires` block lies",
                            r.id, r.blend_mode
                        ),
                    }
                })?;
                let format = texture_format_from_str(&r.pixels.format).ok_or_else(|| {
                    LoadError::CorruptManifest {
                        reason: format!(
                            "node {} uses unknown texture format '{}'",
                            r.id, r.pixels.format
                        ),
                    }
                })?;
                doc.entities.insert_with_key(|key| {
                    Entity::Node(LayerNode::Layer(Layer::Raster(RasterLayer {
                        id: key,
                        common: NodeCommon {
                            name: r.name.clone(),
                            visible: r.visible,
                            locked: r.locked,
                        },
                        blend: BlendProps {
                            opacity: r.opacity,
                            blend_mode: blend_reg,
                        },
                        pixels: PixelBuffer::new(r.pixels.bounds, format),
                        modifiers: Vec::new(), // filled below
                    })))
                })
            }
            ManifestNode::Group(g) => {
                let blend_reg = blend_registry.get(&g.blend_mode).ok_or_else(|| {
                    LoadError::CorruptManifest {
                        reason: format!(
                            "group {} references undeclared blend_mode/{} \
                                 — `requires` block lies",
                            g.id, g.blend_mode
                        ),
                    }
                })?;
                doc.entities.insert_with_key(|key| {
                    Entity::Node(LayerNode::Group(LayerGroup {
                        id: key,
                        common: NodeCommon {
                            name: g.name.clone(),
                            visible: g.visible,
                            locked: g.locked,
                        },
                        blend: BlendProps {
                            opacity: g.opacity,
                            blend_mode: blend_reg,
                        },
                        children: Vec::new(), // filled below
                        modifiers: Vec::new(),
                        passthrough: g.passthrough,
                        collapsed: g.collapsed,
                    }))
                })
            }
        };
        id_map.insert(node.id(), new_id);
    }

    for m in &manifest.modifiers {
        let new_id = match m {
            ManifestModifier::Mask(mask) => doc.entities.insert_with_key(|key| {
                Entity::Modifier(Modifier {
                    id: key,
                    common: NodeCommon {
                        name: mask.name.clone(),
                        visible: mask.visible,
                        locked: mask.locked,
                    },
                    kind: ModifierKind::mask_with_bounds(mask.pixels.bounds),
                })
            }),
            ManifestModifier::Selection(sel) => doc.entities.insert_with_key(|key| {
                Entity::Modifier(Modifier {
                    id: key,
                    common: NodeCommon {
                        name: sel.name.clone(),
                        visible: sel.visible,
                        locked: sel.locked,
                    },
                    kind: ModifierKind::Selection(SelectionModifier {
                        pixels: PixelBuffer::new(sel.pixels.bounds, wgpu::TextureFormat::R8Unorm),
                        cpu_cache: SelectionCpuCache::new(),
                        pixel_bounds: None,
                    }),
                })
            }),
        };
        id_map.insert(m.id(), new_id);
    }

    // Second pass: rewrite cross-references with the new id map and
    // populate the parent secondary-map. Children are sorted into
    // the parent's `children` Vec in the order they appear on the
    // manifest (display order, bottom-to-top).
    for node in &manifest.tree.nodes {
        let new_id = *id_map.get(&node.id()).expect("id map miss");
        match node {
            ManifestNode::Raster(r) => {
                let modifier_ids: Vec<LayerId> = r
                    .modifiers
                    .iter()
                    .filter_map(|m| id_map.get(m).copied())
                    .collect();
                if let Some(Entity::Node(LayerNode::Layer(Layer::Raster(raster)))) =
                    doc.entities.get_mut(new_id)
                {
                    raster.modifiers = modifier_ids.clone();
                }
                for mid in modifier_ids {
                    doc.parent.insert(mid, new_id);
                }
            }
            ManifestNode::Group(g) => {
                let child_ids: Vec<LayerId> = g
                    .children
                    .iter()
                    .filter_map(|c| id_map.get(c).copied())
                    .collect();
                let modifier_ids: Vec<LayerId> = g
                    .modifiers
                    .iter()
                    .filter_map(|m| id_map.get(m).copied())
                    .collect();
                if let Some(Entity::Node(LayerNode::Group(group))) = doc.entities.get_mut(new_id) {
                    group.children = child_ids.clone();
                    group.modifiers = modifier_ids.clone();
                }
                for cid in child_ids {
                    doc.parent.insert(cid, new_id);
                }
                for mid in modifier_ids {
                    doc.parent.insert(mid, new_id);
                }
            }
        }
    }

    // Selection modifier (if any) hooks into Document::selection rather
    // than parent. Wire up the field if the manifest references one.
    if manifest.selection.is_some() {
        if let Some(sel_old_id) = manifest.modifiers.iter().find_map(|m| match m {
            ManifestModifier::Selection(s) => Some(s.id),
            _ => None,
        }) {
            if let Some(new_id) = id_map.get(&sel_old_id) {
                doc.selection = Some(*new_id);
            }
        }
    }

    Ok((doc, id_map))
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
    id_map: HashMap<u64, LayerId>,
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
/// matching pixel bytes from the zip. Layers go through
/// `ensure_raster_layer`; masks + selection go through
/// `ensure_node_texture`. Both share the `gpu.queue.write_texture`
/// upload path used by `paste_image`.
///
/// Infallible — pixel-buffer size mismatches log and continue (the
/// surrounding load is already past every refusal check and we'd
/// rather show a half-loaded layer than abort with a fresh
/// compositor sitting around).
fn upload_loaded_pixels(
    engine: &mut DarklyEngine,
    manifest: &Manifest,
    id_map: &HashMap<u64, LayerId>,
    entries: &HashMap<String, Vec<u8>>,
) {
    for node in &manifest.tree.nodes {
        if let ManifestNode::Raster(r) = node {
            let new_id = match id_map.get(&r.id) {
                Some(id) => *id,
                None => continue,
            };
            engine.compositor.ensure_raster_layer(
                &engine.gpu.device,
                &engine.gpu.queue,
                new_id,
                r.pixels.bounds,
            );
            if let Some(bytes) = entries.get(&r.pixels.pixels) {
                upload_pixels(
                    engine,
                    new_id,
                    wgpu::TextureFormat::Rgba8Unorm,
                    r.pixels.bounds,
                    bytes,
                );
            }
        }
    }

    for m in &manifest.modifiers {
        match m {
            ManifestModifier::Mask(mask) => {
                let new_id = match id_map.get(&mask.id) {
                    Some(id) => *id,
                    None => continue,
                };
                engine.compositor.ensure_node_texture(
                    &engine.gpu.device,
                    &engine.gpu.queue,
                    new_id,
                    wgpu::TextureFormat::R8Unorm,
                    mask.pixels.bounds,
                );
                if let Some(bytes) = entries.get(&mask.pixels.pixels) {
                    upload_pixels(
                        engine,
                        new_id,
                        wgpu::TextureFormat::R8Unorm,
                        mask.pixels.bounds,
                        bytes,
                    );
                }
            }
            ManifestModifier::Selection(_) => {
                // Selection texture is allocated by `ensure_selection_state`
                // below; the upload happens once that's wired up.
            }
        }
    }
}

/// Upload raw pixel bytes into a node's GPU texture. Mirrors the
/// `paste_image` upload path — `write_texture` + the explicit row
/// stride matching `bounds.width * bpp`.
fn upload_pixels(
    engine: &DarklyEngine,
    node_id: LayerId,
    format: wgpu::TextureFormat,
    bounds: CanvasRect,
    bytes: &[u8],
) {
    let Some(layer_tex) = engine.compositor.node_texture(node_id) else {
        return;
    };
    let bpp = format.block_copy_size(None).unwrap_or(1);
    let expected = (bounds.width * bounds.height * bpp) as usize;
    if bytes.len() < expected {
        log::error!(
            "load: pixel buffer too small for node {:?}: {} < {expected}",
            node_id.to_ffi(),
            bytes.len()
        );
        return;
    }
    engine.gpu.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &layer_tex.texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &bytes[..expected],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bounds.width * bpp),
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width: bounds.width,
            height: bounds.height,
            depth_or_array_layers: 1,
        },
    );
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
