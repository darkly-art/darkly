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
//! rationale — short version: the mark is the write-site's job so callers
//! can't forget and produce thumbnail-less layers.

use super::DarklyEngine;
use crate::layer::{Layer, LayerId, LayerNode};

impl DarklyEngine {
    /// Every pixel-bearing node id under `root` — raster layers, mask
    /// modifiers, and any other modifiers that own a GPU texture in the
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
            LayerNode::Layer(Layer::Raster(_)) => {
                out.push(id);
                let mods = node.modifiers().to_vec();
                for m_id in mods {
                    if let Some(m) = self.doc.find_modifier(m_id) {
                        if m.pixels().is_some() {
                            out.push(m_id);
                        }
                    }
                }
            }
            LayerNode::Layer(Layer::Void(_)) => {
                // Voids hold no pixel data of their own — the procedural
                // texture is GPU-regenerable from params, so bake collection
                // skips the void itself. Modifier pixels (e.g. a mask
                // attached to the void) still need to participate.
                let mods = node.modifiers().to_vec();
                for m_id in mods {
                    if let Some(m) = self.doc.find_modifier(m_id) {
                        if m.pixels().is_some() {
                            out.push(m_id);
                        }
                    }
                }
            }
            LayerNode::Group(g) => {
                let mods = g.modifiers.clone();
                let children = g.children.clone();
                for m_id in mods {
                    if let Some(m) = self.doc.find_modifier(m_id) {
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
    /// Both nodes must already have textures of the same format and extent
    /// — typically because the destination was just allocated with the
    /// source's bounds. Submits a single `copy_texture_to_texture`.
    ///
    /// Marks `dst_id` thumbnail-dirty before returning per the write-site
    /// invariant — callers don't need to do it.
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
}
