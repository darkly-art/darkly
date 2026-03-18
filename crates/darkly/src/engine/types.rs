//! FFI/serialization types — serde-serializable for any WASM bridge.

use crate::gpu::params::{ParamDef, ParamValue};

#[derive(serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LayerInfo {
    #[serde(rename_all = "camelCase")]
    Raster {
        id: f64, name: String, visible: bool, opacity: f32, blend_mode: u32,
        has_mask: bool, mask_enabled: bool, show_mask: bool,
    },
    #[serde(rename_all = "camelCase")]
    Group {
        id: f64, name: String, visible: bool, collapsed: bool, passthrough: bool,
        opacity: f32, blend_mode: u32,
        has_mask: bool, mask_enabled: bool, show_mask: bool,
        children: Vec<LayerInfo>,
    },
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeilTypeInfo {
    #[serde(rename = "type")]
    pub type_id: &'static str,
    pub params: Vec<ParamInfo>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeilInfo {
    #[serde(rename = "type")]
    pub type_id: String,
    pub visible: bool,
    pub index: usize,
    pub params: Vec<ParamInfo>,
}

/// Flat serialization-friendly view of a parameter definition + current value.
/// Avoids nesting a tagged enum (ParamDef) which serde_wasm_bindgen can't handle.
#[derive(serde::Serialize)]
pub struct ParamInfo {
    pub kind: &'static str,
    pub name: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    pub default: ParamValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<ParamValue>,
}

impl ParamInfo {
    pub fn from_def(def: &ParamDef, value: Option<&ParamValue>) -> Self {
        match def {
            ParamDef::Float { name, min, max, default } => ParamInfo {
                kind: "float", name,
                min: Some(*min as f64), max: Some(*max as f64),
                default: ParamValue::Float(*default),
                value: value.cloned(),
            },
            ParamDef::Int { name, min, max, default } => ParamInfo {
                kind: "int", name,
                min: Some(*min as f64), max: Some(*max as f64),
                default: ParamValue::Int(*default),
                value: value.cloned(),
            },
            ParamDef::Bool { name, default } => ParamInfo {
                kind: "bool", name,
                min: None, max: None,
                default: ParamValue::Bool(*default),
                value: value.cloned(),
            },
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum StrokeOp {
    PaintCircle { x: f32, y: f32, radius: f32, r: u8, g: u8, b: u8, a: u8 },
    EraseCircle { x: f32, y: f32, radius: f32 },
    FloodFill { x: f32, y: f32, r: u8, g: u8, b: u8, a: u8, tolerance: u8 },
    LinearGradient {
        x0: f32, y0: f32, x1: f32, y1: f32,
        r0: u8, g0: u8, b0: u8, a0: u8,
        r1: u8, g1: u8, b1: u8, a1: u8,
    },
    /// Node-graph brush stroke event with full tablet data.
    BrushStroke {
        x: f32, y: f32,
        pressure: f32,
        x_tilt: f32, y_tilt: f32,
        rotation: f32,
        tangential_pressure: f32,
        time_ms: f64,
        /// Foreground color as linear RGBA floats (0-1).
        cr: f32, cg: f32, cb: f32, ca: f32,
    },
}

/// Data returned to the WASM bridge on copy/cut — always RGBA pixels regardless
/// of the internal clipboard variant.
#[derive(serde::Serialize)]
pub struct ClipboardExport {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub offset_x: i32,
    pub offset_y: i32,
}

pub(crate) fn node_to_layer_info(node: &crate::layer::LayerNode) -> LayerInfo {
    use crate::layer::{Layer, LayerNode};
    match node {
        LayerNode::Layer(layer) => match layer {
            Layer::Raster(r) => LayerInfo::Raster {
                id: r.id as f64,
                name: r.name.clone(),
                visible: r.visible,
                opacity: r.opacity,
                blend_mode: r.blend_mode as u32,
                has_mask: r.has_mask,
                mask_enabled: r.mask_enabled,
                show_mask: r.show_mask,
            },
        },
        LayerNode::Group(g) => LayerInfo::Group {
            id: g.id as f64,
            name: g.name.clone(),
            visible: g.visible,
            collapsed: g.collapsed,
            passthrough: g.passthrough,
            opacity: g.opacity,
            blend_mode: g.blend_mode as u32,
            has_mask: g.has_mask,
            mask_enabled: g.mask_enabled,
            show_mask: g.show_mask,
            children: g.children.iter().rev().map(node_to_layer_info).collect(),
        },
    }
}
