//! Layer CRUD and property operations.

use darkly_macros::handlers;

use super::types::{node_to_layer_info, LayerTree};
use super::DarklyEngine;
use crate::document::{MoveTarget, TreeSlot};
use crate::engine::protocol::{params_from_json, RawParams};
use crate::layer::{Layer, LayerId, LayerNode};
use crate::undo::property::Property;
use crate::undo::{
    CompoundAction, EntityAddAction, EntityRemoveAction, LayerMoveAction, PropertyAction,
    ScreenSpaceBoundaryAction, UndoAction,
};

/// Convert Darkly's row-major `[a, b, tx, c, d, ty]` affine (point map
/// `x' = a·x + b·y + tx`, `y' = c·x + d·y + ty`) into kurbo's column-major
/// `[a, b, c, d, e, f]` (`x' = a·x + c·y + e`, `y' = b·x + d·y + f`).
fn transform_to_kurbo(t: &crate::transform::Transform) -> kurbo::Affine {
    let [a, b, tx, c, d, ty] = t.to_affine();
    kurbo::Affine::new([a as f64, c as f64, b as f64, d as f64, tx as f64, ty as f64])
}

/// Inverse of [`transform_to_kurbo`]: kurbo column-major `[a, b, c, d, e, f]`
/// → Darkly row-major `[a, c, e, b, d, f]`. The one place this reordering
/// lives, so consumers never reshuffle affine components inline.
fn kurbo_to_affine(a: kurbo::Affine) -> crate::transform::Affine2D {
    let [k0, k1, k2, k3, k4, k5] = a.as_coeffs();
    [
        k0 as f32, k2 as f32, k4 as f32, k1 as f32, k3 as f32, k5 as f32,
    ]
}

/// The logical operations that route through one `Property::VectorObjects`
/// undo kind. Each gets its own coalesce lane so a typing run, a style-slider
/// drag, and a gizmo drag don't merge into a single undo step.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum VectorOpKind {
    Content,
    Style,
    Transform,
    /// A text box-frame resize (origin + box size). Its own lane so a resize
    /// drag coalesces to one undo step, distinct from a glyph transform.
    Box,
}

/// Coalesce discriminator for one object's edits: distinct per `(object, op)`,
/// so adjacent same-object same-op edits merge but a different object or op
/// starts a fresh undo step. (Coalescing only inspects the stack top, so
/// non-adjacent same-tag ops never merge across an intervening different one.)
fn vector_coalesce_tag(object: crate::layer::ObjectId, op: VectorOpKind) -> u64 {
    let op = match op {
        VectorOpKind::Content => 0,
        VectorOpKind::Style => 1,
        VectorOpKind::Transform => 2,
        VectorOpKind::Box => 3,
    };
    (object.0 << 2) | op
}

/// Extract an RGBA byte tuple from an optional solid fill, defaulting to opaque
/// black for a missing or non-solid brush (mirrors the render-time fallback).
fn brush_rgba(fill: &Option<peniko::Brush>) -> [u8; 4] {
    match fill {
        Some(peniko::Brush::Solid(c)) => {
            let rgba8 = c.to_rgba8();
            [rgba8.r, rgba8.g, rgba8.b, rgba8.a]
        }
        _ => [0, 0, 0, 255],
    }
}

/// One text object's content, style, and fill color — the data the
/// text-properties panel binds each editor block to. Carries no geometry:
/// the panel edits off-canvas, so shaping/bounds are never needed.
pub struct TextObjectEntry {
    pub object: crate::layer::ObjectId,
    pub content: String,
    pub font_family: String,
    pub size: f32,
    /// Variable-font axis values (tag → value), including `wght`. Empty for an
    /// untouched/static font.
    pub variations: std::collections::BTreeMap<String, f32>,
    /// OpenType feature values (tag → value).
    pub features: std::collections::BTreeMap<String, u32>,
    pub letter_spacing: f32,
    pub word_spacing: f32,
    pub line_height: f32,
    pub italic: bool,
    pub align: crate::layer::TextAlign,
    pub color: [u8; 4],
    /// `Some((w, h))` for area text, `None` for point text — lets the panel
    /// tell which mode an object is in.
    pub box_size: Option<(f32, f32)>,
}

#[handlers]
impl DarklyEngine {
    // --- Layer CRUD ---

    pub fn add_raster_layer(&mut self, anchor: Option<LayerId>) -> LayerId {
        let id = self.doc.add_raster_layer(anchor);
        let bounds = match self.doc.layer(id) {
            Some(Layer::Raster(r)) => r.pixels.bounds,
            _ => self.doc.canvas_rect(),
        };
        self.compositor
            .ensure_raster_layer(&self.gpu.device, &self.gpu.queue, id, bounds);
        self.compositor.mark_dirty();

        let slot = self.doc.slot_of(id).unwrap_or_default();
        self.push_undo(Box::new(EntityAddAction::new(id, slot)));

        id
    }

    // --- Wire entry points ---
    //
    // These are the protocol verbs the frontend calls: they take wire-friendly
    // arguments (`RawParams`, `Option<LayerId>`) and forward to the typed
    // primitives above/below. The param-bearing ones own the single coercion
    // seam (`coerce_void_params`), pairing a raw params object with the sibling
    // field that names its schema — the one thing generic request routing can't
    // do for itself. Direct Rust callers (and tests) use the typed primitives.

    /// Wire entry for `add_raster` — see [`Self::add_raster_layer`].
    #[handler]
    pub fn add_raster(&mut self, anchor: Option<LayerId>) -> LayerId {
        self.add_raster_layer(anchor)
    }

    /// Wire entry for `add_void` — coerces `params` against the void type's
    /// schema, then [`Self::add_void_layer`].
    #[handler]
    pub fn add_void(
        &mut self,
        void_type: String,
        params: RawParams,
        anchor: Option<LayerId>,
    ) -> Option<LayerId> {
        let pv = self.coerce_void_params(&void_type, &params.0);
        self.add_void_layer(&void_type, pv, anchor)
    }

    /// Wire entry for `add_filter` — coerces `params` against the filter type's
    /// schema (defaults fill any omitted values), then [`Self::add_filter_layer`].
    #[handler]
    pub fn add_filter(
        &mut self,
        pipeline: String,
        params: RawParams,
        anchor: Option<LayerId>,
    ) -> Option<LayerId> {
        let pv = params_from_json(&params.0, self.filter_param_defs(&pipeline));
        self.add_filter_layer(&pipeline, pv, anchor)
    }

    /// Wire entry for `set_void_params` — resolves the layer's void type,
    /// coerces `params` against its schema, then [`Self::update_void_params`].
    /// A non-void (or stale) id is a silent no-op.
    #[handler]
    pub fn set_void_params(&mut self, id: LayerId, params: RawParams) {
        let Some(type_id) = self.void_layer_type(id) else {
            return;
        };
        let pv = self.coerce_void_params(&type_id, &params.0);
        self.update_void_params(id, pv);
    }

    /// Wire entry for `set_filter_params` — resolves the layer's filter type,
    /// coerces `params` against its schema, then [`Self::update_filter_params`].
    /// A non-filter (or stale) id is a silent no-op. The exact analog of
    /// [`Self::set_void_params`].
    #[handler]
    pub fn set_filter_params(&mut self, id: LayerId, params: RawParams) {
        let Some(type_id) = self.filter_layer_pipeline(id) else {
            return;
        };
        let pv = params_from_json(&params.0, self.filter_param_defs(&type_id));
        self.update_filter_params(id, pv);
    }

    // --- Vector / text layers ---

    /// Family names available to the font picker. Bundled fonts today.
    pub fn list_fonts(&self) -> Vec<String> {
        self.fonts.list_fonts().to_vec()
    }

    /// Register a font blob (uploaded `.ttf`/`.otf` or a decoded Google import)
    /// into this engine's font collection. Returns the family names it
    /// contributed so the frontend library can index them. A thin passthrough to
    /// [`crate::text::FontRegistry::register_font`] — the bytes are content-hashed
    /// and cached there so the families round-trip through `.darkly` save/load.
    pub fn register_font(&mut self, bytes: Vec<u8>) -> Vec<String> {
        self.fonts.register_font(bytes)
    }

    /// Local bbox of an owned object. Takes the object by reference so callers
    /// iterating a cloned object list can shape each without re-borrowing the
    /// document while `&mut self.fonts` is live.
    fn object_bbox(&mut self, obj: &crate::layer::VectorObject) -> Option<kurbo::Rect> {
        use kurbo::Shape;
        match &obj.source {
            crate::layer::ObjectSource::Text(t) => {
                // Area text's extent is its fixed box; point text's is the
                // natural shaped size. The box drives both the gizmo frame
                // (`vector_object_info`) and hit-testing.
                if let Some((w, h)) = t.layout.area_size() {
                    Some(kurbo::Rect::new(0.0, 0.0, w as f64, h as f64))
                } else {
                    let layout = self.fonts.shape(t);
                    Some(kurbo::Rect::new(
                        0.0,
                        0.0,
                        layout.width() as f64,
                        layout.height() as f64,
                    ))
                }
            }
            crate::layer::ObjectSource::Path(p) => Some(p.bounding_box()),
        }
    }

