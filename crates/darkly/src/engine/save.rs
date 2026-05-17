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
//! See [the plan's Save flow section](../../../../darkly-file-format-plan.md#save-flow)
//! for the concurrent-edit rationale.

use std::collections::{HashMap, HashSet};

use super::{DarklyEngine, ReadbackContext};
use crate::document::{Modifier, ModifierKind};
use crate::format::manifest::{
    texture_format_to_str, Manifest, ManifestCanvas, ManifestGroupNode, ManifestMaskModifier,
    ManifestModifier, ManifestNode, ManifestPixelRef, ManifestRasterNode, ManifestRequires,
    ManifestSelection, ManifestSelectionModifier, ManifestTree, ManifestVeil, ManifestWriter,
    SaveBlob, SaveBundle, CONTAINER_VERSION, FORMAT_TAG,
};
use crate::format::registry_io::InstancePayload;
use crate::gpu::readback;
use crate::layer::{Layer, LayerId, LayerNode};

/// Errors `start_save_document` can return synchronously.
#[derive(Debug)]
pub enum SaveError {
    /// A save is already in flight on this engine. Wait for
    /// `poll_save_result` to return `Some` before kicking off another.
    /// The UI disables the Save action for that tab during a save.
    InProgress,
    /// A pixel-bearing entity referenced a texture format the on-disk
    /// schema doesn't yet name (see [`texture_format_to_str`]). New
    /// formats need a string variant added to that map first — this
    /// guards against silently dropping pixels on disk.
    UnsupportedTextureFormat(wgpu::TextureFormat),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::InProgress => write!(f, "a save is already in flight"),
            SaveError::UnsupportedTextureFormat(fmt) => {
                write!(f, "save: no wire-format slug for {fmt:?}")
            }
        }
    }
}

impl std::error::Error for SaveError {}

/// Which kind of texture a pending [`ReadbackContext::SaveDocument`]
/// readback is sourced from. Drives how the completed pixels are
/// stitched into the [`SaveJob`]: per-blob bytes for everything pixel-bearing,
/// `(width, height, rgba)` for the composite.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SaveReadbackKind {
    LayerPixels,
    MaskPixels,
    SelectionMask,
    Composite,
}

