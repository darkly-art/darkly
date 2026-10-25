//! Parametric shape objects on vector layers: place (live, during a drag),
//! edit, list.

use darkly_macros::handlers;

use super::layers::{brush_rgba, solid_rgba, VectorOpKind};
use super::pending_edit::{EditSessionKind, PendingEdit};
use super::types::ParamInfo;
use super::DarklyEngine;
use crate::engine::protocol::{params_from_json, RawParams};
use crate::layer::{Layer, LayerId, LayerNode, ObjectId, ObjectSource, ShapeProps, VectorObject};

/// A solid border: width in object-local pixels, colour as RGBA 0-255.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct StrokeWire {
    pub width: f32,
    pub color: [u8; 4],
}

impl StrokeWire {
    /// Mitered so a rectangle or sharp polygon keeps its corners. A width of
    /// zero (or less, or non-finite) is no border, so the document never holds
    /// a degenerate stroke.
    fn to_stroke(self) -> Option<(kurbo::Stroke, peniko::Brush)> {
        (self.width > 0.0 && self.width.is_finite()).then(|| {
            (
                kurbo::Stroke::new(self.width as f64).with_join(kurbo::Join::Miter),
                solid_rgba(self.color),
            )
        })
    }

    fn from_stroke(stroke: &Option<(kurbo::Stroke, peniko::Brush)>) -> Option<Self> {
        stroke.as_ref().map(|(s, b)| StrokeWire {
            width: s.width as f32,
            color: brush_rgba(&Some(b.clone())),
        })
    }
}

/// The layer and object a placed shape landed on.
#[derive(serde::Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct ShapeObjectRef {
    pub id: LayerId,
    pub object: ObjectId,
}

/// One shape object as the properties panel binds it: family, parameter
/// schema with current values, box size and style.
#[derive(serde::Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub struct ShapeObjectInfo {
    pub object: ObjectId,
    pub family: String,
    pub params: Vec<ParamInfo>,
    pub width: f32,
    pub height: f32,
    pub fill: Option<[u8; 4]>,
    pub stroke: Option<StrokeWire>,
}

/// The shape a drag is placing: a document layer with no undo record until
/// the drag ends.
pub(crate) struct ShapePlacement {
    layer: LayerId,
    object: ObjectId,
}

/// A shape being placed. It lasts only for the drag that places it, so it
/// resolves like any pending edit but is never presented as a session.
pub(crate) struct ShapePlacementEdit;

impl PendingEdit for ShapePlacementEdit {
    fn kind(&self) -> EditSessionKind {
        EditSessionKind {
            id: "shapePlacement",
            display_name: "Placing shape",
            icon: "fa6-solid:shapes",
        }
    }

    fn held(&self, engine: &DarklyEngine) -> bool {
        engine.shape_placement.is_some()
    }

    fn presented(&self, _engine: &DarklyEngine) -> bool {
        false
    }

    fn apply(&self, engine: &mut DarklyEngine) {
        let _ = engine.commit_shape();
    }

    fn discard(&self, engine: &mut DarklyEngine) {
        engine.cancel_shape();
    }
}

/// A box the engine can draw: positive and finite on both axes.
fn is_drawable_box(width: f32, height: f32) -> bool {
    width > 0.0 && height > 0.0 && width.is_finite() && height.is_finite()
}

#[handlers]
impl DarklyEngine {
    /// Start placing a shape: a new layer named after the family above
    /// `anchor`, holding one shape whose local box `[0, width] x [0, height]`
    /// sits at plane `(x, y)`, live on the canvas. `params` is coerced against
    /// the family schema (absent values take defaults). No undo step until
    /// [`Self::commit_shape`]. Resolves pending edits first, which commits a
    /// stale placement. No-op for an unknown family or an empty box.
    #[handler]
    #[allow(clippy::too_many_arguments)]
    pub fn begin_shape(
        &mut self,
        family: String,
        params: RawParams,
        x: f64,
        y: f64,
        width: f32,
        height: f32,
        fill: Option<[u8; 4]>,
        stroke: Option<StrokeWire>,
        anchor: Option<LayerId>,
    ) {
        self.commit_pending_edits();
        let Some(reg) = crate::shape::find(&family) else {
            return;
        };
        if !is_drawable_box(width, height) {
            return;
        }
        let props = ShapeProps {
            family,
            params: params_from_json(&params.0, reg.params),
            width,
            height,
        };
        let obj = VectorObject::shape(
            props,
            kurbo::Affine::translate((x, y)),
            fill.map(solid_rgba),
            stroke.and_then(StrokeWire::to_stroke),
        );
        let (layer, object) = self.insert_vector_layer_with(reg.display_name, obj, anchor);
        self.shape_placement = Some(ShapePlacement { layer, object });
    }

