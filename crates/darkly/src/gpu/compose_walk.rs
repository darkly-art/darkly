//! The composite walk: the recursive traversal that blends the document's
//! layer tree into group accumulators (group recursion, per-child dispatch,
//! the blend/effect/group compose arms, the de-fused leaf-mask projection,
//! and the in-place apply passes shared by effect layers and masked
//! passthrough groups) plus the per-host [`ProjectionState`] lifecycle and
//! its pre-walk uniform sync. A sibling `impl Compositor` block over
//! `pub(super)` fields, split from `gpu/compositor.rs` the same way Krita
//! separates the merge walk (`kis_async_merger.cpp`) from the image.

use crate::document::Document;
use crate::gpu::blend::BlendPipelines;
use crate::gpu::compositor::{AccumPair, BlendUniforms, Compositor, NodeSlot};
use crate::gpu::{blit_region, clear_view_transparent, create_uniform_buffer};
use crate::layer::{FilterLayer, Layer, LayerId};
use smallvec::SmallVec;
use std::collections::HashMap;

/// Stack-side capacity for the per-frame `children_of(...)` snapshot used by
/// the composite walk. Typical documents have single-digit children per group;
/// wider groups (paste-many-layers, stress tests) spill to the heap without
/// ceremony.
type ChildIds = SmallVec<[LayerId; 8]>;

/// Build a bind group for the blend pipelines' 4-entry group 0 layout:
/// `[before/src texture, after/layer texture, sampler, blend uniforms]`.
/// Specific to that layout (which is why it lives here beside the blend
/// pipelines' owner, not in `gpu/mod.rs`): every blend / in-place-apply
/// draw builds its group 0 through this one function.
pub(super) fn blend_bind_group(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    src_view: &wgpu::TextureView,
    layer_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    uniform: wgpu::BindingResource,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(src_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(layer_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: uniform,
            },
        ],
    })
}

/// Encode one scissored full-screen triangle draw against the blend-family
/// bind-group plan: group 0 = blend bind group, group 1 = mask, group 2 =
/// shared canvas geometry. Every composite-walk draw (layer blend, group
/// blend, projection passes) is exactly this sequence with a different
/// pipeline, target, and load op.
#[allow(clippy::too_many_arguments)]
fn draw_blend_pass(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    target: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
    mask_bg: &wgpu::BindGroup,
    canvas_bg: &wgpu::BindGroup,
    scissor: (u32, u32, u32, u32),
) {
    let (sx, sy, sw, sh) = scissor;
    let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    });
    rpass.set_scissor_rect(sx, sy, sw, sh);
    rpass.set_pipeline(pipeline);
    rpass.set_bind_group(0, bind_group, &[]);
    rpass.set_bind_group(1, mask_bg, &[]);
    rpass.set_bind_group(2, canvas_bg, &[]);
    rpass.draw(0..3, 0..1);
}

/// True if the renderer should descend into / render `id` under the given
/// isolation target. When no target is set, every id qualifies. Otherwise
/// the path is `ancestors(target) ∪ {target} ∪ descendants(target)`:
/// ancestors so the walk reaches the target, descendants so an isolated
/// group renders its contents. Filters naturally fall in via their
/// host (which is the filter's `parent_of`); they have no children, so
/// isolating a filter limits the visible canvas to the host plus the
/// filter itself, which the host's blend pass then renders as
/// grayscale via `sync_compositor_layers` setting `isolated=true`.
///
/// Isolation is session state owned by `engine.isolated_node`; the walk
/// receives it as a per-frame parameter, never stores it.
fn is_in_isolation_path(doc: &Document, isolated: Option<LayerId>, id: LayerId) -> bool {
    let Some(target) = isolated else {
        return true;
    };
    if id == target {
        return true;
    }
    // Is `id` an ancestor of the target?
    let mut cur = doc.parent_of(target);
    while let Some(p) = cur {
        if p == id {
            return true;
        }
        cur = doc.parent_of(p);
    }
    // Is `id` a descendant of the target?
    let mut cur = doc.parent_of(id);
    while let Some(p) = cur {
        if p == target {
            return true;
        }
        cur = doc.parent_of(p);
    }
    false
}

/// The first child index at which two stamp lists disagree, or `None` when
/// they are identical. A length change diverges at the shorter length, since
/// everything from there on is a different walk.
fn first_divergence(cached: &[ChildStamp], fresh: &[ChildStamp]) -> Option<usize> {
    let common = cached.len().min(fresh.len());
    (0..common)
        .find(|&i| cached[i] != fresh[i])
        .or((cached.len() != fresh.len()).then_some(common))
}

/// What the walk's cache remembers about one child of a group, in document
/// order. Plain-equality comparison, no hashing: this decides whether pixels
/// destined for an exported file may be reused, and a hash would trade that
/// certainty for a collision probability.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct ChildStamp {
    id: LayerId,
    /// Whether the walk actually drew this child: the skip chain plus the
    /// node's own [`LayerNode::compose_ready`]. A child that flips between
    /// drawing and not changes the group's output as surely as an edit does.
    included: bool,
    /// Latest content revision anywhere in this child's subtree, filters
    /// included. Zero when `included` is false: an excluded subtree
    /// contributes nothing, and whatever later includes it bumps `document`.
    rev: crate::gpu::revisions::Tick,
}

/// One group's memory of its last walk: what its children were, and how far up
/// the stack a reusable composite was captured.
///
/// Compositor-owned derived state, rebuilt freely, losing it costs one full
/// walk and nothing else.
pub(super) struct WalkCache {
    /// Per-child stamps from the last walk, in document order.
    stamps: Vec<ChildStamp>,
    /// Registry ticks the stamps were taken under. Any drift and no reuse is
    /// attempted at all, which is what makes every coarse `mark_dirty()` in
    /// the codebase safe without auditing it.
    built_document: crate::gpu::revisions::Tick,
    built_targets: crate::gpu::revisions::Tick,
    /// The composite of `children[..=through]`, captured mid-walk.
    prefix: Option<Prefix>,
    /// First-dirty index of the previous walk. The snapshot only advances
    /// when the same depth is dirty twice running, so alternating edit depths
    /// leave the prefix pinned below both instead of thrashing.
    last_d: Option<usize>,
}

/// A captured composite of a group's lower children.
pub(super) struct Prefix {
    texture: wgpu::Texture,
    /// Child index this prefix includes through.
    through: usize,
}

