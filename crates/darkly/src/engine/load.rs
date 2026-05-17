//! `.darkly` load flow — minimal happy-path implementation for Phase 3.
//!
//! Reads a `.darkly` zip, parses the manifest, rebuilds a fresh
//! [`Document`] with remapped slotmap ids, allocates GPU textures, and
//! uploads pixel data. Phase 4 hardens this with container-version /
//! `requires` pre-checks and the staging-doc atomic-install pattern;
//! Phase 3 just gets the data path round-tripping so the kitchen-sink
//! test can drive end-to-end save → reload.
//!
//! Anything other than the success path returns [`LoadError::Io`],
//! [`LoadError::Zip`], or [`LoadError::Json`] today — the precise
//! refusal diagnostics ([`LoadError::ContainerTooNew`],
//! [`LoadError::UnsupportedFeatures`], etc.) land in Phase 4.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use super::DarklyEngine;
use crate::coord::CanvasRect;
use crate::document::{layer_kind, modifier};
use crate::document::{Document, Entity, ModifierKind};
use crate::document::{Modifier, SelectionCpuCache, SelectionModifier};
use crate::format::error::LoadError;
use crate::format::manifest::{texture_format_from_str, Manifest, ManifestModifier, ManifestNode};
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
    /// Load a `.darkly` zip into this engine. Replaces every piece of
    /// document and compositor state with what the file describes; the
    /// undo stack is cleared and session state (active tool, isolation,
    /// in-flight stroke) is reset.
    ///
    /// Minimal Phase 3 behaviour — no pre-checks, no atomic install.
    /// Failures part-way through leave the engine in an inconsistent
    /// state; Phase 4 lands the atomic-install + pre-check pattern that
    /// makes this all-or-nothing.
    pub fn open_document(&mut self, bytes: &[u8]) -> Result<(), LoadError> {
        let entries = read_zip_entries(bytes)?;
        let manifest_bytes = entries
            .get("manifest.json")
            .ok_or_else(|| LoadError::Json("missing manifest.json".to_string()))?;
        let manifest: Manifest = serde_json::from_slice(manifest_bytes)?;

        let (staging, id_map) = build_staging_document(&manifest)?;

        // Replace the document. Compositor follows below — we rebuild
        // from scratch so every cached texture / bind group keyed by
        // the old slotmap is dropped.
        self.doc = staging;
        // Cancel any in-flight readbacks that referenced the previous
        // document — their context ids are stale once we install.
        self.readbacks.cancel(|_| true);
        self.compositor = Compositor::new(
            &self.gpu.device,
            &self.gpu.queue,
            self.gpu.surface_format(),
            self.doc.width,
            self.doc.height,
            self.doc.root_id(),
        );

        upload_loaded_pixels(self, &manifest, &id_map, &entries)?;
        // The veil chain sizes to the surface in production (via
        // `resize()`); on a freshly-loaded compositor it's still 0×0,
        // and `add_veil`'s `ensure_textures` would no-op silently and
        // then unwrap on `views`. Seed to canvas dimensions so the
        // restore path always sees a sized viewport — the next real
        // resize cascades to the right surface size automatically.
        self.compositor.veil_chain_mut().resize(
            &self.gpu.device,
            &self.gpu.queue,
            self.doc.width,
            self.doc.height,
        );
        restore_veils(self, &manifest);
        ensure_selection_state(self);
        self.sync_compositor_layers();

        // Reset session-level state so the loaded doc starts clean.
        self.active_stroke_layer = None;
        self.isolated_node = None;
        self.floating = None;
        self.selection_overlay.clear();
        self.tool_overlay.clear();
        self.undo_stack = crate::undo::UndoStack::new(50);
        self.thumbnail_cache = super::ThumbnailCache::new();
        self.thumbnail_version = self.thumbnail_version.wrapping_add(1);

        self.compositor.mark_dirty();
        self.compositor.mark_needs_present();
        Ok(())
    }
}

/// Extract every entry from a `.darkly` zip into a path → bytes map.
/// Mirrors the test-only `format::zip_io::extract_zip` helper but
/// converts zip errors into structured [`LoadError`] for the engine.
fn read_zip_entries(bytes: &[u8]) -> Result<HashMap<String, Vec<u8>>, LoadError> {
    let cursor = Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| LoadError::Zip(e.to_string()))?;
    let mut entries = HashMap::with_capacity(archive.len());
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| LoadError::Zip(e.to_string()))?;
        let path = entry.name().to_string();
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut buf)
            .map_err(|e| LoadError::Zip(e.to_string()))?;
        entries.insert(path, buf);
    }
    Ok(entries)
}

