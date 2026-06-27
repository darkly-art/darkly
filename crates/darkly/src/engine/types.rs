//! FFI/serialization types — serde-serializable for any WASM bridge.

use crate::gpu::params::{ParamDef, ParamValue};

/// Cached, synchronously-consumable snapshot of engine state that the frontend
/// mirrors. Returned by `render` each frame (a downhill projection of the one
/// borrow render already holds — no extra query, no per-frame poll) so
/// synchronous UI consumers (`$derived`, menu `enabled()`, `beforeunload`) read
/// a local mirror instead of awaiting the engine.
///
/// This is a single struct *by design*: every field here exists for the same
/// reason — frontend mirroring — so they ride together rather than as a
/// proliferating handful of return scalars. Mixes document state (`dirty`,
/// `has_selection`) with compositor/session signals (`frame_count`,
/// `thumbnail_version`); the unifying purpose is "values the UI caches," not a
/// document/compositor distinction — hence the name. Grow it as the UI needs
/// more; adding a field requires no new per-value plumbing on either side.
///
/// `frame_count` is `f64` (not `u64`) so it crosses the wasm boundary as a JS
/// `number`, not a `BigInt` — values up to 2^53 round-trip exactly.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineState {
    /// Compositor master tick (post-increment for this frame). Drives JS-side
    /// divisor phase-locking (camera upload throttle).
    pub frame_count: f64,
    /// Bumped each time a thumbnail readback lands; the layer panel mirrors it
    /// into a reactive epoch so thumbnail `$derived`s re-run.
    pub thumbnail_version: u32,
    /// Document has unsaved changes (`is_dirty`). Backs the close-tab guard.
    pub dirty: bool,
    /// Document has an active selection. Backs selection-gated menu items.
    pub has_selection: bool,
}

