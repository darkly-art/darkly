pub mod modifier;
pub mod modifiers;

pub use modifier::{Modifier, ModifierKind, ModifierRegistration};
pub use modifiers::mask::MaskModifier;
pub use modifiers::selection::{SelectionCpuCache, SelectionModifier};

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

/// Well-known ID for the implicit root group.
pub const ROOT_ID: LayerId = 0;

pub struct Document {
    /// The root of the layer tree. All layers live inside this group.
    /// The root group itself is never exposed to the UI — only its children are.
    pub root: LayerGroup,
    pub width: u32,
    pub height: u32,
    /// Global selection — a typed [`Modifier`] attached at the document root
    /// (rather than on a host's `modifiers` list). `None` until the engine
    /// allocates one; once allocated it stays for the document's lifetime,
    /// with `common.visible` toggling whether ops respect the selection.
    /// The R8 pixel data lives in the compositor's selection sub-system; the
    /// document model owns id, name, visibility, lock, bounds, and the CPU
    /// readback cache via [`SelectionModifier`].
    pub selection: Option<Modifier>,
    next_id: LayerId,
}

// --- Tree traversal helpers (free functions for borrow-splitting) ---

fn find_layer_in(nodes: &[LayerNode], id: LayerId) -> Option<&Layer> {
    for node in nodes {
        match node {
            LayerNode::Layer(l) if l.id() == id => return Some(l),
            LayerNode::Group(g) => {
                if let Some(l) = find_layer_in(&g.children.0, id) {
                    return Some(l);
                }
            }
            _ => {}
        }
    }
    None
}

fn find_layer_in_mut(nodes: &mut [LayerNode], id: LayerId) -> Option<&mut Layer> {
    for node in nodes {
        match node {
            LayerNode::Layer(l) if l.id() == id => return Some(l),
            LayerNode::Group(g) => {
                if let Some(l) = find_layer_in_mut(&mut g.children.0, id) {
                    return Some(l);
                }
            }
            _ => {}
        }
    }
    None
}

fn find_node_in(nodes: &[LayerNode], id: LayerId) -> Option<&LayerNode> {
    for node in nodes {
        if node.id() == id {
            return Some(node);
        }
        if let LayerNode::Group(g) = node {
            if let Some(n) = find_node_in(&g.children.0, id) {
                return Some(n);
            }
        }
    }
    None
}

fn find_node_in_mut(nodes: &mut [LayerNode], id: LayerId) -> Option<&mut LayerNode> {
    for node in nodes {
        if node.id() == id {
            return Some(node);
        }
        if let LayerNode::Group(g) = node {
            if let Some(n) = find_node_in_mut(&mut g.children.0, id) {
                return Some(n);
            }
        }
    }
    None
}

fn find_modifier_in(nodes: &[LayerNode], id: LayerId) -> Option<&Modifier> {
    for node in nodes {
        for m in node.modifiers().iter() {
            if m.id == id {
                return Some(m);
            }
        }
        if let LayerNode::Group(g) = node {
            if let Some(m) = find_modifier_in(&g.children.0, id) {
                return Some(m);
            }
        }
    }
    None
}

fn find_modifier_in_mut(nodes: &mut [LayerNode], id: LayerId) -> Option<&mut Modifier> {
    for node in nodes {
        let pos = node.modifiers().iter().position(|m| m.id == id);
        if let Some(pos) = pos {
            return Some(&mut node.modifiers_mut().0[pos]);
        }
        if let LayerNode::Group(g) = node {
            if let Some(m) = find_modifier_in_mut(&mut g.children.0, id) {
                return Some(m);
            }
        }
    }
    None
}

fn find_modifier_host_in(nodes: &[LayerNode], modifier_id: LayerId) -> Option<&LayerNode> {
    for node in nodes {
        if node.modifiers().iter().any(|m| m.id == modifier_id) {
            return Some(node);
        }
        if let LayerNode::Group(g) = node {
            if let Some(host) = find_modifier_host_in(&g.children.0, modifier_id) {
                return Some(host);
            }
        }
    }
    None
}