    /// Hit-test a plane-space point against the objects of a vector layer,
    /// returning the topmost object covering it (objects draw bottom-to-top, so
    /// the iteration is reversed). The query point is mapped into each object's
    /// local frame by the inverse of `layer_transform * obj.transform` — the
    /// same composition `build_scene` draws through — so the test is tight even
    /// for rotated/scaled text. `None` for a miss, a locked/hidden layer, or a
    /// non-vector id. `(x, y)` are PLANE coordinates (see
    /// `docs/coordinate-systems.md`).
    pub fn hit_test_vector_object(
        &mut self,
        layer_id: LayerId,
        x: f64,
        y: f64,
    ) -> Option<crate::layer::ObjectId> {
        if !self.doc.is_node_editable(layer_id) || !self.doc.effective_visible(layer_id) {
            return None;
        }
        let (objects, layer_affine) = match self.doc.layer(layer_id) {
            Some(Layer::Vector(v)) => (v.objects.clone(), transform_to_kurbo(&v.transform)),
            _ => return None,
        };
        let query = kurbo::Point::new(x, y);
        for obj in objects.iter().rev() {
            let Some(bbox) = self.object_bbox(obj) else {
                continue;
            };
            let composed = layer_affine * obj.transform;
            if composed.determinant().abs() < 1e-12 {
                continue;
            }
            let local = composed.inverse() * query;
            if bbox.contains(local) {
                return Some(obj.id);
            }
        }
        None
    }

    /// Add a vector layer seeded with one text object, placed so the text's
    /// top-left baseline origin sits at canvas `(x, y)`, filled with `color`
    /// (RGBA, 0–255). Returns the new layer id and the stamped object id. One
    /// undo step.
    pub fn add_text_layer(
        &mut self,
        text: crate::layer::TextProps,
        x: f64,
        y: f64,
        color: [u8; 4],
        anchor: Option<LayerId>,
    ) -> (LayerId, crate::layer::ObjectId) {
        let id = self.doc.add_vector_layer(anchor);
        let fill = peniko::Brush::Solid(peniko::Color::from_rgba8(
            color[0], color[1], color[2], color[3],
        ));
        let obj = crate::layer::VectorObject::text(text, kurbo::Affine::translate((x, y)), fill);
        let object_id = match self.doc.find_node_mut(id) {
            Some(LayerNode::Layer(Layer::Vector(v))) => v.push_object(obj),
            _ => crate::layer::ObjectId::UNASSIGNED,
        };
        self.sync_vector_layer(id);
        self.compositor.mark_dirty();

        let slot = self.doc.slot_of(id).unwrap_or_default();
        self.push_undo(Box::new(EntityAddAction::new(id, slot)));
        (id, object_id)
    }

    /// Add a text object to an *existing* vector layer, placed so its origin
    /// sits at canvas `(x, y)`, filled with `color`. The vector layer owns an
    /// ordered list of objects, so this is the natural "another text box on the
    /// same layer" path. One undo step (the object list before/after, via
    /// [`Property::VectorObjects`] — the same undo kind [`Self::edit_vector_object`]
    /// records). Returns the stamped object id, or `None` for a non-vector id.
    pub fn add_text_object(
        &mut self,
        layer_id: LayerId,
        text: crate::layer::TextProps,
        x: f64,
        y: f64,
        color: [u8; 4],
    ) -> Option<crate::layer::ObjectId> {
        let old = match self.doc.layer(layer_id) {
            Some(Layer::Vector(v)) => v.objects.clone(),
            _ => return None,
        };
        let fill = peniko::Brush::Solid(peniko::Color::from_rgba8(
            color[0], color[1], color[2], color[3],
        ));
        let obj = crate::layer::VectorObject::text(text, kurbo::Affine::translate((x, y)), fill);
        let object_id = match self.doc.find_node_mut(layer_id) {
            Some(LayerNode::Layer(Layer::Vector(v))) => v.push_object(obj),
            _ => return None,
        };
        let new = match self.doc.layer(layer_id) {
            Some(Layer::Vector(v)) => v.objects.clone(),
            _ => return None,
        };
        self.push_undo(Box::new(PropertyAction::new(
            layer_id,
            Property::VectorObjects(old),
            Property::VectorObjects(new),
        )));
        self.sync_vector_layer(layer_id);
        self.compositor.mark_dirty();
        Some(object_id)
    }

    /// Ensure the compositor's GPU state for a vector layer exists and rebuild
    /// its `vello::Scene` from the document's authoritative objects. Idempotent
    /// — safe after any object/style/transform change, on load, and on
    /// undo/redo (`sync_compositor_layers` calls it for every vector layer).
    pub(crate) fn sync_vector_layer(&mut self, id: LayerId) {
        self.compositor
            .ensure_vector_layer(&self.gpu.device, &self.gpu.queue, id);
        let Some(Layer::Vector(v)) = self.doc.layer(id) else {
            return;
        };
        let objects = v.objects.clone();
        let layer_affine = transform_to_kurbo(&v.transform);
        let scene = self.fonts.build_scene(&objects, layer_affine);
        self.compositor.set_vector_scene(id, scene);
    }

    /// Replace the content string of one text object on a vector layer.
    pub fn set_text_content(
        &mut self,
        id: LayerId,
        object: crate::layer::ObjectId,
        content: String,
    ) {
        self.edit_vector_object(id, object, VectorOpKind::Content, |obj| {
            if let crate::layer::ObjectSource::Text(t) = &mut obj.source {
                t.content = content;
            }
        });
    }

    /// Update one or more style fields (and/or fill color) of one text object on
    /// a vector layer. `None` arguments leave that field unchanged. `variations`
    /// and `features` are **merged** into the object's maps — passing one axis
    /// keeps the rest — so the panel can edit a single slider without clobbering
    /// the others. Box/layout is *not* here: it stays owned by
    /// [`Self::set_text_box`] on its own undo lane (there is no second mutation
    /// path for it).
    #[allow(clippy::too_many_arguments)]
    pub fn set_text_style(
        &mut self,
        id: LayerId,
        object: crate::layer::ObjectId,
        font_family: Option<String>,
        size: Option<f32>,
        variations: Option<std::collections::BTreeMap<String, f32>>,
        features: Option<std::collections::BTreeMap<String, u32>>,
        letter_spacing: Option<f32>,
        word_spacing: Option<f32>,
        line_height: Option<f32>,
        italic: Option<bool>,
        align: Option<crate::layer::TextAlign>,
        color: Option<[u8; 4]>,
    ) {
        self.edit_vector_object(id, object, VectorOpKind::Style, |obj| {
            if let crate::layer::ObjectSource::Text(t) = &mut obj.source {
                if let Some(f) = font_family {
                    t.font_family = f;
                }
                if let Some(s) = size {
                    t.size = s;
                }
                if let Some(vars) = variations {
                    t.variations.extend(vars);
                }
                if let Some(feats) = features {
                    t.features.extend(feats);
                }
                if let Some(ls) = letter_spacing {
                    t.letter_spacing = ls;
                }
                if let Some(ws) = word_spacing {
                    t.word_spacing = ws;
                }
                if let Some(lh) = line_height {
                    t.line_height = lh;
                }
                if let Some(i) = italic {
                    t.style = if i {
                        crate::layer::TextStyle::Italic
                    } else {
                        crate::layer::TextStyle::Normal
                    };
                }
                if let Some(a) = align {
                    t.align = a;
                }
            }
            if let Some(c) = color {
                obj.fill = Some(peniko::Brush::Solid(peniko::Color::from_rgba8(
                    c[0], c[1], c[2], c[3],
                )));
            }
        });
    }

    /// Report a font family's capabilities (variable axes + real italic face) so
    /// the UI can render font-driven controls. Thin passthrough to
    /// [`crate::text::FontRegistry::font_axes`].
    pub fn font_axes(&mut self, family: &str) -> crate::text::FontCapabilities {
        self.fonts.font_axes(family)
    }

