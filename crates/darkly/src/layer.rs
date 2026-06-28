use kurbo::{Affine, BezPath, Stroke};
use peniko::Brush;
use serde::{Deserialize, Serialize};

use crate::coord::CanvasRect;
use crate::gpu::blend_mode::{self, BlendModeRegistration};
use crate::gpu::params::ParamValue;

slotmap::new_key_type! {
    /// Unique identifier for any node, group, or filter in a [`Document`].
    /// Backed by a slotmap key — generational, so stale ids return `None` from
    /// [`Document`] lookups instead of aliasing onto a recycled slot.
    ///
    /// At the WASM/JS boundary, marshal as `u64` via [`LayerId::to_ffi`] /
    /// [`LayerId::from_ffi`].
    ///
    /// [`Document`]: crate::document::Document
    pub struct LayerId;
}

impl LayerId {
    /// Pack into a `u64` for the WASM/JS boundary. Index in the low 32 bits,
    /// generation in the high 32. Round-trips losslessly through
    /// [`LayerId::from_ffi`].
    pub fn to_ffi(self) -> u64 {
        slotmap::Key::data(&self).as_ffi()
    }

    /// Unpack a `u64` previously produced by [`LayerId::to_ffi`]. The result
    /// is only meaningful within the [`Document`] that minted the original key.
    ///
    /// [`Document`]: crate::document::Document
    pub fn from_ffi(v: u64) -> Self {
        slotmap::KeyData::from_ffi(v).into()
    }
}

/// Properties shared by every node in the tree — raster layers, groups, and
/// filters. Lock prevents any mutation; lives on every node by construction
/// so the universal check is one line at every mutation entry point.
pub struct NodeCommon {
    pub name: String,
    pub visible: bool,
    pub locked: bool,
}

impl NodeCommon {
    pub fn new(name: String) -> Self {
        NodeCommon {
            name,
            visible: true,
            locked: false,
        }
    }
}

/// Compositing properties for nodes that participate in normal blending
/// (raster layers and groups). Filters don't have one — masks structurally
/// have no opacity or blend mode.
///
/// `blend_mode` is a registry reference, not an enum: `type_id` is the
/// identity (used by the wire format, undo, and `set_blend_mode`), and
/// `gpu_value` is the integer the composite shader switches on. There is no
/// parallel enum representation — registry-resolved registrations are the
/// only carrier.
pub struct BlendProps {
    pub opacity: f32,
    pub blend_mode: &'static BlendModeRegistration,
}

impl BlendProps {
    pub fn new() -> Self {
        BlendProps {
            opacity: 1.0,
            blend_mode: blend_mode::registry().default(),
        }
    }
}

impl Default for BlendProps {
    fn default() -> Self {
        Self::new()
    }
}

/// Pixel-storage metadata for any node holding GPU pixels (raster layers, mask
/// filters, future filter caches). Bulk pixel data is GPU-authoritative; this
/// struct only carries canvas-space metadata: extent and texture format.
///
/// Every `PixelBuffer` is sampled independently — the blend shader computes UV
/// from each buffer's own bounds. Lockstep growth (host + non-locked mask grow
/// together) is a document-side convenience that drops out for free when both
/// buffers receive the same rasterized transform.
pub struct PixelBuffer {
    pub bounds: CanvasRect,
    pub format: wgpu::TextureFormat,
}

impl PixelBuffer {
    pub fn new(bounds: CanvasRect, format: wgpu::TextureFormat) -> Self {
        PixelBuffer { bounds, format }
    }
}

/// A raster (pixel) layer.
pub struct RasterLayer {
    pub id: LayerId,
    pub common: NodeCommon,
    pub blend: BlendProps,
    pub pixels: PixelBuffer,
    /// Filters attached to this layer, in bottom-up order. Each entry is a
    /// [`LayerId`] resolvable in the owning [`Document`]'s entity store.
    ///
    /// [`Document`]: crate::document::Document
    pub filters: Vec<LayerId>,
}