fn find_modifier_host_in_mut(
    nodes: &mut [LayerNode],
    modifier_id: LayerId,
) -> Option<&mut LayerNode> {
    for node in nodes {
        if node.modifiers().iter().any(|m| m.id == modifier_id) {
            return Some(node);
        }
        if let LayerNode::Group(g) = node {
            if let Some(host) = find_modifier_host_in_mut(&mut g.children.0, modifier_id) {
                return Some(host);
            }
        }
    }
    None
}

fn detach_node(nodes: &mut Vec<LayerNode>, id: LayerId) -> Option<LayerNode> {
    if let Some(pos) = nodes.iter().position(|n| n.id() == id) {
        return Some(nodes.remove(pos));
    }
    for node in nodes.iter_mut() {
        if let LayerNode::Group(g) = node {
            if let Some(n) = detach_node(&mut g.children.0, id) {
                return Some(n);
            }
        }
    }
    None
}

fn detach_modifier(nodes: &mut [LayerNode], modifier_id: LayerId) -> Option<Modifier> {
    for node in nodes.iter_mut() {
        let pos = node.modifiers().iter().position(|m| m.id == modifier_id);
        if let Some(pos) = pos {
            return Some(node.modifiers_mut().0.remove(pos));
        }
        if let LayerNode::Group(g) = node {
            if let Some(m) = detach_modifier(&mut g.children.0, modifier_id) {
                return Some(m);
            }
        }
    }
    None
}

/// Find the position path of a node in the tree. Returns the path of indices.
fn find_position(nodes: &[LayerNode], id: LayerId) -> Option<Vec<usize>> {
    for (i, node) in nodes.iter().enumerate() {
        if node.id() == id {
            return Some(vec![i]);
        }
        if let LayerNode::Group(g) = node {
            if let Some(mut path) = find_position(&g.children.0, id) {
                path.insert(0, i);
                return Some(path);
            }
        }
    }
    None
}

/// Get mutable reference to the container (Vec<LayerNode>) at a path.
fn container_at_path<'a>(nodes: &'a mut Vec<LayerNode>, path: &[usize]) -> &'a mut Vec<LayerNode> {
    if path.len() <= 1 {
        return nodes;
    }
    let is_group = matches!(&nodes[path[0]], LayerNode::Group(_));
    if is_group {
        match &mut nodes[path[0]] {
            LayerNode::Group(g) => container_at_path(&mut g.children.0, &path[1..]),
            _ => unreachable!(),
        }
    } else {
        nodes
    }
}

fn flatten_nodes<'a>(nodes: &'a [LayerNode], out: &mut Vec<&'a Layer>) {
    for node in nodes {
        match node {
            LayerNode::Layer(l) => out.push(l),
            LayerNode::Group(g) => {
                if !g.common.visible {
                    continue;
                }
                // Passthrough groups: children composited directly into parent.
                // Normal groups: TODO — needs isolated compositing buffer.
                // For now, flatten children in both modes.
                flatten_nodes(&g.children.0, out);
            }
        }
    }
}

fn collect_raster_layers<'a>(nodes: &'a [LayerNode], out: &mut Vec<&'a RasterLayer>) {
    for node in nodes {
        match node {
            LayerNode::Layer(Layer::Raster(r)) => out.push(r),
            LayerNode::Group(g) => collect_raster_layers(&g.children.0, out),
        }
    }
}

fn collect_groups<'a>(nodes: &'a [LayerNode], out: &mut Vec<&'a LayerGroup>) {
    for node in nodes {
        if let LayerNode::Group(g) = node {
            out.push(g);
            collect_groups(&g.children.0, out);
        }
    }
}

fn collect_modifiers<'a>(nodes: &'a [LayerNode], out: &mut Vec<&'a Modifier>) {
    for node in nodes {
        for m in node.modifiers().iter() {
            out.push(m);
        }
        if let LayerNode::Group(g) = node {
            collect_modifiers(&g.children.0, out);
        }
    }
}

fn find_parent_of(nodes: &[LayerNode], id: LayerId) -> Option<LayerId> {
    for node in nodes {
        if let LayerNode::Group(g) = node {
            for child in g.children.iter() {
                if child.id() == id {
                    return Some(g.id);
                }
            }
            if let Some(parent) = find_parent_of(&g.children.0, id) {
                return Some(parent);
            }
        }
    }
    None
}