    /// The single chokepoint for editing one object on a vector layer in place.
    /// Clones the object list, mutates the object matched by `object`, swaps the
    /// list back, and records the whole swap as one undo step — coalesced on
    /// `(object, op)` so a typing run, a style-slider drag, or a gizmo drag each
    /// collapse to one step, but switching object or op kind starts a new one
    /// (see [`crate::undo::PropertyAction::new_coalescing`]). Re-realizes the
    /// layer's vello scene. No-op if the id pair doesn't resolve.
    pub(crate) fn edit_vector_object<F: FnOnce(&mut crate::layer::VectorObject)>(
        &mut self,
        id: LayerId,
        object: crate::layer::ObjectId,
        op: VectorOpKind,
        f: F,
    ) {
        let Some(Layer::Vector(v)) = self.doc.layer(id) else {
            return;
        };
        let old = v.objects.clone();
        let mut new = old.clone();
        let Some(obj) = new.iter_mut().find(|o| o.id == object) else {
            return;
        };
        f(obj);
        if let Some(LayerNode::Layer(Layer::Vector(vm))) = self.doc.find_node_mut(id) {
            vm.objects = new.clone();
        }
        self.coalesce_property_undo(PropertyAction::new_coalescing(
            id,
            Property::VectorObjects(old),
            Property::VectorObjects(new),
            vector_coalesce_tag(object, op),
        ));
        self.sync_vector_layer(id);
        self.compositor.mark_dirty();
    }

    /// List every text object on a vector layer with its content, style, and
    /// fill color (RGBA 0–255) — what the text-properties panel binds one
    /// editor block to per object. Empty for a non-vector layer or one with no
    /// text objects. Carries no geometry: the panel edits off-canvas, so
    /// shaping/bounds are never needed.
    pub fn text_objects(&self, id: LayerId) -> Vec<TextObjectEntry> {
        let Some(Layer::Vector(v)) = self.doc.layer(id) else {
            return Vec::new();
        };
        v.objects
            .iter()
            .filter_map(|obj| match &obj.source {
                crate::layer::ObjectSource::Text(t) => Some(TextObjectEntry {
                    object: obj.id,
                    content: t.content.clone(),
                    font_family: t.font_family.clone(),
                    size: t.size,
                    variations: t.variations.clone(),
                    features: t.features.clone(),
                    letter_spacing: t.letter_spacing,
                    word_spacing: t.word_spacing,
                    line_height: t.line_height,
                    italic: matches!(t.style, crate::layer::TextStyle::Italic),
                    align: t.align,
                    color: brush_rgba(&obj.fill),
                    box_size: t.layout.area_size(),
                }),
                crate::layer::ObjectSource::Path(_) => None,
            })
            .collect()
    }

    /// Read one object's gizmo geometry: its local bbox size plus the full
    /// canvas affine `G = layer_transform * obj.transform`, reordered into the
    /// frontend's row-major `Affine2D`. The gizmo's origin is `(0, 0)` — the
    /// placement is folded into `G`, so `toCanvas(local) = G·local` draws the
    /// box tight around the object. `None` if the id pair doesn't resolve.
    pub fn vector_object_info(
        &mut self,
        id: LayerId,
        object: crate::layer::ObjectId,
    ) -> Option<(f32, f32, f32, f32, crate::transform::Affine2D)> {
        let (obj, layer_affine) = match self.doc.layer(id) {
            Some(Layer::Vector(v)) => {
                (v.object(object).cloned()?, transform_to_kurbo(&v.transform))
            }
            _ => return None,
        };
        let bbox = self.object_bbox(&obj)?;
        let g = layer_affine * obj.transform;
        Some((
            0.0,
            0.0,
            bbox.width() as f32,
            bbox.height() as f32,
            kurbo_to_affine(g),
        ))
    }

    /// Set one object's transform from the gizmo's output. `gizmo_canvas` is the
    /// full canvas affine `G` the gizmo produced, in Darkly's row-major
    /// [`crate::transform::Transform`]; the layer transform is stripped
    /// (`obj.transform = layer_transform⁻¹ · G`) so per-object transform stays
    /// correct once a layer-level vector transform ships. Reads the object's
    /// current transform as the undo `old`, coalesced under
    /// [`VectorOpKind::Transform`] so a whole gizmo drag is one undo step.
    /// Re-realizes the vello scene each call (raster-first; the same per-frame
    /// rebuild `update_void_transform` does).
    pub fn set_vector_object_transform(
        &mut self,
        id: LayerId,
        object: crate::layer::ObjectId,
        gizmo_canvas: crate::transform::Transform,
    ) {
        let layer_affine = match self.doc.layer(id) {
            Some(Layer::Vector(v)) => transform_to_kurbo(&v.transform),
            _ => return,
        };
        if layer_affine.determinant().abs() < 1e-12 {
            return;
        }
        let new_transform = layer_affine.inverse() * transform_to_kurbo(&gizmo_canvas);
        self.edit_vector_object(id, object, VectorOpKind::Transform, |obj| {
            obj.transform = new_transform;
        });
    }

    /// Resize a text object's layout box from the box gizmo's output, setting
    /// the object transform (the box's moved origin, as the full canvas affine
    /// `G`) and the box size atomically. Like
    /// [`Self::set_vector_object_transform`] the layer transform is stripped
    /// (`obj.transform = layer_transform⁻¹ · G`); both fields move under one
    /// [`VectorOpKind::Box`] step so a whole resize drag is one undo. Converts a
    /// point-text object to area text. No-op for a non-text object or a singular
    /// layer transform.
    pub fn set_text_box(
        &mut self,
        id: LayerId,
        object: crate::layer::ObjectId,
        gizmo_canvas: crate::transform::Transform,
        box_size: (f32, f32),
    ) {
        let layer_affine = match self.doc.layer(id) {
            Some(Layer::Vector(v)) => transform_to_kurbo(&v.transform),
            _ => return,
        };
        if layer_affine.determinant().abs() < 1e-12 {
            return;
        }
        let new_transform = layer_affine.inverse() * transform_to_kurbo(&gizmo_canvas);
        self.edit_vector_object(id, object, VectorOpKind::Box, |obj| {
            obj.transform = new_transform;
            if let crate::layer::ObjectSource::Text(t) = &mut obj.source {
                t.layout = crate::layer::TextLayout::Area {
                    width: box_size.0,
                    height: box_size.1,
                };
            }
        });
    }

    #[handler]
    pub fn add_group(&mut self, anchor: Option<LayerId>) -> LayerId {
        let id = self.doc.add_group(anchor);

        let slot = self.doc.slot_of(id).unwrap_or_default();
        self.push_undo(Box::new(EntityAddAction::new(id, slot)));

        id
    }

    /// Create a new group and move every id in `ids` into it, preserving
    /// their relative panel order. The group ends up at the panel-topmost
    /// selected layer's slot — its parent, its position — so the act
    /// "wraps" the selection in place. Cross-parent selections are
    /// supported; sources from other groups get pulled into the new group.
    ///
    /// Locked layers and any id whose ancestor is also in `ids` are
    /// skipped; the new group is created only if at least one editable
    /// source remains. The whole op is one [`CompoundAction`], so a
    /// single undo restores the original tree.
    #[handler]
    pub fn group_layers(&mut self, ids: Vec<LayerId>) -> Result<LayerId, String> {
        if ids.is_empty() {
            return Err("Need at least one layer to group".into());
        }

        let mut editable: Vec<LayerId> = Vec::with_capacity(ids.len());
        for &id in &ids {
            if self.doc.find_node(id).is_none() {
                continue;
            }
            if !self.doc.is_node_editable(id) {
                continue;
            }
            // Drop any id whose ancestor is also in the batch — moving
            // the ancestor brings the descendant along; processing both
            // would yank the descendant out of its group.
            if ids
                .iter()
                .any(|&other| other != id && self.doc.is_ancestor_of(other, id))
            {
                continue;
            }
            editable.push(id);
        }
        if editable.is_empty() {
            return Err("No editable layers to group".into());
        }

        // Sort by panel order so the last entry is the panel-topmost
        // editable source — its slot is where the new group will live.
        let order = self.doc.all_node_ids_in_order();
        let order_idx = |id: LayerId| order.iter().position(|&x| x == id).unwrap_or(usize::MAX);
        editable.sort_by_key(|&id| order_idx(id));
        let topmost = *editable.last().expect("non-empty");
        let topmost_parent = self.doc.parent_of(topmost);
        let topmost_pos = self.doc.position_in_parent(topmost).unwrap_or(0);

        // Create the group at the top of root — a stable spot that
        // can't accidentally land it inside one of the sources (which
        // would happen if we anchored on a Group-typed `topmost`, since
        // `add_group(Some(group))` resolves to `IntoGroupTop(group)`).
        // We'll move the group to the topmost's slot at the end.
        let group_id = self.doc.add_group(None);
        let group_initial_slot = self.doc.slot_of(group_id).unwrap_or_default();

        let mut actions: Vec<Box<dyn UndoAction>> = Vec::with_capacity(editable.len() + 2);
        actions.push(Box::new(EntityAddAction::new(group_id, group_initial_slot)));

        // Move sources into the group, preserving bottom-first ordering
        // (so panel order top-first reads as the original layout). The
        // first source goes to IntoGroupBottom; subsequent sources chain
        // `After(prev)` so they land contiguously in the same relative
        // order they had before.
        let mut prev: Option<LayerId> = None;
        for id in editable {
            let target = match prev {
                None => MoveTarget::IntoGroupBottom(group_id),
                Some(p) => MoveTarget::After(p),
            };
            if let Some(a) = self.move_layer_inner(id, target) {
                actions.push(a);
                prev = Some(id);
            }
        }

        // Reposition the group at the topmost's original slot. The
        // topmost has already been detached (it was the last source
        // moved into the group), so its parent's children Vec is short
        // one entry — `topmost_pos` now points at whatever was just
        // above topmost in panel order. Inserting the group there
        // lands it exactly where topmost used to be.
        let group_pre_move_slot = self.doc.slot_of(group_id).unwrap_or_default();
        self.doc.detach_for_undo(group_id);
        let clamped_pos = topmost_pos.min(match topmost_parent {
            Some(p) => self.doc.children_of(p).len(),
            None => self.doc.children_of(self.doc.root_id()).len(),
        });
        self.doc.reinsert_entity(
            group_id,
            TreeSlot {
                parent: topmost_parent,
                position: clamped_pos,
                screen_space: false,
            },
        );
        let group_final_slot = self.doc.slot_of(group_id).unwrap_or_default();
        if group_pre_move_slot != group_final_slot {
            actions.push(Box::new(LayerMoveAction::new(
                group_id,
                group_pre_move_slot,
                group_final_slot,
            )));
        }

        self.push_undo(Box::new(CompoundAction::new(actions)));
        self.compositor.mark_dirty();
        Ok(group_id)
    }