impl RasterLayer {
    /// Construct a raster layer. `name` is the display name shown in the
    /// layer panel — owners (the [`Document`]) supply a sequential
    /// "Layer N" string rather than letting each constructor invent one
    /// from the slotmap key, which would surface raw ffi values like
    /// "Layer 4294967301" to the user.
    pub fn new(id: LayerId, bounds: CanvasRect, name: String) -> Self {
        RasterLayer {
            id,
            common: NodeCommon::new(name),
            blend: BlendProps::new(),
            pixels: PixelBuffer::new(bounds, wgpu::TextureFormat::Rgba8Unorm),
            filters: Vec::new(),
        }
    }
}

/// A void (procedural) layer. Generates its pixels from a GPU shader instead
/// of storing them — see [`crate::gpu::void::Void`] for the trait + registry,
/// and the README's "Voids" section for the user-facing concept.
///
/// Void state is exactly: a [`crate::gpu::void::VoidRegistration::type_id`]
/// string identifying which procedural kind to run, plus the parameter
/// values for that kind. There is no pixel buffer — the compositor allocates
/// a derived texture on demand and re-renders it from these inputs.
pub struct VoidLayer {
    pub id: LayerId,
    pub common: NodeCommon,
    pub blend: BlendProps,
    /// Which void type from [`crate::gpu::void::VoidRegistry`] this layer
    /// runs. Stable string id (e.g. `"noise"`), not a registration pointer:
    /// the registry is a process-global and the document must survive
    /// serialization without holding live pointers.
    pub void_type: String,
    /// Parameter values matching the void type's
    /// [`crate::gpu::void::ParamDef`] schema, in order.
    pub params: Vec<ParamValue>,
    /// User transform (pan / scale / rotate) applied to the void's output,
    /// edited by the generic transform gizmo. Persistent / undoable /
    /// serializable document state. Voids that don't opt into live transform
    /// (see [`crate::gpu::void::VoidRegistration::supports_live_transform`])
    /// simply leave this at identity. Default `Basic(IDENTITY)`.
    pub transform: crate::transform::Transform,
    pub filters: Vec<LayerId>,
    /// Optional persistent frame snapshot. Most voids leave this `None`
    /// (their output is purely procedural — replays from params). The
    /// camera void uses it to round-trip the last received webcam frame
    /// through save/load so reopening a `.darkly` doesn't show a black
    /// rectangle until permission is regranted. When set, the save flow
    /// readbacks the void's aux texture into a pixel blob keyed
    /// `"layers/<id>.pixels"`, and the load flow restores it via
    /// [`crate::gpu::compositor::Compositor::restore_void_pixels`].
    pub frame: Option<crate::format::manifest::ManifestPixelRef>,
}

impl VoidLayer {
    pub fn new(id: LayerId, name: String, void_type: String, params: Vec<ParamValue>) -> Self {
        VoidLayer {
            id,
            common: NodeCommon::new(name),
            blend: BlendProps::new(),
            void_type,
            params,
            transform: crate::transform::Transform::identity(),
            filters: Vec::new(),
            frame: None,
        }
    }
}

/// A filter layer — a non-destructive procedural *transform* in the layer
/// tree. Where a [`VoidLayer`] is a procedural *source* (it generates pixels),
/// a filter layer transforms the composite of everything below it in place
/// (the group accumulator), leaving the lower layers' own pixels untouched.
/// This is Krita's *adjustment layer*. Scope it to one layer by placing it in a
/// non-passthrough (isolated) group.
///
/// State is exactly: a `pipeline` id naming which
/// [`crate::gpu::filter::FilterPipelineRegistry`] transform to run (e.g.
/// `"invert"`), plus that transform's parameter values. There is no pixel
/// buffer — the compositor runs the shared filter pipeline over the running
/// accumulator each frame.
pub struct FilterLayer {
    pub id: LayerId,
    pub common: NodeCommon,
    pub blend: BlendProps,
    /// Stable `type_id` from [`crate::gpu::filter::FilterPipelineRegistry`]
    /// (e.g. `"invert"`). Named `pipeline` rather than `filter_type` because
    /// `filters` (below) already means the attached mask/selection list — two
    /// "filter" fields on one struct would be a footgun.
    pub pipeline: String,
    /// Parameter values matching the filter pipeline's schema, in order. Empty
    /// for parameter-free filters like invert.
    pub params: Vec<ParamValue>,
    /// Attached mask / selection filters (same polymorphic list every layer
    /// kind carries).
    pub filters: Vec<LayerId>,
}

