use crate::coord::CanvasRect;
use crate::document::Modifier;

pub type LayerId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u32)]
pub enum BlendMode {
    Normal = 0,
    Multiply = 1,
    Screen = 2,
    Overlay = 3,
}

impl BlendMode {
    pub fn from_u32(v: u32) -> Self {
        match v {
            0 => BlendMode::Normal,
            1 => BlendMode::Multiply,
            2 => BlendMode::Screen,
            3 => BlendMode::Overlay,
            _ => BlendMode::Normal,
        }
    }
}

/// Properties shared by every node in the tree — raster layers, groups, and
/// modifiers. Lock prevents any mutation; lives on every node by construction
/// so the universal check is one line at every mutation entry point.
pub struct NodeCommon {
    pub name: String,
    pub visible: bool,
    pub locked: bool,
}

impl NodeCommon {
    pub fn new(name: String) -> Self {
        NodeCommon {
            name,
            visible: true,
            locked: false,
        }
    }
}

/// Compositing properties for nodes that participate in normal blending
/// (raster layers and groups). Modifiers don't have one — masks structurally
/// have no opacity or blend mode.
pub struct BlendProps {
    pub opacity: f32,
    pub blend_mode: BlendMode,
}

impl BlendProps {
    pub fn new() -> Self {
        BlendProps {
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
        }
    }
}

impl Default for BlendProps {
    fn default() -> Self {
        Self::new()
    }
}

/// Pixel-storage metadata for any node holding GPU pixels (raster layers, mask
/// modifiers, future filter caches). Bulk pixel data is GPU-authoritative; this
/// struct only carries canvas-space metadata: extent and texture format.
///
/// Every `PixelBuffer` is sampled independently — the blend shader computes UV
/// from each buffer's own bounds. Lockstep growth (host + non-locked mask grow
/// together) is a document-side convenience that drops out for free when both
/// buffers receive the same rasterized transform.
pub struct PixelBuffer {
    pub bounds: CanvasRect,
    pub format: wgpu::TextureFormat,
}

impl PixelBuffer {
    pub fn new(bounds: CanvasRect, format: wgpu::TextureFormat) -> Self {
        PixelBuffer { bounds, format }
    }
}

/// Modifiers attached to a host (raster layer or group). Order is bottom-up —
/// later entries run after earlier ones. Today: typically zero or one mask.
pub struct ModifierList(pub Vec<Modifier>);

