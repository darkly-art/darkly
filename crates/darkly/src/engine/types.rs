//! FFI/serialization types: serde-serializable for any WASM bridge.

use crate::coord::CanvasRect;
use crate::gpu::params::{ParamDef, ParamKind, ParamValue};
use crate::units::UnitType;

/// Cached, synchronously-consumable snapshot of engine state that the frontend
/// mirrors. Returned by `render` each frame (a downhill projection of the one
/// borrow render already holds, with no extra query or per-frame poll) so
/// synchronous UI consumers (`$derived`, menu `enabled()`, `beforeunload`) read
/// a local mirror instead of awaiting the engine.
///
/// This is a single struct *by design*: every field here exists for the same
/// reason (frontend mirroring), so they ride together rather than as a
/// proliferating handful of return scalars. Mixes document state (`dirty`,
/// `has_selection`) with compositor/session signals (`frame_count`,
/// `thumbnail_version`); the unifying purpose is "values the UI caches," not a
/// document/compositor distinction, hence the name. Grow it as the UI needs
/// more; adding a field requires no new per-value plumbing on either side.
///
/// `frame_count` is `f64` (not `u64`) so it crosses the wasm boundary as a JS
/// `number`, not a `BigInt`: values up to 2^53 round-trip exactly.
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
/// stable registry `type_id`s; display labels are looked up by the UI through
/// the matching `*_types()` table, never carried alongside as a redundant copy.
///
/// `canHaveMask` / `canRename` / `hasThumbnail` / `icon` / `kindName` are
/// per-kind capability flags sourced from the layer's
/// [`crate::document::LayerKindRegistration`]. The frontend reads these instead
/// of branching on `type`: a new layer kind declares its capabilities in its
/// own registration and the UI follows with no consumer-side edit.
#[derive(serde::Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub enum LayerInfo {
    #[serde(rename_all = "camelCase")]
    Raster {
        id: f64,
        name: String,
        visible: bool,
        locked: bool,
        /// Effective editability: `false` when this node *or any ancestor*
        /// carries `locked = true`. Mirrors `Document::is_node_editable`;
        /// the UI consumes this directly to grey out controls so the
        /// inheritance rule lives in one place (the document predicate)
        /// rather than being recomputed by every Svelte component.
        editable: bool,
        /// Whether paint ops have somewhere to land on this node, mirroring
        /// `DarklyEngine::is_node_paintable`. False for kinds whose pixels are
        /// generated (void, filter, vector) and for groups; the panel reads it
        /// to offer "Rasterize" instead of branching on `type`.
        paintable: bool,
        can_have_mask: bool,
        can_rename: bool,
        has_thumbnail: bool,
        /// Whether this row offers "Convert to Smart Object"; mirrors
        /// `DarklyEngine::can_convert_layer_to_smart_object`. The engine
        /// answers so the rule (owns its pixels, editable, no mask) lives with
        /// the operation instead of being restated by the panel.
        can_become_smart_object: bool,
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
    /// Void (procedural-content) layer. Carries no pixel buffer: its
    /// content is generated from `voidType` + `params` each frame.
    #[serde(rename_all = "camelCase")]
    Void {
        id: f64,
        name: String,
        visible: bool,
        locked: bool,
        editable: bool,
        /// Whether paint ops have somewhere to land on this node, mirroring
        /// `DarklyEngine::is_node_paintable`. False for kinds whose pixels are
        /// generated (void, filter, vector) and for groups; the panel reads it
        /// to offer "Rasterize" instead of branching on `type`.
        paintable: bool,
        can_have_mask: bool,
        can_rename: bool,
        has_thumbnail: bool,
        /// Whether this row offers "Convert to Smart Object"; mirrors
        /// `DarklyEngine::can_convert_layer_to_smart_object`. The engine
        /// answers so the rule (owns its pixels, editable, no mask) lives with
        /// the operation instead of being restated by the panel.
        can_become_smart_object: bool,
        /// Iconify icon for this void kind (e.g. `"tabler:galaxy"`), resolved
        /// per-subtype from the void's registration. The layer panel renders
        /// it as the void layer's thumbnail.
        icon: &'static str,
        kind_name: &'static str,
        opacity: f32,
        blend_mode: &'static str,
        modifiers: Vec<ModifierInfo>,
        /// Stable `type_id` from the void registry; UI resolves to a
        /// display label via `void_types()`.
        void_type: String,
        /// Param schema + current values, in the order the void's
        /// `ParamDef` slice declares them. Same shape the veil panel uses.
        params: Vec<ParamInfo>,
    },
    /// Filter (non-destructive procedural-transform) layer. Carries no pixel
    /// buffer: it transforms the composite of everything below it each frame.
    #[serde(rename_all = "camelCase")]
    Filter {
        id: f64,
        name: String,
        visible: bool,
        locked: bool,
        editable: bool,
        /// Whether paint ops have somewhere to land on this node, mirroring
        /// `DarklyEngine::is_node_paintable`. False for kinds whose pixels are
        /// generated (void, filter, vector) and for groups; the panel reads it
        /// to offer "Rasterize" instead of branching on `type`.
        paintable: bool,
        can_have_mask: bool,
        can_rename: bool,
        has_thumbnail: bool,
        /// Whether this row offers "Convert to Smart Object"; mirrors
        /// `DarklyEngine::can_convert_layer_to_smart_object`. The engine
        /// answers so the rule (owns its pixels, editable, no mask) lives with
        /// the operation instead of being restated by the panel.
        can_become_smart_object: bool,
        icon: &'static str,
        kind_name: &'static str,
        opacity: f32,
        blend_mode: &'static str,
        modifiers: Vec<ModifierInfo>,
        /// Stable filter `type_id` (e.g. `"invert"`); UI resolves to a
        /// display label via `filter_types()`.
        pipeline: String,
        /// Param schema + current values, in the order the filter's `ParamDef`
        /// slice declares them. Empty for parameter-free filters (invert);
        /// carries the five tone curves for `curves`. Same shape the void panel
        /// uses.
        params: Vec<ParamInfo>,
    },
    /// Vector-object layer (text today). Carries no pixel buffer: the texture
    /// is realized from its `objects`.
    #[serde(rename_all = "camelCase")]
    Vector {
        id: f64,
        name: String,
        visible: bool,
        locked: bool,
        editable: bool,
        /// Whether paint ops have somewhere to land on this node, mirroring
        /// `DarklyEngine::is_node_paintable`. False for kinds whose pixels are
        /// generated (void, filter, vector) and for groups; the panel reads it
        /// to offer "Rasterize" instead of branching on `type`.
        paintable: bool,
        can_have_mask: bool,
        can_rename: bool,
        has_thumbnail: bool,
        /// Whether this row offers "Convert to Smart Object"; mirrors
        /// `DarklyEngine::can_convert_layer_to_smart_object`. The engine
        /// answers so the rule (owns its pixels, editable, no mask) lives with
        /// the operation instead of being restated by the panel.
        can_become_smart_object: bool,
        icon: &'static str,
        kind_name: &'static str,
        opacity: f32,
        blend_mode: &'static str,
        modifiers: Vec<ModifierInfo>,
    },
    #[serde(rename_all = "camelCase")]
    Group {
        id: f64,
        name: String,
        visible: bool,
        locked: bool,
        editable: bool,
        /// Whether paint ops have somewhere to land on this node, mirroring
        /// `DarklyEngine::is_node_paintable`. False for kinds whose pixels are
        /// generated (void, filter, vector) and for groups; the panel reads it
        /// to offer "Rasterize" instead of branching on `type`.
        paintable: bool,
        can_have_mask: bool,
        can_rename: bool,
        has_thumbnail: bool,
        /// Whether this row offers "Convert to Smart Object"; mirrors
        /// `DarklyEngine::can_convert_layer_to_smart_object`. The engine
        /// answers so the rule (owns its pixels, editable, no mask) lives with
        /// the operation instead of being restated by the panel.
        can_become_smart_object: bool,
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
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct ModifierInfo {
    pub id: f64,
    pub kind: &'static str,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    /// Whether this modifier participates in transforms with its host.
    pub linked_to_host: bool,
    /// See [`LayerInfo::Raster::editable`]: a modifier is editable when
    /// neither it nor its host (nor any ancestor of the host) is locked.
    pub editable: bool,
}

/// Per-instance view of a veil in the chain. `type` is the registry `type_id`;
/// resolve to a display label via `veil_types()`; never duplicate it here.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct VeilInfo {
    #[serde(rename = "type")]
    pub type_id: String,
    pub visible: bool,
    pub index: usize,
    pub params: Vec<ParamInfo>,
}

/// Range and default rendered for reading: each number converted into its
/// display unit and suffixed. Carried alongside the raw numbers so a consumer
/// that only wants to *show* the schema needs no unit table of its own.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct ParamDisplay {
    pub min: Option<String>,
    pub max: Option<String>,
    pub default: Option<String>,
    /// The unit suffix alone, for a column header. Empty for unitless values.
    pub unit: &'static str,
}