impl FilterLayer {
    pub fn new(id: LayerId, name: String, pipeline: String, params: Vec<ParamValue>) -> Self {
        FilterLayer {
            id,
            common: NodeCommon::new(name),
            blend: BlendProps::new(),
            pipeline,
            params,
            filters: Vec::new(),
        }
    }
}

/// Horizontal text alignment within a [`TextProps`] block. Maps onto parley's
/// `Alignment` at shape time and is the only alignment authority — the renderer
/// never re-derives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextAlign {
    Start,
    Center,
    End,
    Justified,
}

/// Optional slant for a [`TextProps`] block. Kept as a small enum (rather than
/// parley's `FontStyle`, which carries an oblique angle we don't expose yet) so
/// the document model owns a stable, serializable vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextStyle {
    Normal,
    Italic,
}

/// Editable text — the one bespoke vector source Darkly adds. Its persistent
/// state is a string plus a font selection, **not** glyph outlines: the layer
/// re-shapes (parley) and re-rasterizes (vello) whenever any field changes.
/// Everything else about a vector object is generic kurbo/peniko geometry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextProps {
    pub content: String,
    /// Family name resolved against the engine's font collection at shape time
    /// (e.g. `"Inter"`). A family the binary doesn't ship falls back to the
    /// collection default — see the font-portability open risk in the plan.
    pub font_family: String,
    pub style: TextStyle,
    /// CSS-style weight (100–900, 400 = regular).
    pub weight: f32,
    /// Font size in canvas pixels (raster-first: this is the rasterization
    /// size, not a resolution-independent point size).
    pub size: f32,
    /// Multiplier on the font's natural line height (1.0 = natural).
    pub line_height: f32,
    pub align: TextAlign,
}

impl TextProps {
    pub fn new(content: String) -> Self {
        TextProps {
            content,
            font_family: "sans-serif".to_string(),
            style: TextStyle::Normal,
            weight: 400.0,
            size: 48.0,
            line_height: 1.2,
            align: TextAlign::Start,
        }
    }
}

/// What a [`VectorObject`] draws. Two variants and no growth axis: shapes are
/// plain [`BezPath`]s (rectangles/ellipses convert into one), and the only
/// editable, non-path source is [`TextProps`]. No trait, no registry — a third
/// kind would only ever be another bespoke editable source, added here.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ObjectSource {
    Path(BezPath),
    Text(TextProps),
}

/// One drawable object on a [`VectorLayer`]: geometry/text plus its local
/// transform and fill/stroke style. Geometry and style speak the standard
/// kurbo/peniko vocabulary every renderer in this space consumes, so objects
/// persist through their derived `serde` impls almost for free.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VectorObject {
    /// Object-local affine, applied on top of the layer transform. For text
    /// this carries the placement (the caret position the tool clicked).
    pub transform: Affine,
    pub fill: Option<Brush>,
    pub stroke: Option<(Stroke, Brush)>,
    pub source: ObjectSource,
}

impl VectorObject {
    /// A text object filled with a solid color, placed by `transform`.
    pub fn text(text: TextProps, transform: Affine, fill: Brush) -> Self {
        VectorObject {
            transform,
            fill: Some(fill),
            stroke: None,
            source: ObjectSource::Text(text),
        }
    }
}

