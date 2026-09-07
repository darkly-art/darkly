//! GPU bake service for Merge Down and Flatten Image: composites a flat
//! list of source nodes into a destination raster layer's texture in a
//! single submit, through a transient [`Compositor`] group state.

use crate::document::Document;
use crate::gpu::compositor::Compositor;
use crate::gpu::{blit_region, clear_view_transparent};
use crate::layer::LayerId;

impl Compositor {
    /// Composite a flat list of source node ids into a target raster layer's
    /// texture, on the GPU, in one submit. Used by Merge Down and Flatten
    /// Image: both operations consume some sources, allocate a destination
    /// raster, and need the destination to hold the baked composite of the
    /// sources under their normal blend modes.
    ///
    /// `source_ids` is bottom-to-top order. Each source may be a raster, a
    /// non-passthrough group (its `composite_cache` must already be current),
    /// or a passthrough group (children inlined). The destination's GPU
    /// texture must already exist and be canvas-sized (the engine allocates
    /// it via `ensure_raster_layer` before calling).
    ///
    /// The bake runs through a transient `GroupState` keyed by slotmap's
    /// null `LayerId` so it doesn't collide with any real group. After
    /// composing, the final accum is `copy_texture_to_texture`'d into the
    /// destination's GPU texture — no CPU readback — and the transient
    /// state (three canvas-sized textures, plus any blend bind groups the
    /// walk cached against it) is released before returning. Merge and
    /// flatten are user-action-rate operations, so the per-bake
    /// allocate/release (and the effect-instance rebuild its `targets`
    /// bump triggers) is the right trade against holding the textures for
    /// the rest of the session.
    pub fn bake_subtree_to_layer(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        doc: &Document,
        source_ids: &[LayerId],
        dest_layer_id: LayerId,
    ) {
        if !self.node_textures.contains_key(&dest_layer_id) {
            debug_assert!(false, "bake_subtree_to_layer: dest texture missing");
            return;
        }

        // Sentinel parent id — slotmap's null key never collides with a
        // minted LayerId, so we can stash a transient GroupState here.
        let bake_parent = LayerId::from_ffi(0);
        self.revisions.bump_targets();
        let gs = Self::create_group_state(
            device,
            queue,
            self.canvas_width,
            self.canvas_height,
            self.canvas_origin,
            bake_parent,
        );
        self.group_state.insert(bake_parent, gs);

        let scissor = (0u32, 0u32, self.canvas_width, self.canvas_height);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("bake-subtree"),
        });

        // Clear the bake accum so the composite starts from transparent.
        {
            let gs = self.group_state.get_mut(&bake_parent).unwrap();
            gs.current_accum = 0;
            clear_view_transparent(&mut encoder, &gs.accum.views[0], "clear-bake-accum");
        }

        // Refresh per-host projection uniforms so masked leaves in the baked
        // subtree composite through the same projection path the live render
        // uses. Session isolation must not filter the bake — it represents
        // "what would these layers look like, composited as-is" — so the
        // walk runs with no isolation target.
        self.sync_projection_states(device, queue, doc, None);

        // Composite the sources into the bake accum. `compose_children`
        // handles rasters, groups (recursing through `compose_group` which
        // updates each group's own composite_cache), and passthrough groups.
        self.compose_children(
            &mut encoder,
            device,
            doc,
            bake_parent,
            source_ids,
            scissor,
            None,
        );

        // Copy the final accum into the destination layer's texture.
        let gs = self
            .group_state
            .get(&bake_parent)
            .expect("bake group state allocated above");
        let src_accum = gs.current_accum;
        let dest_tex = &self
            .node_textures
            .get(&dest_layer_id)
            .expect("dest texture presence checked above")
            .texture;
        blit_region(
            &mut encoder,
            &gs.accum.textures[src_accum],
            (0, 0),
            dest_tex.texture(),
            (0, 0),
            self.canvas_width,
            self.canvas_height,
        );

        queue.submit(std::iter::once(encoder.finish()));

        // Release the transient bake state: the GroupState's three
        // canvas-sized textures and every blend bind group the walk cached
        // under the sentinel parent (they reference the textures being
        // dropped). The `targets` bump tells revision-stamped consumers the
        // texture set changed.
        self.group_state.remove(&bake_parent);
        self.blend_bind_groups
            .retain(|(p, _, _), _| *p != bake_parent);
        self.revisions.bump_targets();

        self.mark_node_pixels_dirty(dest_layer_id);
        self.mark_dirty();
    }
}
