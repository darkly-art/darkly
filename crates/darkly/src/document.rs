use crate::dirty::DirtyRegion;
use crate::layer::*;
use crate::paint::{self, MaskPaintTarget, PaintTarget, Surface};
use crate::tile::{AlphaF32, AlphaMask, Memento, Rgba, TILE_SIZE, TileGrid};
use std::collections::HashMap;

/// What kind of tile data was captured during a transaction.
/// Returned by `commit_transaction` so the undo system can handle both
/// layer tiles and mask tiles with one action type.
pub enum TransactionMemento {
    Tiles(HashMap<LayerId, Memento<Rgba>>),
    Mask(LayerId, Memento<AlphaF32>),
}

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
    pub dirty: HashMap<LayerId, DirtyRegion>,
    /// Dirty regions for layer mask tiles (separate from layer tile dirty).
    pub mask_dirty: HashMap<LayerId, DirtyRegion>,
    pub selection: Option<AlphaMask>,
    /// Which layer (if any) is having its mask edited instead of its pixels.
    /// Set by the engine before beginning a stroke, cleared after committing.
    mask_editing: Option<LayerId>,
    next_id: LayerId,
}

// --- Tree traversal helpers (free functions for borrow-splitting) ---

fn find_layer_in<'a>(nodes: &'a [LayerNode], id: LayerId) -> Option<&'a Layer> {
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

fn find_layer_in_mut<'a>(nodes: &'a mut [LayerNode], id: LayerId) -> Option<&'a mut Layer> {
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

fn find_node_in<'a>(nodes: &'a [LayerNode], id: LayerId) -> Option<&'a LayerNode> {
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

fn find_node_in_mut<'a>(nodes: &'a mut [LayerNode], id: LayerId) -> Option<&'a mut LayerNode> {
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