impl ModifierList {
    pub fn new() -> Self {
        ModifierList(Vec::new())
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Modifier> {
        self.0.iter()
    }

    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, Modifier> {
        self.0.iter_mut()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// First mask modifier on this list, if any. Today's UI exposes at most one
    /// mask per host, so this is the canonical lookup. The model supports N.
    pub fn mask(&self) -> Option<&Modifier> {
        self.0
            .iter()
            .find(|m| matches!(&m.kind, crate::document::ModifierKind::Mask(_)))
    }

    pub fn mask_mut(&mut self) -> Option<&mut Modifier> {
        self.0
            .iter_mut()
            .find(|m| matches!(&m.kind, crate::document::ModifierKind::Mask(_)))
    }
}

impl Default for ModifierList {
    fn default() -> Self {
        Self::new()
    }
}

/// Child nodes of a group — raster siblings or sub-groups. Modifiers are NOT
/// in this list (they're attached via the host's `modifiers` field).
pub struct ChildList(pub Vec<LayerNode>);

impl ChildList {
    pub fn new() -> Self {
        ChildList(Vec::new())
    }

    pub fn iter(&self) -> std::slice::Iter<'_, LayerNode> {
        self.0.iter()
    }

    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, LayerNode> {
        self.0.iter_mut()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn get(&self, idx: usize) -> Option<&LayerNode> {
        self.0.get(idx)
    }

    pub fn get_mut(&mut self, idx: usize) -> Option<&mut LayerNode> {
        self.0.get_mut(idx)
    }

    pub fn push(&mut self, node: LayerNode) {
        self.0.push(node)
    }
}

impl Default for ChildList {
    fn default() -> Self {
        Self::new()
    }
}

/// A raster (pixel) layer.
pub struct RasterLayer {
    pub id: LayerId,
    pub common: NodeCommon,
    pub blend: BlendProps,
    pub pixels: PixelBuffer,
    pub modifiers: ModifierList,
}

impl RasterLayer {
    pub fn new(id: LayerId, bounds: CanvasRect) -> Self {
        RasterLayer {
            id,
            common: NodeCommon::new(format!("Layer {id}")),
            blend: BlendProps::new(),
            pixels: PixelBuffer::new(bounds, wgpu::TextureFormat::Rgba8Unorm),
            modifiers: ModifierList::new(),
        }
    }
}

pub struct LayerGroup {
    pub id: LayerId,
    pub common: NodeCommon,
    pub blend: BlendProps,
    pub children: ChildList,
    pub modifiers: ModifierList,
    /// True = passthrough (default), false = normal isolated group.
    pub passthrough: bool,
    /// UI state: whether the group is visually collapsed in the layer panel.
    pub collapsed: bool,
}

impl LayerGroup {
    pub fn new(id: LayerId) -> Self {
        LayerGroup {
            id,
            common: NodeCommon::new(format!("Group {id}")),
            blend: BlendProps::new(),
            children: ChildList::new(),
            modifiers: ModifierList::new(),
            passthrough: true,
            collapsed: false,
        }
    }
}

/// A node in the layer tree — either a leaf layer or a group containing children.
/// Modifiers are NOT LayerNodes; they're reachable only through their host's
/// `modifiers` field, by construction.
pub enum LayerNode {
    Layer(Layer),
    Group(LayerGroup),
}

impl LayerNode {
    pub fn id(&self) -> LayerId {
        match self {
            LayerNode::Layer(l) => l.id(),
            LayerNode::Group(g) => g.id,
        }
    }

    pub fn common(&self) -> &NodeCommon {
        match self {
            LayerNode::Layer(l) => l.common(),
            LayerNode::Group(g) => &g.common,
        }
    }

    pub fn common_mut(&mut self) -> &mut NodeCommon {
        match self {
            LayerNode::Layer(l) => l.common_mut(),
            LayerNode::Group(g) => &mut g.common,
        }
    }

    pub fn blend(&self) -> &BlendProps {
        match self {
            LayerNode::Layer(l) => l.blend(),
            LayerNode::Group(g) => &g.blend,
        }
    }

    pub fn blend_mut(&mut self) -> &mut BlendProps {
        match self {
            LayerNode::Layer(l) => l.blend_mut(),
            LayerNode::Group(g) => &mut g.blend,
        }
    }

    pub fn modifiers(&self) -> &ModifierList {
        match self {
            LayerNode::Layer(l) => l.modifiers(),
            LayerNode::Group(g) => &g.modifiers,
        }
    }

    pub fn modifiers_mut(&mut self) -> &mut ModifierList {
        match self {
            LayerNode::Layer(l) => l.modifiers_mut(),
            LayerNode::Group(g) => &mut g.modifiers,
        }
    }

    pub fn pixels(&self) -> Option<&PixelBuffer> {
        match self {
            LayerNode::Layer(Layer::Raster(r)) => Some(&r.pixels),
            LayerNode::Group(_) => None,
        }
    }

    pub fn pixels_mut(&mut self) -> Option<&mut PixelBuffer> {
        match self {
            LayerNode::Layer(Layer::Raster(r)) => Some(&mut r.pixels),
            LayerNode::Group(_) => None,
        }
    }

    pub fn visible(&self) -> bool {
        self.common().visible
    }

    pub fn locked(&self) -> bool {
        self.common().locked
    }
}

pub enum Layer {
    Raster(RasterLayer),
}

impl Layer {
    pub fn id(&self) -> LayerId {
        match self {
            Layer::Raster(r) => r.id,
        }
    }

    pub fn common(&self) -> &NodeCommon {
        match self {
            Layer::Raster(r) => &r.common,
        }
    }

    pub fn common_mut(&mut self) -> &mut NodeCommon {
        match self {
            Layer::Raster(r) => &mut r.common,
        }
    }

    pub fn blend(&self) -> &BlendProps {
        match self {
            Layer::Raster(r) => &r.blend,
        }
    }

    pub fn blend_mut(&mut self) -> &mut BlendProps {
        match self {
            Layer::Raster(r) => &mut r.blend,
        }
    }

    pub fn modifiers(&self) -> &ModifierList {
        match self {
            Layer::Raster(r) => &r.modifiers,
        }
    }

    pub fn modifiers_mut(&mut self) -> &mut ModifierList {
        match self {
            Layer::Raster(r) => &mut r.modifiers,
        }
    }

    pub fn pixels(&self) -> &PixelBuffer {
        match self {
            Layer::Raster(r) => &r.pixels,
        }
    }

    pub fn pixels_mut(&mut self) -> &mut PixelBuffer {
        match self {
            Layer::Raster(r) => &mut r.pixels,
        }
    }

    pub fn visible(&self) -> bool {
        self.common().visible
    }

    pub fn locked(&self) -> bool {
        self.common().locked
    }
}
