//! FFI/serialization types — serde-serializable for any WASM bridge.

use crate::gpu::params::{ParamDef, ParamValue};

/// Per-instance view of a tree node. `type` (variant tag) and `blendMode` are
/// stable registry `type_id`s — display labels are looked up by the UI through
/// the matching `*_types()` table, never carried alongside as a redundant copy.
#[derive(serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LayerInfo {
    #[serde(rename_all = "camelCase")]
    Raster {
        id: f64,
        name: String,
        visible: bool,
        locked: bool,
        opacity: f32,
        /// Stable `type_id` from the blend-mode registry (snake_case, e.g.
        /// `"normal"`, `"color_burn"`). Resolve to a display label via the
        /// blend-mode registry, not a sibling field on this struct.
        blend_mode: &'static str,
        /// Modifiers attached to this layer (today: at most one mask).
        modifiers: Vec<ModifierInfo>,
        /// Pixel-space bounds of the layer's GPU texture in canvas coords.
        bounds: crate::coord::CanvasRect,
    },
    #[serde(rename_all = "camelCase")]
    Group {
        id: f64,
        name: String,
        visible: bool,
        locked: bool,
        collapsed: bool,
        passthrough: bool,
        opacity: f32,
        blend_mode: &'static str,
        modifiers: Vec<ModifierInfo>,
        children: Vec<LayerInfo>,
    },
}

/// Serializable view of a single modifier attached to a host. Carries enough
/// metadata for the frontend to render the modifier as a sub-row in the layer
/// panel (name, visibility, lock toggles). `kind` is the registry `type_id`;
/// resolve to a display label via `modifier_types()`.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModifierInfo {
    pub id: f64,
    pub kind: &'static str,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeilTypeInfo {
    #[serde(rename = "type")]
    pub type_id: &'static str,
    pub display_name: &'static str,
    pub params: Vec<ParamInfo>,
}

/// Flat serialization-friendly view of a tool's registration metadata.
/// Mirrors `VeilTypeInfo` so the UI consumes both in the same shape.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolTypeInfo {
    #[serde(rename = "type")]
    pub type_id: &'static str,
    pub display_name: &'static str,
    pub params: Vec<ParamInfo>,
}

/// Flat view of a registered blend mode for the layer-properties dropdown.
/// `category` drives the `<optgroup>` grouping (Darken / Lighten / etc.).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlendModeTypeInfo {
    #[serde(rename = "type")]
    pub type_id: &'static str,
    pub display_name: &'static str,
    pub category: &'static str,
}

/// Registry view of a modifier kind — the UI uses this to render the
/// "Add modifier" menu and to look up display labels for `ModifierInfo.kind`.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModifierTypeInfo {
    #[serde(rename = "type")]
    pub type_id: &'static str,
    pub display_name: &'static str,
}

/// Registry view of a layer kind — used by the layer panel to render labels
/// like "Raster Layer" / "Group" for the layer's own `type` discriminator.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerKindTypeInfo {
    #[serde(rename = "type")]
    pub type_id: &'static str,
    pub display_name: &'static str,
}

/// Per-instance view of a veil in the chain. `type` is the registry `type_id`;
/// resolve to a display label via `veil_types()` — never duplicate it here.
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
    /// Enum: `["Label1", "Label2", ...]`.
    /// Icon: `[["fa-class", "Label"], ...]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<serde_json::Value>,
}