/// Build a fresh [`Document`] from a [`Manifest`], producing an
/// `old_id → new_id` map so the loader can rewrite cross-references
/// (children, modifiers, host) into the new slotmap.
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

    // First pass: insert raw entities into the slotmap (no parent links
    // yet — the rewrite pass below patches children/modifiers/parent
    // once the full id map is known).
    let layer_kind_registry = layer_kind::registry();
    let modifier_registry = modifier::registry();
    let blend_registry = blend_mode::registry();

    for node in &manifest.tree.nodes {
        if node.id() == manifest.tree.root {
            continue;
        }
        let new_id =
            match node {
                ManifestNode::Raster(r) => {
                    if layer_kind_registry.get("raster").is_none() {
                        return Err(LoadError::UnknownTypeId {
                            kind: "layer_kind",
                            id: "raster".to_string(),
                        });
                    }
                    let blend_reg = blend_registry.get(&r.blend_mode).ok_or_else(|| {
                        LoadError::UnknownTypeId {
                            kind: "blend_mode",
                            id: r.blend_mode.clone(),
                        }
                    })?;
                    let format = texture_format_from_str(&r.pixels.format).ok_or_else(|| {
                        LoadError::CorruptManifest {
                            reason: format!("unknown texture format '{}'", r.pixels.format),
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
                    if layer_kind_registry.get("group").is_none() {
                        return Err(LoadError::UnknownTypeId {
                            kind: "layer_kind",
                            id: "group".to_string(),
                        });
                    }
                    let blend_reg = blend_registry.get(&g.blend_mode).ok_or_else(|| {
                        LoadError::UnknownTypeId {
                            kind: "blend_mode",
                            id: g.blend_mode.clone(),
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
            ManifestModifier::Mask(mask) => {
                if modifier_registry.get("mask").is_none() {
                    return Err(LoadError::UnknownTypeId {
                        kind: "modifier",
                        id: "mask".to_string(),
                    });
                }
                doc.entities.insert_with_key(|key| {
                    Entity::Modifier(Modifier {
                        id: key,
                        common: NodeCommon {
                            name: mask.name.clone(),
                            visible: mask.visible,
                            locked: mask.locked,
                        },
                        kind: ModifierKind::mask_with_bounds(mask.pixels.bounds),
                    })
                })
            }
            ManifestModifier::Selection(sel) => {
                if modifier_registry.get("selection").is_none() {
                    return Err(LoadError::UnknownTypeId {
                        kind: "modifier",
                        id: "selection".to_string(),
                    });
                }
                doc.entities.insert_with_key(|key| {
                    Entity::Modifier(Modifier {
                        id: key,
                        common: NodeCommon {
                            name: sel.name.clone(),
                            visible: sel.visible,
                            locked: sel.locked,
                        },
                        kind: ModifierKind::Selection(SelectionModifier {
                            pixels: PixelBuffer::new(
                                sel.pixels.bounds,
                                wgpu::TextureFormat::R8Unorm,
                            ),
                            cpu_cache: SelectionCpuCache::new(),
                            pixel_bounds: None,
                        }),
                    })
                })
            }
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

/// Allocate GPU textures for every loaded entity and upload the
/// matching pixel bytes from the zip. Layers go through
/// `ensure_raster_layer`; masks + selection go through
/// `ensure_node_texture`. Both share the `gpu.queue.write_texture`
/// upload path used by `paste_image`.
fn upload_loaded_pixels(
    engine: &mut DarklyEngine,
    manifest: &Manifest,
    id_map: &HashMap<u64, LayerId>,
    entries: &HashMap<String, Vec<u8>>,
) -> Result<(), LoadError> {
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
                // below; the upload happens in `restore_selection_pixels`.
            }
        }
    }

    Ok(())
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

/// Rebuild the veil chain from `manifest.veils`. Veils that the binary
/// doesn't recognize are skipped silently for the Phase 3 happy path;
/// Phase 4's `requires` pre-check raises the structured refusal before
/// we get here in normal flow.
fn restore_veils(engine: &mut DarklyEngine, manifest: &Manifest) {
    engine.compositor.veil_chain_mut().clear_veils();
    for veil in &manifest.veils {
        let type_id = veil.instance.type_id.clone();
        let params = veil.instance.params.clone();
        let registry_has = engine
            .compositor
            .veil_chain()
            .registry()
            .param_defs(&type_id)
            .iter()
            .next()
            .is_some()
            || engine
                .compositor
                .veil_chain()
                .registry()
                .display_name(&type_id)
                .is_empty()
                .then_some(false)
                .is_none(); // not in registry → display_name == ""
        if !registry_has {
            // Unknown veil — Phase 4 will raise this through the
            // `requires` pre-check. For Phase 3, skip silently.
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