/// In-flight save state — created by [`DarklyEngine::start_save_document`]
/// and drained by [`DarklyEngine::poll_save_result`]. The fields capture
/// everything the save needs that's *not* a pixel readback (those land
/// asynchronously into `pending_blobs` / `composite`).
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
    /// readback is in flight; populated by `complete_readback` as each
    /// `LayerPixels` / `MaskPixels` / `SelectionMask` readback lands.
    pending_blobs: HashMap<String, Option<Vec<u8>>>,
    /// Composite readback result. `None` until the `Composite` arm
    /// fires.
    composite: Option<(u32, u32, Vec<u8>)>,
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
    pub fn start_save_document(&mut self) -> Result<(), SaveError> {
        if self.active_save_job.is_some() {
            return Err(SaveError::InProgress);
        }

        let manifest = build_manifest(self)?;

        // Force an offscreen composite so the composite texture is fresh,
        // even when this engine is headless (no surface present has run
        // since the last doc mutation).
        self.compositor
            .render_offscreen(&self.gpu.device, &self.gpu.queue, &mut self.doc);

        let canvas_w = self.compositor.canvas_width();
        let canvas_h = self.compositor.canvas_height();

        let mut pinned_textures = Vec::new();
        let mut pending_blobs: HashMap<String, Option<Vec<u8>>> = HashMap::new();

        // Walk the manifest's tree + modifiers + selection and queue a
        // readback per pixel-bearing entry. We walk the *manifest* (not
        // the live document) so the id mapping inside the manifest stays
        // authoritative — the readback `key` matches the
        // `ManifestPixelRef::pixels` path the load path will look up.
        for node in &manifest.tree.nodes {
            if let ManifestNode::Raster(r) = node {
                let live_id = LayerId::from_ffi(r.id);
                queue_pixel_readback(
                    self,
                    live_id,
                    &r.pixels,
                    SaveReadbackKind::LayerPixels,
                    &mut pinned_textures,
                    &mut pending_blobs,
                )?;
            }
        }
        for m in &manifest.modifiers {
            match m {
                ManifestModifier::Mask(mask) => {
                    let live_id = LayerId::from_ffi(mask.id);
                    queue_pixel_readback(
                        self,
                        live_id,
                        &mask.pixels,
                        SaveReadbackKind::MaskPixels,
                        &mut pinned_textures,
                        &mut pending_blobs,
                    )?;
                }
                ManifestModifier::Selection(sel) => {
                    let live_id = LayerId::from_ffi(sel.id);
                    queue_pixel_readback(
                        self,
                        live_id,
                        &sel.pixels,
                        SaveReadbackKind::SelectionMask,
                        &mut pinned_textures,
                        &mut pending_blobs,
                    )?;
                }
            }
        }
        if let Some(sel) = manifest.selection.as_ref() {
            // The selection modifier id is stored in
            // `Document::selection`; the manifest references the same
            // texture, so we look it up the same way.
            if let Some(id) = self.doc.selection_id() {
                queue_pixel_readback(
                    self,
                    id,
                    &sel.pixels,
                    SaveReadbackKind::SelectionMask,
                    &mut pinned_textures,
                    &mut pending_blobs,
                )?;
            }
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
                [0, 0, canvas_w, canvas_h],
            );
            self.readbacks.submit(
                request,
                ReadbackContext::SaveDocument {
                    kind: SaveReadbackKind::Composite,
                    key: String::new(),
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
        });

        Ok(())
    }

    /// Drain a completed save. Returns `None` while any readback is
    /// still in flight; returns `Some(SaveBundle)` once every pixel
    /// blob and the composite have landed.
    pub fn poll_save_result(&mut self) -> Option<SaveBundle> {
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
        // Stable ordering for tests + bit-stable output.
        blobs.sort_by(|a, b| a.path.cmp(&b.path));
        Some(SaveBundle {
            manifest_json,
            composite_width,
            composite_height,
            composite_rgba,
            blobs,
        })
    }

    /// Dispatch from `handle_completed_readback` — populate the matching
    /// blob slot or the composite triple. Unknown keys are silently
    /// dropped (the save was cancelled or a stale readback completed
    /// after `poll_save_result` drained the job).
    pub(crate) fn complete_save_readback(
        &mut self,
        kind: SaveReadbackKind,
        key: String,
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
            SaveReadbackKind::LayerPixels
            | SaveReadbackKind::MaskPixels
            | SaveReadbackKind::SelectionMask => {
                if let Some(slot) = job.pending_blobs.get_mut(&key) {
                    *slot = Some(pixels);
                }
            }
        }
    }
}