/// Per-instance view of a tree node. `type` (variant tag) and `blendMode` are
/// stable registry `type_id`s — display labels are looked up by the UI through
/// the matching `*_types()` table, never carried alongside as a redundant copy.
///
/// `canHaveMask` / `canRename` / `hasThumbnail` / `icon` / `kindName` are
/// per-kind capability flags sourced from the layer's
/// [`crate::document::LayerKindRegistration`]. The frontend reads these instead
/// of branching on `type` — a new layer kind declares its capabilities in its
/// own registration and the UI follows with no consumer-side edit.
#[derive(serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LayerInfo {
    #[serde(rename_all = "camelCase")]
    Raster {
        id: f64,
        name: String,
        visible: bool,
        locked: bool,
        /// Effective editability — `false` when this node *or any ancestor*
        /// carries `locked = true`. Mirrors `Document::is_node_editable`;
        /// the UI consumes this directly to grey out controls so the
        /// inheritance rule lives in one place (the document predicate)
        /// rather than being recomputed by every Svelte component.
        editable: bool,
        can_have_mask: bool,
        can_rename: bool,
        has_thumbnail: bool,
        icon: &'static str,
        kind_name: &'static str,
        opacity: f32,
        /// Stable `type_id` from the blend-mode registry (snake_case, e.g.
        /// `"normal"`, `"color_burn"`). Resolve to a display label via the
        /// blend-mode registry, not a sibling field on this struct.
        blend_mode: &'static str,
        /// Filters attached to this layer (today: at most one mask).
        modifiers: Vec<ModifierInfo>,
        /// Pixel-space bounds of the layer's GPU texture in canvas coords.
        bounds: crate::coord::CanvasRect,
    },
    /// Void (procedural-content) layer. Carries no pixel buffer — its
    /// content is generated from `voidType` + `params` each frame.
    #[serde(rename_all = "camelCase")]
    Void {
        id: f64,
        name: String,
        visible: bool,
        locked: bool,
        editable: bool,
        can_have_mask: bool,
        can_rename: bool,
        has_thumbnail: bool,
        /// Iconify icon for this void kind (e.g. `"tabler:galaxy"`), resolved
        /// per-subtype from the void's registration. The layer panel renders
        /// it as the void layer's thumbnail.
        icon: &'static str,
        kind_name: &'static str,
        opacity: f32,
        blend_mode: &'static str,
        modifiers: Vec<ModifierInfo>,
        /// Stable `type_id` from the void registry — UI resolves to a
        /// display label via `void_types()`.
        void_type: String,
        /// Param schema + current values, in the order the void's
        /// `ParamDef` slice declares them. Same shape the veil panel uses.
        params: Vec<ParamInfo>,
    },
    /// Filter (non-destructive procedural-transform) layer. Carries no pixel
    /// buffer — it transforms the composite of everything below it each frame.
    #[serde(rename_all = "camelCase")]
    Filter {
        id: f64,
        name: String,
        visible: bool,
        locked: bool,
        editable: bool,
        can_have_mask: bool,
        can_rename: bool,
        has_thumbnail: bool,
        icon: &'static str,
        kind_name: &'static str,
        opacity: f32,
        blend_mode: &'static str,
        modifiers: Vec<ModifierInfo>,
        /// Stable filter `type_id` (e.g. `"invert"`) — UI resolves to a
        /// display label via `filter_types()`.
        pipeline: String,
    },
    #[serde(rename_all = "camelCase")]
    Group {
        id: f64,
        name: String,
        visible: bool,
        locked: bool,
        editable: bool,
        can_have_mask: bool,
        can_rename: bool,
        has_thumbnail: bool,
        icon: &'static str,
        kind_name: &'static str,
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
    /// See [`LayerInfo::Raster::editable`] — a modifier is editable when
    /// neither it nor its host (nor any ancestor of the host) is locked.
    pub editable: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeilTypeInfo {
    #[serde(rename = "type")]
    pub type_id: &'static str,
    pub display_name: &'static str,
    pub params: Vec<ParamInfo>,
}

/// Registry view of a void type for the "Add Void" picker. Mirrors
/// [`VeilTypeInfo`] but additionally carries the void's iconify `icon` (always
/// present — the picker's fallback when there's no rendered preview) and
/// `supportsPreview` (whether to render a live thumbnail at all). `icon` is
/// void-only and intentionally not symmetrized onto [`VeilTypeInfo`].
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoidTypeInfo {
    #[serde(rename = "type")]
    pub type_id: &'static str,
    pub display_name: &'static str,
    pub params: Vec<ParamInfo>,
    pub icon: &'static str,
    pub supports_preview: bool,
    /// How the browser captures this void's external frames (`"camera"` /
    /// `"display"`), or absent for procedural voids. The frontend builds a
    /// `voidType → CaptureKind` map from this to pick `getUserMedia` vs
    /// `getDisplayMedia` and to drive the generic MediaStream lifecycle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_kind: Option<crate::gpu::void::CaptureKind>,
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
    /// Icon: `[["fa6-solid:icon-name", "Label"], ...]`.
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
    void_registry: &crate::gpu::void::VoidRegistry,
    node_id: crate::layer::LayerId,
) -> Option<LayerInfo> {
    use crate::layer::{Layer, LayerNode};
    let node = doc.find_node(node_id)?;
    let editable = doc.is_node_editable(node_id);
    let kind = node.kind();
    let info = match node {
        LayerNode::Layer(layer) => match layer {
            Layer::Raster(r) => LayerInfo::Raster {
                id: r.id.to_ffi() as f64,
                name: r.common.name.clone(),
                visible: r.common.visible,
                locked: r.common.locked,
                editable,
                can_have_mask: kind.can_have_mask,
                can_rename: kind.can_rename,
                has_thumbnail: kind.has_thumbnail,
                icon: kind.icon,
                kind_name: kind.display_name,
                opacity: r.blend.opacity,
                blend_mode: r.blend.blend_mode.type_id,
                modifiers: r
                    .filters
                    .iter()
                    .filter_map(|mid| doc.find_filter(*mid).map(|m| modifier_to_info(doc, m)))
                    .collect(),
                bounds: r.pixels.bounds,
            },
            Layer::Void(v) => {
                let param_defs = void_registry.param_defs(&v.void_type);
                let params = param_defs
                    .iter()
                    .enumerate()
                    .map(|(j, def)| ParamInfo::from_def(def, v.params.get(j)))
                    .collect();
                let subtype_icon = void_registry.icon(&v.void_type);
                LayerInfo::Void {
                    id: v.id.to_ffi() as f64,
                    name: v.common.name.clone(),
                    visible: v.common.visible,
                    locked: v.common.locked,
                    editable,
                    can_have_mask: kind.can_have_mask,
                    can_rename: kind.can_rename,
                    has_thumbnail: kind.has_thumbnail,
                    icon: if subtype_icon.is_empty() {
                        kind.icon
                    } else {
                        subtype_icon
                    },
                    kind_name: kind.display_name,
                    opacity: v.blend.opacity,
                    blend_mode: v.blend.blend_mode.type_id,
                    modifiers: v
                        .filters
                        .iter()
                        .filter_map(|mid| doc.find_filter(*mid).map(|m| modifier_to_info(doc, m)))
                        .collect(),
                    void_type: v.void_type.clone(),
                    params,
                }
            }
            Layer::Filter(f) => LayerInfo::Filter {
                id: f.id.to_ffi() as f64,
                name: f.common.name.clone(),
                visible: f.common.visible,
                locked: f.common.locked,
                editable,
                can_have_mask: kind.can_have_mask,
                can_rename: kind.can_rename,
                has_thumbnail: kind.has_thumbnail,
                icon: kind.icon,
                kind_name: kind.display_name,
                opacity: f.blend.opacity,
                blend_mode: f.blend.blend_mode.type_id,
                modifiers: f
                    .filters
                    .iter()
                    .filter_map(|mid| doc.find_filter(*mid).map(|m| modifier_to_info(doc, m)))
                    .collect(),
                pipeline: f.pipeline.clone(),
            },
        },
        LayerNode::Group(g) => LayerInfo::Group {
            id: g.id.to_ffi() as f64,
            name: g.common.name.clone(),
            visible: g.common.visible,
            locked: g.common.locked,
            editable,
            can_have_mask: kind.can_have_mask,
            can_rename: kind.can_rename,
            has_thumbnail: kind.has_thumbnail,
            icon: kind.icon,
            kind_name: kind.display_name,
            collapsed: g.collapsed,
            passthrough: g.passthrough,
            opacity: g.blend.opacity,
            blend_mode: g.blend.blend_mode.type_id,
            modifiers: g
                .filters
                .iter()
                .filter_map(|mid| doc.find_filter(*mid).map(|m| modifier_to_info(doc, m)))
                .collect(),
            children: g
                .children
                .iter()
                .rev()
                .filter_map(|cid| node_to_layer_info(doc, void_registry, *cid))
                .collect(),
        },
    };
    Some(info)
}

pub(crate) fn modifier_to_info(
    doc: &crate::document::Document,
    modifier: &crate::document::Filter,
) -> ModifierInfo {
    ModifierInfo {
        id: modifier.id.to_ffi() as f64,
        kind: modifier.type_id(),
        name: modifier.common.name.clone(),
        visible: modifier.common.visible,
        locked: modifier.common.locked,
        editable: doc.is_node_editable(modifier.id),
    }
}
