pub type LayerId = u64;

/// Pixel-space bounds of a raster layer's GPU texture, in canvas coordinates.
///
/// `(offset_x, offset_y)` is the canvas-space position of the layer texture's
/// (0, 0) pixel. `(width, height)` is the texture's allocated size. Bounds
/// may extend beyond the canvas; the compositor clips to canvas at
/// scissor time, but the underlying pixels are preserved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerBounds {
    pub offset_x: i32,
    pub offset_y: i32,
    pub width: u32,
    pub height: u32,
}

impl LayerBounds {
    pub fn canvas(width: u32, height: u32) -> Self {
        LayerBounds {
            offset_x: 0,
            offset_y: 0,
            width,
            height,
        }
    }

    /// Smallest LayerBounds containing both `self` and `other`, growing the
    /// extents in canvas coordinates and keeping the result within the
    /// passed canvas dimensions when `clamp_to_canvas` is true.
    pub fn union(&self, other: &LayerBounds) -> LayerBounds {
        let ax0 = self.offset_x;
        let ay0 = self.offset_y;
        let ax1 = self.offset_x + self.width as i32;
        let ay1 = self.offset_y + self.height as i32;
        let bx0 = other.offset_x;
        let by0 = other.offset_y;
        let bx1 = other.offset_x + other.width as i32;
        let by1 = other.offset_y + other.height as i32;
        let x0 = ax0.min(bx0);
        let y0 = ay0.min(by0);
        let x1 = ax1.max(bx1);
        let y1 = ay1.max(by1);
        LayerBounds {
            offset_x: x0,
            offset_y: y0,
            width: (x1 - x0).max(0) as u32,
            height: (y1 - y0).max(0) as u32,
        }
    }
}

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

/// A raster (pixel) layer. Pixel data lives exclusively on GPU textures;
/// this struct holds only metadata and compositing properties.
pub struct RasterLayer {
    pub id: LayerId,
    pub name: String,
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub visible: bool,
    /// Whether this layer has an associated mask texture on the GPU.
    /// The mask pixel data is GPU-authoritative — this is just a flag.
    pub has_mask: bool,
    /// Whether the mask affects compositing (GIMP's `apply_mask`).
    pub mask_enabled: bool,
    /// Display the mask as grayscale instead of layer content.
    pub show_mask: bool,
    /// Pixel-space bounds of the layer's GPU texture in canvas coordinates.
    /// Initialized to canvas bounds at layer creation; grows to fit pasted
    /// or transformed content that extends beyond the canvas.
    pub bounds: LayerBounds,
}

impl RasterLayer {
    pub fn new(id: LayerId, bounds: LayerBounds) -> Self {
        RasterLayer {
            id,
            name: format!("Layer {id}"),
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
            visible: true,
            has_mask: false,
            mask_enabled: true,
            show_mask: false,
            bounds,
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
    pub passthrough: bool, // true = passthrough (default), false = normal group
    pub collapsed: bool,   // UI state: whether the group is visually collapsed
    /// Whether this group has an associated mask texture on the GPU.
    pub has_mask: bool,
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
            has_mask: false,
            mask_enabled: true,
            show_mask: false,
        }
    }
}

/// Snapshot of mask boolean state — used for undo actions.
#[derive(Clone, Copy)]
pub struct MaskSnapshot {
    pub has_mask: bool,
    pub mask_enabled: bool,
    pub show_mask: bool,
}

/// Common mask interface shared by RasterLayer and LayerGroup.
/// Mask pixel data is GPU-authoritative — these methods only track
/// the boolean flag and compositing toggles.
pub trait Masked {
    fn has_mask(&self) -> bool;
    fn set_has_mask(&mut self, has: bool);
    fn mask_enabled(&self) -> bool;
    fn set_mask_enabled(&mut self, enabled: bool);
    fn show_mask(&self) -> bool;
    fn set_show_mask(&mut self, show: bool);

    fn mask_snapshot(&self) -> MaskSnapshot {
        MaskSnapshot {
            has_mask: self.has_mask(),
            mask_enabled: self.mask_enabled(),
            show_mask: self.show_mask(),
        }
    }
}

macro_rules! impl_masked {
    ($t:ty) => {
        impl Masked for $t {
            fn has_mask(&self) -> bool {
                self.has_mask
            }
            fn set_has_mask(&mut self, has: bool) {
                self.has_mask = has;
            }
            fn mask_enabled(&self) -> bool {
                self.mask_enabled
            }
            fn set_mask_enabled(&mut self, enabled: bool) {
                self.mask_enabled = enabled;
            }
            fn show_mask(&self) -> bool {
                self.show_mask
            }
            fn set_show_mask(&mut self, show: bool) {
                self.show_mask = show;
            }
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