    /// Add a new void (procedural) layer. `params` is matched against the
    /// void type's `ParamDef` schema by index — callers that don't have a
    /// hand-rolled slice should use the type's defaults via
    /// `void_param_defs(type).iter().map(ParamDef::default_value)`.
    ///
    /// Returns `None` if `void_type` is not a registered void kind. (We
    /// surface this rather than silently fall back, matching how
    /// `set_blend_mode` rejects unknown blend ids.)
    pub fn add_void_layer(
        &mut self,
        void_type: &str,
        params: Vec<crate::gpu::params::ParamValue>,
        anchor: Option<LayerId>,
    ) -> Option<LayerId> {
        self.add_void_layer_with_transform(void_type, params, anchor, None)
    }

    /// [`Self::add_void_layer`] with an explicit initial transform, overriding
    /// the kind's registered seed. Placement uses it: a smart object's opening
    /// transform is derived from the source image's dimensions, which the
    /// registration — a plain `fn(u32, u32)` over the canvas size — cannot see.
    pub fn add_void_layer_with_transform(
        &mut self,
        void_type: &str,
        params: Vec<crate::gpu::params::ParamValue>,
        anchor: Option<LayerId>,
        transform_override: Option<crate::transform::Transform>,
    ) -> Option<LayerId> {
        if !self.compositor.void_registry().has(void_type) {
            return None;
        }
        // Default-name the layer after the void's display label so the
        // panel reads "Noise 1" / "Noise 2" rather than a generic "Void N".
        let display_label = self.compositor.void_registry().display_name(void_type);
        // Seed the kind's initial gizmo transform (camera = selfie flip,
        // everything else = identity) atomically with creation, so it's one
        // undo step and round-trips through save/load like any later edit.
        let canvas = self.doc.canvas_rect();
        let initial_transform = transform_override.unwrap_or_else(|| {
            self.compositor.void_registry().default_transform(
                void_type,
                canvas.width,
                canvas.height,
            )
        });
        let id = self.doc.add_void_layer(
            void_type.to_string(),
            display_label,
            params.clone(),
            initial_transform,
            anchor,
        );
        // Build the trait object here (engine), then hand it to the
        // compositor — the compositor stops caring about `(type_id,
        // params)` as a pair, owning only the constructed `Box<dyn Void>`.
        let format = self.compositor.canvas_content_format();
        let void = self.compositor.void_registry_mut().create_void(
            void_type,
            &params,
            &self.gpu.device,
            format,
        );
        self.compositor
            .ensure_void_layer(&self.gpu.device, &self.gpu.queue, id, void);
        // A fresh void instance starts at identity, so push the seeded
        // transform down now rather than waiting for the next frame's
        // doc→compositor sync. Otherwise the layer renders untransformed for
        // one frame — and never gets corrected at all on paths that composite
        // without going through `render()`.
        self.compositor
            .update_void_layer_transform(&self.gpu.queue, id, &initial_transform);
        self.compositor.mark_dirty();

        let slot = self.doc.slot_of(id).unwrap_or_default();
        self.push_undo(Box::new(EntityAddAction::new(id, slot)));

        Some(id)
    }

    /// Add a new filter layer — a non-destructive transform of the composite
    /// below it. `pipeline` names a registered filter type (e.g. `"invert"`);
    /// `params` is matched against that type's schema by index (empty for
    /// parameter-free filters).
    ///
    /// Returns `None` if `pipeline` is not a registered filter type — surfaced
    /// rather than silently falling back, the same as [`Self::add_void_layer`].
    /// Unlike a void layer there is no per-instance GPU resource to build: the
    /// filter pipeline is shared and resolved lazily in `compose_filter_arm`.
    pub fn add_filter_layer(
        &mut self,
        pipeline: &str,
        params: Vec<crate::gpu::params::ParamValue>,
        anchor: Option<LayerId>,
    ) -> Option<LayerId> {
        if !self.compositor.effect_registry().has(pipeline) {
            return None;
        }
        let display_label = self.compositor.effect_registry().display_name(pipeline);
        let id = self
            .doc
            .add_filter_layer(pipeline.to_string(), display_label, params, anchor);
        self.compositor.mark_dirty();

        let slot = self.doc.slot_of(id).unwrap_or_default();
        self.push_undo(Box::new(EntityAddAction::new(id, slot)));

        Some(id)
    }

    /// Replace a void layer's parameter values. Coalesces with prior
    /// `VoidParams` edits on the same layer so a slider drag is one undo
    /// step, mirroring how `set_opacity` already behaves.
    pub fn update_void_params(
        &mut self,
        layer_id: LayerId,
        new_params: Vec<crate::gpu::params::ParamValue>,
    ) {
        if !self.doc.is_node_editable(layer_id) {
            return;
        }
        let old_params = match self.doc.find_node(layer_id) {
            Some(LayerNode::Layer(Layer::Void(v))) => v.params.clone(),
            _ => return,
        };
        if let Some(LayerNode::Layer(Layer::Void(v))) = self.doc.find_node_mut(layer_id) {
            v.params = new_params.clone();
        }
        self.compositor
            .update_void_layer_params(&self.gpu.queue, layer_id, &new_params);
        self.compositor.mark_dirty();

        self.coalesce_property_undo(PropertyAction::new(
            layer_id,
            Property::VoidParams(old_params),
            Property::VoidParams(new_params),
        ));
    }

    /// Resolve a layer id to its filter `pipeline` id, if the layer is a filter.
    /// Lets the protocol handler fetch the filter's param schema without
    /// importing the layer enum.
    pub fn filter_layer_pipeline(&self, layer_id: LayerId) -> Option<String> {
        match self.doc.find_node(layer_id) {
            Some(LayerNode::Layer(Layer::Filter(f))) => Some(f.pipeline.clone()),
            _ => None,
        }
    }