    /// Move and resize the placing shape's box to plane `(x, y)` and
    /// `width x height`. No undo step. No-op without a placement or for an
    /// empty box.
    #[handler]
    pub fn update_shape(&mut self, x: f64, y: f64, width: f32, height: f32) {
        let Some(ShapePlacement { layer, object }) = self.shape_placement else {
            return;
        };
        if !is_drawable_box(width, height) {
            return;
        }
        // A fresh vector layer has an identity transform, so plane and
        // layer-local coincide.
        let Some(LayerNode::Layer(Layer::Vector(v))) = self.doc.find_node_mut(layer) else {
            return;
        };
        let Some(obj) = v.object_mut(object) else {
            return;
        };
        obj.transform = kurbo::Affine::translate((x, y));
        if let ObjectSource::Shape(s) = &mut obj.source {
            s.width = width;
            s.height = height;
        }
        self.sync_vector_layer(layer);
        self.compositor.mark_dirty();
    }

    /// Record the placement as one undo step and return where it landed.
    /// `None` without a placement, or when the layer was swallowed mid-drag
    /// (flatten and merge adopt it without going through the pending-edit
    /// chokepoint).
    #[handler]
    pub fn commit_shape(&mut self) -> Option<ShapeObjectRef> {
        let ShapePlacement { layer, object } = self.take_live_placement()?;
        self.record_entity_add(layer);
        Some(ShapeObjectRef { id: layer, object })
    }

    /// Drop the placement: the layer is removed outright and its GPU state
    /// released, leaving no layer and no undo step. No-op without a
    /// placement.
    #[handler]
    pub fn cancel_shape(&mut self) {
        if let Some(p) = self.take_live_placement() {
            self.discard_unrecorded_layer(p.layer);
        }
    }

    /// Replace one shape's parameters, coerced against its family schema
    /// (absent values take defaults). Coalesces on the `Content` lane, so a
    /// slider drag is one undo step. No-op for a non-shape object.
    #[handler]
    pub fn set_shape_params(&mut self, id: LayerId, object: ObjectId, params: RawParams) {
        let Some(Layer::Vector(v)) = self.doc.layer(id) else {
            return;
        };
        let Some(ObjectSource::Shape(s)) = v.object(object).map(|o| &o.source) else {
            return;
        };
        let Some(reg) = crate::shape::find(&s.family) else {
            return;
        };
        let values = params_from_json(&params.0, reg.params);
        self.edit_vector_object(id, object, VectorOpKind::Content, |obj| {
            if let ObjectSource::Shape(s) = &mut obj.source {
                s.params = values;
            }
        });
    }

    /// Replace one shape's fill and border wholesale (`null` removes either).
    /// Coalesces on the `Style` lane.
    #[handler]
    pub fn set_shape_style(
        &mut self,
        id: LayerId,
        object: ObjectId,
        fill: Option<[u8; 4]>,
        stroke: Option<StrokeWire>,
    ) {
        self.edit_vector_object(id, object, VectorOpKind::Style, |obj| {
            if matches!(obj.source, ObjectSource::Shape(_)) {
                obj.fill = fill.map(solid_rgba);
                obj.stroke = stroke.and_then(StrokeWire::to_stroke);
            }
        });
    }

    /// Every shape object on a vector layer, bottom to top. Empty for a
    /// non-vector layer or one holding no shapes.
    #[handler(send)]
    pub fn shape_objects(&self, id: LayerId) -> Vec<ShapeObjectInfo> {
        let Some(Layer::Vector(v)) = self.doc.layer(id) else {
            return Vec::new();
        };
        v.objects
            .iter()
            .filter_map(|obj| {
                let ObjectSource::Shape(s) = &obj.source else {
                    return None;
                };
                let defs = crate::shape::find(&s.family).map_or(&[][..], |r| r.params);
                Some(ShapeObjectInfo {
                    object: obj.id,
                    family: s.family.clone(),
                    params: defs
                        .iter()
                        .enumerate()
                        .map(|(i, d)| ParamInfo::from_def(d, s.params.get(i)))
                        .collect(),
                    width: s.width,
                    height: s.height,
                    fill: obj.fill.is_some().then(|| brush_rgba(&obj.fill)),
                    stroke: StrokeWire::from_stroke(&obj.stroke),
                })
            })
            .collect()
    }
}

impl DarklyEngine {
    /// Test-only: the layer a placement in progress is drawing on.
    #[cfg(any(test, feature = "testing"))]
    pub fn shape_placement_layer_for_test(&self) -> Option<LayerId> {
        self.shape_placement.as_ref().map(|p| p.layer)
    }

    /// End the placement, returning it only while its layer is still
    /// reachable from the root. A detached parent still answers `slot_of`, so
    /// reachability is the test.
    fn take_live_placement(&mut self) -> Option<ShapePlacement> {
        let p = self.shape_placement.take()?;
        self.doc.is_ancestor_of(self.doc.root, p.layer).then_some(p)
    }
}