impl ParamInfo {
    pub fn from_def(def: &ParamDef, value: Option<&ParamValue>) -> Self {
        match def {
            ParamDef::Float {
                name,
                min,
                max,
                default,
            } => ParamInfo {
                kind: "float",
                name,
                min: Some(*min as f64),
                max: Some(*max as f64),
                default: ParamValue::Float(*default),
                value: value.cloned(),
                options: None,
            },
            ParamDef::Int {
                name,
                min,
                max,
                default,
            } => ParamInfo {
                kind: "int",
                name,
                min: Some(*min as f64),
                max: Some(*max as f64),
                default: ParamValue::Int(*default),
                value: value.cloned(),
                options: None,
            },
            ParamDef::Bool { name, default } => ParamInfo {
                kind: "bool",
                name,
                min: None,
                max: None,
                default: ParamValue::Bool(*default),
                value: value.cloned(),
                options: None,
            },
            ParamDef::String { name, default } => ParamInfo {
                kind: "string",
                name,
                min: None,
                max: None,
                default: ParamValue::String(default.to_string()),
                value: value.cloned(),
                options: None,
            },
            ParamDef::Curve { name, default } => ParamInfo {
                kind: "curve",
                name,
                min: None,
                max: None,
                default: ParamValue::Curve(default.to_vec()),
                value: value.cloned(),
                options: None,
            },
            ParamDef::Enum {
                name,
                options,
                default,
            } => ParamInfo {
                kind: "enum",
                name,
                min: None,
                max: None,
                default: ParamValue::Int(*default),
                value: value.cloned(),
                options: Some(serde_json::json!(options)),
            },
            ParamDef::FloatInput {
                name,
                min,
                max,
                default,
            } => ParamInfo {
                kind: "floatInput",
                name,
                min: Some(*min as f64),
                max: Some(*max as f64),
                default: ParamValue::Float(*default),
                value: value.cloned(),
                options: None,
            },
            ParamDef::Icon {
                name,
                options,
                default,
            } => ParamInfo {
                kind: "icon",
                name,
                min: None,
                max: None,
                default: ParamValue::String(default.to_string()),
                value: value.cloned(),
                options: Some(serde_json::json!(options)),
            },
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum StrokeOp {
    FloodFill {
        x: f32,
        y: f32,
        r: u8,
        g: u8,
        b: u8,
        a: u8,
        tolerance: u8,
    },
    LinearGradient {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        r0: u8,
        g0: u8,
        b0: u8,
        a0: u8,
        r1: u8,
        g1: u8,
        b1: u8,
        a1: u8,
    },
    /// Node-graph brush stroke event with full tablet data.
    BrushStroke {
        x: f32,
        y: f32,
        pressure: f32,
        x_tilt: f32,
        y_tilt: f32,
        rotation: f32,
        tangential_pressure: f32,
        time_ms: f64,
        /// Foreground color as linear RGBA floats (0-1).
        cr: f32,
        cg: f32,
        cb: f32,
        ca: f32,
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

pub(crate) fn node_to_layer_info(
    doc: &crate::document::Document,
    node_id: crate::layer::LayerId,
) -> Option<LayerInfo> {
    use crate::layer::{Layer, LayerNode};
    let node = doc.find_node(node_id)?;
    let info = match node {
        LayerNode::Layer(layer) => match layer {
            Layer::Raster(r) => LayerInfo::Raster {
                id: r.id.to_ffi() as f64,
                name: r.common.name.clone(),
                visible: r.common.visible,
                locked: r.common.locked,
                opacity: r.blend.opacity,
                blend_mode: r.blend.blend_mode.type_id,
                modifiers: r
                    .modifiers
                    .iter()
                    .filter_map(|mid| doc.find_modifier(*mid).map(modifier_to_info))
                    .collect(),
                bounds: r.pixels.bounds,
            },
        },
        LayerNode::Group(g) => LayerInfo::Group {
            id: g.id.to_ffi() as f64,
            name: g.common.name.clone(),
            visible: g.common.visible,
            locked: g.common.locked,
            collapsed: g.collapsed,
            passthrough: g.passthrough,
            opacity: g.blend.opacity,
            blend_mode: g.blend.blend_mode.type_id,
            modifiers: g
                .modifiers
                .iter()
                .filter_map(|mid| doc.find_modifier(*mid).map(modifier_to_info))
                .collect(),
            children: g
                .children
                .iter()
                .rev()
                .filter_map(|cid| node_to_layer_info(doc, *cid))
                .collect(),
        },
    };
    Some(info)
}

pub(crate) fn modifier_to_info(modifier: &crate::document::Modifier) -> ModifierInfo {
    ModifierInfo {
        id: modifier.id.to_ffi() as f64,
        kind: modifier.type_id(),
        name: modifier.common.name.clone(),
        visible: modifier.common.visible,
        locked: modifier.common.locked,
    }
}
