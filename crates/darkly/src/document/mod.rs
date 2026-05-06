pub mod modifier;
pub mod modifiers;

pub use modifier::{Modifier, ModifierKind, ModifierRegistration};
pub use modifiers::mask::MaskModifier;
pub use modifiers::selection::{SelectionCpuCache, SelectionModifier};

use slotmap::{SecondaryMap, SlotMap};

use crate::coord::CanvasRect;
use crate::layer::*;

pub enum SelectionMode {
    Replace,
    Add,
    Subtract,
    Intersect,
}

pub enum MoveTarget {
    Before(LayerId),
    After(LayerId),
    IntoGroupTop(LayerId),
    IntoGroupBottom(LayerId),
}

/// One slot in [`Document::entities`]. Tree nodes (layers + groups) and
/// modifiers share a single id space and a single storage map so callers can
/// pass any [`LayerId`] through the same lookup surface.
pub enum Entity {
    Node(LayerNode),
    Modifier(Modifier),
}

impl Entity {
    pub fn as_node(&self) -> Option<&LayerNode> {
        match self {
            Entity::Node(n) => Some(n),
            Entity::Modifier(_) => None,
        }
    }

    pub fn as_node_mut(&mut self) -> Option<&mut LayerNode> {
        match self {
            Entity::Node(n) => Some(n),
            Entity::Modifier(_) => None,
        }
    }

    pub fn as_modifier(&self) -> Option<&Modifier> {
        match self {
            Entity::Modifier(m) => Some(m),
            Entity::Node(_) => None,
        }
    }

    pub fn as_modifier_mut(&mut self) -> Option<&mut Modifier> {
        match self {
            Entity::Modifier(m) => Some(m),
            Entity::Node(_) => None,
        }
    }
}

pub struct Document {
    pub width: u32,
    pub height: u32,

    /// Single shared slot store for every layer, group, and modifier in this
    /// document. Lookups are O(1); generational keys mean stale ids return
    /// `None` instead of aliasing onto a recycled slot.
    pub entities: SlotMap<LayerId, Entity>,

    /// Parent pointer for every linked entity:
    /// - For a tree node: its tree parent (a group). Root has no entry.
    /// - For a modifier: its host node. Selection has no entry.
    ///
    /// Entities present in `entities` but absent from `parent` are *orphans* —
    /// either the root itself, the selection modifier, or a subtree that has
    /// been detached for undo and is waiting to be reattached.
    pub parent: SecondaryMap<LayerId, LayerId>,

    /// The implicit root group's id. Allocated in [`Document::new`]; replaces
    /// the old well-known `ROOT_ID = 0` constant. The root group itself is
    /// never exposed to the UI — only its children are.
    pub root: LayerId,

    /// Global selection modifier id, if allocated. The modifier itself lives
    /// in [`Self::entities`]; this just remembers which entry it is. Once
    /// allocated it stays for the document's lifetime, with `common.visible`
    /// toggling whether ops respect the selection.
    pub selection: Option<LayerId>,

    /// Monotonic counters for default display names — "Layer 1", "Layer 2",
    /// etc. Per-kind so renumbering is independent. They survive deletes
    /// within a session (the next add doesn't reuse a freed number, just
    /// like Photoshop and Krita), giving stable, readable labels even
    /// after heavy churn.
    next_raster_number: u32,
    next_group_number: u32,
    next_mask_number: u32,
}

impl Document {
    pub fn new(width: u32, height: u32) -> Self {
        let mut entities: SlotMap<LayerId, Entity> = SlotMap::with_key();
        // Allocate the root with a placeholder id, then patch the id after
        // insertion. SlotMap::insert_with_key does this in one step. The
        // root is never shown in the UI, so its name is internal only.
        let root = entities.insert_with_key(|key| {
            Entity::Node(LayerNode::Group(LayerGroup::new(key, "Root".to_string())))
        });
        Document {
            width,
            height,
            entities,
            parent: SecondaryMap::new(),
            root,
            selection: None,
            next_raster_number: 1,
            next_group_number: 1,
            next_mask_number: 1,
        }
    }

    /// Id of the implicit root group. Replaces the old `ROOT_ID` constant.
    pub fn root_id(&self) -> LayerId {
        self.root
    }

    // ---------------------------------------------------------------
    // Lookup — every method is O(1) (tree walks happen only in the
    // explicitly-named whole-tree queries: `flat_layers`, `all_*`,
    // `node_count`).
    // ---------------------------------------------------------------

    pub fn find_node(&self, id: LayerId) -> Option<&LayerNode> {
        self.entities.get(id).and_then(Entity::as_node)
    }

    pub fn find_node_mut(&mut self, id: LayerId) -> Option<&mut LayerNode> {
        self.entities.get_mut(id).and_then(Entity::as_node_mut)
    }