fn count_nodes(nodes: &[LayerNode]) -> usize {
    let mut count = 0;
    for node in nodes {
        count += 1;
        if let LayerNode::Group(g) = node {
            count += count_nodes(&g.children.0);
        }
    }
    count
}

fn position_in(nodes: &[LayerNode], id: LayerId) -> Option<usize> {
    nodes.iter().position(|n| n.id() == id)
}

impl Document {
    pub fn new(width: u32, height: u32) -> Self {
        Document {
            root: LayerGroup::new(ROOT_ID),
            width,
            height,
            selection: None,
            next_id: 1,
        }
    }

    fn alloc_id(&mut self) -> LayerId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Add a new raster layer at the root top.
    pub fn add_raster_layer(&mut self) -> LayerId {
        let id = self.alloc_id();
        let layer = RasterLayer::new(id, CanvasRect::from_xywh(0, 0, self.width, self.height));
        self.root
            .children
            .0
            .push(LayerNode::Layer(Layer::Raster(layer)));
        id
    }

    /// Add a new raster layer inside a group (or at root if parent is None).
    pub fn add_raster_layer_in(&mut self, parent: Option<LayerId>) -> LayerId {
        let id = self.alloc_id();
        let layer = RasterLayer::new(id, CanvasRect::from_xywh(0, 0, self.width, self.height));
        let node = LayerNode::Layer(Layer::Raster(layer));

        match parent {
            Some(group_id) => {
                if let Some(LayerNode::Group(g)) =
                    find_node_in_mut(&mut self.root.children.0, group_id)
                {
                    g.children.0.push(node);
                } else {
                    self.root.children.0.push(node);
                }
            }
            None => self.root.children.0.push(node),
        }
        id
    }

    /// Add a new empty group at the root top.
    pub fn add_group(&mut self) -> LayerId {
        let id = self.alloc_id();
        let group = LayerGroup::new(id);
        self.root.children.0.push(LayerNode::Group(group));
        id
    }

    /// Flatten the layer tree into display order (bottom-to-top) for compositing.
    /// Hidden groups exclude all children. Passthrough groups flatten children directly.
    pub fn flat_layers(&self) -> Vec<&Layer> {
        let mut out = Vec::new();
        flatten_nodes(&self.root.children.0, &mut out);
        out
    }

    /// Get all raster layers in the tree (regardless of visibility).
    /// Used for GPU sync — we keep GPU textures in sync even for hidden layers.
    pub fn all_raster_layers(&self) -> Vec<&RasterLayer> {
        let mut out = Vec::new();
        collect_raster_layers(&self.root.children.0, &mut out);
        out
    }

    pub fn all_groups(&self) -> Vec<&LayerGroup> {
        let mut out = Vec::new();
        collect_groups(&self.root.children.0, &mut out);
        out
    }

    /// Every modifier attached to any host in the tree.
    pub fn all_modifiers(&self) -> Vec<&Modifier> {
        let mut out = Vec::new();
        collect_modifiers(&self.root.children.0, &mut out);
        out
    }

    /// Count all nodes (layers + groups) in the tree. Excludes modifiers.
    pub fn node_count(&self) -> usize {
        count_nodes(&self.root.children.0)
    }