    /// Parameter schema for a filter type (empty for parameter-free filters).
    /// Backs the protocol handler's JSON→`ParamValue` conversion.
    pub fn filter_param_defs(&self, type_id: &str) -> &'static [crate::gpu::params::ParamDef] {
        self.compositor.effect_registry().params(type_id)
    }

    /// Replace a filter layer's parameter values. Coalesces with prior
    /// `FilterParams` edits on the same layer so a curve drag is one undo step —
    /// the exact analog of [`Self::update_void_params`]. The compositor rebuilds
    /// any param-derived GPU resources (the curves LUT) lazily on the next
    /// `sync_projection_states`, keyed by the param fingerprint.
    pub fn update_filter_params(
        &mut self,
        layer_id: LayerId,
        new_params: Vec<crate::gpu::params::ParamValue>,
    ) {
        if !self.doc.is_node_editable(layer_id) {
            return;
        }
        let old_params = match self.doc.find_node(layer_id) {
            Some(LayerNode::Layer(Layer::Filter(f))) => f.params.clone(),
            _ => return,
        };
        if let Some(LayerNode::Layer(Layer::Filter(f))) = self.doc.find_node_mut(layer_id) {
            f.params = new_params.clone();
        }
        self.compositor.mark_dirty();

        self.coalesce_property_undo(PropertyAction::new(
            layer_id,
            Property::FilterParams(old_params),
            Property::FilterParams(new_params),
        ));
    }

    /// Set a void layer's user transform (the void *consuming* the generic
    /// transform gizmo's output). Mirrors [`Self::update_void_params`]:
    /// reads the layer's CURRENT transform as the undo `old_value` before
    /// writing, so a whole gizmo drag coalesces into one undo step that
    /// restores the true pre-drag state — never identity.
    #[handler]
    pub fn update_void_transform(&mut self, id: LayerId, transform: crate::transform::Transform) {
        if !self.doc.is_node_editable(id) {
            return;
        }
        let old_transform = match self.doc.find_node(id) {
            Some(LayerNode::Layer(Layer::Void(v))) => v.transform,
            _ => return,
        };
        if let Some(LayerNode::Layer(Layer::Void(v))) = self.doc.find_node_mut(id) {
            v.transform = transform;
        }
        self.compositor
            .update_void_layer_transform(&self.gpu.queue, id, &transform);
        self.compositor.mark_dirty();

        self.coalesce_property_undo(PropertyAction::new(
            id,
            Property::Transform(old_transform),
            Property::Transform(transform),
        ));
    }

    /// Read a void layer's current transform + the gizmo bbox to draw around
    /// its active pixels. Returns `(origin_x, origin_y, w, h, transform)` in
    /// PLANE space. The bbox is the void's
    /// [`crate::gpu::void::Void::content_extent`], which already answers in
    /// plane space — canvas-filling for most voids, the cover-fit rect for a
    /// stream (extending beyond the canvas), the source's natural rect for a
    /// placed image. Falls back to the canvas rect if the void instance isn't
    /// realized yet. `None` if `layer_id` isn't a void.
    pub fn void_transform_info(
        &self,
        layer_id: LayerId,
    ) -> Option<(f32, f32, f32, f32, crate::transform::Transform)> {
        let transform = match self.doc.find_node(layer_id) {
            Some(LayerNode::Layer(Layer::Void(v))) => v.transform,
            _ => return None,
        };
        let canvas = self.doc.canvas_rect();
        let content = self
            .compositor
            .void_content_extent(layer_id)
            .unwrap_or_else(|| crate::gpu::void::ContentRect::covering(canvas));
        Some((
            content.x,
            content.y,
            content.width,
            content.height,
            transform,
        ))
    }

    /// How the user may transform a layer — `live` / `destructive` / `none`.
    /// Resolves the void's static capability through the compositor-owned
    /// registry. Returned as a stable string for the WASM boundary.
    #[handler]
    pub fn layer_transform_capability(&self, id: LayerId) -> &'static str {
        use crate::layer::TransformCapability;
        let cap = match self.doc.find_node(id) {
            Some(LayerNode::Layer(l))
                if l.transform_capability(self.compositor.void_registry())
                    == TransformCapability::Live =>
            {
                TransformCapability::Live
            }
            _ if self.plan_pixel_transform(id).is_ok() => TransformCapability::Destructive,
            _ => TransformCapability::None,
        };
        match cap {
            TransformCapability::Live => "live",
            TransformCapability::Destructive => "destructive",
            TransformCapability::None => "none",
        }
    }

    /// Hand a fresh external image frame to a void's input texture.
    /// Wraps [`crate::gpu::compositor::Compositor::upload_void_external_image`];
    /// no-ops if `layer_id` isn't a void or the void doesn't consume external
    /// input. Frames flow through here every animation frame from the JS
    /// render loop for camera / future screenshare voids.
    ///
    /// Also syncs the doc-side [`crate::layer::VoidLayer::frame`] when the
    /// void declares a new persistent frame size. The save flow reads that
    /// field to decide whether to emit a pixel blob for the void, so
    /// keeping it current here is what makes the last frame round-trip
    /// through `.darkly`.
    pub fn upload_void_external_image(
        &mut self,
        layer_id: LayerId,
        source: crate::gpu::void::ExternalImageSource,
    ) {
        // Visibility gate: a hidden layer (or any hidden ancestor) means the
        // composited output ignores this layer entirely, so the canvas blit
        // upstream of us plus this GPU copy plus the void's encode pass plus
        // the compositor recomposite would all be pure waste. The
        // authoritative answer lives in the doc — `effective_visible` walks
        // ancestors. The JS-side `CameraSource.tick()` also short-circuits
        // on visibility, but this guard is the canonical correctness one:
        // any future caller (tests, a different frontend, IPC) gets the
        // same behaviour without needing to remember the JS optimization.
        if !self.doc.effective_visible(layer_id) {
            return;
        }
        self.compositor.upload_void_external_image(
            &self.gpu.device,
            &self.gpu.queue,
            layer_id,
            source,
        );
        self.sync_void_persistent_frame(layer_id);
    }

    /// Pull the void's current `persistent_frame_size` from the compositor
    /// and mirror it onto [`crate::layer::VoidLayer::frame`]. Cheap when
    /// nothing changed (compares before writing). Called after every
    /// external-image upload and once at document open after a successful
    /// `restore_void_pixels` so saves and reloads stay consistent.
    /// Place `rgba` as a smart object — a layer that holds the image at its
    /// native resolution and displays it through a stored transform, so
    /// resizing it is a change to that transform rather than a resample of the
    /// pixels. Returns the new layer's id, or `None` if the dimensions are
    /// degenerate or don't match the buffer.
    ///
    /// `rgba` is **straight-alpha** RGBA8, as it arrives from an image decode.
    /// It is premultiplied here, at the boundary, because that is the
    /// convention every sampled source stores.
    ///
    /// The whole placement is one undo step: the layer, its transform and its
    /// pixels all land together, so there is no intermediate state in which an
    /// empty smart object exists.
    ///
    /// Not a `#[handler]`: the pixels ride the protocol's binary side-channel
    /// rather than the JSON payload, so the request is registered by hand in
    /// `protocol::handlers::image_io` alongside the paste requests.
    pub fn place_smart_object(
        &mut self,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
        anchor: Option<LayerId>,
    ) -> Option<LayerId> {
        if width == 0 || height == 0 {
            return None;
        }
        let expected = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        if rgba.len() != expected {
            return None;
        }

        let mut rgba = rgba;
        crate::gpu::premultiply_rgba8_in_place(&mut rgba);

        let transform = Self::initial_placement_transform(self.doc.canvas_rect(), width, height);
        let id = self.add_void_layer_with_transform(
            crate::gpu::voids::smart_object::TYPE_ID,
            Vec::new(),
            anchor,
            Some(transform),
        )?;

        self.compositor.set_void_source_pixels(
            &self.gpu.device,
            &self.gpu.queue,
            id,
            width,
            height,
            &rgba,
        );
        self.sync_void_persistent_frame(id);
        self.compositor.mark_dirty();
        Some(id)
    }

    /// Opening transform for a placed image: centred in the canvas, scaled
    /// down to fit if it is larger than the canvas, never scaled up.
    ///
    /// A smart object's content rect is its source's natural size at the plane
    /// origin, so without this a 6000px photo dropped on a 1000px canvas would
    /// land almost entirely off-screen with no visible handle to grab. Matches
    /// what every editor does on Place.
    fn initial_placement_transform(
        canvas: crate::coord::CanvasRect,
        width: u32,
        height: u32,
    ) -> crate::transform::Transform {
        let (cw, ch) = (canvas.width as f32, canvas.height as f32);
        let (sw, sh) = (width as f32, height as f32);
        let scale = (cw / sw).min(ch / sh).min(1.0);
        // Translate in plane terms: the content rect starts at the plane
        // origin, so centring it in the window means offsetting by the window's
        // own origin plus the leftover margin.
        let tx = canvas.origin.x as f32 + (cw - sw * scale) * 0.5;
        let ty = canvas.origin.y as f32 + (ch - sh * scale) * 0.5;
        crate::transform::Transform::from_affine([scale, 0.0, tx, 0.0, scale, ty])
    }

    pub(crate) fn sync_void_persistent_frame(&mut self, layer_id: LayerId) {
        let Some((w, h)) = self.compositor.void_persistent_frame_size(layer_id) else {
            return;
        };
        let blob_key = format!("layers/{}.pixels", layer_id.to_ffi());
        let next = crate::format::manifest::ManifestPixelRef {
            format: crate::format::manifest::texture_format_to_str(wgpu::TextureFormat::Rgba8Unorm)
                .to_string(),
            pixels: blob_key,
            bounds: crate::coord::CanvasRect::from_xywh(0, 0, w, h),
        };
        if let Some(crate::layer::LayerNode::Layer(crate::layer::Layer::Void(v))) =
            self.doc.find_node_mut(layer_id)
        {
            if v.frame.as_ref() != Some(&next) {
                v.frame = Some(next);
                self.doc.dirty = true;
            }
        }
    }

    pub fn has_layer(&self, layer_id: LayerId) -> bool {
        // "Has" means linked into the tree — not just sitting orphaned in the
        // document's slotmap waiting on an undo reattach. Detached-for-undo
        // layers must report `false` so callers (and the layer panel) treat
        // them as gone until reattach.
        self.doc.layer(layer_id).is_some() && self.doc.parent_of(layer_id).is_some()
    }

    /// Returns the layer's pixel-space bounds in canvas coordinates.
    /// Used by tests and the WASM bridge to report storage extent.
    pub fn layer_bounds(&self, layer_id: LayerId) -> Option<crate::coord::CanvasRect> {
        match self.doc.layer(layer_id)? {
            Layer::Raster(r) => Some(r.pixels.bounds),
            // Voids, filter, and vector layers store no pixels — their "bounds"
            // concept is the canvas itself, which callers can ask for directly
            // via `canvas_dimensions`.
            Layer::Void(_) | Layer::Filter(_) | Layer::Vector(_) => None,
        }
    }

    /// Returns the pixel-space bounds of any pixel-bearing node id (raster
    /// layer or mask filter). Generalization of [`Self::layer_bounds`] —
    /// when callers hold a node id without knowing its kind, this resolves
    /// against the document's unified `pixels()` accessor. Returns `None`
    /// for groups (no pixel buffer) or unknown ids.
    pub fn node_pixel_bounds(&self, node_id: LayerId) -> Option<crate::coord::CanvasRect> {
        if let Some(rect) = self.layer_bounds(node_id) {
            return Some(rect);
        }
        self.doc
            .find_filter(node_id)
            .and_then(|m| m.pixels())
            .map(|p| p.bounds)
    }

    #[handler]
    pub fn remove_layer(&mut self, id: LayerId) -> Result<(), String> {
        if !self.resolve_transform_conflict() {
            return Err("Active transform could not be committed".into());
        }
        if !self.doc.is_node_editable(id) {
            return Err("Layer is locked".into());
        }
        // A modifier is a selectable row, so the delete hotkey forwards its id
        // here — but it hangs off a host rather than occupying a slot in the
        // tree, so it never counts toward the last-layer floor.
        if !self.doc.is_filter(id) && self.doc.node_count() <= 1 {
            return Err("Cannot delete the last layer".into());
        }

        if let Some(action) = self.detach_for_remove(id) {
            self.push_undo(action);
        }
        self.compositor.mark_dirty();
        Ok(())
    }

    /// Detach a single entity for removal and return the matching undo action
    /// without pushing it. Returns `None` if `id` isn't attached. The caller is
    /// responsible for any editability or "last layer" checks; this is the raw
    /// mutation half shared between [`Self::remove_layer`] and
    /// [`Self::remove_layers`].
    ///
    /// Dispatches on the entity's own kind: modifiers go to the modifier path,
    /// which owns the GPU bookkeeping their pixels need, and everything else is
    /// a tree node. Callers pass an id and don't ask what it is.
    pub(crate) fn detach_for_remove(&mut self, id: LayerId) -> Option<Box<dyn UndoAction>> {
        // Session state must not outlive the entity it points at: an isolation
        // target detached from the tree is unreachable from the root walk, so
        // every node would test as off-path and the canvas would go blank.
        if self.isolated_node == Some(id) {
            self.isolated_node = None;
        }
        if self.doc.is_filter(id) {
            return self.detach_modifier_for_remove(id);
        }
        let slot = self.doc.slot_of(id).unwrap_or_default();
        // Collect tombstones before detaching — `detach_for_undo` severs
        // the parent links `collect_pixel_node_ids` walks to enumerate
        // the subtree.
        let tombstones = self.collect_pixel_node_ids(id);
        self.doc.detach_for_undo(id)?;
        Some(Box::new(EntityRemoveAction::new(id, slot, tombstones)))
    }

    /// Remove every id in `ids` in a single undo step. Locked layers and
    /// any id whose ancestor is also in `ids` are silently skipped; the
    /// return value is the count of locked layers that were ignored so the
    /// UI can surface a "N locked layers skipped" toast. Errors only when
    /// removing the editable set would leave zero layers in the document.
    #[handler]
    pub fn remove_layers(&mut self, ids: Vec<LayerId>) -> Result<usize, String> {
        if !self.resolve_transform_conflict() {
            return Err("Active transform could not be committed".into());
        }
        let mut editable = Vec::with_capacity(ids.len());
        let mut skipped_locked = 0usize;
        let mut node_removals = 0usize;
        for &id in &ids {
            // Modifiers are selectable rows and so can arrive in a batch; they
            // resolve through their host rather than the tree.
            let is_modifier = self.doc.is_filter(id);
            if !is_modifier && self.doc.find_node(id).is_none() {
                continue;
            }
            if !self.doc.is_node_editable(id) {
                skipped_locked += 1;
                continue;
            }
            // Drop any id that's a descendant of another id already in
            // the batch — removing the ancestor takes the subtree with it.
            // A modifier whose host is also in the batch is covered the same
            // way: the host's removal takes its filters along.
            if ids
                .iter()
                .any(|&other| other != id && self.doc.is_ancestor_of(other, id))
            {
                continue;
            }
            if !is_modifier {
                node_removals += 1;
            }
            editable.push(id);
        }
        if editable.is_empty() {
            self.compositor.mark_dirty();
            return Ok(skipped_locked);
        }
        // Only tree nodes count against the floor — removing every modifier in
        // the document still leaves its layers behind.
        if node_removals > 0 && self.doc.node_count().saturating_sub(node_removals) == 0 {
            return Err("Cannot delete the last layer".into());
        }

        self.batched_undo(&editable, |engine, id| engine.detach_for_remove(id));
        self.compositor.mark_dirty();
        Ok(skipped_locked)
    }

    #[handler]
    pub fn move_layer(&mut self, id: LayerId, target: MoveTarget) {
        if !self.resolve_transform_conflict() {
            return;
        }
        if let Some(action) = self.move_layer_inner(id, target) {
            self.push_undo(action);
        }
        self.compositor.mark_dirty();
    }

    /// Move a single layer and return the matching undo action without
    /// pushing it. Returns `None` if `layer_id` is locked or not in the
    /// tree. Shared between [`Self::move_layer`] and
    /// [`Self::move_layers`].
    fn move_layer_inner(
        &mut self,
        layer_id: LayerId,
        target: MoveTarget,
    ) -> Option<Box<dyn UndoAction>> {
        if !self.doc.is_node_editable(layer_id) {
            return None;
        }
        let old = self.doc.slot_of(layer_id)?;
        self.doc.move_layer(layer_id, target);
        let new = self.doc.slot_of(layer_id).unwrap_or_default();
        Some(Box::new(LayerMoveAction::new(layer_id, old, new)))
    }

    /// Move every id in `ids` to land contiguously at `target`, preserving
    /// their current relative tree order. Locked layers and any id whose
    /// ancestor is also in `ids` are silently skipped; returns the count
    /// of locked-layer skips so the UI can toast. Errors when `target`
    /// references an id in `ids` or a descendant of one (the drop is
    /// self-referential).
    #[handler]
    pub fn move_layers(&mut self, ids: Vec<LayerId>, target: MoveTarget) -> Result<usize, String> {
        if !self.resolve_transform_conflict() {
            return Err("Active transform could not be committed".into());
        }
        let target_id = target.reference();
        for &id in &ids {
            if id == target_id || self.doc.is_ancestor_of(id, target_id) {
                return Err("Cannot move a layer into itself".into());
            }
        }

        let mut editable: Vec<LayerId> = Vec::with_capacity(ids.len());
        let mut skipped_locked = 0usize;
        for &id in &ids {
            if self.doc.find_node(id).is_none() {
                continue;
            }
            if !self.doc.is_node_editable(id) {
                skipped_locked += 1;
                continue;
            }
            if ids
                .iter()
                .any(|&other| other != id && self.doc.is_ancestor_of(other, id))
            {
                continue;
            }
            editable.push(id);
        }
        if editable.is_empty() {
            return Ok(skipped_locked);
        }

        // Sort by document DFS order so subsequent `After(prev)` chaining
        // preserves the user's original top-to-bottom layout at the
        // destination.
        let order = self.doc.all_node_ids_in_order();
        let order_idx = |id: LayerId| order.iter().position(|&x| x == id).unwrap_or(usize::MAX);
        editable.sort_by_key(|&id| order_idx(id));

        let mut actions: Vec<Box<dyn UndoAction>> = Vec::with_capacity(editable.len());
        let mut prev: Option<LayerId> = None;
        for id in editable {
            let step_target = match prev {
                None => target,
                Some(p) => MoveTarget::After(p),
            };
            if let Some(a) = self.move_layer_inner(id, step_target) {
                actions.push(a);
                prev = Some(id);
            }
        }
        if !actions.is_empty() {
            self.push_undo(Box::new(CompoundAction::new(actions)));
        }
        self.compositor.mark_dirty();
        Ok(skipped_locked)
    }

    // --- Layer properties ---

    #[handler]
    pub fn set_opacity(&mut self, id: LayerId, opacity: f32) {
        if !self.doc.is_node_editable(id) {
            return;
        }
        let old_opacity = match self.doc.find_node(id) {
            Some(n) => n.blend().opacity,
            None => return,
        };
        if let Some(node) = self.doc.find_node_mut(id) {
            node.blend_mut().opacity = opacity;
        } else {
            return;
        }

        self.refresh_blend_uniforms(id);
        self.compositor.mark_dirty();

        self.coalesce_property_undo(PropertyAction::new(
            id,
            Property::Opacity(old_opacity),
            Property::Opacity(opacity),
        ));
    }

    #[handler]
    pub fn set_blend_mode(&mut self, id: LayerId, type_id: &str) {
        if !self.doc.is_node_editable(id) {
            return;
        }
        // Unknown blend-mode strings keep the existing mode rather than
        // silently snapping to Normal — the UI should only ever pass a
        // registered id, so an unknown one is a bug worth surfacing.
        let blend_mode = match crate::gpu::blend_mode::registry().get(type_id) {
            Some(reg) => reg,
            None => return,
        };
        let old_mode = match self.doc.find_node(id) {
            Some(n) => n.blend().blend_mode,
            None => return,
        };
        // Picking a blend mode on a passthrough group implicitly switches it
        // to isolated — passthrough ignores the group's blend mode, so the
        // user's choice would have no visible effect otherwise.
        let was_passthrough = matches!(
            self.doc.find_node(id),
            Some(LayerNode::Group(g)) if g.passthrough,
        );
        if let Some(node) = self.doc.find_node_mut(id) {
            node.blend_mut().blend_mode = blend_mode;
            if was_passthrough {
                if let LayerNode::Group(g) = node {
                    g.passthrough = false;
                }
            }
        } else {
            return;
        }

        if was_passthrough {
            self.compositor
                .ensure_group_state(&self.gpu.device, &self.gpu.queue, id);
        }
        self.refresh_blend_uniforms(id);
        self.compositor.mark_dirty();

        let blend_action: Box<dyn UndoAction> = Box::new(PropertyAction::new(
            id,
            Property::BlendMode(old_mode),
            Property::BlendMode(blend_mode),
        ));
        if was_passthrough {
            let passthrough_action: Box<dyn UndoAction> = Box::new(PropertyAction::new(
                id,
                Property::Passthrough(true),
                Property::Passthrough(false),
            ));
            self.push_undo(Box::new(CompoundAction::new(vec![
                blend_action,
                passthrough_action,
            ])));
        } else {
            self.push_undo(blend_action);
        }
    }

    /// Set the `visible` flag on any node — layer, group, or filter.
    /// Works uniformly across kinds because they all carry [`NodeCommon`].
    #[handler]
    pub fn set_layer_visible(&mut self, id: LayerId, visible: bool) {
        // Try layers/groups first; fall through to filters.
        let old_visible = if let Some(node) = self.doc.find_node_mut(id) {
            let old = node.common().visible;
            node.common_mut().visible = visible;
            Some(old)
        } else if let Some(filter) = self.doc.find_filter_mut(id) {
            let old = filter.common.visible;
            filter.common.visible = visible;
            Some(old)
        } else {
            None
        };
        if let Some(old) = old_visible {
            self.compositor.mark_dirty();
            self.push_undo(Box::new(crate::undo::NodeVisibleAction::new(id, old)));
        }
    }

    /// Set the `locked` flag on any node — layer, group, or filter.
    #[handler]
    pub fn set_node_locked(&mut self, id: LayerId, locked: bool) {
        if !self.resolve_transform_conflict() {
            return;
        }
        let old_locked = if let Some(node) = self.doc.find_node_mut(id) {
            let old = node.common().locked;
            node.common_mut().locked = locked;
            Some(old)
        } else if let Some(filter) = self.doc.find_filter_mut(id) {
            let old = filter.common.locked;
            filter.common.locked = locked;
            Some(old)
        } else {
            None
        };
        if let Some(old) = old_locked {
            self.push_undo(Box::new(crate::undo::NodeLockedAction::new(id, old)));
        }
    }

    /// Set the session-level "isolate this node" flag.
    ///
    /// When `Some(id)`, the renderer treats `id`'s subtree as the only
    /// thing on the canvas: the compose walk skips off-path siblings and,
    /// when `id` is a mask filter, the host's blend pass renders the
    /// mask channel as grayscale.
    ///
    /// Pure session state — no document mutation. The eye-icon column on
    /// every layer is independent: toggling visibility while isolated
    /// modifies that layer's `visible` field, and clearing isolation
    /// preserves whatever the user set.
    #[handler]
    pub fn set_isolated_node(&mut self, id: Option<LayerId>) -> Option<LayerId> {
        if self.isolated_node == id {
            return self.isolated_node;
        }
        if !self.resolve_transform_conflict() {
            return self.isolated_node;
        }
        self.isolated_node = id;
        // Mirror to the compositor so the render walk can filter off-path
        // subtrees, then resync host uniforms — the `isolated` flag on a
        // host flips depending on whether one of its filters is the new
        // target.
        self.compositor.set_isolated_node(id);
        self.sync_compositor_layers();
        self.compositor.mark_dirty();
        self.isolated_node
    }

    /// Read the current isolated-node id, if any.
    pub fn isolated_node(&self) -> Option<LayerId> {
        self.isolated_node
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn test_compositor_isolated_node(&self) -> Option<LayerId> {
        self.compositor.test_isolated_node()
    }

    /// True when the host's `isolated` blend uniform should fire — i.e. the
    /// current isolation target is one of `host_id`'s filters (the user
    /// asked to see the mask channel as grayscale on canvas). Isolating the
    /// host itself doesn't trigger this; the host renders normally and the
    /// compose walk hides its siblings instead.
    pub(crate) fn host_renders_isolated(&self, host_id: LayerId) -> bool {
        match self.isolated_node {
            Some(t) => self.doc.filters_of(host_id).contains(&t),
            None => false,
        }
    }

    /// User-visible document name. Backs the tab title and the Save As
    /// picker's `suggestedName`. Persisted on disk as `manifest.name`.
    #[handler]
    pub fn document_name(&self) -> &str {
        &self.doc.name
    }

    /// Current document canvas dimensions in pixels. Read by the WASM
    /// bridge so the JS coord transforms can mirror the actual per-doc
    /// size (rather than the global `canvas.width` config default, which
    /// only seeds new docs).
    pub fn canvas_dimensions(&self) -> (u32, u32) {
        (self.doc.width, self.doc.height)
    }

    /// True when the document has unsaved changes. Set sticky at the
    /// [`crate::undo::UndoStack::push`] chokepoint; cleared on a
    /// successful save (`poll_save_result`) or load (`open_document`
    /// installs a fresh `dirty = false` doc). UI close-tab and
    /// `beforeunload` flows consult this to decide whether to prompt.
    #[handler]
    pub fn is_dirty(&self) -> bool {
        self.doc.dirty
    }

    /// Snapshot the engine state the frontend mirrors. Cheap CPU reads; `render`
    /// returns this each frame so synchronous UI consumers read a local mirror
    /// instead of awaiting per-value queries. See [`crate::engine::EngineState`].
    /// Call *after* `render` so `frame_count` is the post-increment value.
    pub fn engine_state(&self) -> crate::engine::EngineState {
        crate::engine::EngineState {
            frame_count: self.frame_count() as f64,
            thumbnail_version: self.thumbnail_version(),
            dirty: self.is_dirty(),
            has_selection: self.has_selection(),
        }
    }

    /// Force the document into the unsaved state. Used after restoring a
    /// crash-recovery snapshot: the restored document is unsaved work
    /// with no backing file handle, so it must read as dirty (otherwise
    /// closing the tab would silently discard the recovered work).
    #[handler]
    pub fn mark_dirty(&mut self) {
        self.doc.dirty = true;
    }

    /// Rename the document. Not undoable — renaming is a metadata change
    /// users expect to be free-standing, matching every other editor's
    /// "title bar rename" affordance. The save flow picks the new name
    /// up from `doc.name` the next time `start_save_document` runs.
    #[handler]
    pub fn set_document_name(&mut self, name: String) {
        self.doc.name = name;
    }

    #[handler]
    pub fn set_layer_name(&mut self, id: LayerId, name: &str) {
        if !self.doc.is_node_editable(id) {
            return;
        }
        let old_name = match self.doc.find_node(id) {
            Some(n) => n.common().name.clone(),
            None => return,
        };
        if let Some(node) = self.doc.find_node_mut(id) {
            node.common_mut().name = name.to_string();
        } else {
            return;
        }

        self.push_undo(Box::new(PropertyAction::new(
            id,
            Property::Name(old_name),
            Property::Name(name.to_string()),
        )));
    }

    /// Push the current opacity/blend_mode of a layer or group into the
    /// compositor's uniform buffer for that node. Group isolation is driven
    /// by `engine.isolated_node` and reflected uniformly across node kinds.
    pub(crate) fn refresh_blend_uniforms(&mut self, layer_id: LayerId) {
        let Some((opacity, blend_mode_gpu)) = self
            .doc
            .find_node(layer_id)
            .map(|n| (n.blend().opacity, n.blend().blend_mode.gpu_value))
        else {
            return;
        };
        let isolated = self.host_renders_isolated(layer_id);
        self.write_blend_uniforms(layer_id, opacity, blend_mode_gpu, isolated);
    }

    /// Write arbitrary blend uniforms for a node, routing to the group or the
    /// layer pool as the node's kind requires. The one place that dispatch
    /// lives: [`Self::refresh_blend_uniforms`] uses it to push the document's
    /// values, and a bake uses it to neutralize a node's own blend so the
    /// baked result doesn't apply it twice.
    pub(crate) fn write_blend_uniforms(
        &mut self,
        layer_id: LayerId,
        opacity: f32,
        blend_mode_gpu: u32,
        isolated: bool,
    ) {
        match self.doc.find_node(layer_id) {
            Some(LayerNode::Layer(_)) => self.compositor.update_layer_uniforms(
                &self.gpu.queue,
                layer_id,
                opacity,
                blend_mode_gpu,
                isolated,
            ),
            Some(LayerNode::Group(_)) => self.compositor.update_group_uniforms(
                &self.gpu.queue,
                layer_id,
                opacity,
                blend_mode_gpu,
                isolated,
            ),
            None => {}
        }
    }

    #[handler]
    pub fn set_group_collapsed(&mut self, id: LayerId, collapsed: bool) {
        if let Some(LayerNode::Group(g)) = self.doc.find_node_mut(id) {
            g.collapsed = collapsed;
        }
    }

    #[handler]
    pub fn set_group_passthrough(&mut self, id: LayerId, passthrough: bool) {
        if !self.doc.is_node_editable(id) {
            return;
        }
        let old = match self.doc.find_node(id) {
            Some(LayerNode::Group(g)) => g.passthrough,
            _ => return,
        };
        if let Some(LayerNode::Group(g)) = self.doc.find_node_mut(id) {
            g.passthrough = passthrough;
        }
        if !passthrough {
            self.compositor
                .ensure_group_state(&self.gpu.device, &self.gpu.queue, id);
            let isolated = self.host_renders_isolated(id);
            if let Some(LayerNode::Group(g)) = self.doc.find_node(id) {
                self.compositor.update_group_uniforms(
                    &self.gpu.queue,
                    id,
                    g.blend.opacity,
                    g.blend.blend_mode.gpu_value,
                    isolated,
                );
            }
        }
        self.compositor.mark_dirty();
        self.push_undo(Box::new(PropertyAction::new(
            id,
            Property::Passthrough(old),
            Property::Passthrough(passthrough),
        )));
    }

    /// The root's children, top-first, with the viewport divider's position.
    #[handler]
    pub fn layer_tree(&self) -> LayerTree {
        LayerTree {
            layers: self
                .doc
                .children_of(self.doc.root_id())
                .iter()
                .rev()
                .filter_map(|id| {
                    node_to_layer_info(
                        &self.doc,
                        self.compositor.void_registry(),
                        self.compositor.effect_registry(),
                        *id,
                    )
                })
                .collect(),
            screen_space_count: self.doc.screen_space_run().len(),
        }
    }

    /// Move the viewport divider: `count` is how many of the root's topmost
    /// children become viewport-only.
    ///
    /// The request is clamped to what the tree can actually support, so a drag
    /// that overshoots a raster stops at it rather than being rejected — the
    /// panel clamps for responsiveness, this clamps for correctness, and they
    /// agree because both ask the document the same question.
    #[handler]
    pub fn set_screen_space_boundary(&mut self, count: usize) {
        let clamped = self.doc.clamp_screen_space_count(count);
        let old = self.doc.screen_space_count;
        if clamped == old {
            return;
        }
        self.doc.screen_space_count = clamped;
        self.push_undo(Box::new(ScreenSpaceBoundaryAction::new(old, clamped)));
        // Members leaving the run rejoin the canvas composite and members
        // entering it stop being part of the image, so both sides are stale.
        self.compositor.mark_dirty();
        self.compositor.mark_needs_present();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coord::CanvasPoint;
    use crate::gpu::context::GpuContext;
    use crate::gpu::test_utils::test_device;
    use crate::layer::{TextProps, VectorObject};

    fn test_engine() -> DarklyEngine {
        let (device, queue) = test_device();
        let gpu = GpuContext::new_headless(device, queue);
        DarklyEngine::new(gpu, 512, 256)
    }

    /// Push a second/third text object onto an existing vector layer (the
    /// public API mints one layer per text; multi-object layers exercise the
    /// topmost-wins iteration). Returns the stamped id.
    fn push_text(
        engine: &mut DarklyEngine,
        layer: LayerId,
        s: &str,
        x: f64,
        y: f64,
    ) -> crate::layer::ObjectId {
        let mut text = TextProps::new(s.to_string());
        text.size = 40.0;
        let fill = peniko::Brush::Solid(peniko::Color::from_rgba8(255, 255, 255, 255));
        let obj = VectorObject::text(text, kurbo::Affine::translate((x, y)), fill);
        let id = match engine.doc.find_node_mut(layer) {
            Some(LayerNode::Layer(Layer::Vector(v))) => v.push_object(obj),
            _ => panic!("not a vector layer"),
        };
        engine.sync_vector_layer(layer);
        id
    }

    #[test]
    fn hit_test_finds_topmost_object() {
        let mut engine = test_engine();
        // A non-zero canvas origin must not shift hit-testing: the query and the
        // object placement are both PLANE coords, so a crop can't break it.
        engine.doc.canvas_origin = CanvasPoint::new(40, 25);

        let mut text = TextProps::new("Ag".to_string());
        text.size = 40.0;
        let (layer, a) = engine.add_text_layer(text, 10.0, 10.0, [255, 255, 255, 255], None);
        // A second object, far to the right, that doesn't overlap `a`.
        let b = push_text(&mut engine, layer, "Ag", 200.0, 10.0);

        // A point a few px into each glyph box hits that object…
        assert_eq!(engine.hit_test_vector_object(layer, 14.0, 22.0), Some(a));
        assert_eq!(engine.hit_test_vector_object(layer, 204.0, 22.0), Some(b));
        // …empty space hits nothing.
        assert_eq!(engine.hit_test_vector_object(layer, 480.0, 240.0), None);

        // A third object stacked directly over `a` — drawn last, so it wins the
        // overlap (topmost-first via reverse iteration).
        let c = push_text(&mut engine, layer, "Ag", 10.0, 10.0);
        assert_eq!(engine.hit_test_vector_object(layer, 14.0, 22.0), Some(c));
    }

    #[test]
    fn locked_or_hidden_layer_is_not_hit() {
        let mut engine = test_engine();
        let mut text = TextProps::new("Ag".to_string());
        text.size = 40.0;
        let (layer, _a) = engine.add_text_layer(text, 10.0, 10.0, [255, 255, 255, 255], None);
        assert!(engine.hit_test_vector_object(layer, 14.0, 22.0).is_some());

        engine.set_node_locked(layer, true);
        assert_eq!(engine.hit_test_vector_object(layer, 14.0, 22.0), None);
        engine.set_node_locked(layer, false);
        engine.set_layer_visible(layer, false);
        assert_eq!(engine.hit_test_vector_object(layer, 14.0, 22.0), None);
    }

    #[test]
    fn vector_object_info_matrix_round_trips_through_set() {
        let mut engine = test_engine();
        let mut text = TextProps::new("Ag".to_string());
        text.size = 40.0;
        let (layer, obj) = engine.add_text_layer(text, 30.0, 20.0, [255, 255, 255, 255], None);

        // The reported gizmo matrix, fed straight back as the gizmo's output,
        // reproduces the same object transform (origin is folded into `G`).
        let (ox, oy, w, h, matrix) = engine.vector_object_info(layer, obj).expect("info");
        assert_eq!((ox, oy), (0.0, 0.0));
        assert!(w > 0.0 && h > 0.0);

        let before = match engine.doc.layer(layer) {
            Some(Layer::Vector(v)) => v.object(obj).unwrap().transform,
            _ => unreachable!(),
        };
        engine.set_vector_object_transform(
            layer,
            obj,
            crate::transform::Transform::from_affine(matrix),
        );
        let after = match engine.doc.layer(layer) {
            Some(Layer::Vector(v)) => v.object(obj).unwrap().transform,
            _ => unreachable!(),
        };
        for (b, a) in before.as_coeffs().iter().zip(after.as_coeffs().iter()) {
            assert!((b - a).abs() < 1e-4, "round-trip drift: {b} vs {a}");
        }
    }
}