/// A vector-object layer — a layer owning an ordered list of [`VectorObject`]s
/// (text today; paths/shapes later, reusing this layer and renderer). The
/// objects are the authoritative, editable, serializable description; the GPU
/// texture is a rebuildable realization. Darkly is raster-first: the texture
/// regenerates only when objects/style/transform change, never on view zoom.
pub struct VectorLayer {
    pub id: LayerId,
    pub common: NodeCommon,
    pub blend: BlendProps,
    pub objects: Vec<VectorObject>,
    /// Layer-level user transform (gizmo-edited). Baked into the realized
    /// texture at rasterization — a change is an object change and re-rasters
    /// on commit, never a shader-time affine on a stale texture.
    pub transform: crate::transform::Transform,
    pub filters: Vec<LayerId>,
}

impl VectorLayer {
    pub fn new(id: LayerId, name: String) -> Self {
        VectorLayer {
            id,
            common: NodeCommon::new(name),
            blend: BlendProps::new(),
            objects: Vec::new(),
            transform: crate::transform::Transform::identity(),
            filters: Vec::new(),
        }
    }
}

pub struct LayerGroup {
    pub id: LayerId,
    pub common: NodeCommon,
    pub blend: BlendProps,
    /// Child node ids in display order (bottom-to-top). Resolve via the owning
    /// [`Document`]'s entity store.
    ///
    /// [`Document`]: crate::document::Document
    pub children: Vec<LayerId>,
    pub filters: Vec<LayerId>,
    /// True = passthrough (default), false = normal isolated group.
    pub passthrough: bool,
    /// UI state: whether the group is visually collapsed in the layer panel.
    pub collapsed: bool,
}

impl LayerGroup {
    /// Construct a group. `name` is the display name; same rationale as
    /// [`RasterLayer::new`] — owners pass a sequential string.
    pub fn new(id: LayerId, name: String) -> Self {
        LayerGroup {
            id,
            common: NodeCommon::new(name),
            blend: BlendProps::new(),
            children: Vec::new(),
            filters: Vec::new(),
            passthrough: true,
            collapsed: false,
        }
    }
}

