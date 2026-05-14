use crate::coord::CanvasRect;
use crate::gpu::blend_mode::{self, BlendModeRegistration};

slotmap::new_key_type! {
    /// Unique identifier for any node, group, or modifier in a [`Document`].
    /// Backed by a slotmap key — generational, so stale ids return `None` from
    /// [`Document`] lookups instead of aliasing onto a recycled slot.
    ///
    /// At the WASM/JS boundary, marshal as `u64` via [`LayerId::to_ffi`] /
    /// [`LayerId::from_ffi`].
    ///
    /// [`Document`]: crate::document::Document
    pub struct LayerId;
}

impl LayerId {
    /// Pack into a `u64` for the WASM/JS boundary. Index in the low 32 bits,
    /// generation in the high 32. Round-trips losslessly through
    /// [`LayerId::from_ffi`].
    pub fn to_ffi(self) -> u64 {
        slotmap::Key::data(&self).as_ffi()
    }

    /// Unpack a `u64` previously produced by [`LayerId::to_ffi`]. The result
    /// is only meaningful within the [`Document`] that minted the original key.
    ///
    /// [`Document`]: crate::document::Document
    pub fn from_ffi(v: u64) -> Self {
        slotmap::KeyData::from_ffi(v).into()
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
///
/// `blend_mode` is a registry reference, not an enum: `type_id` is the
/// identity (used by the wire format, undo, and `set_blend_mode`), and
/// `gpu_value` is the integer the composite shader switches on. There is no
/// parallel enum representation — registry-resolved registrations are the
/// only carrier.
pub struct BlendProps {
    pub opacity: f32,
    pub blend_mode: &'static BlendModeRegistration,
}

impl BlendProps {
    pub fn new() -> Self {
        BlendProps {
            opacity: 1.0,
            blend_mode: blend_mode::registry().default(),
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

/// A raster (pixel) layer.
pub struct RasterLayer {
    pub id: LayerId,
    pub common: NodeCommon,
    pub blend: BlendProps,
    pub pixels: PixelBuffer,
    /// Modifiers attached to this layer, in bottom-up order. Each entry is a
    /// [`LayerId`] resolvable in the owning [`Document`]'s entity store.
    ///
    /// [`Document`]: crate::document::Document
    pub modifiers: Vec<LayerId>,
}

impl RasterLayer {
    /// Construct a raster layer. `name` is the display name shown in the
    /// layer panel — owners (the [`Document`]) supply a sequential
    /// "Layer N" string rather than letting each constructor invent one
    /// from the slotmap key, which would surface raw ffi values like
    /// "Layer 4294967301" to the user.
    pub fn new(id: LayerId, bounds: CanvasRect, name: String) -> Self {
        RasterLayer {
            id,
            common: NodeCommon::new(name),
            blend: BlendProps::new(),
            pixels: PixelBuffer::new(bounds, wgpu::TextureFormat::Rgba8Unorm),
            modifiers: Vec::new(),
        }
    }
}

pub struct LayerGroup {
    pub id: LayerId,
    pub common: NodeCommon,
    pub blend: BlendProps,
    /// Child node ids in display order (bottom-to-top). Resolve via the owning
    /// [`Document`]'s entity store.
    ///
    /// [`Document`]: crate::document::Document
    pub children: Vec<LayerId>,
    pub modifiers: Vec<LayerId>,
    /// True = passthrough (default), false = normal isolated group.
    pub passthrough: bool,
    /// UI state: whether the group is visually collapsed in the layer panel.
    pub collapsed: bool,
}

impl LayerGroup {
    /// Construct a group. `name` is the display name; same rationale as
    /// [`RasterLayer::new`] — owners pass a sequential string.
    pub fn new(id: LayerId, name: String) -> Self {
        LayerGroup {
            id,
            common: NodeCommon::new(name),
            blend: BlendProps::new(),
            children: Vec::new(),
            modifiers: Vec::new(),
            passthrough: true,
            collapsed: false,
        }
    }
}

/// A node in the layer tree — either a leaf layer or a group containing children.
/// Modifiers are NOT [`LayerNode`]s; they live on a host's `modifiers` list as
/// [`LayerId`] references and are resolved through the owning [`Document`].
///
/// [`Document`]: crate::document::Document
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

    pub fn modifiers(&self) -> &[LayerId] {
        match self {
            LayerNode::Layer(l) => l.modifiers(),
            LayerNode::Group(g) => &g.modifiers,
        }
    }

    pub fn modifiers_mut(&mut self) -> &mut Vec<LayerId> {
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

    /// The registration record for this node's kind — owns `type_id` (wire
    /// format), `display_name` (UI), and any future per-kind metadata. The
    /// match arms reference each kind module's own `TYPE_ID` constant rather
    /// than re-typing the string literal, so there is no parallel name to
    /// keep in sync with the registration files.
    pub fn kind(&self) -> &'static crate::document::LayerKindRegistration {
        use crate::document::layer_kind::registry;
        use crate::document::layer_kinds::{group, raster};
        match self {
            LayerNode::Layer(Layer::Raster(_)) => registry().get(raster::TYPE_ID).unwrap(),
            LayerNode::Group(_) => registry().get(group::TYPE_ID).unwrap(),
        }
    }

    /// Convenience for the wire format / save file — just the stable `type_id`
    /// string from `kind()`.
    pub fn type_id(&self) -> &'static str {
        self.kind().type_id
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

    pub fn modifiers(&self) -> &[LayerId] {
        match self {
            Layer::Raster(r) => &r.modifiers,
        }
    }

    pub fn modifiers_mut(&mut self) -> &mut Vec<LayerId> {
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