/// Format a display-space number without trailing zeros: `180.0` reads
/// `"180"`, `0.25` stays `"0.25"`.
fn fmt_display(value: f32, unit: UnitType) -> String {
    let v = unit.to_display(value);
    let mut s = format!("{v:.3}");
    if s.contains('.') {
        s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    format!("{s}{}", unit.suffix())
}

/// Flat serialization-friendly view of a parameter definition + current value.
/// Avoids nesting a tagged enum (ParamDef) which serde_wasm_bindgen can't handle.
#[derive(serde::Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct ParamInfo {
    pub kind: &'static str,
    pub name: &'static str,
    /// Display label. `None` → the UI title-cases `name`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<&'static str>,
    /// How to render this parameter's editor. One closed set, which both
    /// `ParamKind` and the settings schema's `WidgetHint` map into:
    /// `"auto"`, `"numberInput"`, `"icon"`, `"hotkey"`, `"color"`, `"hidden"`.
    pub widget: &'static str,
    pub unit: UnitType,
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
    #[cfg_attr(feature = "ts-export", ts(type = "JsonValue | null"))]
    pub options: Option<serde_json::Value>,
    pub display: ParamDisplay,
}

impl ParamInfo {
    pub fn from_def(def: &ParamDef, value: Option<&ParamValue>) -> Self {
        // `name`, `label`, `description` and `unit` are the same for every
        // kind, so only the value-shaped part is projected per variant.
        let (kind, widget, min, max, options) = match &def.kind {
            ParamKind::Float { min, max, .. } => {
                ("float", "auto", Some(*min as f64), Some(*max as f64), None)
            }
            ParamKind::Int { min, max, .. } => {
                ("int", "auto", Some(*min as f64), Some(*max as f64), None)
            }
            ParamKind::Bool { .. } => ("bool", "auto", None, None, None),
            ParamKind::String { .. } => ("string", "auto", None, None, None),
            ParamKind::Curve { .. } => ("curve", "auto", None, None, None),
            ParamKind::Levels { .. } => ("levels", "auto", None, None, None),
            ParamKind::Enum { options, .. } => {
                ("enum", "auto", None, None, Some(serde_json::json!(options)))
            }
            ParamKind::FloatInput { min, max, .. } => (
                "floatInput",
                "numberInput",
                Some(*min as f64),
                Some(*max as f64),
                None,
            ),
            ParamKind::Icon { options, .. } => {
                ("icon", "icon", None, None, Some(serde_json::json!(options)))
            }
            ParamKind::Color { .. } => ("color", "auto", None, None, None),
            // For a vec2 the flat `max` field carries the magnitude clamp
            // (the offset pad's edge radius).
            ParamKind::Vec2 { max, .. } => ("vec2", "auto", None, Some(*max as f64), None),
            ParamKind::List { item, max_len, .. } => (
                "list",
                "auto",
                None,
                // For a list the flat `max` field carries the entry cap so the
                // editor disables "Add" at the limit without an effect-specific
                // constant.
                Some(*max_len as f64),
                // The item schema rides the same kind-discriminated `options`
                // channel Enum/Icon use (here a `Vec<ParamInfo>` of the item
                // defs) so the list editor can render each entry's fields.
                Some(serde_json::json!(item
                    .iter()
                    .map(|d| ParamInfo::from_def(d, None))
                    .collect::<Vec<_>>())),
            ),
        };

        let default = def.default_value();
        // Only a scalar range renders: a curve or a list has no single number
        // to show, and `Vec2`'s `max` is a magnitude rather than a bound.
        let scalar_default = match &default {
            ParamValue::Float(f) => Some(*f),
            ParamValue::Int(i) => Some(*i as f32),
            _ => None,
        };
        let renders_range = matches!(
            def.kind,
            ParamKind::Float { .. } | ParamKind::Int { .. } | ParamKind::FloatInput { .. }
        );
        let display = ParamDisplay {
            min: renders_range.then(|| fmt_display(min.unwrap_or(0.0) as f32, def.unit)),
            max: renders_range.then(|| fmt_display(max.unwrap_or(0.0) as f32, def.unit)),
            default: scalar_default.map(|d| fmt_display(d, def.unit)),
            unit: def.unit.suffix(),
        };

        ParamInfo {
            kind,
            name: def.name,
            label: def.label,
            description: def.description,
            widget,
            unit: def.unit,
            min,
            max,
            default,
            value: value.cloned(),
            options,
            display,
        }
    }

