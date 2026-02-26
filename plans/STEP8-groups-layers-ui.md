# Phase 2, Session 4 — Layer Groups + Layer Panel UI

## Scope

Steps 6 and 8 from the Phase 2 plan. Build the Rust layer group data structure with tree operations and passthrough compositing, then build the right sidebar layer panel UI with tree rendering, drag-and-drop reorder, and layer controls.

These steps are combined because the layer panel UI directly renders the tree structure that Step 6 creates — they must be built together.

## Prerequisites

Session 3 complete: tool system working with stroke lifecycle, all 5 tools functional. The existing layer model uses `Vec<Layer>` (flat list).

## Done When

- `LayerGroup` and `LayerNode` tree structure implemented in Rust
- `flat_layers()` returns correct bottom-to-top order respecting group visibility
- Layer move/reorder within and between groups works
- Compositor renders identically with grouped layers (passthrough mode)
- Right sidebar shows layer tree with indented groups
- Drag-and-drop reorder works (between layers, into/out of groups)
- Layer controls work: visibility toggle, opacity slider, blend mode, rename
- New layer, new group, and delete buttons work

---

## Step 6: Layer groups (Rust)

### `layer.rs` additions

```rust
pub struct LayerGroup {
    pub id: LayerId,
    pub name: String,
    pub children: Vec<LayerNode>,
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub visible: bool,
    pub passthrough: bool,  // true = passthrough (default)
    pub collapsed: bool,    // UI state
}

/// A node in the layer tree.
pub enum LayerNode {
    Layer(Layer),
    Group(LayerGroup),
}

impl LayerNode {
    pub fn id(&self) -> LayerId;
    pub fn visible(&self) -> bool;
}
```

### `document.rs` changes

`Document::layers` changes from `Vec<Layer>` to `Vec<LayerNode>` — a forest of layer trees.

```rust
pub struct Document {
    pub layers: Vec<LayerNode>,     // root-level nodes (bottom to top)
    pub width: u32,
    pub height: u32,
    pub dirty: HashMap<LayerId, DirtyRegion>,
    next_id: LayerId,
}

pub enum MoveTarget {
    Before(LayerId),
    After(LayerId),
    IntoGroupTop(LayerId),
    IntoGroupBottom(LayerId),
}

impl Document {
    pub fn add_group(&mut self) -> LayerId;
    pub fn add_raster_layer_in(&mut self, parent: Option<LayerId>) -> LayerId;

    /// Flatten the layer tree into display order (bottom-to-top) for compositing.
    /// Passthrough groups are transparent — children yielded directly.
    /// Hidden groups exclude all children.
    pub fn flat_layers(&self) -> Vec<&Layer>;
    fn flatten_nodes<'a>(nodes: &'a [LayerNode], out: &mut Vec<&'a Layer>);

    pub fn move_layer(&mut self, layer_id: LayerId, target: MoveTarget);
    fn detach_node(&mut self, layer_id: LayerId) -> Option<LayerNode>;
    fn insert_node(&mut self, node: LayerNode, target: MoveTarget);
    pub fn find_node(&self, id: LayerId) -> Option<&LayerNode>;
    pub fn find_node_mut(&mut self, id: LayerId) -> Option<&mut LayerNode>;
    pub fn parent_of(&self, id: LayerId) -> Option<LayerId>;
    pub fn remove_layer(&mut self, id: LayerId);
}
```

### Compositor changes

Change compositor to call `doc.flat_layers()` instead of iterating `doc.layers` directly. All existing compositing logic (ping-pong, scissor, cache) works unchanged on the flat list.

Cache invalidation: any structural change (move, add/remove group, visibility toggle, reorder) -> full cache invalidation. This is handled by `mark_dirty()` which sets both `needs_composite = true` and `cache_valid_through = None` (see STEP3 bug fix note — both flags are required for non-tile-dirty events like filter layer deletion to take immediate visual effect).

### Unit tests

- Create groups, add layers inside, verify `flat_layers()` returns correct order
- Move layers between groups, verify ordering
- Hide a group, verify its children excluded from `flat_layers()`
- Nested groups flatten correctly
- `detach_node` + `insert_node` round-trip preserves the node

---

## Step 8: Right sidebar — layer panel UI

### WASM bridge — layer tree methods

