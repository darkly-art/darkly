use crate::layer::*;
use crate::tile::AlphaMask;

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
    /// CPU-side selection mask. Kept on CPU because selection operations
    /// (boolean add/subtract/intersect, contour extraction for marching ants,
    /// feathering, SDF rasterization) are infrequent, irregular-shaped, and
    /// need random-access CPU reads that don't justify a GPU round-trip.
    pub selection: Option<AlphaMask>,
    next_id: LayerId,
}

// --- Tree traversal helpers (free functions for borrow-splitting) ---

fn find_layer_in(nodes: &[LayerNode], id: LayerId) -> Option<&Layer> {
    for node in nodes {
        match node {
            LayerNode::Layer(l) if l.id() == id => return Some(l),
            LayerNode::Group(g) => {
                if let Some(l) = find_layer_in(&g.children, id) {
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
                if let Some(l) = find_layer_in_mut(&mut g.children, id) {
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
            if let Some(n) = find_node_in(&g.children, id) {
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
            if let Some(n) = find_node_in_mut(&mut g.children, id) {
                return Some(n);
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
            if let Some(n) = detach_node(&mut g.children, id) {
                return Some(n);
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
            if let Some(mut path) = find_position(&g.children, id) {
                path.insert(0, i);
                return Some(path);
            }
        }
    }
    None
}

/// Get mutable reference to the container (Vec<LayerNode>) at a path.
/// Path should have at least one element; the last element is the index within the container.
fn container_at_path<'a>(nodes: &'a mut Vec<LayerNode>, path: &[usize]) -> &'a mut Vec<LayerNode> {
    if path.len() <= 1 {
        return nodes;
    }
    // Check if the node at path[0] is a group before borrowing mutably
    let is_group = matches!(&nodes[path[0]], LayerNode::Group(_));
    if is_group {
        match &mut nodes[path[0]] {
            LayerNode::Group(g) => container_at_path(&mut g.children, &path[1..]),
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
                flatten_nodes(&g.children, out);
            }
        }
    }
}

fn collect_raster_layers<'a>(nodes: &'a [LayerNode], out: &mut Vec<&'a RasterLayer>) {
    for node in nodes {
        match node {
            LayerNode::Layer(Layer::Raster(r)) => out.push(r),
            LayerNode::Group(g) => collect_raster_layers(&g.children, out),
        }
    }
}

fn collect_groups<'a>(nodes: &'a [LayerNode], out: &mut Vec<&'a LayerGroup>) {
    for node in nodes {
        if let LayerNode::Group(g) = node {
            out.push(g);
            collect_groups(&g.children, out);
        }
    }
}

fn find_parent_of(nodes: &[LayerNode], id: LayerId) -> Option<LayerId> {
    for node in nodes {
        if let LayerNode::Group(g) = node {
            for child in &g.children {
                if child.id() == id {
                    return Some(g.id);
                }
            }
            if let Some(parent) = find_parent_of(&g.children, id) {
                return Some(parent);
            }
        }
    }
    None
}

/// Recursively collect all raster layer IDs under a node.
fn count_nodes(nodes: &[LayerNode]) -> usize {
    let mut count = 0;
    for node in nodes {
        count += 1;
        if let LayerNode::Group(g) = node {
            count += count_nodes(&g.children);
        }
    }
    count
}

/// Find the index of a node within its immediate parent container.
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
        let layer = RasterLayer::new(
            id,
            crate::coord::CanvasRect::from_xywh(0, 0, self.width, self.height),
        );
        self.root
            .children
            .push(LayerNode::Layer(Layer::Raster(layer)));
        id
    }

    /// Add a new raster layer inside a group (or at root if parent is None).
    pub fn add_raster_layer_in(&mut self, parent: Option<LayerId>) -> LayerId {
        let id = self.alloc_id();
        let layer = RasterLayer::new(
            id,
            crate::coord::CanvasRect::from_xywh(0, 0, self.width, self.height),
        );
        let node = LayerNode::Layer(Layer::Raster(layer));

        match parent {
            Some(group_id) => {
                if let Some(LayerNode::Group(g)) =
                    find_node_in_mut(&mut self.root.children, group_id)
                {
                    g.children.push(node);
                } else {
                    self.root.children.push(node);
                }
            }
            None => self.root.children.push(node),
        }
        id
    }

    /// Add a new empty group at the root top.
    pub fn add_group(&mut self) -> LayerId {
        let id = self.alloc_id();
        let group = LayerGroup::new(id);
        self.root.children.push(LayerNode::Group(group));
        id
    }

    /// Flatten the layer tree into display order (bottom-to-top) for compositing.
    /// Hidden groups exclude all children. Passthrough groups flatten children directly.
    pub fn flat_layers(&self) -> Vec<&Layer> {
        let mut out = Vec::new();
        flatten_nodes(&self.root.children, &mut out);
        out
    }

    /// Get all raster layers in the tree (regardless of visibility).
    /// Used for GPU sync — we keep GPU textures in sync even for hidden layers.
    pub fn all_raster_layers(&self) -> Vec<&RasterLayer> {
        let mut out = Vec::new();
        collect_raster_layers(&self.root.children, &mut out);
        out
    }

    pub fn all_groups(&self) -> Vec<&LayerGroup> {
        let mut out = Vec::new();
        collect_groups(&self.root.children, &mut out);
        out
    }

    /// Compute the flat (display order) index of a layer by id.
    /// Count all nodes (layers + groups) in the tree.
    pub fn node_count(&self) -> usize {
        count_nodes(&self.root.children)
    }

    pub fn flat_layer_index(&self, id: LayerId) -> Option<usize> {
        self.flat_layers().iter().position(|l| l.id() == id)
    }

    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        find_layer_in(&self.root.children, id)
    }

    pub fn layer_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        find_layer_in_mut(&mut self.root.children, id)
    }

    pub fn find_node(&self, id: LayerId) -> Option<&LayerNode> {
        find_node_in(&self.root.children, id)
    }

    pub fn find_node_mut(&mut self, id: LayerId) -> Option<&mut LayerNode> {
        find_node_in_mut(&mut self.root.children, id)
    }

    pub fn parent_of(&self, id: LayerId) -> Option<LayerId> {
        find_parent_of(&self.root.children, id)
    }

    /// Index of a node within its parent container (root list or group children).
    pub fn position_in_parent(&self, id: LayerId) -> Option<usize> {
        let parent = self.parent_of(id);
        match parent {
            Some(pid) => {
                if let Some(LayerNode::Group(g)) = find_node_in(&self.root.children, pid) {
                    position_in(&g.children, id)
                } else {
                    None
                }
            }
            None => position_in(&self.root.children, id),
        }
    }

    /// Detach a node from the tree for undo purposes.
    pub fn detach_for_undo(&mut self, id: LayerId) -> Option<LayerNode> {
        detach_node(&mut self.root.children, id)
    }

    /// Reinsert a previously detached node at a specific position.
    pub fn reinsert_node(&mut self, node: LayerNode, parent: Option<LayerId>, position: usize) {
        match parent {
            Some(pid) => {
                if let Some(LayerNode::Group(g)) = find_node_in_mut(&mut self.root.children, pid) {
                    let pos = position.min(g.children.len());
                    g.children.insert(pos, node);
                } else {
                    // Parent gone (shouldn't happen) — insert at root.
                    let pos = position.min(self.root.children.len());
                    self.root.children.insert(pos, node);
                }
            }
            None => {
                let pos = position.min(self.root.children.len());
                self.root.children.insert(pos, node);
            }
        }
    }

    /// Move a node to a new position in the tree.
    pub fn move_layer(&mut self, layer_id: LayerId, target: MoveTarget) {
        let node = match detach_node(&mut self.root.children, layer_id) {
            Some(n) => n,
            None => return,
        };
        self.insert_node(node, target);
    }

    fn insert_node(&mut self, node: LayerNode, target: MoveTarget) {
        match target {
            MoveTarget::Before(ref_id) => {
                if let Some(path) = find_position(&self.root.children, ref_id) {
                    let idx = *path.last().unwrap();
                    let container = container_at_path(&mut self.root.children, &path);
                    container.insert(idx, node);
                } else {
                    self.root.children.push(node);
                }
            }
            MoveTarget::After(ref_id) => {
                if let Some(path) = find_position(&self.root.children, ref_id) {
                    let idx = *path.last().unwrap();
                    let container = container_at_path(&mut self.root.children, &path);
                    container.insert(idx + 1, node);
                } else {
                    self.root.children.push(node);
                }
            }
            MoveTarget::IntoGroupTop(group_id) => {
                if let Some(LayerNode::Group(g)) =
                    find_node_in_mut(&mut self.root.children, group_id)
                {
                    g.children.push(node);
                } else {
                    self.root.children.push(node);
                }
            }
            MoveTarget::IntoGroupBottom(group_id) => {
                if let Some(LayerNode::Group(g)) =
                    find_node_in_mut(&mut self.root.children, group_id)
                {
                    g.children.insert(0, node);
                } else {
                    self.root.children.push(node);
                }
            }
        }
    }

    /// Remove a node (layer or group) from the tree.
    pub fn remove_node(&mut self, id: LayerId) {
        detach_node(&mut self.root.children, id);
    }

    /// Apply a shape mask to the document selection using the given mode.
    pub fn apply_selection(&mut self, shape_mask: AlphaMask, mode: SelectionMode) {
        match mode {
            SelectionMode::Replace => {
                self.selection = Some(shape_mask);
            }
            SelectionMode::Add => match &mut self.selection {
                Some(sel) => sel.boolean_add(&shape_mask),
                None => self.selection = Some(shape_mask),
            },
            SelectionMode::Subtract => {
                if let Some(sel) = &mut self.selection {
                    sel.boolean_subtract(&shape_mask);
                }
            }
            SelectionMode::Intersect => {
                // intersect with nothing = nothing
                if let Some(sel) = &mut self.selection {
                    sel.boolean_intersect(&shape_mask);
                }
            }
        }
    }

    // --- Layer Mask Operations ---
    // Mask pixel data is GPU-authoritative. These methods only toggle the
    // boolean flag; the engine is responsible for creating/destroying GPU
    // textures and saving pixel data via RegionStore for undo.

    /// Mark a node as having a mask. Returns the previous has_mask state.
    pub fn add_mask(&mut self, layer_id: LayerId) -> bool {
        let node = match find_node_in_mut(&mut self.root.children, layer_id) {
            Some(n) => n,
            None => return false,
        };
        let c = node.common_mut();
        let old = c.has_mask;
        c.has_mask = true;
        c.mask_enabled = true;
        c.show_mask = false;
        old
    }

    /// Mark a node as not having a mask. Returns the previous has_mask state.
    pub fn remove_mask(&mut self, layer_id: LayerId) -> bool {
        let node = match find_node_in_mut(&mut self.root.children, layer_id) {
            Some(n) => n,
            None => return false,
        };
        let c = node.common_mut();
        let old = c.has_mask;
        c.has_mask = false;
        c.mask_enabled = true;
        c.show_mask = false;
        old
    }

    pub fn set_mask_enabled(&mut self, layer_id: LayerId, enabled: bool) {
        if let Some(node) = find_node_in_mut(&mut self.root.children, layer_id) {
            node.common_mut().mask_enabled = enabled;
        }
    }

    pub fn set_show_mask(&mut self, layer_id: LayerId, show: bool) {
        if let Some(node) = find_node_in_mut(&mut self.root.children, layer_id) {
            node.common_mut().show_mask = show;
        }
    }

    /// Convert the current selection to a layer mask (sets flag only).
    /// The engine is responsible for uploading the selection data to the GPU mask texture.
    pub fn selection_to_mask(&mut self, layer_id: LayerId) {
        if self.selection.is_none() {
            return;
        }
        if let Some(node) = find_node_in_mut(&mut self.root.children, layer_id) {
            let c = node.common_mut();
            c.has_mask = true;
            c.mask_enabled = true;
            c.show_mask = false;
        }
    }

    /// Convert a layer mask to a selection.
    /// The engine handles GPU readback of mask data into an AlphaMask.
    /// This method just clears the mask flag if requested by the caller.
    pub fn mask_to_selection(&mut self, _layer_id: LayerId) {
        // Engine does the GPU readback and calls doc.selection = Some(mask).
        // This is now a no-op on the document side; the engine drives the process.
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

        // Both layers visible
        let flat = doc.flat_layers();
        assert_eq!(flat.len(), 2);
        assert_eq!(flat[0].id(), l1);
        assert_eq!(flat[1].id(), l2);

        // Hide the group — its children should be excluded
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

        // l2 is inside g1. Move it to root before l1.
        doc.move_layer(l2, MoveTarget::Before(l1));
        let flat = doc.flat_layers();
        assert_eq!(flat[0].id(), l2);
        assert_eq!(flat[1].id(), l1);
    }

    #[test]
    fn nested_groups_flatten_correctly() {
        let mut doc = Document::new(256, 256);
        let l1 = doc.add_raster_layer(); // root
        let g1 = doc.add_group(); // root group
        let l2 = doc.add_raster_layer_in(Some(g1)); // inside g1

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

        // Move l1 to after l3 (top)
        doc.move_layer(l1, MoveTarget::After(l3));
        let flat = doc.flat_layers();
        assert_eq!(flat[0].id(), l2);
        assert_eq!(flat[1].id(), l3);
        assert_eq!(flat[2].id(), l1);
    }

    #[test]
    fn mask_flag_operations() {
        let mut doc = Document::new(256, 256);
        let id = doc.add_raster_layer();

        // Initially no mask
        if let Some(Layer::Raster(r)) = doc.layer(id) {
            assert!(!r.common.has_mask);
        }

        // Add mask
        let old = doc.add_mask(id);
        assert!(!old); // was false
        if let Some(Layer::Raster(r)) = doc.layer(id) {
            assert!(r.common.has_mask);
            assert!(r.common.mask_enabled);
        }

        // Remove mask
        let old = doc.remove_mask(id);
        assert!(old); // was true
        if let Some(Layer::Raster(r)) = doc.layer(id) {
            assert!(!r.common.has_mask);
        }
    }
}
