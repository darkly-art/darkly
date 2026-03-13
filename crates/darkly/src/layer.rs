use crate::tile::{AlphaMask, TileGrid};

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

pub struct RasterLayer {
    pub id: LayerId,
    pub name: String,
    pub tiles: TileGrid,
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub visible: bool,
    /// Optional layer mask (white=reveal, black=hide). Modulates alpha during compositing.
    pub mask: Option<AlphaMask>,
    /// Whether the mask affects compositing (GIMP's `apply_mask`).
    pub mask_enabled: bool,
    /// Display the mask as grayscale instead of layer content.
    pub show_mask: bool,
}

impl RasterLayer {
    pub fn new(id: LayerId) -> Self {
        RasterLayer {
            id,
            name: format!("Layer {id}"),
            tiles: TileGrid::new(),
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
            visible: true,
            mask: None,
            mask_enabled: true,
            show_mask: false,
        }
    }
}

pub struct LayerGroup {
    pub id: LayerId,
    pub name: String,
    pub children: Vec<LayerNode>,
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub visible: bool,
    pub passthrough: bool,  // true = passthrough (default), false = normal group
    pub collapsed: bool,    // UI state: whether the group is visually collapsed
    /// Optional group mask (data only — GPU compositing deferred until group isolation).
    pub mask: Option<AlphaMask>,
    pub mask_enabled: bool,
    pub show_mask: bool,
}

impl LayerGroup {
    pub fn new(id: LayerId) -> Self {
        LayerGroup {
            id,
            name: format!("Group {id}"),
            children: Vec::new(),
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
            visible: true,
            passthrough: true,
            collapsed: false,
            mask: None,
            mask_enabled: true,
            show_mask: false,
        }
    }
}

/// Common mask interface shared by RasterLayer and LayerGroup.
pub trait Masked {
    fn mask(&self) -> &Option<AlphaMask>;
    fn mask_mut(&mut self) -> &mut Option<AlphaMask>;
    fn mask_enabled(&self) -> bool;
    fn set_mask_enabled(&mut self, enabled: bool);
    fn show_mask(&self) -> bool;
    fn set_show_mask(&mut self, show: bool);
}

macro_rules! impl_masked {
    ($t:ty) => {
        impl Masked for $t {
            fn mask(&self) -> &Option<AlphaMask> { &self.mask }
            fn mask_mut(&mut self) -> &mut Option<AlphaMask> { &mut self.mask }
            fn mask_enabled(&self) -> bool { self.mask_enabled }
            fn set_mask_enabled(&mut self, enabled: bool) { self.mask_enabled = enabled; }
            fn show_mask(&self) -> bool { self.show_mask }
            fn set_show_mask(&mut self, show: bool) { self.show_mask = show; }
        }
    };
}

impl_masked!(RasterLayer);
impl_masked!(LayerGroup);

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

    pub fn visible(&self) -> bool {
        match self {
            LayerNode::Layer(l) => l.visible(),
            LayerNode::Group(g) => g.visible,
        }
    }

    pub fn as_masked(&self) -> &dyn Masked {
        match self {
            LayerNode::Layer(Layer::Raster(r)) => r,
            LayerNode::Group(g) => g,
        }
    }

    pub fn as_masked_mut(&mut self) -> &mut dyn Masked {
        match self {
            LayerNode::Layer(Layer::Raster(r)) => r,
            LayerNode::Group(g) => g,
        }
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

    pub fn visible(&self) -> bool {
        match self {
            Layer::Raster(r) => r.visible,
        }
    }
}