/// A node in the layer tree — either a leaf layer or a group containing children.
/// Filters are NOT [`LayerNode`]s; they live on a host's `filters` list as
/// [`LayerId`] references and are resolved through the owning [`Document`].
///
/// [`Document`]: crate::document::Document
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

    pub fn common(&self) -> &NodeCommon {
        match self {
            LayerNode::Layer(l) => l.common(),
            LayerNode::Group(g) => &g.common,
        }
    }

    pub fn common_mut(&mut self) -> &mut NodeCommon {
        match self {
            LayerNode::Layer(l) => l.common_mut(),
            LayerNode::Group(g) => &mut g.common,
        }
    }

    pub fn blend(&self) -> &BlendProps {
        match self {
            LayerNode::Layer(l) => l.blend(),
            LayerNode::Group(g) => &g.blend,
        }
    }

    pub fn blend_mut(&mut self) -> &mut BlendProps {
        match self {
            LayerNode::Layer(l) => l.blend_mut(),
            LayerNode::Group(g) => &mut g.blend,
        }
    }

    pub fn filters(&self) -> &[LayerId] {
        match self {
            LayerNode::Layer(l) => l.filters(),
            LayerNode::Group(g) => &g.filters,
        }
    }

    pub fn modifiers_mut(&mut self) -> &mut Vec<LayerId> {
        match self {
            LayerNode::Layer(l) => l.modifiers_mut(),
            LayerNode::Group(g) => &mut g.filters,
        }
    }

    pub fn pixels(&self) -> Option<&PixelBuffer> {
        match self {
            LayerNode::Layer(l) => l.pixels(),
            LayerNode::Group(_) => None,
        }
    }

    pub fn pixels_mut(&mut self) -> Option<&mut PixelBuffer> {
        match self {
            LayerNode::Layer(l) => l.pixels_mut(),
            LayerNode::Group(_) => None,
        }
    }

    pub fn visible(&self) -> bool {
        self.common().visible
    }

    pub fn locked(&self) -> bool {
        self.common().locked
    }

    /// The registration record for this node's kind — owns `type_id` (wire
    /// format), `display_name` (UI), and any future per-kind metadata. The
    /// match arms reference each kind module's own `TYPE_ID` constant rather
    /// than re-typing the string literal, so there is no parallel name to
    /// keep in sync with the registration files.
    pub fn kind(&self) -> &'static crate::document::LayerKindRegistration {
        use crate::document::layer_kind::registry;
        use crate::document::layer_kinds::{filter, group, raster, vector, void};
        match self {
            LayerNode::Layer(Layer::Raster(_)) => registry().get(raster::TYPE_ID).unwrap(),
            LayerNode::Layer(Layer::Void(_)) => registry().get(void::TYPE_ID).unwrap(),
            LayerNode::Layer(Layer::Filter(_)) => registry().get(filter::TYPE_ID).unwrap(),
            LayerNode::Layer(Layer::Vector(_)) => registry().get(vector::TYPE_ID).unwrap(),
            LayerNode::Group(_) => registry().get(group::TYPE_ID).unwrap(),
        }
    }

    /// Convenience for the wire format / save file — just the stable `type_id`
    /// string from `kind()`.
    pub fn type_id(&self) -> &'static str {
        self.kind().type_id
    }

    /// Composite this node into its parent group's accumulators. The
    /// variant dispatch is owned by `LayerNode` so the compositor's child
    /// walk never re-introduces a centralised match on node kind. Each arm
    /// delegates back through `ctx` into a compositor-private method that
    /// owns the GPU work — variant *knows itself*, compositor *does the
    /// work*.
    pub fn compose_into(&self, ctx: &mut crate::gpu::compositor::CompositionContext<'_>) {
        match self {
            LayerNode::Layer(layer) => ctx.compose_layer(layer),
            LayerNode::Group(group) => ctx.compose_group(group),
        }
    }

    /// Whether this node transforms the running parent accumulator *in place*
    /// (the composite of everything below it) rather than blending its own
    /// discrete texture in. A passthrough group inlines its children into the
    /// parent; a filter layer runs its pipeline over the accumulator. Both want
    /// a snapshot+lerp detour when they carry a visible mask, so the compositor
    /// keys its mask-snapshot resource off this predicate instead of
    /// enumerating which kinds qualify — a new in-place kind is purely additive.
    /// Isolated groups, raster, and void layers blend a texture and answer
    /// `false`.
    pub fn composites_in_place(&self) -> bool {
        match self {
            LayerNode::Group(g) => g.passthrough,
            LayerNode::Layer(Layer::Filter(_)) => true,
            LayerNode::Layer(_) => false,
        }
    }
}

/// How a layer answers "can the user transform me, and how?" — consumed by the
/// Transform tool to pick which binding drives the generic gizmo. This is the
/// layer describing *itself* (type-owned dispatch); the transform subsystem
/// never branches on layer kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransformCapability {
    /// Live, non-destructive, persistent transform property (e.g. a camera
    /// void). The gizmo edits the stored [`crate::transform::Transform`].
    Live,
    /// Destructive extract-and-commit (raster layers — today's floating).
    Destructive,
    /// Not user-transformable (groups, non-transformable voids).
    None,
}

pub enum Layer {
    Raster(RasterLayer),
    Void(VoidLayer),
    Filter(FilterLayer),
    Vector(VectorLayer),
}

impl Layer {
    /// Whether and how the user can transform this layer. The void's opinion is
    /// static, looked up from the registry by `void_type` — passed in because
    /// [`VoidRegistry`] is owned by the compositor, not a global.
    ///
    /// [`VoidRegistry`]: crate::gpu::void::VoidRegistry
    pub fn transform_capability(
        &self,
        reg: &crate::gpu::void::VoidRegistry,
    ) -> TransformCapability {
        match self {
            Layer::Raster(_) => TransformCapability::Destructive,
            Layer::Void(v) => {
                if reg.supports_live_transform(&v.void_type) {
                    TransformCapability::Live
                } else {
                    TransformCapability::None
                }
            }
            // A full-frame filter has no meaningful transform — there are no
            // pixels of its own to move.
            Layer::Filter(_) => TransformCapability::None,
            // A vector layer's transform re-rasterizes on commit (raster-first;
            // not `Live`'s shader-time affine on a stale texture). The gizmo
            // wiring for that commit path is future work — text-first ships
            // placement via per-object affines — so it is not user-draggable yet.
            Layer::Vector(_) => TransformCapability::None,
        }
    }