fn find_raster_in_mut<'a>(nodes: &'a mut [LayerNode], id: LayerId) -> Option<&'a mut RasterLayer> {
    for node in nodes {
        match node {
            LayerNode::Layer(Layer::Raster(r)) if r.id == id => return Some(r),
            LayerNode::Group(g) => {
                if let Some(r) = find_raster_in_mut(&mut g.children, id) {
                    return Some(r);
                }
            }
            _ => {}
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
                if !g.visible {
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

/// Recursively collect all IDs under a node (including the node itself).
fn collect_all_ids(node: &LayerNode, out: &mut Vec<LayerId>) {
    out.push(node.id());
    if let LayerNode::Group(g) = node {
        for child in &g.children {
            collect_all_ids(child, out);
        }
    }
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

fn collect_raster_ids(node: &LayerNode, out: &mut Vec<LayerId>) {
    match node {
        LayerNode::Layer(Layer::Raster(r)) => out.push(r.id),
        LayerNode::Group(g) => {
            for child in &g.children {
                collect_raster_ids(child, out);
            }
        }
    }
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
            dirty: HashMap::new(),
            mask_dirty: HashMap::new(),
            selection: None,
            mask_editing: None,
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
        let layer = RasterLayer::new(id);
        self.root.children.push(LayerNode::Layer(Layer::Raster(layer)));
        self.dirty.insert(id, DirtyRegion::new());
        id
    }

    /// Add a new raster layer inside a group (or at root if parent is None).
    pub fn add_raster_layer_in(&mut self, parent: Option<LayerId>) -> LayerId {
        let id = self.alloc_id();
        let layer = RasterLayer::new(id);
        let node = LayerNode::Layer(Layer::Raster(layer));
        self.dirty.insert(id, DirtyRegion::new());

        match parent {
            Some(group_id) => {
                if let Some(LayerNode::Group(g)) = find_node_in_mut(&mut self.root.children, group_id) {
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
    /// Used for tile upload — we keep GPU textures in sync even for hidden layers.
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
    /// Removes the node and cleans up dirty regions, returning the detached node.
    pub fn detach_for_undo(&mut self, id: LayerId) -> Option<LayerNode> {
        let node = detach_node(&mut self.root.children, id)?;
        let mut ids = Vec::new();
        collect_all_ids(&node, &mut ids);
        for removed_id in ids {
            self.dirty.remove(&removed_id);
        }
        Some(node)
    }

    /// Reinsert a previously detached node at a specific position.
    /// Sets up dirty regions and marks all tiles dirty for GPU upload.
    pub fn reinsert_node(&mut self, node: LayerNode, parent: Option<LayerId>, position: usize) {
        // Collect raster layer IDs before we move the node into the tree.
        let mut raster_ids = Vec::new();
        collect_raster_ids(&node, &mut raster_ids);

        // Insert into the tree.
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

        // Set up dirty regions and mark all existing tiles for upload.
        for &id in &raster_ids {
            let dirty = self.dirty.entry(id).or_insert_with(DirtyRegion::new);
            if let Some(Layer::Raster(r)) = find_layer_in(&self.root.children, id) {
                for ((tx, ty), _) in r.tiles.iter() {
                    dirty.mark(tx, ty);
                }
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
                if let Some(LayerNode::Group(g)) = find_node_in_mut(&mut self.root.children, group_id) {
                    g.children.push(node);
                } else {
                    self.root.children.push(node);
                }
            }
            MoveTarget::IntoGroupBottom(group_id) => {
                if let Some(LayerNode::Group(g)) = find_node_in_mut(&mut self.root.children, group_id) {
                    g.children.insert(0, node);
                } else {
                    self.root.children.push(node);
                }
            }
        }
    }

    /// Remove a node (layer or group) from the tree. Also removes dirty regions.
    pub fn remove_node(&mut self, id: LayerId) {
        if let Some(node) = detach_node(&mut self.root.children, id) {
            let mut ids = Vec::new();
            collect_all_ids(&node, &mut ids);
            for removed_id in ids {
                self.dirty.remove(&removed_id);
            }
        }
    }

    /// Borrow-split factory that returns a `Surface` — either layer tiles or mask,
    /// depending on `mask_editing`. Callers never branch on surface type.
    fn make_surface<'a>(
        layers: &'a mut Vec<LayerNode>,
        dirty: &'a mut HashMap<LayerId, DirtyRegion>,
        mask_dirty: &'a mut HashMap<LayerId, DirtyRegion>,
        selection: Option<&'a AlphaMask>,
        mask_editing: Option<LayerId>,
        layer_id: LayerId,
    ) -> Option<Surface<'a>> {
        if mask_editing == Some(layer_id) {
            let raster = find_raster_in_mut(layers, layer_id)?;
            let mask = raster.mask.as_mut()?;
            let dirty_region = mask_dirty.get_mut(&layer_id)?;
            Some(Surface::Mask(MaskPaintTarget::new(mask, dirty_region, selection)))
        } else {
            let raster = find_raster_in_mut(layers, layer_id)?;
            let dirty_region = dirty.get_mut(&layer_id)?;
            Some(Surface::Layer(PaintTarget::new(&mut raster.tiles, dirty_region, selection)))
        }
    }

    /// Get a mutable reference to the raster layer's tiles and the dirty region simultaneously.
    /// Uses borrow splitting: layers and dirty are separate fields.
    /// Used by operations that need raw tile access without selection masking (e.g. fill_gradient).
    fn raster_tiles_and_dirty<'a>(
        layers: &'a mut Vec<LayerNode>,
        dirty: &'a mut HashMap<LayerId, DirtyRegion>,
        layer_id: LayerId,
    ) -> Option<(&'a mut TileGrid, &'a mut DirtyRegion)> {
        let raster = find_raster_in_mut(layers, layer_id)?;
        let dirty_region = dirty.get_mut(&layer_id)?;
        Some((&mut raster.tiles, dirty_region))
    }

    /// Paint a filled circle on a raster layer (or its mask when mask_editing is set).
    pub fn paint_circle(&mut self, layer_id: LayerId, cx: f32, cy: f32, radius: f32, color: [u8; 4]) {
        if let Some(mut s) = Self::make_surface(
            &mut self.root.children, &mut self.dirty, &mut self.mask_dirty,
            self.selection.as_ref(), self.mask_editing, layer_id,
        ) {
            paint::paint_circle(&mut s, cx, cy, radius, color);
        }
    }

    /// Erase a filled circle on a raster layer (or its mask when mask_editing is set).
    pub fn erase_circle(&mut self, layer_id: LayerId, cx: f32, cy: f32, radius: f32) {
        if let Some(mut s) = Self::make_surface(
            &mut self.root.children, &mut self.dirty, &mut self.mask_dirty,
            self.selection.as_ref(), self.mask_editing, layer_id,
        ) {
            paint::erase_circle(&mut s, cx, cy, radius);
        }
    }

    /// Flood fill from a seed point with a color, using tolerance-based matching.
    pub fn flood_fill(&mut self, layer_id: LayerId, seed_x: i32, seed_y: i32, color: [u8; 4], tolerance: u8) {
        if seed_x < 0 || seed_y < 0 || seed_x >= self.width as i32 || seed_y >= self.height as i32 {
            return;
        }
        let w = self.width as i32;
        let h = self.height as i32;
        if let Some(mut s) = Self::make_surface(
            &mut self.root.children, &mut self.dirty, &mut self.mask_dirty,
            self.selection.as_ref(), self.mask_editing, layer_id,
        ) {
            paint::flood_fill(&mut s, seed_x, seed_y, w, h, color, tolerance);
        }
    }

    /// Clear (erase to transparent) all pixels within the current selection on a raster layer.
    /// Iterates only over tiles where the selection mask exists, for efficiency.
    pub fn clear_selection_contents(&mut self, layer_id: LayerId) {
        if self.selection.is_none() {
            return;
        }

        let ts = TILE_SIZE as i32;
        let tile_keys: Vec<(i32, i32)> = self.selection.as_ref().unwrap()
            .iter().map(|((tx, ty), _)| (tx, ty)).collect();

        let mut surface = match Self::make_surface(
            &mut self.root.children, &mut self.dirty, &mut self.mask_dirty,
            self.selection.as_ref(), self.mask_editing, layer_id,
        ) {
            Some(s) => s,
            None => return,
        };

        for (tx, ty) in tile_keys {
            let base_x = tx * ts;
            let base_y = ty * ts;
            for ly in 0..TILE_SIZE {
                for lx in 0..TILE_SIZE {
                    surface.erase(base_x + lx as i32, base_y + ly as i32, 1.0);
                }
            }
        }
    }

    /// Draw a linear gradient between two points on a raster layer (or its mask).
    pub fn linear_gradient(
        &mut self,
        layer_id: LayerId,
        x0: f32, y0: f32, x1: f32, y1: f32,
        color0: [u8; 4], color1: [u8; 4],
    ) {
        let width = self.width;
        let height = self.height;
        if let Some(mut s) = Self::make_surface(
            &mut self.root.children, &mut self.dirty, &mut self.mask_dirty,
            self.selection.as_ref(), self.mask_editing, layer_id,
        ) {
            paint::linear_gradient(&mut s, x0, y0, x1, y1, color0, color1, width, height);
        }
    }

    /// Begin recording tile changes on a raster layer (or its mask) for undo.
    /// Routes to the mask automatically when `mask_editing` is set.
    pub fn begin_transaction(&mut self, layer_id: LayerId) {
        let editing_mask = self.mask_editing == Some(layer_id);
        if let Some(Layer::Raster(r)) = self.layer_mut(layer_id) {
            if editing_mask {
                if let Some(mask) = &mut r.mask {
                    mask.begin_transaction();
                }
            } else {
                r.tiles.begin_transaction();
            }
        }
    }

    /// Commit the active transaction and return a memento for undo.
    /// Returns the appropriate variant depending on whether we're editing
    /// the mask or the layer tiles.
    pub fn commit_transaction(&mut self, layer_id: LayerId) -> Option<TransactionMemento> {
        let editing_mask = self.mask_editing == Some(layer_id);
        if let Some(Layer::Raster(r)) = self.layer_mut(layer_id) {
            if editing_mask {
                if let Some(mask) = &mut r.mask {
                    return mask.commit_transaction()
                        .map(|m| TransactionMemento::Mask(layer_id, m));
                }
            } else {
                if let Some(memento) = r.tiles.commit_transaction() {
                    let mut mementos = HashMap::new();
                    mementos.insert(layer_id, memento);
                    return Some(TransactionMemento::Tiles(mementos));
                }
            }
        }
        None
    }

    /// Apply a shape mask to the document selection using the given mode.
    pub fn apply_selection(&mut self, shape_mask: AlphaMask, mode: SelectionMode) {
        match mode {
            SelectionMode::Replace => {
                self.selection = Some(shape_mask);
            }
            SelectionMode::Add => {
                match &mut self.selection {
                    Some(sel) => sel.boolean_add(&shape_mask),
                    None => self.selection = Some(shape_mask),
                }
            }
            SelectionMode::Subtract => {
                if let Some(sel) = &mut self.selection {
                    sel.boolean_subtract(&shape_mask);
                }
            }
            SelectionMode::Intersect => {
                match &mut self.selection {
                    Some(sel) => sel.boolean_intersect(&shape_mask),
                    None => {} // intersect with nothing = nothing
                }
            }
        }
    }

    /// Tell the document which layer (if any) should route operations to its mask.
    pub fn set_mask_editing(&mut self, layer_id: Option<LayerId>) {
        self.mask_editing = layer_id;
    }

    // --- Layer Mask Operations ---

    /// Add a white (reveal-all) mask to a raster layer. Returns the previous mask state.
    pub fn add_mask(&mut self, layer_id: LayerId) -> Option<AlphaMask> {
        let raster = find_raster_in_mut(&mut self.root.children, layer_id)?;
        let old = raster.mask.take();
        raster.mask = Some(AlphaMask::new()); // empty store; get_or_create_full provides 1.0 default
        raster.mask_enabled = true;
        raster.show_mask = false;
        self.mask_dirty.entry(layer_id).or_insert_with(DirtyRegion::new);
        old
    }

    /// Remove the mask from a raster layer. Returns the removed mask.
    pub fn remove_mask(&mut self, layer_id: LayerId) -> Option<AlphaMask> {
        let raster = find_raster_in_mut(&mut self.root.children, layer_id)?;
        let old = raster.mask.take();
        raster.mask_enabled = true;
        raster.show_mask = false;
        self.mask_dirty.remove(&layer_id);
        old
    }

    pub fn set_mask_enabled(&mut self, layer_id: LayerId, enabled: bool) {
        if let Some(LayerNode::Layer(Layer::Raster(r))) = find_node_in_mut(&mut self.root.children, layer_id) {
            r.mask_enabled = enabled;
        }
    }

    pub fn set_show_mask(&mut self, layer_id: LayerId, show: bool) {
        if let Some(LayerNode::Layer(Layer::Raster(r))) = find_node_in_mut(&mut self.root.children, layer_id) {
            r.show_mask = show;
        }
    }



    /// Convert the current selection to a layer mask (replaces existing mask).
    pub fn selection_to_mask(&mut self, layer_id: LayerId) {
        let sel = match &self.selection {
            Some(s) => s.clone(),
            None => return,
        };
        if let Some(Layer::Raster(r)) = self.layer_mut(layer_id) {
            r.mask = Some(sel);
            r.mask_enabled = true;
            r.show_mask = false;
            self.mask_dirty.entry(layer_id).or_insert_with(DirtyRegion::new);
        }
    }

    /// Convert a layer mask to a selection (replaces current selection).
    pub fn mask_to_selection(&mut self, layer_id: LayerId) {
        let mask_clone = match self.layer(layer_id) {
            Some(Layer::Raster(r)) => r.mask.clone(),
            _ => None,
        };
        if let Some(mask) = mask_clone {
            self.selection = Some(mask);
        }
    }

    /// Destructively apply the mask to the layer's alpha channel, then remove the mask.
    /// Each pixel's alpha is multiplied by the mask value.
    pub fn apply_mask_destructive(&mut self, layer_id: LayerId) {
        // First, collect the mask tile data we need
        let mask_tiles: Vec<((i32, i32), Vec<f32>)> = match self.layer(layer_id) {
            Some(Layer::Raster(r)) => match &r.mask {
                Some(mask) => mask.iter().map(|((tx, ty), tile)| {
                    ((tx, ty), tile.data().0.to_vec())
                }).collect(),
                None => return,
            },
            _ => return,
        };

        let raster = match find_raster_in_mut(&mut self.root.children, layer_id) {
            Some(r) => r,
            None => return,
        };

        let dirty = self.dirty.entry(layer_id).or_insert_with(DirtyRegion::new);

        // Multiply layer alpha by mask value for tiles that exist in the mask
        for ((tx, ty), mask_data) in &mask_tiles {
            if let Some(tile) = raster.tiles.get(tx.clone(), ty.clone()) {
                let _ = tile; // just check existence
            } else {
                continue; // no layer tile = nothing to multiply
            }
            let layer_tile = raster.tiles.get_or_create(*tx, *ty);
            let data = layer_tile.write();
            for ly in 0..TILE_SIZE {
                for lx in 0..TILE_SIZE {
                    let mask_val = mask_data[ly * TILE_SIZE + lx];
                    if mask_val < 1.0 {
                        let px = data.pixel_mut(lx, ly);
                        px[3] = (px[3] as f32 * mask_val).round() as u8;
                    }
                }
            }
            dirty.mark(*tx, *ty);
        }

        // Remove the mask
        raster.mask = None;
        raster.mask_enabled = true;
        raster.show_mask = false;
        self.mask_dirty.remove(&layer_id);
    }

    /// Fill a raster layer with a horizontal gradient (demo helper).
    pub fn fill_gradient(&mut self, layer_id: LayerId) {
        let width = self.width;
        let height = self.height;

        let (tiles, dirty) =
            match Self::raster_tiles_and_dirty(&mut self.root.children, &mut self.dirty, layer_id) {
                Some(v) => v,
                None => return,
            };

        let tile_size = TILE_SIZE as i32;
        let tiles_x = (width as i32 + tile_size - 1) / tile_size;
        let tiles_y = (height as i32 + tile_size - 1) / tile_size;

        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let tile = tiles.get_or_create(tx, ty);
                let data = tile.write();
                for ly in 0..TILE_SIZE {
                    for lx in 0..TILE_SIZE {
                        let px = tx * tile_size + lx as i32;
                        let py = ty * tile_size + ly as i32;
                        if px < width as i32 && py < height as i32 {
                            let t = px as f32 / width as f32;
                            let r = (40.0 + t * 80.0) as u8;
                            let g = (20.0 + t * 40.0) as u8;
                            let b = (80.0 + t * 120.0) as u8;
                            data.pixel_mut(lx, ly).copy_from_slice(&[r, g, b, 255]);
                        }
                    }
                }
                dirty.mark(tx, ty);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_layers_and_paint() {
        let mut doc = Document::new(256, 256);
        let id = doc.add_raster_layer();
        doc.paint_circle(id, 32.0, 32.0, 5.0, [255, 0, 0, 255]);

        let dirty = doc.dirty.get(&id).unwrap();
        assert!(!dirty.is_empty());

        if let Some(Layer::Raster(r)) = doc.layer(id) {
            let tile = r.tiles.get(0, 0).unwrap();
            let px = tile.data().pixel(32, 32);
            assert_eq!(px, &[255, 0, 0, 255]);
        } else {
            panic!("Layer not found");
        }
    }

    #[test]
    fn fill_gradient_marks_dirty() {
        let ts = TILE_SIZE as u32;
        let canvas = ts * 2;
        let mut doc = Document::new(canvas, canvas);
        let id = doc.add_raster_layer();
        doc.fill_gradient(id);

        let dirty = doc.dirty.get(&id).unwrap();
        assert_eq!(dirty.len(), 4);

        if let Some(Layer::Raster(r)) = doc.layer(id) {
            let tile = r.tiles.get(0, 0).unwrap();
            let px = tile.data().pixel(0, 0);
            assert_eq!(px[3], 255);
        }
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
            g.visible = false;
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
        let g1 = doc.add_group();        // root group
        let l2 = doc.add_raster_layer_in(Some(g1)); // inside g1

        let flat = doc.flat_layers();
        assert_eq!(flat.len(), 2);
        assert_eq!(flat[0].id(), l1);
        assert_eq!(flat[1].id(), l2);
    }

    #[test]
    fn remove_group_removes_children_dirty() {
        let mut doc = Document::new(256, 256);
        let g1 = doc.add_group();
        let l1 = doc.add_raster_layer_in(Some(g1));

        assert!(doc.dirty.contains_key(&l1));
        doc.remove_node(g1);
        assert!(!doc.dirty.contains_key(&l1));
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
}