/// Lean per-host projection for a leaf layer (raster / void) that carries a
/// visible mask filter. The host's content composites into this isolated
/// window-sized buffer; the mask modulates it (`apply_mask`); the finished
/// projection blends down onto the parent, so the mask never samples the host
/// layer's texture or geometry. This is the de-fused replacement for the fused
/// mask that used to live inside the layer-blend pass.
///
/// Leaner than a [`GroupState`]: just a ping-pong pair (content → masked) and
/// the three uniform buffers the three passes need. No composite cache, no
/// child caching: a leaf has exactly one piece of content.
pub(super) struct ProjectionState {
    /// `[0]` receives the composited host content; `[1]` receives the
    /// mask-modulated result. (Two textures, not a true ping-pong loop.)
    accum: AccumPair,
    /// Compose-content-into-projection uniform: opacity 1, Normal blend, the
    /// host layer texture's own offset/size (so the content samples cleanly).
    content_uniform_buf: wgpu::Buffer,
    /// Down-composite uniform: the host's opacity + blend mode, canvas-window
    /// geometry (the projection occupies exactly the canvas window in plane).
    down_uniform_buf: wgpu::Buffer,
    /// `apply_mask` uniform: the mask texture's plane offset/size + isolated.
    mask_uniform_buf: wgpu::Buffer,
    /// Dimensions this state was allocated at; rebuilt when the canvas resizes.
    padded_w: u32,
    padded_h: u32,
}

/// Carrier passed to [`LayerNode::compose_into`] so the dispatch hop can
/// reach the compositor and the per-walk parameters without exploding the
/// compositor's private surface. Built once per child by `compose_children`
/// and discarded after the call.
pub struct CompositionContext<'a> {
    pub(super) compositor: &'a mut Compositor,
    pub(super) encoder: &'a mut wgpu::CommandEncoder,
    pub(super) device: &'a wgpu::Device,
    pub(super) doc: &'a Document,
    pub(super) parent_group: LayerId,
    pub(super) scissor: (u32, u32, u32, u32),
    /// The session isolation target (`engine.isolated_node`), threaded
    /// per frame so group recursion keeps filtering off-path subtrees.
    pub(super) isolated: Option<LayerId>,
}

impl<'a> CompositionContext<'a> {
    /// Blend-content hop: composite a layer that contributes its own texture
    /// through the standard blend path. Mirrors the
    /// [`LayerKindGpu::realize_in`] split: the arm bodies live on
    /// [`Compositor`] (where they touch its private fields), and the
    /// dispatch is owned by the variant via [`Layer::compose_into`].
    pub(crate) fn compose_layer(&mut self, layer: &Layer) {
        self.compositor.compose_layer_arm(
            self.encoder,
            self.device,
            self.doc,
            self.parent_group,
            layer,
            self.scissor,
        );
    }

    /// Effect hop: transform the running group accumulator in place rather
    /// than blending a texture in. Reached from [`Layer::compose_into`]'s
    /// filter arm.
    pub(crate) fn compose_effect(&mut self, filter: &FilterLayer) {
        self.compositor.compose_effect_arm(
            self.encoder,
            self.device,
            self.doc,
            self.parent_group,
            filter,
            self.scissor,
        );
    }

    pub(crate) fn compose_group(&mut self, group: &crate::layer::LayerGroup) {
        self.compositor.compose_group_arm(
            self.encoder,
            self.device,
            self.doc,
            self.parent_group,
            group,
            self.scissor,
            self.isolated,
        );
    }
}

impl Compositor {
    /// Effective mask bind group for a host raster/group during compositing
    ///: substitutes the preview-mask bind group when one of the host's
    /// filters is the floating target. Fall-through resolves the live
    /// mask through the existing `mask_bind_group` lookup.
    pub(crate) fn effective_mask_bind_group(
        &self,
        doc: &Document,
        host_id: LayerId,
    ) -> &wgpu::BindGroup {
        Self::effective_mask_bind_group_fields(
            &self.node_textures,
            &self.default_mask_bind_group,
            self.transform_session.as_ref(),
            self.transform_pass.paste.as_ref(),
            doc,
            host_id,
        )
    }