/// Walk the live document and produce a [`Manifest`] capturing every
/// piece of state that survives save: tree, modifiers, selection,
/// veils. Synchronous — runs as part of `start_save_document`'s
/// prelude.
fn build_manifest(engine: &DarklyEngine) -> Result<Manifest, SaveError> {
    let doc = &engine.doc;
    let mut nodes: Vec<ManifestNode> = Vec::new();
    let mut modifiers: Vec<ManifestModifier> = Vec::new();
    let mut selection: Option<ManifestSelection> = None;

    // Walk every entity (slotmap-backed, so iteration is stable within
    // this snapshot but the order is implementation-defined; we sort by
    // id below for bit-stable output).
    for (id, entity) in doc.entities.iter() {
        match entity {
            crate::document::Entity::Node(node) => {
                let manifest_node = build_manifest_node(id, node)?;
                nodes.push(manifest_node);
            }
            crate::document::Entity::Modifier(modifier) => {
                let host = doc.parent_of(id).map(LayerId::to_ffi);
                let manifest_modifier = build_manifest_modifier(id, modifier, host)?;

                if doc.selection_id() == Some(id) {
                    // The global selection modifier appears at both
                    // [`Manifest::modifiers`] (for the entity entry) and
                    // [`Manifest::selection`] (so the loader can find it
                    // without scanning).
                    selection = Some(ManifestSelection {
                        pixels: pixels_ref_for(modifier, &blob_path_for(id, modifier))?,
                    });
                }

                modifiers.push(manifest_modifier);
            }
        }
    }

    // Stable order for diffability + reliable id remap during load.
    nodes.sort_by_key(ManifestNode::id);
    modifiers.sort_by_key(ManifestModifier::id);

    let veils = build_manifest_veils(engine);
    let requires = requires_from_doc(engine);

    Ok(Manifest {
        format: FORMAT_TAG.to_string(),
        container_version: CONTAINER_VERSION,
        writer: ManifestWriter::current(),
        name: doc.name.clone(),
        canvas: ManifestCanvas {
            width: doc.width,
            height: doc.height,
        },
        requires,
        composite: "composite.png".to_string(),
        tree: ManifestTree {
            root: doc.root_id().to_ffi(),
            nodes,
        },
        modifiers,
        selection,
        veils,
    })
}

fn build_manifest_node(id: LayerId, node: &LayerNode) -> Result<ManifestNode, SaveError> {
    match node {
        LayerNode::Layer(Layer::Raster(r)) => Ok(ManifestNode::Raster(ManifestRasterNode {
            id: id.to_ffi(),
            name: r.common.name.clone(),
            visible: r.common.visible,
            locked: r.common.locked,
            opacity: r.blend.opacity,
            blend_mode: r.blend.blend_mode.type_id.to_string(),
            pixels: ManifestPixelRef {
                format: format_slug(r.pixels.format)?,
                pixels: format!("layers/{}.pixels", id.to_ffi()),
                bounds: r.pixels.bounds,
            },
            modifiers: r.modifiers.iter().map(|mid| mid.to_ffi()).collect(),
        })),
        LayerNode::Group(g) => Ok(ManifestNode::Group(ManifestGroupNode {
            id: id.to_ffi(),
            name: g.common.name.clone(),
            visible: g.common.visible,
            locked: g.common.locked,
            opacity: g.blend.opacity,
            blend_mode: g.blend.blend_mode.type_id.to_string(),
            passthrough: g.passthrough,
            collapsed: g.collapsed,
            children: g.children.iter().map(|cid| cid.to_ffi()).collect(),
            modifiers: g.modifiers.iter().map(|mid| mid.to_ffi()).collect(),
        })),
    }
}

fn build_manifest_modifier(
    id: LayerId,
    modifier: &Modifier,
    host: Option<u64>,
) -> Result<ManifestModifier, SaveError> {
    match &modifier.kind {
        ModifierKind::Mask(m) => Ok(ManifestModifier::Mask(ManifestMaskModifier {
            id: id.to_ffi(),
            host: host.unwrap_or(0),
            name: modifier.common.name.clone(),
            visible: modifier.common.visible,
            locked: modifier.common.locked,
            pixels: ManifestPixelRef {
                format: format_slug(m.pixels.format)?,
                pixels: format!("layers/{}.mask.pixels", id.to_ffi()),
                bounds: m.pixels.bounds,
            },
        })),
        ModifierKind::Selection(s) => Ok(ManifestModifier::Selection(ManifestSelectionModifier {
            id: id.to_ffi(),
            name: modifier.common.name.clone(),
            visible: modifier.common.visible,
            locked: modifier.common.locked,
            pixels: ManifestPixelRef {
                format: format_slug(s.pixels.format)?,
                pixels: "selection.pixels".to_string(),
                bounds: s.pixels.bounds,
            },
        })),
    }
}

