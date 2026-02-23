use crate::gpu::filter::Filter;
use crate::tile::TileGrid;

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
    pub tiles: TileGrid,
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub visible: bool,
}

impl RasterLayer {
    pub fn new(id: LayerId) -> Self {
        RasterLayer {
            id,
            tiles: TileGrid::new(),
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
            visible: true,
        }
    }
}

pub struct FilterLayer {
    pub id: LayerId,
    pub filter: Box<dyn Filter>,
    pub visible: bool,
}

pub enum Layer {
    Raster(RasterLayer),
    Filter(FilterLayer),
}

impl Layer {
    pub fn id(&self) -> LayerId {
        match self {
            Layer::Raster(r) => r.id,
            Layer::Filter(f) => f.id,
        }
    }

    pub fn visible(&self) -> bool {
        match self {
            Layer::Raster(r) => r.visible,
            Layer::Filter(f) => f.visible,
        }
    }
}
