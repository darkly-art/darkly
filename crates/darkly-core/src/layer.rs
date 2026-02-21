use crate::tile::TileGrid;

pub type LayerId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlendMode {
    Normal,
    Multiply,
    Screen,
    Overlay,
}

impl BlendMode {
    pub fn as_u32(self) -> u32 {
        match self {
            BlendMode::Normal => 0,
            BlendMode::Multiply => 1,
            BlendMode::Screen => 2,
            BlendMode::Overlay => 3,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterType {
    GaussianBlur,
}

#[derive(Clone, Debug)]
pub struct FilterParams {
    pub radius: f32,
}

impl FilterParams {
    pub fn blur(radius: f32) -> Self {
        FilterParams { radius }
    }
}

pub struct FilterLayer {
    pub id: LayerId,
    pub filter_type: FilterType,
    pub params: FilterParams,
    pub visible: bool,
}

impl FilterLayer {
    pub fn new(id: LayerId, filter_type: FilterType, params: FilterParams) -> Self {
        FilterLayer {
            id,
            filter_type,
            params,
            visible: true,
        }
    }
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