fn blob_path_for(id: LayerId, modifier: &Modifier) -> String {
    match &modifier.kind {
        ModifierKind::Mask(_) => format!("layers/{}.mask.pixels", id.to_ffi()),
        ModifierKind::Selection(_) => "selection.pixels".to_string(),
    }
}

fn pixels_ref_for(modifier: &Modifier, blob_path: &str) -> Result<ManifestPixelRef, SaveError> {
    let buf = modifier
        .pixels()
        .expect("modifier pixels() should be Some for save");
    Ok(ManifestPixelRef {
        format: format_slug(buf.format)?,
        pixels: blob_path.to_string(),
        bounds: buf.bounds,
    })
}

fn format_slug(format: wgpu::TextureFormat) -> Result<String, SaveError> {
    texture_format_to_str(format)
        .map(str::to_string)
        .ok_or(SaveError::UnsupportedTextureFormat(format))
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
            crate::document::Entity::Node(node) => {
                layer_kinds.insert(node.type_id().to_string());
                blend_modes.insert(node.blend().blend_mode.type_id.to_string());
            }
            crate::document::Entity::Modifier(m) => {
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
    id: LayerId,
    pixels: &ManifestPixelRef,
    kind: SaveReadbackKind,
    pinned: &mut Vec<wgpu::Texture>,
    blobs: &mut HashMap<String, Option<Vec<u8>>>,
) -> Result<(), SaveError> {
    let key = pixels.pixels.clone();

    // Mask and layer textures resolve through the unified node-texture
    // pool. Selection lives in its own SelectionState — handle below.
    let (texture, format, width, height) = match kind {
        SaveReadbackKind::SelectionMask => match engine.compositor.selection_state() {
            Some(sel) => {
                let frame = sel.canvas_frame();
                (
                    frame.texture.clone(),
                    wgpu::TextureFormat::R8Unorm,
                    frame.canvas_extent.width,
                    frame.canvas_extent.height,
                )
            }
            None => return Ok(()),
        },
        SaveReadbackKind::LayerPixels | SaveReadbackKind::MaskPixels => {
            match engine.compositor.node_texture(id) {
                Some(t) => (t.texture.clone(), t.format, t.width, t.height),
                None => return Ok(()),
            }
        }
        SaveReadbackKind::Composite => unreachable!("composite queued via dedicated path"),
    };

    pinned.push(texture.clone());
    blobs.insert(key.clone(), None);

    engine.gpu.encode("save-pixel-readback", |encoder| {
        let request = readback::request_readback(
            &engine.gpu.device,
            encoder,
            &texture,
            format,
            [0, 0, width, height],
        );
        engine.readbacks.submit(
            request,
            ReadbackContext::SaveDocument {
                kind,
                key,
                width,
                height,
            },
        );
    });

    Ok(())
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
        engine.start_save_document().expect("first save kicks off");
        let err = engine
            .start_save_document()
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
            .veil_param_defs("noise")
            .iter()
            .map(crate::gpu::params::ParamDef::default_value)
            .collect();
        engine.add_veil("noise", &defaults);

        let requires = requires_from_doc(&engine);
        assert!(
            requires.veil.iter().any(|v| v == "noise"),
            "requires.veil should list noise (got {:?})",
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

    /// The save snapshot must survive concurrent edits — the manifest
    /// is built at submit time, GPU textures are refcount-pinned, and
    /// readbacks see GPU command-buffer state at submit time. Adding a
    /// layer between start_save and poll_save_result must *not* end up
    /// in the saved manifest.
    #[test]
    fn save_concurrent_edit_does_not_corrupt() {
        let mut engine = headless_engine(32, 32);
        let _baseline = engine.add_raster_layer(None);
        engine.start_save_document().expect("save kicks off");

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
            .tree
            .nodes
            .iter()
            .filter(|n| matches!(n, ManifestNode::Raster(_)))
            .count();
        assert_eq!(
            raster_count, 1,
            "snapshot must reflect doc state at start_save_document time, \
             not the post-mutation state — found {raster_count} rasters"
        );
    }
}