    /// Whether this layer participates in the standard blend pipeline — i.e.
    /// it has a node texture + blend uniforms in the compositor's
    /// `layer_cache`. Raster and void layers do; a filter layer transforms the
    /// running group accumulator instead of contributing a texture of its own,
    /// so it is realized by `compose_filter_arm`, not the content walk.
    pub fn is_blend_content(&self) -> bool {
        !matches!(self, Layer::Filter(_))
    }

    pub fn id(&self) -> LayerId {
        match self {
            Layer::Raster(r) => r.id,
            Layer::Void(v) => v.id,
            Layer::Filter(f) => f.id,
            Layer::Vector(v) => v.id,
        }
    }

    pub fn common(&self) -> &NodeCommon {
        match self {
            Layer::Raster(r) => &r.common,
            Layer::Void(v) => &v.common,
            Layer::Filter(f) => &f.common,
            Layer::Vector(v) => &v.common,
        }
    }

    pub fn common_mut(&mut self) -> &mut NodeCommon {
        match self {
            Layer::Raster(r) => &mut r.common,
            Layer::Void(v) => &mut v.common,
            Layer::Filter(f) => &mut f.common,
            Layer::Vector(v) => &mut v.common,
        }
    }

    pub fn blend(&self) -> &BlendProps {
        match self {
            Layer::Raster(r) => &r.blend,
            Layer::Void(v) => &v.blend,
            Layer::Filter(f) => &f.blend,
            Layer::Vector(v) => &v.blend,
        }
    }

    pub fn blend_mut(&mut self) -> &mut BlendProps {
        match self {
            Layer::Raster(r) => &mut r.blend,
            Layer::Void(v) => &mut v.blend,
            Layer::Filter(f) => &mut f.blend,
            Layer::Vector(v) => &mut v.blend,
        }
    }

    pub fn filters(&self) -> &[LayerId] {
        match self {
            Layer::Raster(r) => &r.filters,
            Layer::Void(v) => &v.filters,
            Layer::Filter(f) => &f.filters,
            Layer::Vector(v) => &v.filters,
        }
    }

    pub fn modifiers_mut(&mut self) -> &mut Vec<LayerId> {
        match self {
            Layer::Raster(r) => &mut r.filters,
            Layer::Void(v) => &mut v.filters,
            Layer::Filter(f) => &mut f.filters,
            Layer::Vector(v) => &mut v.filters,
        }
    }

    /// Pixel buffer for this layer, if any. Void, filter, and vector layers
    /// have no authoritative pixels — a void regenerates from `params`, a
    /// filter transforms the accumulator below it, and a vector layer's texture
    /// is a realization of its `objects`.
    pub fn pixels(&self) -> Option<&PixelBuffer> {
        match self {
            Layer::Raster(r) => Some(&r.pixels),
            Layer::Void(_) | Layer::Filter(_) | Layer::Vector(_) => None,
        }
    }

    pub fn pixels_mut(&mut self) -> Option<&mut PixelBuffer> {
        match self {
            Layer::Raster(r) => Some(&mut r.pixels),
            Layer::Void(_) | Layer::Filter(_) | Layer::Vector(_) => None,
        }
    }

    pub fn visible(&self) -> bool {
        self.common().visible
    }

    pub fn locked(&self) -> bool {
        self.common().locked
    }

    /// Regenerable procedural state — `(params, transform)` — for void layers,
    /// `None` for raster. Used by `sync_compositor_layers` to push the doc's
    /// authoritative void state downhill to the compositor after any doc
    /// mutation (undo / redo / load), so the running void instance never drifts
    /// from the document.
    pub fn void_state(&self) -> Option<(&[ParamValue], &crate::transform::Transform)> {
        match self {
            Layer::Void(v) => Some((&v.params, &v.transform)),
            Layer::Raster(_) | Layer::Filter(_) | Layer::Vector(_) => None,
        }
    }
}