    pub fn find_modifier(&self, id: LayerId) -> Option<&Modifier> {
        self.entities.get(id).and_then(Entity::as_modifier)
    }

    pub fn find_modifier_mut(&mut self, id: LayerId) -> Option<&mut Modifier> {
        self.entities.get_mut(id).and_then(Entity::as_modifier_mut)
    }

    /// Find the host node a modifier is attached to (the layer or group that
    /// owns it on its `modifiers` list).
    pub fn find_modifier_host(&self, modifier_id: LayerId) -> Option<&LayerNode> {
        let host_id = *self.parent.get(modifier_id)?;
        self.find_node(host_id)
    }

    pub fn find_modifier_host_mut(&mut self, modifier_id: LayerId) -> Option<&mut LayerNode> {
        let host_id = *self.parent.get(modifier_id)?;
        self.find_node_mut(host_id)
    }

    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        match self.find_node(id)? {
            LayerNode::Layer(l) => Some(l),
            LayerNode::Group(_) => None,
        }
    }

    pub fn layer_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        match self.find_node_mut(id)? {
            LayerNode::Layer(l) => Some(l),
            LayerNode::Group(_) => None,
        }
    }

    /// True if `id` refers to a modifier (rather than a layer/group). Cheap
    /// disambiguator for callers that hold a node id and need to dispatch.
    pub fn is_modifier(&self, id: LayerId) -> bool {
        matches!(self.entities.get(id), Some(Entity::Modifier(_)))
    }

    /// Pixel-buffer accessor that works for any pixel-bearing entity — raster
    /// layers and pixel-storing modifiers (today: masks, selection). Returns
    /// `None` for groups, pure-effect modifiers, or unknown ids.
    pub fn pixel_buffer(&self, id: LayerId) -> Option<&PixelBuffer> {
        match self.entities.get(id)? {
            Entity::Node(n) => n.pixels(),
            Entity::Modifier(m) => m.pixels(),
        }
    }

    pub fn pixel_buffer_mut(&mut self, id: LayerId) -> Option<&mut PixelBuffer> {
        match self.entities.get_mut(id)? {
            Entity::Node(n) => n.pixels_mut(),
            Entity::Modifier(m) => m.pixels_mut(),
        }
    }

    pub fn parent_of(&self, id: LayerId) -> Option<LayerId> {
        self.parent.get(id).copied()
    }

    /// Index of an entity within its parent container.
    /// - For a tree node: position in its parent group's `children` Vec.
    /// - For a modifier: position in its host node's `modifiers` Vec.
    pub fn position_in_parent(&self, id: LayerId) -> Option<usize> {
        let parent_id = *self.parent.get(id)?;
        let parent_node = self.find_node(parent_id)?;
        if self.is_modifier(id) {
            parent_node.modifiers().iter().position(|c| *c == id)
        } else {
            match parent_node {
                LayerNode::Group(g) => g.children.iter().position(|c| *c == id),
                LayerNode::Layer(_) => None,
            }
        }
    }

    /// First mask modifier on a host, if any. Replaces the old
    /// `host.modifiers().mask()` pattern — that helper used to live on
    /// `ModifierList`, but with id-references it needs the document to
    /// resolve.
    pub fn mask_modifier_id(&self, host_id: LayerId) -> Option<LayerId> {
        let host = self.find_node(host_id)?;
        host.modifiers().iter().copied().find(|mid| {
            self.find_modifier(*mid)
                .map(|m| matches!(m.kind, ModifierKind::Mask(_)))
                .unwrap_or(false)
        })
    }

    pub fn mask_modifier(&self, host_id: LayerId) -> Option<&Modifier> {
        self.find_modifier(self.mask_modifier_id(host_id)?)
    }

    pub fn has_mask(&self, host_id: LayerId) -> bool {
        self.mask_modifier_id(host_id).is_some()
    }

    /// Modifier-id list for a host node, in bottom-up order.
    pub fn modifiers_of(&self, host_id: LayerId) -> &[LayerId] {
        match self.find_node(host_id) {
            Some(n) => n.modifiers(),
            None => &[],
        }
    }

    /// Children of a group, in display order.
    pub fn children_of(&self, group_id: LayerId) -> &[LayerId] {
        match self.find_node(group_id) {
            Some(LayerNode::Group(g)) => &g.children,
            _ => &[],
        }
    }

    // ---------------------------------------------------------------
    // Whole-tree queries — these enumerate the world by definition,
    // so they're O(N). They walk from `root` and so naturally exclude
    // any orphans parked in the slotmap awaiting undo reattach.
    // ---------------------------------------------------------------

    pub fn flat_layers(&self) -> Vec<&Layer> {
        let mut out = Vec::new();
        self.flatten_into(self.root, &mut out);
        out
    }

    fn flatten_into<'a>(&'a self, group_id: LayerId, out: &mut Vec<&'a Layer>) {
        let Some(LayerNode::Group(g)) = self.find_node(group_id) else {
            return;
        };
        for &child_id in &g.children {
            match self.find_node(child_id) {
                Some(LayerNode::Layer(l)) => out.push(l),
                Some(LayerNode::Group(child)) if !child.common.visible => {}
                Some(LayerNode::Group(_)) => {
                    // Passthrough groups: children composited directly into
                    // parent. Normal groups: TODO — needs isolated compositing
                    // buffer. For now, flatten children in both modes.
                    self.flatten_into(child_id, out);
                }
                None => {}
            }
        }
    }

    /// All raster layers in the tree, regardless of visibility. Used for GPU
    /// sync — we keep GPU textures in sync even for hidden layers.
    pub fn all_raster_layers(&self) -> Vec<&RasterLayer> {
        let mut out = Vec::new();
        self.collect_raster_layers(self.root, &mut out);
        out
    }

    fn collect_raster_layers<'a>(&'a self, group_id: LayerId, out: &mut Vec<&'a RasterLayer>) {
        let Some(LayerNode::Group(g)) = self.find_node(group_id) else {
            return;
        };
        for &child_id in &g.children {
            match self.find_node(child_id) {
                Some(LayerNode::Layer(Layer::Raster(r))) => out.push(r),
                Some(LayerNode::Group(_)) => self.collect_raster_layers(child_id, out),
                _ => {}
            }
        }
    }

    pub fn all_groups(&self) -> Vec<&LayerGroup> {
        let mut out = Vec::new();
        self.collect_groups(self.root, &mut out);
        out
    }

    fn collect_groups<'a>(&'a self, group_id: LayerId, out: &mut Vec<&'a LayerGroup>) {
        let Some(LayerNode::Group(g)) = self.find_node(group_id) else {
            return;
        };
        for &child_id in &g.children {
            if let Some(LayerNode::Group(child)) = self.find_node(child_id) {
                out.push(child);
                self.collect_groups(child_id, out);
            }
        }
    }

    /// Every modifier attached to any host in the tree. The global selection
    /// modifier is *not* included (it has no host); use
    /// [`Document::selection_id`] for that.
    pub fn all_modifiers(&self) -> Vec<&Modifier> {
        let mut out = Vec::new();
        self.collect_modifiers(self.root, &mut out);
        out
    }

    fn collect_modifiers<'a>(&'a self, group_id: LayerId, out: &mut Vec<&'a Modifier>) {
        let Some(LayerNode::Group(g)) = self.find_node(group_id) else {
            return;
        };
        // Modifiers on the group itself.
        for &mid in &g.modifiers {
            if let Some(m) = self.find_modifier(mid) {
                out.push(m);
            }
        }
        // Then descend.
        for &child_id in &g.children {
            match self.find_node(child_id) {
                Some(LayerNode::Layer(l)) => {
                    for &mid in l.modifiers() {
                        if let Some(m) = self.find_modifier(mid) {
                            out.push(m);
                        }
                    }
                }
                Some(LayerNode::Group(_)) => self.collect_modifiers(child_id, out),
                None => {}
            }
        }
    }

    /// Count of all tree nodes (layers + groups, excluding the root and
    /// modifiers). Walks the tree; intended for tests and rare diagnostics.
    pub fn node_count(&self) -> usize {
        fn walk(doc: &Document, group_id: LayerId, counter: &mut usize) {
            let Some(LayerNode::Group(g)) = doc.find_node(group_id) else {
                return;
            };
            for &child_id in &g.children {
                *counter += 1;
                if matches!(doc.find_node(child_id), Some(LayerNode::Group(_))) {
                    walk(doc, child_id, counter);
                }
            }
        }
        let mut n = 0;
        walk(self, self.root, &mut n);
        n
    }

    pub fn flat_layer_index(&self, id: LayerId) -> Option<usize> {
        self.flat_layers().iter().position(|l| l.id() == id)
    }

    // ---------------------------------------------------------------
    // Mutation — every entry point that adds, removes, or reparents an
    // entity goes through here so the slotmap, the parent map, and the
    // children/modifier Vecs stay consistent.
    // ---------------------------------------------------------------

    /// Add a new raster layer, positioning it relative to `anchor` per
    /// [`Document::resolve_anchor_target`].
    pub fn add_raster_layer(&mut self, anchor: Option<LayerId>) -> LayerId {
        let bounds = CanvasRect::from_xywh(0, 0, self.width, self.height);
        let name = format!("Layer {}", self.next_raster_number);
        self.next_raster_number += 1;
        let id = self.entities.insert_with_key(|key| {
            Entity::Node(LayerNode::Layer(Layer::Raster(RasterLayer::new(
                key, bounds, name,
            ))))
        });
        let target = self.resolve_anchor_target(anchor);
        self.attach_at_target(id, target);
        id
    }

    /// Add a new empty group; positioning follows the same rules as
    /// [`Document::add_raster_layer`].
    pub fn add_group(&mut self, anchor: Option<LayerId>) -> LayerId {
        let name = format!("Group {}", self.next_group_number);
        self.next_group_number += 1;
        let id = self
            .entities
            .insert_with_key(|key| Entity::Node(LayerNode::Group(LayerGroup::new(key, name))));
        let target = self.resolve_anchor_target(anchor);
        self.attach_at_target(id, target);
        id
    }

    /// Add a [`MaskModifier`] to a host node, returning the new modifier's id.
    /// Bounds default to the host's pixel bounds (raster) or canvas (group).
    /// Returns `None` if the host id is unknown.
    ///
    /// Note: only one mask per host is enforced at the UI layer, not here —
    /// the model supports N. Callers that want the singleton invariant should
    /// check [`Document::has_mask`] before adding.
    pub fn add_mask_modifier(&mut self, host_id: LayerId) -> Option<LayerId> {
        let bounds = self.host_default_bounds(host_id)?;
        let name = format!("Mask {}", self.next_mask_number);
        self.next_mask_number += 1;
        let id = self.entities.insert_with_key(|key| {
            Entity::Modifier(Modifier {
                id: key,
                common: NodeCommon::new(name),
                kind: ModifierKind::mask_with_bounds(bounds),
            })
        });
        // Patch the modifier's id field to match its slot key.
        if let Some(Entity::Modifier(m)) = self.entities.get_mut(id) {
            m.id = id;
        }
        // Link to the host.
        if let Some(host) = self.find_node_mut(host_id) {
            host.modifiers_mut().push(id);
            self.parent.insert(id, host_id);
            Some(id)
        } else {
            self.entities.remove(id);
            None
        }
    }

    /// Allocate the global selection modifier if not already present, sized
    /// to the canvas. Idempotent — returns the modifier id either way.
    pub fn ensure_selection_modifier(&mut self) -> LayerId {
        if let Some(id) = self.selection {
            return id;
        }
        let bounds = CanvasRect::from_xywh(0, 0, self.width, self.height);
        let id = self.entities.insert_with_key(|key| {
            let mut m = Modifier {
                id: key,
                common: NodeCommon::new("Selection".to_string()),
                kind: ModifierKind::selection_with_bounds(bounds),
            };
            // Default `visible = false` mirrors today's "always allocated,
            // .active toggles whether ops respect it" semantics.
            m.common.visible = false;
            Entity::Modifier(m)
        });
        // Patch id field to slot key.
        if let Some(Entity::Modifier(m)) = self.entities.get_mut(id) {
            m.id = id;
        }
        self.selection = Some(id);
        // Per the plan, the selection lives "at the document root rather than
        // on a host's `modifiers` list" — so no entry in `parent`.
        id
    }

    /// Selection modifier id, if allocated.
    pub fn selection_id(&self) -> Option<LayerId> {
        self.selection
    }

    /// True when the selection modifier is allocated AND its `common.visible`
    /// flag is set — equivalent to today's `gpu_selection.active`.
    pub fn selection_active(&self) -> bool {
        self.selection
            .and_then(|id| self.find_modifier(id))
            .is_some_and(|m| m.common.visible)
    }

    /// Move a node to a new position in the tree.
    pub fn move_layer(&mut self, layer_id: LayerId, target: MoveTarget) {
        if self.unlink_node(layer_id).is_none() {
            return;
        }
        self.attach_at_target(layer_id, target);
    }

    /// Detach a node from the tree for undo purposes, leaving it parked in
    /// `entities` so its id stays stable across undo/redo. Returns the id on
    /// success. Reattach with [`Document::reinsert_node`]; if the undo entry
    /// is later discarded, call [`Document::remove_node`] to actually free it.
    pub fn detach_for_undo(&mut self, id: LayerId) -> Option<LayerId> {
        self.unlink_node(id)
    }

    /// Reinsert a previously detached node at a specific position.
    pub fn reinsert_node(&mut self, id: LayerId, parent: Option<LayerId>, position: usize) {
        if !self.entities.contains_key(id) {
            return;
        }
        let parent_id = self.resolve_parent_group(parent);
        self.link_child(id, parent_id, Some(position));
    }

    /// Detach a modifier from its host for undo purposes. The modifier stays
    /// in `entities`, so reattach via [`Document::reinsert_modifier`] preserves
    /// the id.
    pub fn detach_modifier_for_undo(&mut self, id: LayerId) -> Option<LayerId> {
        self.unlink_modifier(id)
    }

    /// Reattach a previously detached modifier to a host. Append-only — the
    /// model doesn't expose a position parameter today because masks use
    /// "first mask" lookup rather than positional access.
    pub fn reinsert_modifier(&mut self, modifier_id: LayerId, host_id: LayerId) {
        if !self.is_modifier(modifier_id) {
            return;
        }
        if let Some(host) = self.find_node_mut(host_id) {
            host.modifiers_mut().push(modifier_id);
            self.parent.insert(modifier_id, host_id);
        }
    }

    /// Remove a node (layer or group) and everything beneath it (descendant
    /// nodes, all modifiers on every node in the subtree) from `entities`.
    /// Permanent — call [`Document::detach_for_undo`] instead if the caller
    /// wants id-stable detach for undo.
    pub fn remove_node(&mut self, id: LayerId) {
        if id == self.root {
            return;
        }
        // Unlink first so the parent's children Vec is consistent if anyone
        // observes mid-purge.
        self.unlink_node(id);
        self.purge_subtree(id);
    }

    /// Permanently remove a modifier from `entities` (and its host's modifier
    /// list, if still attached).
    pub fn remove_modifier(&mut self, id: LayerId) {
        self.unlink_modifier(id);
        // Selection sentinel: if this was the selection, clear the field too.
        if self.selection == Some(id) {
            self.selection = None;
        }
        self.entities.remove(id);
    }

    // ---------------------------------------------------------------
    // Internal helpers — every mutation path funnels through these so
    // the slotmap / parent map / children Vec stay in sync.
    // ---------------------------------------------------------------

    /// Resolve an `Option<LayerId>` parent into a concrete group id, defaulting
    /// to `self.root` on `None` or on an id that isn't a group.
    fn resolve_parent_group(&self, parent: Option<LayerId>) -> LayerId {
        match parent {
            Some(id) if matches!(self.find_node(id), Some(LayerNode::Group(_))) => id,
            _ => self.root,
        }
    }

    /// Insert `child` into `group`'s children Vec at `position` (or at the end
    /// if `None`), and update the parent map. Caller guarantees `child` is in
    /// `entities` and `group` is a group.
    fn link_child(&mut self, child: LayerId, group: LayerId, position: Option<usize>) {
        let Some(LayerNode::Group(g)) = self.find_node_mut(group) else {
            return;
        };
        let pos = position
            .map(|p| p.min(g.children.len()))
            .unwrap_or(g.children.len());
        g.children.insert(pos, child);
        self.parent.insert(child, group);
    }

    /// Unlink a node from its parent's children Vec and from the parent map.
    /// Returns the node's id if it was linked, `None` if it was the root or
    /// already orphaned.
    fn unlink_node(&mut self, id: LayerId) -> Option<LayerId> {
        if id == self.root {
            return None;
        }
        let parent_id = self.parent.remove(id)?;
        if let Some(LayerNode::Group(g)) = self.find_node_mut(parent_id) {
            g.children.retain(|c| *c != id);
        }
        Some(id)
    }

    /// Unlink a modifier from its host's modifiers Vec and from the parent
    /// map. Returns the modifier's id if it was linked.
    fn unlink_modifier(&mut self, id: LayerId) -> Option<LayerId> {
        let host_id = self.parent.remove(id)?;
        if let Some(host) = self.find_node_mut(host_id) {
            host.modifiers_mut().retain(|m| *m != id);
        }
        Some(id)
    }

    /// Resolve a UI "anchor" (typically the currently selected node in the
    /// layers panel) into a [`MoveTarget`] that places a newly-created node
    /// where the user expects.
    ///
    /// - `None` / unknown / stale id → top of root.
    /// - Modifier id → recurse against the modifier's host.
    /// - Group id → top of that group's children.
    /// - Layer id → sibling immediately above the anchor.
    pub fn resolve_anchor_target(&self, anchor: Option<LayerId>) -> MoveTarget {
        let Some(id) = anchor else {
            return MoveTarget::IntoGroupTop(self.root);
        };
        if self.is_modifier(id) {
            return match self.parent.get(id).copied() {
                Some(host) => self.resolve_anchor_target(Some(host)),
                None => MoveTarget::IntoGroupTop(self.root),
            };
        }
        match self.find_node(id) {
            Some(LayerNode::Group(_)) => MoveTarget::IntoGroupTop(id),
            Some(LayerNode::Layer(_)) => MoveTarget::After(id),
            None => MoveTarget::IntoGroupTop(self.root),
        }
    }

    /// Apply a [`MoveTarget`] to a node already unlinked from the tree.
    fn attach_at_target(&mut self, node: LayerId, target: MoveTarget) {
        match target {
            MoveTarget::Before(ref_id) => {
                if let Some(parent_id) = self.parent_of(ref_id) {
                    let pos = self
                        .children_of(parent_id)
                        .iter()
                        .position(|c| *c == ref_id)
                        .unwrap_or(0);
                    self.link_child(node, parent_id, Some(pos));
                } else {
                    self.link_child(node, self.root, None);
                }
            }
            MoveTarget::After(ref_id) => {
                if let Some(parent_id) = self.parent_of(ref_id) {
                    let pos = self
                        .children_of(parent_id)
                        .iter()
                        .position(|c| *c == ref_id)
                        .map(|p| p + 1)
                        .unwrap_or_else(|| self.children_of(parent_id).len());
                    self.link_child(node, parent_id, Some(pos));
                } else {
                    self.link_child(node, self.root, None);
                }
            }
            MoveTarget::IntoGroupTop(group_id) => {
                let group = self.resolve_parent_group(Some(group_id));
                self.link_child(node, group, None);
            }
            MoveTarget::IntoGroupBottom(group_id) => {
                let group = self.resolve_parent_group(Some(group_id));
                self.link_child(node, group, Some(0));
            }
        }
    }

    /// Recursively remove a subtree (the node, its descendants, and every
    /// modifier on every node in the subtree) from `entities` and the parent
    /// map. Caller is responsible for unlinking the subtree's root from its
    /// parent first.
    fn purge_subtree(&mut self, id: LayerId) {
        // Collect ids depth-first so we can remove them all without holding
        // borrows across `entities.remove` calls.
        let mut nodes_to_purge = Vec::new();
        self.collect_subtree_ids(id, &mut nodes_to_purge);
        for nid in nodes_to_purge {
            // Drop modifiers attached to this node first.
            let mod_ids: Vec<LayerId> = match self.find_node(nid) {
                Some(n) => n.modifiers().to_vec(),
                None => Vec::new(),
            };
            for mid in mod_ids {
                self.parent.remove(mid);
                self.entities.remove(mid);
            }
            self.parent.remove(nid);
            self.entities.remove(nid);
        }
    }

    fn collect_subtree_ids(&self, id: LayerId, out: &mut Vec<LayerId>) {
        out.push(id);
        if let Some(LayerNode::Group(g)) = self.find_node(id) {
            // Clone to avoid holding the borrow while recursing.
            let children: Vec<LayerId> = g.children.clone();
            for child in children {
                self.collect_subtree_ids(child, out);
            }
        }
    }

    fn host_default_bounds(&self, host_id: LayerId) -> Option<CanvasRect> {
        match self.find_node(host_id)? {
            LayerNode::Layer(Layer::Raster(r)) => Some(r.pixels.bounds),
            LayerNode::Group(_) => Some(CanvasRect::from_xywh(0, 0, self.width, self.height)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_layers() {
        let mut doc = Document::new(256, 256);
        let id = doc.add_raster_layer(None);
        assert_eq!(doc.flat_layers().len(), 1);
        assert_eq!(doc.flat_layers()[0].id(), id);
    }

    #[test]
    fn flat_layers_respects_group_visibility() {
        let mut doc = Document::new(256, 256);
        let l1 = doc.add_raster_layer(None);
        let g1 = doc.add_group(None);
        let l2 = doc.add_raster_layer(Some(g1));

        let flat = doc.flat_layers();
        assert_eq!(flat.len(), 2);
        assert_eq!(flat[0].id(), l1);
        assert_eq!(flat[1].id(), l2);

        if let Some(LayerNode::Group(g)) = doc.find_node_mut(g1) {
            g.common.visible = false;
        }
        let flat = doc.flat_layers();
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].id(), l1);
    }

    #[test]
    fn move_layer_between_groups() {
        let mut doc = Document::new(256, 256);
        let l1 = doc.add_raster_layer(None);
        let g1 = doc.add_group(None);
        let l2 = doc.add_raster_layer(Some(g1));

        doc.move_layer(l2, MoveTarget::Before(l1));
        let flat = doc.flat_layers();
        assert_eq!(flat[0].id(), l2);
        assert_eq!(flat[1].id(), l1);
    }

    #[test]
    fn nested_groups_flatten_correctly() {
        let mut doc = Document::new(256, 256);
        let l1 = doc.add_raster_layer(None);
        let g1 = doc.add_group(None);
        let l2 = doc.add_raster_layer(Some(g1));

        let flat = doc.flat_layers();
        assert_eq!(flat.len(), 2);
        assert_eq!(flat[0].id(), l1);
        assert_eq!(flat[1].id(), l2);
    }

    #[test]
    fn remove_group_removes_children() {
        let mut doc = Document::new(256, 256);
        let g1 = doc.add_group(None);
        let _l1 = doc.add_raster_layer(Some(g1));

        doc.remove_node(g1);
        assert!(doc.flat_layers().is_empty());
    }

    #[test]
    fn detach_insert_roundtrip() {
        let mut doc = Document::new(256, 256);
        let l1 = doc.add_raster_layer(None);
        let l2 = doc.add_raster_layer(None);
        let l3 = doc.add_raster_layer(None);

        doc.move_layer(l1, MoveTarget::After(l3));
        let flat = doc.flat_layers();
        assert_eq!(flat[0].id(), l2);
        assert_eq!(flat[1].id(), l3);
        assert_eq!(flat[2].id(), l1);
    }

    #[test]
    fn add_modifier_attaches_to_host() {
        let mut doc = Document::new(256, 256);
        let l = doc.add_raster_layer(None);
        assert!(doc.modifiers_of(l).is_empty());

        let mod_id = doc.add_mask_modifier(l).unwrap();
        assert_eq!(doc.modifiers_of(l).len(), 1);
        assert_eq!(doc.mask_modifier_id(l), Some(mod_id));

        assert!(doc.is_modifier(mod_id));
        assert!(!doc.is_modifier(l));
    }

    #[test]
    fn remove_modifier_detaches() {
        let mut doc = Document::new(256, 256);
        let l = doc.add_raster_layer(None);
        let mod_id = doc.add_mask_modifier(l).unwrap();

        doc.remove_modifier(mod_id);
        assert!(doc.modifiers_of(l).is_empty());
        assert!(!doc.is_modifier(mod_id));
        // Truly purged from entities (not just unlinked).
        assert!(doc.find_modifier(mod_id).is_none());
    }

    #[test]
    fn pixel_buffer_dispatches_layer_or_modifier() {
        let mut doc = Document::new(256, 256);
        let l = doc.add_raster_layer(None);
        let mod_id = doc.add_mask_modifier(l).unwrap();

        let layer_buf = doc.pixel_buffer(l).unwrap();
        assert_eq!(layer_buf.format, wgpu::TextureFormat::Rgba8Unorm);

        let mask_buf = doc.pixel_buffer(mod_id).unwrap();
        assert_eq!(mask_buf.format, wgpu::TextureFormat::R8Unorm);

        let g = doc.add_group(None);
        assert!(doc.pixel_buffer(g).is_none());
    }

    #[test]
    fn modifier_host_lookup() {
        let mut doc = Document::new(256, 256);
        let l = doc.add_raster_layer(None);
        let mod_id = doc.add_mask_modifier(l).unwrap();

        let host = doc.find_modifier_host(mod_id).unwrap();
        assert_eq!(host.id(), l);
        assert_eq!(doc.parent_of(mod_id), Some(l));
    }

    // -----------------------------------------------------------------
    // Slotmap-invariant regression tests.
    // -----------------------------------------------------------------

    #[test]
    fn stale_id_returns_none() {
        // After purging a layer, looking it up by its old id MUST return None
        // — slotmap's generational keys make this safe even after another
        // layer is allocated into the same slot.
        let mut doc = Document::new(256, 256);
        let stale = doc.add_raster_layer(None);
        doc.remove_node(stale);
        // Allocate something else; the slot may be recycled with a bumped
        // generation. The stale key must still not resolve.
        let _other = doc.add_raster_layer(None);
        assert!(doc.find_node(stale).is_none());
        assert!(doc.layer(stale).is_none());
        assert!(doc.parent_of(stale).is_none());
        assert!(!doc.is_modifier(stale));
    }

    #[test]
    fn parent_index_consistent_after_move() {
        let mut doc = Document::new(256, 256);
        let g1 = doc.add_group(None);
        let g2 = doc.add_group(None);
        let l = doc.add_raster_layer(Some(g1));
        assert_eq!(doc.parent_of(l), Some(g1));
        doc.move_layer(l, MoveTarget::IntoGroupTop(g2));
        assert_eq!(doc.parent_of(l), Some(g2));
        // And g1 no longer references it.
        assert!(!doc.children_of(g1).contains(&l));
        assert!(doc.children_of(g2).contains(&l));
    }

    #[test]
    fn detach_for_undo_preserves_id() {
        // Detach is orphan-keep: id stays valid in `entities`, modifiers
        // attached to the detached node stay attached, and reattach restores
        // everything at the requested position.
        let mut doc = Document::new(256, 256);
        let l = doc.add_raster_layer(None);
        let m = doc.add_mask_modifier(l).unwrap();

        let parent = doc.parent_of(l);
        let pos = doc.position_in_parent(l).unwrap();

        let detached = doc.detach_for_undo(l).unwrap();
        assert_eq!(detached, l);
        assert!(doc.parent_of(l).is_none());
        // Still resolvable in entities.
        assert!(doc.find_node(l).is_some());
        // Modifier still attached to the detached node.
        assert_eq!(doc.parent_of(m), Some(l));
        assert_eq!(doc.modifiers_of(l), &[m]);
        // Not in the tree.
        assert!(doc.flat_layers().is_empty());

        doc.reinsert_node(l, parent, pos);
        assert_eq!(doc.parent_of(l), parent.or(Some(doc.root)));
        assert_eq!(doc.flat_layers().len(), 1);
        assert_eq!(doc.mask_modifier_id(l), Some(m));
    }

    #[test]
    fn purge_subtree_frees_descendants() {
        // remove_node must actually purge from `entities` — not just unlink.
        let mut doc = Document::new(256, 256);
        let g = doc.add_group(None);
        let l = doc.add_raster_layer(Some(g));
        let m = doc.add_mask_modifier(l).unwrap();
        let inner_g = doc.add_group(None);
        // Reparent inner_g under g.
        doc.move_layer(inner_g, MoveTarget::IntoGroupTop(g));
        let inner_l = doc.add_raster_layer(Some(inner_g));

        doc.remove_node(g);
        for id in [g, l, m, inner_g, inner_l] {
            assert!(
                doc.entities.get(id).is_none(),
                "{:?} should have been purged from entities",
                id.to_ffi()
            );
        }
    }

    // -----------------------------------------------------------------
    // Anchor-aware insertion (`add_raster_layer` / `add_group` with an
    // optional anchor that points at the currently-selected node).
    // -----------------------------------------------------------------

    #[test]
    fn add_raster_layer_no_anchor_lands_at_root_top() {
        let mut doc = Document::new(256, 256);
        let _l1 = doc.add_raster_layer(None);
        let _l2 = doc.add_raster_layer(None);
        let new_id = doc.add_raster_layer(None);
        assert_eq!(doc.children_of(doc.root).last().copied(), Some(new_id));
    }

    #[test]
    fn add_raster_layer_with_layer_anchor_lands_above() {
        let mut doc = Document::new(256, 256);
        let l1 = doc.add_raster_layer(None);
        let l2 = doc.add_raster_layer(None);
        let l3 = doc.add_raster_layer(None);
        let new_id = doc.add_raster_layer(Some(l1));
        assert_eq!(doc.children_of(doc.root), &[l1, new_id, l2, l3]);
    }

    #[test]
    fn add_raster_layer_with_layer_anchor_inside_group() {
        let mut doc = Document::new(256, 256);
        let g = doc.add_group(None);
        let inner_a = doc.add_raster_layer(Some(g));
        let inner_b = doc.add_raster_layer(Some(g));
        let new_id = doc.add_raster_layer(Some(inner_a));
        assert_eq!(doc.parent_of(new_id), Some(g));
        assert_eq!(doc.children_of(g), &[inner_a, new_id, inner_b]);
    }

    #[test]
    fn add_raster_layer_with_group_anchor_lands_inside_group_top() {
        let mut doc = Document::new(256, 256);
        let g = doc.add_group(None);
        let inner_a = doc.add_raster_layer(Some(g));
        let new_id = doc.add_raster_layer(Some(g));
        assert_eq!(doc.parent_of(new_id), Some(g));
        assert_eq!(doc.children_of(g), &[inner_a, new_id]);
    }

    #[test]
    fn add_raster_layer_with_mask_anchor_lands_above_host() {
        let mut doc = Document::new(256, 256);
        let l1 = doc.add_raster_layer(None);
        let l2 = doc.add_raster_layer(None);
        let mask = doc.add_mask_modifier(l1).unwrap();
        let new_id = doc.add_raster_layer(Some(mask));
        // Modifier resolves to its host layer → After(host) in root.
        assert_eq!(doc.children_of(doc.root), &[l1, new_id, l2]);
    }

    #[test]
    fn add_raster_layer_with_stale_anchor_falls_back_to_root_top() {
        let mut doc = Document::new(256, 256);
        let stale = doc.add_raster_layer(None);
        doc.remove_node(stale);
        let _other = doc.add_raster_layer(None);
        let new_id = doc.add_raster_layer(Some(stale));
        assert_eq!(doc.children_of(doc.root).last().copied(), Some(new_id));
    }

    #[test]
    fn add_group_with_layer_anchor_lands_above() {
        let mut doc = Document::new(256, 256);
        let l1 = doc.add_raster_layer(None);
        let l2 = doc.add_raster_layer(None);
        let new_g = doc.add_group(Some(l1));
        assert_eq!(doc.children_of(doc.root), &[l1, new_g, l2]);
    }

    #[test]
    fn add_group_with_group_anchor_lands_inside() {
        let mut doc = Document::new(256, 256);
        let outer = doc.add_group(None);
        let inner_l = doc.add_raster_layer(Some(outer));
        let new_g = doc.add_group(Some(outer));
        assert_eq!(doc.parent_of(new_g), Some(outer));
        assert_eq!(doc.children_of(outer), &[inner_l, new_g]);
    }
}