    pub fn flat_layer_index(&self, id: LayerId) -> Option<usize> {
        self.flat_layers().iter().position(|l| l.id() == id)
    }

    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        find_layer_in(&self.root.children.0, id)
    }

    pub fn layer_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        find_layer_in_mut(&mut self.root.children.0, id)
    }

    pub fn find_node(&self, id: LayerId) -> Option<&LayerNode> {
        find_node_in(&self.root.children.0, id)
    }

    pub fn find_node_mut(&mut self, id: LayerId) -> Option<&mut LayerNode> {
        find_node_in_mut(&mut self.root.children.0, id)
    }

    /// Look up a modifier by id. Modifiers live on host `modifiers` lists, not
    /// in the `LayerNode` tree, so they require their own finder.
    pub fn find_modifier(&self, id: LayerId) -> Option<&Modifier> {
        find_modifier_in(&self.root.children.0, id)
    }

    pub fn find_modifier_mut(&mut self, id: LayerId) -> Option<&mut Modifier> {
        find_modifier_in_mut(&mut self.root.children.0, id)
    }

    /// Find the host node a modifier is attached to (the layer or group that
    /// owns it on its `modifiers` list).
    pub fn find_modifier_host(&self, modifier_id: LayerId) -> Option<&LayerNode> {
        find_modifier_host_in(&self.root.children.0, modifier_id)
    }

    pub fn find_modifier_host_mut(&mut self, modifier_id: LayerId) -> Option<&mut LayerNode> {
        find_modifier_host_in_mut(&mut self.root.children.0, modifier_id)
    }

    /// True if `id` refers to a modifier (rather than a layer/group). Cheap
    /// disambiguator for callers that hold a node id and need to dispatch.
    pub fn is_modifier(&self, id: LayerId) -> bool {
        self.find_modifier(id).is_some()
    }

    /// Pixel-buffer accessor that works for any pixel-bearing node — raster
    /// layers and pixel-storing modifiers (today: masks). Returns `None` for
    /// groups, pure-effect modifiers, or unknown ids.
    ///
    /// This is the polymorphic interface the engine uses for paint, transform,
    /// readback, and dirty-tracking — none of those need to know whether the
    /// id refers to a layer or a modifier.
    pub fn pixel_buffer(&self, id: LayerId) -> Option<&PixelBuffer> {
        if let Some(node) = self.find_node(id) {
            return node.pixels();
        }
        self.find_modifier(id).and_then(|m| m.pixels())
    }

    pub fn pixel_buffer_mut(&mut self, id: LayerId) -> Option<&mut PixelBuffer> {
        if find_node_in(&self.root.children.0, id)
            .and_then(|n| n.pixels())
            .is_some()
        {
            return find_node_in_mut(&mut self.root.children.0, id).and_then(|n| n.pixels_mut());
        }
        find_modifier_in_mut(&mut self.root.children.0, id).and_then(|m| m.pixels_mut())
    }

    pub fn parent_of(&self, id: LayerId) -> Option<LayerId> {
        // For a regular tree node: walk children. For a modifier: return its host.
        if let Some(host) = self.find_modifier_host(id) {
            return Some(host.id());
        }
        find_parent_of(&self.root.children.0, id)
    }

    /// Index of a node within its parent container (root list or group children).
    pub fn position_in_parent(&self, id: LayerId) -> Option<usize> {
        let parent = self.parent_of(id);
        match parent {
            Some(pid) => {
                if let Some(LayerNode::Group(g)) = find_node_in(&self.root.children.0, pid) {
                    position_in(&g.children.0, id)
                } else {
                    None
                }
            }
            None => position_in(&self.root.children.0, id),
        }
    }

    /// Detach a node from the tree for undo purposes.
    pub fn detach_for_undo(&mut self, id: LayerId) -> Option<LayerNode> {
        detach_node(&mut self.root.children.0, id)
    }

    /// Reinsert a previously detached node at a specific position.
    pub fn reinsert_node(&mut self, node: LayerNode, parent: Option<LayerId>, position: usize) {
        match parent {
            Some(pid) => {
                if let Some(LayerNode::Group(g)) = find_node_in_mut(&mut self.root.children.0, pid)
                {
                    let pos = position.min(g.children.0.len());
                    g.children.0.insert(pos, node);
                } else {
                    let pos = position.min(self.root.children.0.len());
                    self.root.children.0.insert(pos, node);
                }
            }
            None => {
                let pos = position.min(self.root.children.0.len());
                self.root.children.0.insert(pos, node);
            }
        }
    }

    /// Move a node to a new position in the tree.
    pub fn move_layer(&mut self, layer_id: LayerId, target: MoveTarget) {
        let node = match detach_node(&mut self.root.children.0, layer_id) {
            Some(n) => n,
            None => return,
        };
        self.insert_node(node, target);
    }

    fn insert_node(&mut self, node: LayerNode, target: MoveTarget) {
        match target {
            MoveTarget::Before(ref_id) => {
                if let Some(path) = find_position(&self.root.children.0, ref_id) {
                    let idx = *path.last().unwrap();
                    let container = container_at_path(&mut self.root.children.0, &path);
                    container.insert(idx, node);
                } else {
                    self.root.children.0.push(node);
                }
            }
            MoveTarget::After(ref_id) => {
                if let Some(path) = find_position(&self.root.children.0, ref_id) {
                    let idx = *path.last().unwrap();
                    let container = container_at_path(&mut self.root.children.0, &path);
                    container.insert(idx + 1, node);
                } else {
                    self.root.children.0.push(node);
                }
            }
            MoveTarget::IntoGroupTop(group_id) => {
                if let Some(LayerNode::Group(g)) =
                    find_node_in_mut(&mut self.root.children.0, group_id)
                {
                    g.children.0.push(node);
                } else {
                    self.root.children.0.push(node);
                }
            }
            MoveTarget::IntoGroupBottom(group_id) => {
                if let Some(LayerNode::Group(g)) =
                    find_node_in_mut(&mut self.root.children.0, group_id)
                {
                    g.children.0.insert(0, node);
                } else {
                    self.root.children.0.push(node);
                }
            }
        }
    }

    /// Remove a node (layer or group) from the tree. To remove a modifier, use
    /// [`Document::remove_modifier`].
    pub fn remove_node(&mut self, id: LayerId) {
        detach_node(&mut self.root.children.0, id);
    }

    // --- Modifier operations ---

    /// Add a [`MaskModifier`] to a host node, returning the new modifier's id.
    /// Bounds default to the host's pixel bounds (raster) or canvas (group).
    /// Returns `None` if the host id is unknown.
    ///
    /// Note: only one mask per host is enforced at the UI layer, not here —
    /// the model supports N. Callers that want the singleton invariant should
    /// check `host.modifiers().mask().is_some()` before adding.
    pub fn add_mask_modifier(&mut self, host_id: LayerId) -> Option<LayerId> {
        let bounds = self.host_default_bounds(host_id)?;
        let mod_id = self.alloc_id();
        let modifier = Modifier {
            id: mod_id,
            common: NodeCommon::new(format!("Mask {mod_id}")),
            kind: ModifierKind::mask_with_bounds(bounds),
        };
        let host = find_node_in_mut(&mut self.root.children.0, host_id)?;
        host.modifiers_mut().0.push(modifier);
        Some(mod_id)
    }

    /// Remove a modifier from its host by id, returning the detached `Modifier`
    /// for undo purposes. Returns `None` if no modifier with that id exists.
    pub fn remove_modifier(&mut self, modifier_id: LayerId) -> Option<Modifier> {
        detach_modifier(&mut self.root.children.0, modifier_id)
    }

    /// Reattach a previously detached modifier to a host.
    pub fn reinsert_modifier(&mut self, host_id: LayerId, modifier: Modifier) {
        if let Some(host) = find_node_in_mut(&mut self.root.children.0, host_id) {
            host.modifiers_mut().0.push(modifier);
        }
    }

    fn host_default_bounds(&self, host_id: LayerId) -> Option<CanvasRect> {
        match self.find_node(host_id)? {
            LayerNode::Layer(Layer::Raster(r)) => Some(r.pixels.bounds),
            LayerNode::Group(_) => Some(CanvasRect::from_xywh(0, 0, self.width, self.height)),
        }
    }

    /// Allocate the global selection modifier if not already present, sized
    /// to the canvas. Idempotent — returns the modifier id either way. The
    /// caller is responsible for matching GPU state in the compositor.
    pub fn ensure_selection_modifier(&mut self) -> LayerId {
        if let Some(s) = self.selection.as_ref() {
            return s.id;
        }
        let id = self.alloc_id();
        let bounds = CanvasRect::from_xywh(0, 0, self.width, self.height);
        let modifier = Modifier {
            id,
            common: NodeCommon::new("Selection".to_string()),
            kind: ModifierKind::selection_with_bounds(bounds),
        };
        // Per the plan §4a, the selection lives "at the document root rather
        // than on a host's `modifiers` list" — store it on the doc directly.
        // Default `visible = false` mirrors today's "always allocated, .active
        // toggles whether ops respect it" semantics for an empty selection.
        let mut modifier = modifier;
        modifier.common.visible = false;
        self.selection = Some(modifier);
        id
    }

    /// Selection modifier id, if allocated.
    pub fn selection_id(&self) -> Option<LayerId> {
        self.selection.as_ref().map(|m| m.id)
    }

    /// True when the selection modifier is allocated AND its `common.visible`
    /// flag is set — equivalent to today's `gpu_selection.active`.
    pub fn selection_active(&self) -> bool {
        self.selection.as_ref().is_some_and(|m| m.common.visible)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_layers() {
        let mut doc = Document::new(256, 256);
        let id = doc.add_raster_layer();
        assert_eq!(doc.flat_layers().len(), 1);
        assert_eq!(doc.flat_layers()[0].id(), id);
    }

    #[test]
    fn flat_layers_respects_group_visibility() {
        let mut doc = Document::new(256, 256);
        let l1 = doc.add_raster_layer();
        let g1 = doc.add_group();
        let l2 = doc.add_raster_layer_in(Some(g1));

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
        let l1 = doc.add_raster_layer();
        let g1 = doc.add_group();
        let l2 = doc.add_raster_layer_in(Some(g1));

        doc.move_layer(l2, MoveTarget::Before(l1));
        let flat = doc.flat_layers();
        assert_eq!(flat[0].id(), l2);
        assert_eq!(flat[1].id(), l1);
    }

    #[test]
    fn nested_groups_flatten_correctly() {
        let mut doc = Document::new(256, 256);
        let l1 = doc.add_raster_layer();
        let g1 = doc.add_group();
        let l2 = doc.add_raster_layer_in(Some(g1));

        let flat = doc.flat_layers();
        assert_eq!(flat.len(), 2);
        assert_eq!(flat[0].id(), l1);
        assert_eq!(flat[1].id(), l2);
    }

    #[test]
    fn remove_group_removes_children() {
        let mut doc = Document::new(256, 256);
        let g1 = doc.add_group();
        let _l1 = doc.add_raster_layer_in(Some(g1));

        doc.remove_node(g1);
        assert!(doc.flat_layers().is_empty());
    }

    #[test]
    fn detach_insert_roundtrip() {
        let mut doc = Document::new(256, 256);
        let l1 = doc.add_raster_layer();
        let l2 = doc.add_raster_layer();
        let l3 = doc.add_raster_layer();

        doc.move_layer(l1, MoveTarget::After(l3));
        let flat = doc.flat_layers();
        assert_eq!(flat[0].id(), l2);
        assert_eq!(flat[1].id(), l3);
        assert_eq!(flat[2].id(), l1);
    }

    #[test]
    fn add_modifier_attaches_to_host() {
        let mut doc = Document::new(256, 256);
        let l = doc.add_raster_layer();
        assert!(doc.layer(l).unwrap().modifiers().is_empty());

        let mod_id = doc.add_mask_modifier(l).unwrap();
        let layer = doc.layer(l).unwrap();
        assert_eq!(layer.modifiers().len(), 1);
        assert_eq!(layer.modifiers().mask().unwrap().id, mod_id);

        assert!(doc.is_modifier(mod_id));
        assert!(!doc.is_modifier(l));
    }

    #[test]
    fn remove_modifier_detaches() {
        let mut doc = Document::new(256, 256);
        let l = doc.add_raster_layer();
        let mod_id = doc.add_mask_modifier(l).unwrap();

        let detached = doc.remove_modifier(mod_id);
        assert!(detached.is_some());
        assert_eq!(detached.unwrap().id, mod_id);
        assert!(doc.layer(l).unwrap().modifiers().is_empty());
        assert!(!doc.is_modifier(mod_id));
    }

    #[test]
    fn pixel_buffer_dispatches_layer_or_modifier() {
        let mut doc = Document::new(256, 256);
        let l = doc.add_raster_layer();
        let mod_id = doc.add_mask_modifier(l).unwrap();

        let layer_buf = doc.pixel_buffer(l).unwrap();
        assert_eq!(layer_buf.format, wgpu::TextureFormat::Rgba8Unorm);

        let mask_buf = doc.pixel_buffer(mod_id).unwrap();
        assert_eq!(mask_buf.format, wgpu::TextureFormat::R8Unorm);

        let g = doc.add_group();
        assert!(doc.pixel_buffer(g).is_none());
    }

    #[test]
    fn modifier_host_lookup() {
        let mut doc = Document::new(256, 256);
        let l = doc.add_raster_layer();
        let mod_id = doc.add_mask_modifier(l).unwrap();

        let host = doc.find_modifier_host(mod_id).unwrap();
        assert_eq!(host.id(), l);
        assert_eq!(doc.parent_of(mod_id), Some(l));
    }
}