    /// Project a declared preference into the same shape an effect parameter
    /// takes. `Pref` and `ParamDef` are isomorphic once both carry a label, a
    /// description and a widget hint, so the settings surface and the effect
    /// panels consume one type rather than two near-identical ones.
    ///
    /// `name` is the pref's dot-path key, and `default` comes from the
    /// editor-agnostic defaults layer: the schema declares type and range, not
    /// values, so the value has to be read from where it actually lives.
    pub fn from_pref(pref: &crate::config::schema::Pref) -> Self {
        use crate::config::schema::{PrefKind, WidgetHint};
        use crate::config::ConfigValue;

        let (kind, min, max, options) = match &pref.kind {
            PrefKind::Bool => ("bool", None, None, None),
            PrefKind::Int { min, max } => ("int", Some(*min as f64), Some(*max as f64), None),
            PrefKind::Float { min, max } => ("float", Some(*min), Some(*max), None),
            PrefKind::Str => ("str", None, None, None),
            PrefKind::Enum { options } => ("enum", None, None, Some(serde_json::json!(options))),
        };

        // Both producers map into one closed widget set; see `ParamInfo.widget`.
        let widget = match pref.widget {
            WidgetHint::Auto => "auto",
            WidgetHint::NumberInput => "numberInput",
            WidgetHint::Hotkey => "hotkey",
            WidgetHint::Color => "color",
            WidgetHint::Hidden => "hidden",
        };

        let default = match crate::config::agnostic_default(pref.key) {
            Some(ConfigValue::Bool(b)) => ParamValue::Bool(b),
            Some(ConfigValue::Int(i)) => ParamValue::Int(i as i32),
            Some(ConfigValue::Float(f)) => ParamValue::Float(f as f32),
            Some(ConfigValue::Str(s)) => ParamValue::String(s),
            // A pref the agnostic layer does not set is one every editor
            // overlay is expected to supply; fall back to the kind's zero so
            // the shape stays total.
            None => match &pref.kind {
                PrefKind::Bool => ParamValue::Bool(false),
                PrefKind::Int { min, .. } => ParamValue::Int(*min as i32),
                PrefKind::Float { min, .. } => ParamValue::Float(*min as f32),
                PrefKind::Str | PrefKind::Enum { .. } => ParamValue::String(String::new()),
            },
        };

        let scalar_default = match &default {
            ParamValue::Float(f) => Some(*f),
            ParamValue::Int(i) => Some(*i as f32),
            _ => None,
        };
        let renders_range = matches!(pref.kind, PrefKind::Int { .. } | PrefKind::Float { .. });
        // Prefs declare no unit; their ranges are already in display space.
        let unit = UnitType::Raw;

        ParamInfo {
            kind,
            name: pref.key,
            label: Some(pref.display_name),
            description: pref.description,
            widget,
            unit,
            min,
            max,
            display: ParamDisplay {
                min: renders_range.then(|| fmt_display(min.unwrap_or(0.0) as f32, unit)),
                max: renders_range.then(|| fmt_display(max.unwrap_or(0.0) as f32, unit)),
                default: scalar_default.map(|d| fmt_display(d, unit)),
                unit: unit.suffix(),
            },
            default,
            value: None,
            options,
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
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
        /// Foreground color as raw sRGB RGBA floats (0-1), as picked; the
        /// compositor is display-referred, so no gamma conversion is applied.
        cr: f32,
        cg: f32,
        cb: f32,
        ca: f32,
    },
}

impl StrokeOp {
    /// The canvas region the paint target must cover before this op runs, or
    /// `None` when the op cannot reach beyond the pixels the target already has.
    ///
    /// A generative op manufactures pixels where there were none, so it claims
    /// the whole canvas window: a target smaller than the canvas (a layer
    /// allocated before a canvas resize, a paste-extent layer) is grown to meet
    /// it rather than silently clipping the result to its allocation. Krita
    /// reaches the same place from the other side, handing its gradient the
    /// image bounds as an apply rect over a paint device that grows implicitly
    /// (`plugins/tools/basictools/kis_tool_gradient.cc`).
    ///
    /// `current` is the target's canvas extent, `canvas` the document window.
    pub(crate) fn required_coverage(
        &self,
        current: CanvasRect,
        canvas: CanvasRect,
    ) -> Option<CanvasRect> {
        match self {
            StrokeOp::LinearGradient { .. } => Some(canvas),

            // The reachable region is the computed fill mask, which does not
            // exist until the async readback lands, and that readback is bounded
            // by the target texture. Coverage is settled there, not here.
            StrokeOp::FloodFill { .. } => None,

            // Growth only once the dab CENTER escapes the target, matching
            // Krita's rule of growing when paint escapes the recorded bounds.
            // A footprint that merely crosses the edge clips, as it would
            // against the canvas-aligned layer it started from.
            StrokeOp::BrushStroke { x, y, .. } => {
                let cx = x.floor() as i32;
                let cy = y.floor() as i32;
                if current.contains(CanvasRect::from_xywh(cx, cy, 1, 1)) {
                    return None;
                }
                // Pad by half a reference dab so the grown extent takes in the
                // dab's footprint, not just its center pixel.
                const HALF: i32 = (crate::brush::DAB_REFERENCE_SIZE / 2) as i32;
                Some(CanvasRect::from_xywh(
                    cx - HALF,
                    cy - HALF,
                    (HALF as u32) * 2,
                    (HALF as u32) * 2,
                ))
            }
        }
    }
}

/// Data returned to the WASM bridge on copy/cut: always RGBA pixels regardless
/// of the internal clipboard variant.
#[derive(serde::Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
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
    filter_registry: &crate::gpu::filter::FilterPipelineRegistry,
    node_id: crate::layer::LayerId,
) -> Option<LayerInfo> {
    use crate::layer::{Layer, LayerNode};
    let node = doc.find_node(node_id)?;
    let editable = doc.is_node_editable(node_id);
    let paintable = doc.pixel_buffer(node_id).is_some();
    let kind = node.kind();
    // Most capability flags are per *kind*, but the thumbnail is a per-layer
    // question: a void holding a supplied image shows the image, where its
    // procedural and live siblings show a glyph. `Layer::has_thumbnail`
    // answers it; a group has no layer to ask, so it keeps the kind flag.
    let node_has_thumbnail = match node {
        LayerNode::Layer(layer) => layer.has_thumbnail(void_registry),
        LayerNode::Group(_) => kind.has_thumbnail,
    };
    let can_become_smart_object =
        crate::engine::smart_object::layer_can_become_smart_object(doc, node_id);
    let info = match node {
        LayerNode::Layer(layer) => match layer {
            Layer::Raster(r) => LayerInfo::Raster {
                id: r.id.to_ffi() as f64,
                name: r.common.name.clone(),
                visible: r.common.visible,
                locked: r.common.locked,
                editable,
                paintable,
                can_have_mask: kind.can_have_mask,
                can_rename: kind.can_rename,
                has_thumbnail: node_has_thumbnail,
                can_become_smart_object,
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
                    paintable,
                    can_have_mask: kind.can_have_mask,
                    can_rename: kind.can_rename,
                    has_thumbnail: node_has_thumbnail,
                    can_become_smart_object,
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
            Layer::Filter(f) => {
                let param_defs = filter_registry.params(&f.pipeline);
                let params = param_defs
                    .iter()
                    .enumerate()
                    .map(|(j, def)| ParamInfo::from_def(def, f.params.get(j)))
                    .collect();
                let pipeline_icon = filter_registry.icon(&f.pipeline);
                LayerInfo::Filter {
                    id: f.id.to_ffi() as f64,
                    name: f.common.name.clone(),
                    visible: f.common.visible,
                    locked: f.common.locked,
                    editable,
                    paintable,
                    can_have_mask: kind.can_have_mask,
                    can_rename: kind.can_rename,
                    has_thumbnail: node_has_thumbnail,
                    can_become_smart_object,
                    icon: if pipeline_icon.is_empty() {
                        kind.icon
                    } else {
                        pipeline_icon
                    },
                    kind_name: kind.display_name,
                    opacity: f.blend.opacity,
                    blend_mode: f.blend.blend_mode.type_id,
                    modifiers: f
                        .filters
                        .iter()
                        .filter_map(|mid| doc.find_filter(*mid).map(|m| modifier_to_info(doc, m)))
                        .collect(),
                    pipeline: f.pipeline.clone(),
                    params,
                }
            }
            Layer::Vector(v) => LayerInfo::Vector {
                id: v.id.to_ffi() as f64,
                name: v.common.name.clone(),
                visible: v.common.visible,
                locked: v.common.locked,
                editable,
                paintable,
                can_have_mask: kind.can_have_mask,
                can_rename: kind.can_rename,
                has_thumbnail: node_has_thumbnail,
                can_become_smart_object,
                icon: kind.icon,
                kind_name: kind.display_name,
                opacity: v.blend.opacity,
                blend_mode: v.blend.blend_mode.type_id,
                modifiers: v
                    .filters
                    .iter()
                    .filter_map(|mid| doc.find_filter(*mid).map(|m| modifier_to_info(doc, m)))
                    .collect(),
            },
        },
        LayerNode::Group(g) => LayerInfo::Group {
            id: g.id.to_ffi() as f64,
            name: g.common.name.clone(),
            visible: g.common.visible,
            locked: g.common.locked,
            editable,
            paintable,
            can_have_mask: kind.can_have_mask,
            can_rename: kind.can_rename,
            has_thumbnail: node_has_thumbnail,
            can_become_smart_object,
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
                .filter_map(|cid| node_to_layer_info(doc, void_registry, filter_registry, *cid))
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
        linked_to_host: match &modifier.kind {
            crate::document::FilterKind::Mask(mask) => mask.linked_to_host,
            crate::document::FilterKind::Selection(_) => false,
        },
        editable: doc.is_node_editable(modifier.id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brush_at(x: f32, y: f32) -> StrokeOp {
        StrokeOp::BrushStroke {
            x,
            y,
            pressure: 1.0,
            x_tilt: 0.0,
            y_tilt: 0.0,
            rotation: 0.0,
            tangential_pressure: 0.0,
            time_ms: 0.0,
            cr: 1.0,
            cg: 0.0,
            cb: 0.0,
            ca: 1.0,
        }
    }

    fn gradient() -> StrokeOp {
        StrokeOp::LinearGradient {
            x0: 0.0,
            y0: 0.0,
            x1: 1.0,
            y1: 0.0,
            r0: 255,
            g0: 0,
            b0: 0,
            a0: 255,
            r1: 0,
            g1: 0,
            b1: 255,
            a1: 255,
        }
    }

    /// A gradient is generative, so it claims the canvas window whether the
    /// target is smaller than it (a layer predating a resize) or larger (a
    /// paste-extent or post-crop layer).
    #[test]
    fn gradient_claims_the_canvas_window() {
        let canvas = CanvasRect::from_xywh(16, 16, 128, 96);
        let smaller = CanvasRect::from_xywh(0, 0, 64, 64);
        let larger = CanvasRect::from_xywh(-256, -256, 1024, 1024);

        assert_eq!(gradient().required_coverage(smaller, canvas), Some(canvas));
        assert_eq!(gradient().required_coverage(larger, canvas), Some(canvas));
    }

    /// A flood fill's reach is settled by its async readback, not here.
    #[test]
    fn flood_fill_claims_nothing() {
        let op = StrokeOp::FloodFill {
            x: 10.0,
            y: 10.0,
            r: 0,
            g: 0,
            b: 0,
            a: 255,
            tolerance: 0,
        };
        let canvas = CanvasRect::from_xywh(0, 0, 128, 96);
        assert_eq!(
            op.required_coverage(CanvasRect::from_xywh(0, 0, 64, 64), canvas),
            None
        );
    }

    /// A brush grows only once the dab CENTER leaves the target, and then by a
    /// half-reference-dab pad around that center rather than by the canvas.
    #[test]
    fn brush_grows_only_when_its_center_escapes() {
        let canvas = CanvasRect::from_xywh(0, 0, 128, 96);
        let current = CanvasRect::from_xywh(0, 0, 64, 64);

        assert_eq!(
            brush_at(32.0, 32.0).required_coverage(current, canvas),
            None
        );
        assert_eq!(brush_at(63.9, 0.0).required_coverage(current, canvas), None);

        const HALF: i32 = (crate::brush::DAB_REFERENCE_SIZE / 2) as i32;
        assert_eq!(
            brush_at(100.0, 20.0).required_coverage(current, canvas),
            Some(CanvasRect::from_xywh(
                100 - HALF,
                20 - HALF,
                HALF as u32 * 2,
                HALF as u32 * 2
            )),
            "an escaped dab claims a pad around itself, not the canvas"
        );
    }
}
