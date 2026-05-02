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

/// Properties shared by every layer node (raster layers and groups).
/// Mask pixel data is GPU-authoritative — `has_mask` is just the doc-side
/// flag the compositor reflects.
pub struct LayerCommon {
    pub name: String,
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub visible: bool,
    pub has_mask: bool,
    pub mask_enabled: bool,
    pub show_mask: bool,
}

impl LayerCommon {
    pub fn new(name: String) -> Self {
        LayerCommon {
            name,
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
            visible: true,
            has_mask: false,
            mask_enabled: true,
            show_mask: false,
        }
    }

    pub fn mask_snapshot(&self) -> MaskSnapshot {
        MaskSnapshot {
            has_mask: self.has_mask,
            mask_enabled: self.mask_enabled,
            show_mask: self.show_mask,
        }
    }
}

/// Snapshot of mask boolean state — used when capturing pre-mutation
/// state for undo actions.
#[derive(Clone, Copy)]
pub struct MaskSnapshot {
    pub has_mask: bool,
    pub mask_enabled: bool,
    pub show_mask: bool,
}

/// A raster (pixel) layer. Pixel data lives exclusively on GPU textures;
/// this struct holds only metadata and compositing properties.
pub struct RasterLayer {
    pub id: LayerId,
    pub common: LayerCommon,
    /// Pixel-space bounds of the layer's GPU texture in canvas coordinates.
    /// Initialized to canvas bounds at layer creation; grows to fit pasted
    /// or transformed content that extends beyond the canvas.
    pub bounds: crate::coord::CanvasRect,
}

impl RasterLayer {
    pub fn new(id: LayerId, bounds: crate::coord::CanvasRect) -> Self {
        RasterLayer {
            id,
            common: LayerCommon::new(format!("Layer {id}")),
            bounds,
        }
    }
}

pub struct LayerGroup {
    pub id: LayerId,
    pub common: LayerCommon,
    pub children: Vec<LayerNode>,
    pub passthrough: bool, // true = passthrough (default), false = normal group
    pub collapsed: bool,   // UI state: whether the group is visually collapsed
}

impl LayerGroup {
    pub fn new(id: LayerId) -> Self {
        LayerGroup {
            id,
            common: LayerCommon::new(format!("Group {id}")),
            children: Vec::new(),
            passthrough: true,
            collapsed: false,
        }
    }
}

/// A node in the layer tree. Either a leaf layer or a group containing children.
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

    pub fn common(&self) -> &LayerCommon {
        match self {
            LayerNode::Layer(Layer::Raster(r)) => &r.common,
            LayerNode::Group(g) => &g.common,
        }
    }

    pub fn common_mut(&mut self) -> &mut LayerCommon {
        match self {
            LayerNode::Layer(Layer::Raster(r)) => &mut r.common,
            LayerNode::Group(g) => &mut g.common,
        }
    }

    pub fn visible(&self) -> bool {
        self.common().visible
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

    pub fn common(&self) -> &LayerCommon {
        match self {
            Layer::Raster(r) => &r.common,
        }
    }

    pub fn common_mut(&mut self) -> &mut LayerCommon {
        match self {
            Layer::Raster(r) => &mut r.common,
        }
    }

    pub fn visible(&self) -> bool {
        self.common().visible
    }
}
