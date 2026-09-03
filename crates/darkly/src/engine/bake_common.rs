//! Shared helpers for bake-style layer ops (duplicate, merge down, flatten).
//!
//! Centralised so each op stays focused on its own document mutation and
//! the tombstone-id collection logic doesn't drift between callers.
//!
//! # Thumbnail invariant
//!
//! Any function here that takes a `LayerId` and writes its texture must
//! call [`crate::gpu::compositor::Compositor::mark_node_pixels_dirty`] on
//! that id before returning. See the docs on that method for the full
//! rationale (short version: the mark is the write-site's job so callers
//! can't forget and produce thumbnail-less layers).

use super::DarklyEngine;
use crate::layer::{LayerId, LayerNode};
use crate::undo::{BakeLayersAction, BakeSourceSlot};

impl DarklyEngine {
    /// Every pixel-bearing node id under `root`: raster layers, mask
    /// filters, and any other filters that own a GPU texture in the
    /// compositor's `node_textures` pool.
    ///
    /// Bake / duplicate actions stash this list in their `on_evict` tombstone
    /// vector so that when the owning undo step leaves the stack, every
    /// associated texture is disposed exactly once.
    pub(crate) fn collect_pixel_node_ids(&self, root: LayerId) -> Vec<LayerId> {
        let mut out = Vec::new();
        self.collect_pixel_node_ids_rec(root, &mut out);
        out
    }

    fn collect_pixel_node_ids_rec(&self, id: LayerId, out: &mut Vec<LayerId>) {
        let Some(node) = self.doc.find_node(id) else {
            return;
        };
        match node {
            LayerNode::Layer(layer) => {
                // The layer answers for itself whether its texture is
                // irreplaceable. A derived one (a procedural void's render, a
                // vector layer's rasterization) is cheaper to rebuild than to
                // retain; a void holding an externally-sourced image is not.
                if layer.owns_disposable_texture() {
                    out.push(id);
                }
                // Attached filter pixels (a mask on the node) participate
                // either way.
                let mods = node.filters().to_vec();
                for m_id in mods {
                    if let Some(m) = self.doc.find_filter(m_id) {
                        if m.pixels().is_some() {
                            out.push(m_id);
                        }
                    }
                }
            }
            LayerNode::Group(g) => {
                let mods = g.filters.clone();
                let children = g.children.clone();
                for m_id in mods {
                    if let Some(m) = self.doc.find_filter(m_id) {
                        if m.pixels().is_some() {
                            out.push(m_id);
                        }
                    }
                }
                for child_id in children {
                    self.collect_pixel_node_ids_rec(child_id, out);
                }
            }
        }
    }

    /// GPU-side copy of every pixel from one node's texture into another's.
    /// Both nodes must already have textures of the same format and extent,
    /// typically because the destination was just allocated with the
    /// source's bounds. Submits a single `copy_texture_to_texture`.
    ///
    /// Marks `dst_id` thumbnail-dirty before returning per the write-site
    /// invariant: callers don't need to do it.
    pub(crate) fn clone_node_pixels(&mut self, src_id: LayerId, dst_id: LayerId) {
        let extent = match self.compositor.node_texture(src_id) {
            Some(t) => t.canvas_extent(),
            None => return,
        };
        let (src_tex, dst_tex) = match (
            self.compositor.node_texture(src_id),
            self.compositor.node_texture(dst_id),
        ) {
            (Some(s), Some(d)) => (s.texture(), d.texture()),
            _ => return,
        };
        let width = extent.width;
        let height = extent.height;
        self.gpu.encode("clone-node-pixels", |encoder| {
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: src_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: dst_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        });
        self.compositor.mark_node_pixels_dirty(dst_id);
    }
    /// Consume `sources` into `result_id` at `(parent, position)` as one undo
    /// step: tombstone every pixel-bearing node under each source, detach them,
    /// land the result in the slot, and push the [`BakeLayersAction`] that
    /// reverses it.
    ///
    /// The shared tail of every bake op: flatten image, rasterize/flatten a
    /// node, merge down, merge a selection. They differ in what they composite
    /// and where the result belongs; from here on they are identical, and were
    /// four verbatim copies before this existed.
    ///
    /// **Must be called while the sources are still attached**, because the tombstone
    /// walk needs `find_node` to reach their subtrees, which `detach_for_undo`
    /// breaks. `sources` carries each one's pre-detach slot so undo can put it
    /// back exactly.
    pub(crate) fn finish_bake(
        &mut self,
        sources: Vec<BakeSourceSlot>,
        result_id: LayerId,
        parent: Option<LayerId>,
        position: usize,
    ) {
        let mut source_tombstones: Vec<LayerId> = Vec::new();
        for slot in &sources {
            source_tombstones.extend(self.collect_pixel_node_ids(slot.id));
        }

        for slot in &sources {
            self.doc.detach_for_undo(slot.id);
        }

        // Detach + reinsert is the exact-slot landing: the result was anchored
        // relative to a source that has just been detached, so its current
        // position means nothing.
        self.doc.detach_for_undo(result_id);
        self.doc.reinsert_entity(result_id, parent, position);

        // Re-read rather than trusting the requested slot: `reinsert_entity`
        // clamps, and undo has to reverse where the node actually landed.
        let result_parent = self.doc.parent_of(result_id);
        let result_position = self.doc.position_in_parent(result_id).unwrap_or(0);

        self.compositor.mark_dirty();
        self.push_undo(Box::new(BakeLayersAction::new(
            sources,
            source_tombstones,
            result_id,
            result_parent,
            result_position,
            // Asking the result what it owns, rather than assuming one texture,
            // so a result that is not a bare raster still disposes correctly.
            self.collect_pixel_node_ids(result_id),
        )));
    }
}