    /// Field-explicit variant of [`Self::effective_mask_bind_group`] so a
    /// caller can hold a disjoint `&mut self.blend_bind_groups` borrow
    /// across this lookup. The method-form takes `&self` whole; the
    /// borrow checker can't split that.
    fn effective_mask_bind_group_fields<'a>(
        node_textures: &'a HashMap<LayerId, NodeSlot>,
        default_mask_bind_group: &'a wgpu::BindGroup,
        transform_session: Option<&'a crate::gpu::floating_preview::TransformGpuSession>,
        paste: Option<&'a crate::gpu::transform::TransformState>,
        doc: &Document,
        host_id: LayerId,
    ) -> &'a wgpu::BindGroup {
        let live_or_default = doc
            .visible_mask_of(host_id)
            .and_then(|m| node_textures.get(&m).and_then(|s| s.mask_bg.as_ref()))
            .unwrap_or(default_mask_bind_group);

        let preview = transform_session
            .filter(|session| session.published_preview_revision > 0)
            .and_then(|session| {
                doc.mask_filter(host_id)
                    .and_then(|mask| session.target(mask.id))
            })
            .or_else(|| paste.filter(|state| doc.parent_of(state.target_layer) == Some(host_id)));
        preview
            .and_then(|state| state.preview_mask_bind_group.as_ref())
            .unwrap_or(live_or_default)
    }

    /// Create a dynamic blend bind group for compositing a layer into a group.
    fn create_blend_bind_group(
        &self,
        device: &wgpu::Device,
        bg_view: &wgpu::TextureView,
        layer_view: &wgpu::TextureView,
        uniform_buf: &wgpu::Buffer,
        label: &str,
    ) -> wgpu::BindGroup {
        blend_bind_group(
            device,
            label,
            &self.blend_pipelines.bind_group_layout,
            bg_view,
            layer_view,
            &self.sampler,
            uniform_buf.as_entire_binding(),
        )
    }

    /// Cached entry point for `create_blend_bind_group`. The key
    /// `(parent_group, child, halves)` uniquely identifies the bg+layer
    /// view pair for a given composite: view handles and uniform buffers
    /// are stable across frames, so caching by key avoids the per-frame
    /// allocator round-trip. `halves` is bit 0 = the parent's source
    /// accumulator index, bit 1 = the child group's output half (always 0
    /// for a leaf, whose texture does not ping-pong). Caller is responsible
    /// for bypassing the cache when the inputs are not stable (e.g.
    /// floating-target preview swap).
    ///
    /// Takes the cache field directly rather than `&mut self` so the caller
    /// can keep other immutable field borrows live across this call. Returns
    /// a borrow into the cache, valid until the cache is mutated again.
    fn get_or_create_blend_bind_group<'a>(
        blend_bind_groups: &'a mut HashMap<(LayerId, LayerId, u8), wgpu::BindGroup>,
        bgl: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        device: &wgpu::Device,
        key: (LayerId, LayerId, u8),
        bg_view: &wgpu::TextureView,
        layer_view: &wgpu::TextureView,
        uniform_buf: &wgpu::Buffer,
        label: &str,
    ) -> &'a wgpu::BindGroup {
        blend_bind_groups.entry(key).or_insert_with(|| {
            blend_bind_group(
                device,
                label,
                bgl,
                bg_view,
                layer_view,
                sampler,
                uniform_buf.as_entire_binding(),
            )
        })
    }

    /// Recursively composite a group's children into its GroupState.
    ///
    /// For passthrough groups, children are inlined into the parent's accum
    /// (same as the old flat loop). For normal groups, children composite
    /// into the group's own accum pair, then the result is blended into the
    /// parent.
    pub(super) fn compose_group(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        doc: &Document,
        group_id: LayerId,
        scissor: (u32, u32, u32, u32),
        isolated: Option<LayerId>,
    ) {
        // Clone the child ids so the borrow on `doc` doesn't outlive the call
        // into `compose_children`, which itself re-borrows `doc`. `SmallVec`
        // absorbs the typical single-digit-children case on the stack.
        let children: ChildIds = ChildIds::from_slice(doc.children_of(group_id));
        let fresh = self.build_child_stamps(doc, &children, isolated);

        // Reuse is gated on the coarse sources first. Every `mark_dirty()` in
        // the codebase bumps `document`, so a mutation nobody narrowed
        // invalidates every walk cache without anyone auditing the call site:
        // forgetting to narrow costs a full walk, never a stale pixel.
        let valid = self.walk_cache_valid(group_id) && !self.histogram_forces_full_walk();
        // Read everything the decision needs out of the cache before touching
        // it, so the branches below are free to mutate the group state.
        let (had_cache, d, prefix, last_d) = match self
            .group_state
            .get(&group_id)
            .and_then(|gs| gs.walk_cache.as_ref())
        {
            Some(c) => (
                true,
                first_divergence(&c.stamps, &fresh),
                c.prefix.as_ref().map(|p| (p.through, p.texture.clone())),
                c.last_d,
            ),
            None => (false, None, None, None),
        };

        // Nothing below this group changed: its accumulators still hold the
        // answer, so the walk draws nothing at all. Krita's `N_BELOW_FILTHY`
        // "nothing to do", reached from the far side.
        if valid && had_cache && d.is_none() {
            #[cfg(any(test, feature = "testing"))]
            {
                self.walk_all_clean += 1;
            }
            self.record_walk_cache(group_id, fresh, None);
            return;
        }

        // Everything below `start` is already in accumulator half 0 once the
        // prefix is restored; `None` means compose from a cleared one.
        let resume = match (valid, d, prefix) {
            (true, Some(d), Some((through, texture))) if through < d => {
                Some((through + 1, texture))
            }
            _ => None,
        };

        let start = match resume {
            Some((start, ref prefix_texture)) => {
                // The restore replaces the clear: the prefix already holds the
                // composite of every child below `start`.
                let gs = self
                    .group_state
                    .get_mut(&group_id)
                    .expect("GroupState missing");
                gs.current_accum = 0;
                blit_region(
                    encoder,
                    prefix_texture,
                    (0, 0),
                    &gs.accum.textures[0],
                    (0, 0),
                    gs.accum.textures[0].width(),
                    gs.accum.textures[0].height(),
                );
                #[cfg(any(test, feature = "testing"))]
                {
                    self.walk_resumes += 1;
                }
                start
            }
            None => {
                // A full walk: reset the accumulator, and drop any prefix. A
                // walk reached here either because a coarse source moved
                // (which can change output below any stamp) or because the
                // stamps themselves diverged low. Keeping the old capture
                // across that would let a later resume restore pre-change
                // pixels.
                if let Some(gs) = self.group_state.get_mut(&group_id) {
                    gs.current_accum = 0;
                    if let Some(cache) = gs.walk_cache.as_mut() {
                        cache.prefix = None;
                    }
                    clear_view_transparent(encoder, &gs.accum.views[0], "clear-accum");
                }
                0
            }
        };

        // A snapshot is only worth taking where the walk keeps splitting: the
        // same depth dirty twice running. An alternating pair of edit depths
        // leaves the prefix pinned below both rather than thrashing between
        // them, and a converged prefix re-captures nothing.
        let snapshot_at = d.filter(|&d| d >= 1 && d > start).and_then(|d| {
            // The same depth dirty twice running: worth capturing below it.
            let stable = last_d == Some(d);
            // A full walk that still trusted its stamps establishes the first
            // capture. A walk whose stamps were *not* trusted must not: its
            // `d` was computed against a comparison a coarse bump already
            // invalidated, so it names the wrong split.
            let first_capture = resume.is_none() && valid;
            (stable || first_capture).then_some(d - 1)
        });

        self.compose_children(
            encoder,
            device,
            doc,
            group_id,
            &children[start..],
            scissor,
            isolated,
            snapshot_at.map(|through| (through - start, through)),
        );

        self.record_walk_cache(group_id, fresh, d);

        // No copy out: the group's output *is* the half the walk ended on,
        // named by `GroupState::output_index`. Consumers select a resource by
        // that index instead of reading one fixed view.
    }

    /// Whether this group's cache was taken under the same coarse sources
    /// that are in force now.
    fn walk_cache_valid(&self, group_id: LayerId) -> bool {
        self.group_state
            .get(&group_id)
            .and_then(|gs| gs.walk_cache.as_ref())
            .is_some_and(|c| {
                c.built_document == self.revisions.document()
                    && c.built_targets == self.revisions.targets()
            })
    }

    /// While a histogram is owed, no group may reuse anything.
    ///
    /// The dispatch that satisfies it happens inside `compose_effect_arm`,
    /// mid-walk, against the effect's live input. Any skipped subtree between
    /// the root and that effect makes the dispatch unreachable and
    /// `pump_node_histogram` waits forever, and the guard cannot be scoped to
    /// the host group, because an ancestor's own reuse is what skips it. A
    /// histogram is owed only while a filter's panel is focused and its result
    /// is one readback away, so refusing reuse outright for that window costs
    /// less than the machinery to be precise about it.
    fn histogram_forces_full_walk(&self) -> bool {
        self.histogram_target
            .is_some_and(|t| self.histogram.needs(&self.revisions, t))
    }

    /// Stamp every child of a group against the state this walk will see.
    fn build_child_stamps(
        &self,
        doc: &Document,
        children: &[LayerId],
        isolated: Option<LayerId>,
    ) -> Vec<ChildStamp> {
        let screen_run = doc.screen_space_run();
        children
            .iter()
            .map(|&id| {
                let included = self.child_included(doc, id, screen_run, isolated);
                ChildStamp {
                    id,
                    included,
                    rev: if included {
                        self.subtree_revision(doc, id, screen_run, isolated)
                    } else {
                        0
                    },
                }
            })
            .collect()
    }

    /// Latest content revision anywhere in a child's subtree.
    ///
    /// Folds the node's own pixel and animation ticks together with those of
    /// its filters (a mask is not in `children_of`, so it has to be reached
    /// explicitly) and recurses through descendants. A passthrough group's
    /// descendants are folded through the same inclusion predicate the walk
    /// applies to them, because they inline into *this* group's accumulator:
    /// an inner child that starts or stops drawing changes this group's output
    /// exactly as a direct child would.
    fn subtree_revision(
        &self,
        doc: &Document,
        id: LayerId,
        screen_run: &[LayerId],
        isolated: Option<LayerId>,
    ) -> crate::gpu::revisions::Tick {
        let mut rev = self
            .revisions
            .node_pixels(id)
            .max(self.revisions.animation(id));
        for &filter in doc.filters_of(id) {
            rev = rev
                .max(self.revisions.node_pixels(filter))
                .max(self.revisions.animation(filter));
        }
        for &child in doc.children_of(id) {
            // An inner child's inclusion is part of what this subtree
            // contributes, so a flip in it must move the fold.
            if !self.child_included(doc, child, screen_run, isolated) {
                continue;
            }
            rev = rev.max(self.subtree_revision(doc, child, screen_run, isolated));
        }
        rev
    }

    /// Store what this walk saw, so the next one can compare against it.
    fn record_walk_cache(
        &mut self,
        group_id: LayerId,
        stamps: Vec<ChildStamp>,
        last_d: Option<usize>,
    ) {
        let (document, targets) = (self.revisions.document(), self.revisions.targets());
        let Some(gs) = self.group_state.get_mut(&group_id) else {
            return;
        };
        match gs.walk_cache.as_mut() {
            Some(cache) => {
                cache.stamps = stamps;
                cache.built_document = document;
                cache.built_targets = targets;
                cache.last_d = last_d;
            }
            None => {
                gs.walk_cache = Some(WalkCache {
                    stamps,
                    built_document: document,
                    built_targets: targets,
                    prefix: None,
                    last_d,
                })
            }
        }
    }

    /// Whether the walk will draw `child_id` into its parent this composite.
    ///
    /// The single answer consumed by both the walk (which children to
    /// dispatch) and the walk cache (which children a stamp describes). One
    /// function rather than two copies of the same chain: a drift between them
    /// would mean a child the cache believes contributes nothing while the
    /// walk draws it, which is exactly how a reuse goes stale.
    fn child_included(
        &self,
        doc: &Document,
        child_id: LayerId,
        screen_run: &[LayerId],
        isolated: Option<LayerId>,
    ) -> bool {
        let Some(node) = doc.find_node(child_id) else {
            return false;
        };
        // Isolation and visibility are orthogonal: the document's eye state
        // is never inspected beyond `visible()`, and isolation never mutates
        // it.
        node.visible()
            && is_in_isolation_path(doc, isolated, child_id)
            // Screen-space members are realized after the present pass, on the
            // view-transformed image, so the canvas-space walk must not draw
            // them. Export, flatten and merge composite through this same
            // walk, which is what makes "viewport only" mean "not in the file"
            // with no code of their own.
            && !screen_run.contains(&child_id)
            // …and the node's own answer about whether its arm can draw.
            && node.compose_ready(self)
    }

    /// Composite a list of children into the parent group's accumulators.
    /// Handles passthrough groups by recursing with the same parent group_id.
    ///
    /// Per-child dispatch goes through [`LayerNode::compose_into`] so each
    /// node variant is responsible for its own compose behaviour: this
    /// walk only owns the per-child filters that are orthogonal to node kind,
    /// through [`Self::child_included`].
    ///
    /// `snapshot_after` is `(position in `children`, that child's index among
    /// the group's full child list)`: the walk sees a suffix slice when it
    /// resumes, while the cache records absolute indices.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn compose_children(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        doc: &Document,
        parent_group: LayerId,
        children: &[LayerId],
        scissor: (u32, u32, u32, u32),
        isolated: Option<LayerId>,
        snapshot_after: Option<(usize, usize)>,
    ) {
        // Resolved once per group rather than per child: the run is a slice of
        // the root's children, so for a nested group this is an empty-match
        // scan.
        let screen_run = doc.screen_space_run();
        for (position, &child_id) in children.iter().enumerate() {
            if self.child_included(doc, child_id, screen_run, isolated) {
                if let Some(node) = doc.find_node(child_id) {
                    let mut ctx = CompositionContext {
                        compositor: self,
                        encoder,
                        device,
                        doc,
                        parent_group,
                        scissor,
                        isolated,
                    };
                    node.compose_into(&mut ctx);
                }
            }
            // Capture is keyed to the loop position, not to a dispatch: a
            // child the walk skipped leaves the accumulator untouched, so the
            // content through this position is the same either way.
            if let Some((at, through)) = snapshot_after {
                if at == position {
                    self.capture_prefix(encoder, device, parent_group, through);
                }
            }
        }
    }

    /// Copy the running accumulator into this group's prefix texture, marking
    /// it as holding the composite through child index `through`.
    ///
    /// Allocated lazily and only for groups that actually split, so a
    /// document that never resumes pays nothing. Sized like the accumulators,
    /// because that is what it stands in for.
    fn capture_prefix(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        group_id: LayerId,
        through: usize,
    ) {
        let Some(gs) = self.group_state.get(&group_id) else {
            return;
        };
        let (w, h) = {
            let t = &gs.accum.textures[0];
            (t.width(), t.height())
        };
        let fits = gs
            .walk_cache
            .as_ref()
            .and_then(|c| c.prefix.as_ref())
            .is_some_and(|p| p.texture.width() == w && p.texture.height() == h);
        if !fits {
            let (texture, _view) =
                Self::make_accum_texture(device, w, h, &format!("walk-prefix-{group_id:?}"));
            let Some(gs) = self.group_state.get_mut(&group_id) else {
                return;
            };
            let Some(cache) = gs.walk_cache.as_mut() else {
                return;
            };
            cache.prefix = Some(Prefix {
                texture,
                through: 0,
            });
        }

        let Some(gs) = self.group_state.get_mut(&group_id) else {
            return;
        };
        let src = gs.current_accum;
        let Some(cache) = gs.walk_cache.as_mut() else {
            return;
        };
        let Some(prefix) = cache.prefix.as_mut() else {
            return;
        };
        prefix.through = through;
        blit_region(
            encoder,
            &gs.accum.textures[src],
            (0, 0),
            &prefix.texture,
            (0, 0),
            w,
            h,
        );
    }

    /// During an active transform, the roles a masked host's preview can play:
    /// `(layer_transforming, mask_transforming)`. Both may be true for one
    /// published linked-transform revision.
    fn projection_transform_roles(&self, host_id: LayerId, mask_id: LayerId) -> (bool, bool) {
        (
            self.transform_preview_target(host_id).is_some(),
            self.transform_preview_target(mask_id).is_some(),
        )
    }

    /// Allocate a [`ProjectionState`] for `host_id` at the given dimensions,
    /// reusing an existing one if it already matches (pooling). Released by
    /// [`Self::dispose_projection_state`] on mask remove/hide / host delete and
    /// cleared wholesale on canvas resize.
    fn ensure_projection_state(
        &mut self,
        device: &wgpu::Device,
        host_id: LayerId,
        padded_w: u32,
        padded_h: u32,
    ) {
        let fits = self
            .projection_states
            .get(&host_id)
            .is_some_and(|ps| ps.padded_w == padded_w && ps.padded_h == padded_h);
        if fits {
            return;
        }
        let ps = Self::create_projection_state(device, padded_w, padded_h, host_id);
        self.projection_states.insert(host_id, ps);
    }

    fn create_projection_state(
        device: &wgpu::Device,
        padded_w: u32,
        padded_h: u32,
        host_id: LayerId,
    ) -> ProjectionState {
        let (a0, v0) =
            Self::make_accum_texture(device, padded_w, padded_h, &format!("proj-{host_id:?}-0"));
        let (a1, v1) =
            Self::make_accum_texture(device, padded_w, padded_h, &format!("proj-{host_id:?}-1"));
        ProjectionState {
            accum: AccumPair {
                textures: [a0, a1],
                views: [v0, v1],
            },
            content_uniform_buf: create_uniform_buffer::<BlendUniforms>(
                device,
                "proj-content-uniform",
            ),
            down_uniform_buf: create_uniform_buffer::<BlendUniforms>(device, "proj-down-uniform"),
            mask_uniform_buf: create_uniform_buffer::<crate::gpu::apply_mask::MaskUniform>(
                device,
                "proj-mask-uniform",
            ),
            padded_w,
            padded_h,
        }
    }

    /// Release a host's pooled projection state. Called on mask remove/hide and
    /// host delete; idempotent.
    pub fn dispose_projection_state(&mut self, host_id: LayerId) {
        self.projection_states.remove(&host_id);
    }

    /// Pre-walk pass (has `queue`): ensure a projection state exists and its
    /// three uniform buffers are current for every leaf host that needs one,
    /// and drop states whose host no longer qualifies (mask hidden/removed).
    /// The compose walk that follows only binds: it never allocates or writes.
    pub(super) fn sync_projection_states(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        doc: &Document,
        isolated: Option<LayerId>,
    ) {
        let padded_w = self.canvas_width;
        let padded_h = self.canvas_height;

        // Drop projections whose host no longer needs one (mask hidden/removed
        // or now transform-previewing): the "released on mask remove/hide"
        // requirement, plus the host-delete case (the layer_cache entry is
        // gone so it never re-qualifies below).
        let stale: Vec<LayerId> = self
            .projection_states
            .keys()
            .copied()
            .filter(|h| doc.visible_mask_of(*h).is_none())
            .collect();
        for h in stale {
            self.projection_states.remove(&h);
        }

        let normal = crate::gpu::blend_mode::registry().default().gpu_value;
        let canvas_size = [padded_w as f32, padded_h as f32];
        let host_ids: Vec<LayerId> = self.layer_cache.keys().copied().collect();
        for host_id in host_ids {
            let mask_id = match doc.visible_mask_of(host_id) {
                Some(m) => m,
                None => continue,
            };
            let layer_ext = match self.node_textures.get(&host_id) {
                Some(s) => s.texture.canvas_extent(),
                None => continue,
            };
            let mask_ext = match self.node_textures.get(&mask_id) {
                Some(s) => s.texture.canvas_extent(),
                None => continue,
            };
            let (opacity, blend_mode) = {
                let u = &self.layer_cache[&host_id].last_uniforms;
                (u.opacity, u.blend_mode)
            };
            let mask_isolated = isolated == Some(mask_id);
            let (layer_transforming, mask_transforming) =
                self.projection_transform_roles(host_id, mask_id);
            let canvas_origin = [self.canvas_origin.x as f32, self.canvas_origin.y as f32];

            self.ensure_projection_state(device, host_id, padded_w, padded_h);

            // Compose-content uniform: straight host content (opacity 1, Normal).
            // When the *layer* is transform-previewing, the content comes from
            // the canvas-aligned preview texture, so it samples at the canvas
            // window; otherwise it samples the live layer in its own frame.
            let content_rect = if layer_transforming {
                self.canvas_rect()
            } else {
                layer_ext
            };
            let content = BlendUniforms::for_extent(1.0, normal, false, content_rect);
            // Down-composite uniform: the host's opacity + blend mode, canvas-
            // window geometry (the projection fills exactly the canvas window).
            let down = BlendUniforms::for_extent(opacity, blend_mode, false, self.canvas_rect());
            // Mask uniform: when the *mask* is transform-previewing, the preview
            // mask is a canvas-aligned R8 texture, so it samples at the canvas
            // window; otherwise the live mask samples in its own frame.
            let (mask_offset, mask_size) = if mask_transforming {
                (canvas_origin, canvas_size)
            } else {
                (
                    [mask_ext.x0() as f32, mask_ext.y0() as f32],
                    [mask_ext.width as f32, mask_ext.height as f32],
                )
            };
            let mu = crate::gpu::apply_mask::MaskUniform {
                mask_offset,
                mask_size,
                isolated: mask_isolated as u32,
                _pad0: 0,
                _pad1: 0,
                _pad2: 0,
            };
            let ps = &self.projection_states[&host_id];
            queue.write_buffer(&ps.content_uniform_buf, 0, bytemuck::bytes_of(&content));
            queue.write_buffer(&ps.down_uniform_buf, 0, bytemuck::bytes_of(&down));
            queue.write_buffer(&ps.mask_uniform_buf, 0, bytemuck::bytes_of(&mu));
        }

        // (Re-)ensure a snapshot exists for every masked passthrough group. The
        // snapshot is parent-accumulator-sized, so `set_canvas_rect` drops it
        // on a crop/resize; without this re-ensure the mask would silently
        // disable after a crop (compose falls back to the unmasked path).
        for host_id in doc.snapshot_in_place_hosts() {
            self.ensure_mask_snapshot_state(device, host_id);
        }

        self.sync_effect_instances(device, queue, doc, isolated);

        // Refresh every masked passthrough host's apply uniform: canvas + mask
        // geometry. Effect layers' uniforms are written by
        // `sync_effect_instances` itself, beside the instances they belong to.
        let normal = crate::gpu::blend_mode::registry().default().gpu_value;
        let hosts: Vec<(LayerId, wgpu::Buffer)> = self
            .mask_snapshot_state
            .iter()
            .map(|(id, pms)| (*id, pms.uniform_buf.clone()))
            .collect();

        for (host_id, buf) in hosts {
            let uniforms =
                self.apply_uniforms_for(doc, host_id, normal, 1.0, canvas_size, isolated);
            queue.write_buffer(&buf, 0, bytemuck::bytes_of(&uniforms));
        }
    }

    /// De-fused leaf-mask compose: host content → projection, mask modulates
    /// the projection (`apply_mask`), projection blends down onto the parent.
    /// The mask samples only `(projection, mask)` in its own space: never the
    /// host layer texture or geometry. Uniforms are pre-written by
    /// [`Self::sync_projection_states`]; this only encodes the three passes.
    fn compose_layer_through_projection(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        parent_group: LayerId,
        host_id: LayerId,
        mask_id: LayerId,
        scissor: (u32, u32, u32, u32),
    ) {
        // Advance the parent ping-pong up front so the rest can borrow `&self`.
        let (parent_src, parent_dst) = {
            let gsp = self.group_state.get_mut(&parent_group).unwrap();
            let src = gsp.current_accum;
            let dst = 1 - src;
            gsp.current_accum = dst;
            (src, dst)
        };

        let ps = match self.projection_states.get(&host_id) {
            Some(p) => p,
            None => return,
        };

        // Transform-preview swap: sample the (canvas-aligned) preview layer when
        // the layer is being dragged, and the preview mask when the mask is.
        // Geometry for these is set to match in `sync_projection_states`.
        let (layer_transforming, mask_transforming) =
            self.projection_transform_roles(host_id, mask_id);
        let layer_preview = self.transform_preview_target(host_id);
        let mask_preview = self.transform_preview_target(mask_id);

        let layer_view = if layer_transforming {
            match layer_preview {
                Some(s) => &s.preview_view,
                None => return,
            }
        } else {
            match self.node_textures.get(&host_id) {
                Some(s) => s.texture.view(),
                None => return,
            }
        };
        let live_mask_bg = self
            .node_textures
            .get(&mask_id)
            .and_then(|s| s.mask_bg.as_ref())
            .unwrap_or(&self.default_mask_bind_group);
        let mask_bg = if mask_transforming {
            mask_preview
                .and_then(|s| s.preview_mask_bind_group.as_ref())
                .unwrap_or(live_mask_bg)
        } else {
            live_mask_bg
        };

        // Build the (non-hot) bind groups fresh: masked leaves are rare.
        // Pass 1 reads the cleared accum[1] as a transparent background.
        let content_bg = self.create_blend_bind_group(
            device,
            &ps.accum.views[1],
            layer_view,
            &ps.content_uniform_buf,
            "proj-content",
        );
        let apply_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("proj-apply-mask-bg"),
            layout: &self.apply_mask_pipeline.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&ps.accum.views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: ps.mask_uniform_buf.as_entire_binding(),
                },
            ],
        });
        let down_bg = self.create_blend_bind_group(
            device,
            &self.group_state[&parent_group].accum.views[parent_src],
            &ps.accum.views[1],
            &ps.down_uniform_buf,
            "proj-down",
        );

        // Pass 0: clear accum[1] (the transparent background for pass 1).
        clear_view_transparent(encoder, &ps.accum.views[1], "proj-clear");

        // Pass 1: composite straight host content into accum[0].
        draw_blend_pass(
            encoder,
            "proj-content",
            &ps.accum.views[0],
            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            self.blend_pipelines.pipeline(),
            &content_bg,
            &self.default_mask_bind_group,
            &self.canvas_bind_group,
            scissor,
        );

        // Pass 2: modulate the projection's alpha by the mask → accum[1].
        draw_blend_pass(
            encoder,
            "proj-apply-mask",
            &ps.accum.views[1],
            wgpu::LoadOp::Load,
            self.apply_mask_pipeline.pipeline(),
            &apply_bg,
            mask_bg,
            &self.canvas_bind_group,
            scissor,
        );

        // Pass 3: blend the masked projection down onto the parent accum.
        draw_blend_pass(
            encoder,
            "proj-down",
            &self.group_state[&parent_group].accum.views[parent_dst],
            wgpu::LoadOp::Load,
            self.blend_pipelines.pipeline(),
            &down_bg,
            &self.default_mask_bind_group,
            &self.canvas_bind_group,
            scissor,
        );
    }

    /// Composite a content layer (raster or void) into its parent group's
    /// ping-pong accumulators. One blend arm for both raster and procedural
    /// content: the procedural texture lives in `node_textures` keyed by
    /// layer id (allocated by `ensure_void_layer` and refreshed by
    /// `encode_dirty_layer_content` before the tree walk), and the blend
    /// uniforms are the same `BlendUniforms` shape in the unified
    /// `layer_cache`, so neither lookup branches on kind here.
    fn compose_layer_arm(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        doc: &Document,
        parent_group: LayerId,
        layer: &Layer,
        scissor: (u32, u32, u32, u32),
    ) {
        let layer_id = layer.id();

        // De-fused leaf mask: a host carrying a visible mask composites through
        // its own projection (content → apply_mask → blend down) so the mask
        // never samples the host's texture or geometry, including while a
        // transform preview is active on the host or its mask, in which case
        // the projection swaps in the preview content/mask (see
        // `compose_layer_through_projection`). The fused path below runs only
        // for unmasked hosts (default white mask).
        if let Some(mask_id) = doc.visible_mask_of(layer_id) {
            self.compose_layer_through_projection(
                encoder,
                device,
                parent_group,
                layer_id,
                mask_id,
                scissor,
            );
            return;
        }

        // Effective view + uniforms: when this layer is the floating
        // target, swap the live texture view for the (canvas-aligned)
        // preview view AND swap the live's layer-aligned blend uniforms
        // for the preview's canvas-aligned ones; both halves must move
        // together or the shader maps fragments to the wrong region.
        // Voids never become floating targets today, so the detour
        // collapses to the live path for them; if voids ever do, the same
        // code path will Just Work.
        let active_floating = self
            .transform_session
            .as_ref()
            .filter(|session| session.published_preview_revision > 0)
            .and_then(|session| session.target(layer_id))
            .or_else(|| {
                self.transform_pass
                    .paste
                    .as_ref()
                    .filter(|state| state.target_layer == layer_id)
            });
        let layer_view = match active_floating {
            Some(s) => &s.preview_view,
            None => match self.node_textures.get(&layer_id) {
                Some(slot) => slot.texture.view(),
                None => return,
            },
        };
        let uniform_buf_ptr = match active_floating {
            Some(s) => &s.preview_blend_uniform_buf,
            None => match self.layer_cache.get(&layer_id) {
                Some(c) => &c.uniform_buf,
                None => return,
            },
        };

        // Ping-pong: read from current accum, write to the other.
        let gs = self.group_state.get_mut(&parent_group).unwrap();
        let src = gs.current_accum;
        let dst = 1 - src;
        gs.current_accum = dst;

        // Floating target swaps the bg/layer view per-frame to the
        // (canvas-aligned) preview texture; skip the cache so preview-state
        // ephemera never leak in. The non-floating path uses stable
        // view+uniform handles so the cache key `(parent, child, src)` is
        // sufficient.
        let fresh_bind_group: Option<wgpu::BindGroup>;
        let cached_bind_group: Option<&wgpu::BindGroup>;
        if active_floating.is_some() {
            fresh_bind_group = Some(self.create_blend_bind_group(
                device,
                &self.group_state[&parent_group].accum.views[src],
                layer_view,
                uniform_buf_ptr,
                "blend-layer",
            ));
            cached_bind_group = None;
        } else {
            let bg_view = &self.group_state[&parent_group].accum.views[src];
            let bgl = &self.blend_pipelines.bind_group_layout;
            let sampler = &self.sampler;
            cached_bind_group = Some(Self::get_or_create_blend_bind_group(
                &mut self.blend_bind_groups,
                bgl,
                sampler,
                device,
                (parent_group, layer_id, src as u8),
                bg_view,
                layer_view,
                uniform_buf_ptr,
                "blend-layer",
            ));
            fresh_bind_group = None;
        }
        let bind_group: &wgpu::BindGroup = cached_bind_group
            .unwrap_or_else(|| fresh_bind_group.as_ref().expect("one branch sets it"));

        let gs = &self.group_state[&parent_group];
        let mask_bg = Self::effective_mask_bind_group_fields(
            &self.node_textures,
            &self.default_mask_bind_group,
            self.transform_session.as_ref(),
            self.transform_pass.paste.as_ref(),
            doc,
            layer_id,
        );
        draw_blend_pass(
            encoder,
            "blend-layer",
            &gs.accum.views[dst],
            wgpu::LoadOp::Load,
            self.blend_pipelines.pipeline(),
            bind_group,
            mask_bg,
            &self.canvas_bind_group,
            scissor,
        );
    }

    /// Compose an effect layer: transform the running group accumulator in
    /// place rather than blending a layer in.
    ///
    /// Because the child walk composites bottom-to-top, `gs.current_accum`
    /// already holds the composite of everything below this effect: lower
    /// siblings plus everything beneath the group, since a passthrough group
    /// inlines into its nearest isolated ancestor's accumulator. That image is
    /// both the effect's input and the "before" its result is applied over:
    ///
    /// ```text
    /// effect:  views[src] ─────────────▶ scratch
    /// apply:  (views[src], scratch) ───▶ views[dst]
    /// ```
    ///
    /// The scratch is what gives the apply pass somewhere to write while still
    /// reading both images. It replaces the accumulator snapshot this path used
    /// to take when masked: same texture count, one fewer full-canvas copy per
    /// effect per frame, and it does not depend on the layer being masked, so
    /// opacity and blend mode work whether or not a mask is present.
    fn compose_effect_arm(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        doc: &Document,
        parent_group: LayerId,
        filter: &FilterLayer,
        scissor: (u32, u32, u32, u32),
    ) {
        // An effect layer with no realized instance (an unknown pipeline id,
        // or a parent whose accumulator did not exist at sync time) composes
        // as a no-op rather than erroring mid-frame.
        if !self.effect_instances.contains_key(&filter.id) {
            return;
        }
        let Some(scratch_view) = self.canvas_apply_scratch.as_ref().map(|(_, v)| v) else {
            return;
        };

        // Ping-pong advance: src = last-written accum, dst = the other half.
        let (src, dst) = {
            let Some(gs) = self.group_state.get_mut(&parent_group) else {
                return;
            };
            let src = gs.current_accum;
            let dst = 1 - src;
            gs.current_accum = dst;
            (src, dst)
        };

        // Bin the effect's *input* (the composite of everything below it, in
        // `src`) into the per-channel histogram when this is the target layer.
        // `src` is only valid mid-composite, so this records into the compose
        // encoder rather than a self-submitted one. Disjoint field borrows keep
        // `group_state` and `histogram` independent.
        if self.histogram_target == Some(filter.id)
            && self.histogram.needs(&self.revisions, filter.id)
        {
            if let Some(gs) = self.group_state.get(&parent_group) {
                let view = &gs.accum.views[src];
                let tex = &gs.accum.textures[src];
                let (w, h) = (tex.width(), tex.height());
                self.histogram
                    .dispatch(device, encoder, &self.revisions, view, w, h, filter.id);
            }
        }

        {
            let inst = &self.effect_instances[&filter.id];
            inst.scaled.encode(
                encoder,
                &*inst.effect,
                &inst.cache,
                &self.canvas_scaling_pipelines,
                src,
                scratch_view,
            );
        }

        let before = {
            let gs = &self.group_state[&parent_group];
            gs.accum.views[src].clone()
        };
        let after = self
            .canvas_apply_scratch
            .as_ref()
            .expect("checked above")
            .1
            .clone();
        let uniform = self.effect_instances[&filter.id]
            .apply_uniform
            .as_entire_binding();
        self.apply_in_place(
            encoder,
            device,
            doc,
            parent_group,
            filter.id,
            &before,
            &after,
            uniform,
            dst,
            scissor,
        );
    }

    /// Snapshot the current parent accumulator into the host's mask-snapshot
    /// texture (the "before" image of the lerp). Step 1 of every in-place
    /// masked composite: shared by the masked passthrough group and the masked
    /// filter layer. The snapshot state must already exist (ensure-driven per
    /// frame); the caller checks before invoking.
    fn snapshot_parent_accum(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        parent_group: LayerId,
        host_id: LayerId,
        scissor: (u32, u32, u32, u32),
    ) {
        let (scissor_x, scissor_y, scissor_w, scissor_h) = scissor;
        let gs = self
            .group_state
            .get(&parent_group)
            .expect("parent GroupState missing");
        let before_idx = gs.current_accum;
        let pms = &self.mask_snapshot_state[&host_id];
        blit_region(
            encoder,
            &gs.accum.textures[before_idx],
            (scissor_x, scissor_y),
            &pms.snapshot,
            (scissor_x, scissor_y),
            scissor_w,
            scissor_h,
        );
    }

    /// The one pass that lands an in-place transform back into the accumulator
    /// it came from: `mix(before, blend(after, before), opacity * mask)` into
    /// `views[dst]`.
    ///
    /// `before` and `after` are just two bound views, which is what lets the
    /// same pass serve both in-place hosts. An effect layer supplies its input
    /// half and its scratch output; a masked passthrough group supplies its
    /// accumulator snapshot and the accumulator its children just wrote. The
    /// mask samples in its own plane space via `sample_mask_window`, and honours
    /// an in-flight transform preview.
    ///
    /// The caller has already advanced `current_accum` to `dst`, because only
    /// the caller knows how many halves its own passes consumed.
    #[allow(clippy::too_many_arguments)]
    fn apply_in_place(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        doc: &Document,
        parent_group: LayerId,
        host_id: LayerId,
        before: &wgpu::TextureView,
        after: &wgpu::TextureView,
        uniform: wgpu::BindingResource,
        dst: usize,
        scissor: (u32, u32, u32, u32),
    ) {
        // Effective mask: live by default, preview-mask when the floating
        // target is this host's mask filter.
        let mask_bg = self.effective_mask_bind_group(doc, host_id);
        let gs = &self.group_state[&parent_group];
        Self::encode_in_place_apply(
            &self.blend_pipelines,
            &self.in_place_apply_pipelines,
            &self.sampler,
            encoder,
            device,
            before,
            after,
            uniform,
            &gs.accum.views[dst],
            mask_bg,
            scissor,
        );
    }

    /// The pass itself, with its destination and its mask handed in.
    ///
    /// Both spaces run exactly this: canvas supplies a group accumulator half
    /// and the host's effective mask, screen supplies a run half and the
    /// identity mask. Field-explicit so the screen caller can hold the
    /// disjoint `effect_instances` borrow across it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode_in_place_apply(
        blend_pipelines: &BlendPipelines,
        in_place_apply_pipelines: &[(wgpu::TextureFormat, wgpu::RenderPipeline)],
        sampler: &wgpu::Sampler,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        before: &wgpu::TextureView,
        after: &wgpu::TextureView,
        uniform: wgpu::BindingResource,
        dst_view: &wgpu::TextureView,
        mask_bg: &wgpu::BindGroup,
        scissor: (u32, u32, u32, u32),
    ) {
        let (scissor_x, scissor_y, scissor_w, scissor_h) = scissor;
        let bind_group = blend_bind_group(
            device,
            "in-place-apply-bg",
            &blend_pipelines.bind_group_layout,
            before,
            after,
            sampler,
            uniform,
        );

        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("in-place-apply"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        rpass.set_scissor_rect(scissor_x, scissor_y, scissor_w, scissor_h);
        rpass.set_pipeline(&in_place_apply_pipelines[0].1);
        rpass.set_bind_group(0, &bind_group, &[]);
        rpass.set_bind_group(1, mask_bg, &[]);
        rpass.draw(0..3, 0..1);
    }

    /// Composite a child group into its parent's ping-pong accumulators.
    /// Passthrough groups inline their children into the parent (with the
    /// Photoshop-style snapshot+lerp detour when a visible mask is
    /// attached); normal groups composite into their own isolated buffer
    /// first and then blend the result into the parent.
    #[allow(clippy::too_many_arguments)]
    fn compose_group_arm(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        doc: &Document,
        parent_group: LayerId,
        group: &crate::layer::LayerGroup,
        scissor: (u32, u32, u32, u32),
        isolated: Option<LayerId>,
    ) {
        let group_id = group.id;

        if group.passthrough {
            // Structural detection: a passthrough group with a visible mask
            // filter triggers Photoshop-style snapshot+lerp; otherwise
            // it's pure passthrough.
            let has_active_mask = doc.visible_mask_of(group_id).is_some();

            if has_active_mask {
                self.compose_passthrough_masked(
                    encoder,
                    device,
                    doc,
                    parent_group,
                    group_id,
                    scissor,
                    isolated,
                );
            } else {
                // Pure passthrough: inline children into parent.
                let inner: ChildIds = ChildIds::from_slice(doc.children_of(group_id));
                self.compose_children(
                    encoder,
                    device,
                    doc,
                    parent_group,
                    &inner,
                    scissor,
                    isolated,
                    None,
                );
            }
            return;
        }

        // Normal group: composite into its own isolated buffer, then blend
        // the result into the parent.
        if !self.group_state.contains_key(&group_id) {
            return;
        }
        self.compose_group(encoder, device, doc, group_id, scissor, isolated);

        // Blend group's composite cache into parent's accumulators.
        let gs_parent = self.group_state.get_mut(&parent_group).unwrap();
        let src = gs_parent.current_accum;
        let dst = 1 - src;
        gs_parent.current_accum = dst;

        // Split-borrow into the cache: bg/layer views and uniform buffer
        // live in distinct fields from `blend_bind_groups`, so we can hold
        // the mutable borrow of the cache and the immutable borrows of the
        // views together. Groups never become floating targets themselves,
        // so the cache always applies here (a filter-as-floating-target
        // only swaps mask_bg via `effective_mask_bind_group`).
        let bg_view = &self.group_state[&parent_group].accum.views[src];
        let gs_child = &self.group_state[&group_id];
        // The child's output is whichever half its own walk ended on, so the
        // cache key carries it alongside the parent's source half: both sides
        // of this bind group can move independently between composites.
        let child_out = gs_child.output_index();
        let child_view = gs_child.output_view();
        let child_uniform = &gs_child.uniform_buf;
        let bgl = &self.blend_pipelines.bind_group_layout;
        let sampler = &self.sampler;
        let bind_group = Self::get_or_create_blend_bind_group(
            &mut self.blend_bind_groups,
            bgl,
            sampler,
            device,
            (
                parent_group,
                group_id,
                (src as u8) | ((child_out as u8) << 1),
            ),
            bg_view,
            child_view,
            child_uniform,
            "blend-group",
        );

        let gs_parent = &self.group_state[&parent_group];
        let child_mask_bg = Self::effective_mask_bind_group_fields(
            &self.node_textures,
            &self.default_mask_bind_group,
            self.transform_session.as_ref(),
            self.transform_pass.paste.as_ref(),
            doc,
            group_id,
        );
        draw_blend_pass(
            encoder,
            "blend-group",
            &gs_parent.accum.views[dst],
            wgpu::LoadOp::Load,
            self.blend_pipelines.pipeline(),
            bind_group,
            child_mask_bg,
            &self.canvas_bind_group,
            scissor,
        );
    }

    /// Composite a passthrough group whose mask is active.
    ///
    /// Snapshots the parent accumulator, composites children (passthrough),
    /// then runs the shared apply pass between the snapshot and the result.
    /// The one in-place host that still snapshots: its "after" is written by an
    /// arbitrary number of child passes straight into the accumulator, so
    /// unlike an effect layer it cannot be redirected into a scratch; see
    /// [`LayerNode::needs_before_snapshot`](crate::layer::LayerNode::needs_before_snapshot).
    #[allow(clippy::too_many_arguments)]
    fn compose_passthrough_masked(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        doc: &Document,
        parent_group: LayerId,
        group_id: LayerId,
        scissor: (u32, u32, u32, u32),
        isolated: Option<LayerId>,
    ) {
        // MaskSnapshotState must exist (ensure-driven per frame). If it isn't
        // ready, inline children without the mask this frame.
        if !self.mask_snapshot_state.contains_key(&group_id) {
            let inner: ChildIds = ChildIds::from_slice(doc.children_of(group_id));
            self.compose_children(
                encoder,
                device,
                doc,
                parent_group,
                &inner,
                scissor,
                isolated,
                None,
            );
            return;
        }

        self.snapshot_parent_accum(encoder, parent_group, group_id, scissor);

        let inner: ChildIds = ChildIds::from_slice(doc.children_of(group_id));
        self.compose_children(
            encoder,
            device,
            doc,
            parent_group,
            &inner,
            scissor,
            isolated,
            None,
        );

        // The children wrote into `current_accum`; the apply pass reads that as
        // its "after" and writes the other half.
        let after_idx = {
            let gs = self.group_state.get_mut(&parent_group).unwrap();
            let after_idx = gs.current_accum;
            gs.current_accum = 1 - after_idx;
            after_idx
        };
        let pms = &self.mask_snapshot_state[&group_id];
        let before = pms.snapshot_view.clone();
        let uniform = pms.uniform_buf.as_entire_binding();
        let after = self.group_state[&parent_group].accum.views[after_idx].clone();
        self.apply_in_place(
            encoder,
            device,
            doc,
            parent_group,
            group_id,
            &before,
            &after,
            uniform,
            1 - after_idx,
            scissor,
        );
    }
}
