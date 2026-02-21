use crate::tile::TileGrid;
use std::any::Any;

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

/// String identifier for a filter type (e.g., "noise", "blur").
/// Each filter module defines its own constant. The layer system
/// never interprets this — it's an opaque key for the filter registry.
pub type FilterTypeId = &'static str;

/// Trait for filter parameters. Implemented by each filter module.
/// The layer system only sees this trait — never concrete param types.
pub trait FilterParams: std::fmt::Debug + Send + Sync {
    fn filter_type_id(&self) -> FilterTypeId;
    fn clone_boxed(&self) -> Box<dyn FilterParams>;
    fn as_any(&self) -> &dyn Any;
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
    pub params: Box<dyn FilterParams>,
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