```rust
impl DarklyHandle {
    /// Returns the layer tree as JSON for UI rendering.
    /// Returned in top-to-bottom display order (reversed from internal order).
    pub fn layer_tree(&self) -> JsValue;

    pub fn add_group(&mut self) -> u64;
    pub fn add_raster_layer_in(&mut self, group_id: u64) -> u64;
    pub fn remove_layer(&mut self, layer_id: u64);
    pub fn move_layer(&mut self, layer_id: u64, target_type: &str, target_id: u64);
    pub fn set_layer_name(&mut self, layer_id: u64, name: &str);
    pub fn set_layer_visible(&mut self, layer_id: u64, visible: bool);
    pub fn set_group_collapsed(&mut self, group_id: u64, collapsed: bool);
}
```

`layer_tree()` return format (top-to-bottom display order):
```json
[
    { "type": "raster", "id": 3, "name": "Layer 2", "visible": true,
      "opacity": 1.0, "blendMode": 0 },
    {
        "type": "group", "id": 5, "name": "Group 1",
        "visible": true, "collapsed": false, "passthrough": true,
        "children": [
            { "type": "raster", "id": 2, "name": "Layer 1", ... }
        ]
    },
    { "type": "filter", "id": 4, "name": "Noise", "visible": true },
    { "type": "raster", "id": 1, "name": "Background", ... }
]
```

### `frontend/src/ui/RightSidebar.svelte`

~260px panel on the right edge containing the layer panel.

### `frontend/src/ui/layers/LayerPanel.svelte`

Layer tree as a vertically scrollable list. Display order: topmost layer first (Photoshop/Krita convention — reversed from internal order). Group children indented.

**Layer tree synchronization:** After each mutation, call `handle.layer_tree()` to get updated tree as JSON. Store in reactive `$state`.

```typescript
let layerTree = $state<LayerTreeNode[]>([]);

function refreshLayerTree() {
    if (app.handle) {
        layerTree = app.handle.layer_tree();
    }
}
```

**Action buttons** at bottom:
- **New Layer** (+) — `handle.add_raster_layer()` or `handle.add_raster_layer_in(activeGroupId)`
- **New Group** (folder icon) — `handle.add_group()`
- **Delete** (trash icon) — `handle.remove_layer(activeLayerId)`

### `frontend/src/ui/layers/LayerItem.svelte`

Single layer row showing:
- **Visibility toggle** — Eye icon, toggles via `handle.set_layer_visible()`
- **Layer name** — Editable on double-click via `handle.set_layer_name()`
- **Opacity slider** — `<input type="range">`, updates via `handle.set_opacity()`
- **Blend mode** — `<select>` dropdown (Normal, Multiply, Screen, Overlay)

### `frontend/src/ui/layers/LayerGroup.svelte`

Group row showing:
- **Collapse toggle** — Triangle icon
- **Group name** — Editable
- **Visibility toggle**
- Children rendered as indented `LayerItem`s / nested `LayerGroup`s

### Drag-and-drop reorder

HTML5 Drag and Drop API:
- `draggable="true"` on each layer/group row
- `dragstart`: store dragged layer ID in `dataTransfer.setData()`
- `dragover`: compute drop position from mouse Y within target element:
  - Top 25% -> drop before (above in display = after in stack order)
  - Bottom 25% -> drop after (below in display = before in stack order)
  - Middle 50% of group row -> drop into group as first child
- `drop`: call `handle.move_layer(draggedId, targetType, targetId)`
- Visual indicator: horizontal line between items (before/after) or highlight (into-group)

### Verification

- Layer panel shows all layers and groups
- Drag-and-drop reorders layers
- Layers can be dragged into/out of groups
- Visibility toggle hides layers (compositor skips them)
- Opacity slider adjusts smoothly
- New layer/group/delete buttons work
- Double-click rename works
- Compositor renders identically whether layers are in groups or at root (passthrough)

---

## Files Created/Modified This Session

```
crates/darkly/src/
├── layer.rs                    # MODIFIED: + LayerGroup, LayerNode
├── document.rs                 # MODIFIED: Vec<LayerNode>, tree ops, flat_layers()
└── gpu/compositor.rs           # MODIFIED: use flat_layers()

frontend/
├── src/
│   ├── ui/
│   │   ├── RightSidebar.svelte # NEW
│   │   └── layers/
│   │       ├── LayerPanel.svelte  # NEW
│   │       ├── LayerItem.svelte   # NEW
│   │       └── LayerGroup.svelte  # NEW
│   └── App.svelte              # MODIFIED: add RightSidebar
└── wasm/src/
    └── api.rs                  # MODIFIED: layer tree methods
```
