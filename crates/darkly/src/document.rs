use crate::dirty::DirtyRegion;
use crate::layer::*;
use crate::tile::{Memento, Rgba, TILE_SIZE, TileGrid};
use std::collections::HashMap;

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
            _ => {}
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
        _ => {}
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

    pub fn add_filter_layer(&mut self, filter: Box<dyn crate::gpu::filter::Filter>) -> LayerId {
        let id = self.alloc_id();
        let layer = FilterLayer {
            id,
            filter,
            visible: true,
        };
        self.root.children.push(LayerNode::Layer(Layer::Filter(layer)));
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

    /// Get a mutable reference to the raster layer's tiles and the dirty region simultaneously.
    /// Uses borrow splitting: layers and dirty are separate fields.
    fn raster_tiles_and_dirty<'a>(
        layers: &'a mut Vec<LayerNode>,
        dirty: &'a mut HashMap<LayerId, DirtyRegion>,
        layer_id: LayerId,
    ) -> Option<(&'a mut TileGrid, &'a mut DirtyRegion)> {
        let raster = find_raster_in_mut(layers, layer_id)?;
        let dirty_region = dirty.get_mut(&layer_id)?;
        Some((&mut raster.tiles, dirty_region))
    }

    /// Paint a filled circle on a raster layer.
    pub fn paint_circle(
        &mut self,
        layer_id: LayerId,
        cx: f32,
        cy: f32,
        radius: f32,
        color: [u8; 4],
    ) {
        let (tiles, dirty) =
            match Self::raster_tiles_and_dirty(&mut self.root.children, &mut self.dirty, layer_id) {
                Some(v) => v,
                None => return,
            };

        let r2 = radius * radius;
        let x_min = (cx - radius).floor() as i32;
        let x_max = (cx + radius).ceil() as i32;
        let y_min = (cy - radius).floor() as i32;
        let y_max = (cy + radius).ceil() as i32;

        let (tx_min, ty_min) = TileGrid::tile_coords_for_pixel(x_min, y_min);
        let (tx_max, ty_max) = TileGrid::tile_coords_for_pixel(x_max, y_max);

        let tile_size = TILE_SIZE as i32;

        for ty in ty_min..=ty_max {
            for tx in tx_min..=tx_max {
                let tile_px_x = tx * tile_size;
                let tile_px_y = ty * tile_size;

                let mut touched = false;
                let tile = tiles.get_or_create(tx, ty);
                let data = tile.write();

                let lx_start = (x_min - tile_px_x).max(0) as usize;
                let lx_end = ((x_max - tile_px_x).min(tile_size) as usize).min(TILE_SIZE);
                let ly_start = (y_min - tile_px_y).max(0) as usize;
                let ly_end = ((y_max - tile_px_y).min(tile_size) as usize).min(TILE_SIZE);

                for ly in ly_start..ly_end {
                    for lx in lx_start..lx_end {
                        let px = (tile_px_x + lx as i32) as f32 + 0.5;
                        let py = (tile_px_y + ly as i32) as f32 + 0.5;
                        let dx = px - cx;
                        let dy = py - cy;
                        if dx * dx + dy * dy <= r2 {
                            let dst = data.pixel_mut(lx, ly);
                            let src_a = color[3] as f32 / 255.0;
                            let dst_a = dst[3] as f32 / 255.0;
                            let out_a = src_a + dst_a * (1.0 - src_a);
                            if out_a > 0.0 {
                                for c in 0..3 {
                                    let src_c = color[c] as f32 / 255.0;
                                    let dst_c = dst[c] as f32 / 255.0;
                                    let out_c =
                                        (src_c * src_a + dst_c * dst_a * (1.0 - src_a)) / out_a;
                                    dst[c] = (out_c * 255.0).round() as u8;
                                }
                                dst[3] = (out_a * 255.0).round() as u8;
                            }
                            touched = true;
                        }
                    }
                }

                if touched {
                    dirty.mark(tx, ty);
                }
            }
        }
    }

    /// Erase a filled circle on a raster layer (set pixels to transparent).
    pub fn erase_circle(
        &mut self,
        layer_id: LayerId,
        cx: f32,
        cy: f32,
        radius: f32,
    ) {
        let (tiles, dirty) =
            match Self::raster_tiles_and_dirty(&mut self.root.children, &mut self.dirty, layer_id) {
                Some(v) => v,
                None => return,
            };

        let r2 = radius * radius;
        let x_min = (cx - radius).floor() as i32;
        let x_max = (cx + radius).ceil() as i32;
        let y_min = (cy - radius).floor() as i32;
        let y_max = (cy + radius).ceil() as i32;

        let (tx_min, ty_min) = TileGrid::tile_coords_for_pixel(x_min, y_min);
        let (tx_max, ty_max) = TileGrid::tile_coords_for_pixel(x_max, y_max);

        let tile_size = TILE_SIZE as i32;

        for ty in ty_min..=ty_max {
            for tx in tx_min..=tx_max {
                let tile_px_x = tx * tile_size;
                let tile_px_y = ty * tile_size;

                let mut touched = false;
                let tile = tiles.get_or_create(tx, ty);
                let data = tile.write();

                let lx_start = (x_min - tile_px_x).max(0) as usize;
                let lx_end = ((x_max - tile_px_x).min(tile_size) as usize).min(TILE_SIZE);
                let ly_start = (y_min - tile_px_y).max(0) as usize;
                let ly_end = ((y_max - tile_px_y).min(tile_size) as usize).min(TILE_SIZE);

                for ly in ly_start..ly_end {
                    for lx in lx_start..lx_end {
                        let px = (tile_px_x + lx as i32) as f32 + 0.5;
                        let py = (tile_px_y + ly as i32) as f32 + 0.5;
                        let dx = px - cx;
                        let dy = py - cy;
                        if dx * dx + dy * dy <= r2 {
                            data.pixel_mut(lx, ly).copy_from_slice(&[0, 0, 0, 0]);
                            touched = true;
                        }
                    }
                }

                if touched {
                    dirty.mark(tx, ty);
                }
            }
        }
    }

    /// Flood fill from a seed point with a color, using tolerance-based matching.
    pub fn flood_fill(
        &mut self,
        layer_id: LayerId,
        seed_x: i32,
        seed_y: i32,
        color: [u8; 4],
        tolerance: u8,
    ) {
        if seed_x < 0 || seed_y < 0 || seed_x >= self.width as i32 || seed_y >= self.height as i32 {
            return;
        }

        let (tiles, dirty) =
            match Self::raster_tiles_and_dirty(&mut self.root.children, &mut self.dirty, layer_id) {
                Some(v) => v,
                None => return,
            };

        let tile_size = TILE_SIZE as i32;

        // Read the target color at seed
        let (stx, sty) = TileGrid::tile_coords_for_pixel(seed_x, seed_y);
        let slx = (seed_x - stx * tile_size) as usize;
        let sly = (seed_y - sty * tile_size) as usize;

        let target_color = match tiles.get(stx, sty) {
            Some(t) => *t.data().pixel(slx, sly),
            None => [0, 0, 0, 0],
        };

        if target_color == color {
            return;
        }

        let tol = tolerance as i16;
        let matches = |px: &[u8; 4]| -> bool {
            (px[0] as i16 - target_color[0] as i16).abs() <= tol
                && (px[1] as i16 - target_color[1] as i16).abs() <= tol
                && (px[2] as i16 - target_color[2] as i16).abs() <= tol
                && (px[3] as i16 - target_color[3] as i16).abs() <= tol
        };

        let w = self.width as i32;
        let h = self.height as i32;
        let mut visited = std::collections::HashSet::new();
        let mut stack = vec![(seed_x, seed_y)];

        while let Some((x, y)) = stack.pop() {
            if x < 0 || y < 0 || x >= w || y >= h {
                continue;
            }
            if !visited.insert((x, y)) {
                continue;
            }

            let (tx, ty_coord) = TileGrid::tile_coords_for_pixel(x, y);
            let lx = (x - tx * tile_size) as usize;
            let ly = (y - ty_coord * tile_size) as usize;

            let current = match tiles.get(tx, ty_coord) {
                Some(t) => *t.data().pixel(lx, ly),
                None => [0, 0, 0, 0],
            };

            if !matches(&current) {
                continue;
            }

            let tile = tiles.get_or_create(tx, ty_coord);
            tile.write().pixel_mut(lx, ly).copy_from_slice(&color);
            dirty.mark(tx, ty_coord);

            stack.push((x + 1, y));
            stack.push((x - 1, y));
            stack.push((x, y + 1));
            stack.push((x, y - 1));
        }
    }

    /// Draw a linear gradient between two points on a raster layer.
    pub fn linear_gradient(
        &mut self,
        layer_id: LayerId,
        x0: f32, y0: f32,
        x1: f32, y1: f32,
        color0: [u8; 4],
        color1: [u8; 4],
    ) {
        let width = self.width;
        let height = self.height;

        let (tiles, dirty) =
            match Self::raster_tiles_and_dirty(&mut self.root.children, &mut self.dirty, layer_id) {
                Some(v) => v,
                None => return,
            };

        let dx = x1 - x0;
        let dy = y1 - y0;
        let len2 = dx * dx + dy * dy;
        if len2 < 0.001 {
            return;
        }

        let tile_size = TILE_SIZE as i32;
        let tiles_x = (width as i32 + tile_size - 1) / tile_size;
        let tiles_y = (height as i32 + tile_size - 1) / tile_size;

        for ty in 0..tiles_y {
            for tx in 0..tiles_x {
                let tile = tiles.get_or_create(tx, ty);
                let data = tile.write();
                for ly in 0..TILE_SIZE {
                    for lx in 0..TILE_SIZE {
                        let px = (tx * tile_size + lx as i32) as f32 + 0.5;
                        let py = (ty * tile_size + ly as i32) as f32 + 0.5;
                        if px >= width as f32 || py >= height as f32 {
                            continue;
                        }

                        let t = ((px - x0) * dx + (py - y0) * dy) / len2;
                        let t = t.clamp(0.0, 1.0);

                        let r = (color0[0] as f32 * (1.0 - t) + color1[0] as f32 * t) as u8;
                        let g = (color0[1] as f32 * (1.0 - t) + color1[1] as f32 * t) as u8;
                        let b = (color0[2] as f32 * (1.0 - t) + color1[2] as f32 * t) as u8;
                        let a = (color0[3] as f32 * (1.0 - t) + color1[3] as f32 * t) as u8;

                        data.pixel_mut(lx, ly).copy_from_slice(&[r, g, b, a]);
                    }
                }
                dirty.mark(tx, ty);
            }
        }
    }

    /// Begin recording tile changes on a raster layer for undo.
    pub fn begin_transaction(&mut self, layer_id: LayerId) {
        if let Some(Layer::Raster(r)) = self.layer_mut(layer_id) {
            r.tiles.begin_transaction();
        }
    }

    /// Commit the active transaction and return per-layer mementos if any tiles changed.
    pub fn commit_transaction(&mut self, layer_id: LayerId) -> Option<HashMap<LayerId, Memento<Rgba>>> {
        if let Some(Layer::Raster(r)) = self.layer_mut(layer_id) {
            if let Some(memento) = r.tiles.commit_transaction() {
                let mut mementos = HashMap::new();
                mementos.insert(layer_id, memento);
                return Some(mementos);
            }
        }
        None
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
