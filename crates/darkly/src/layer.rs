use std::collections::BTreeMap;

use kurbo::{Affine, BezPath, Stroke};
use peniko::Brush;
use serde::{Deserialize, Serialize};

use crate::coord::CanvasRect;
use crate::gpu::blend_mode::{self, BlendModeRegistration};
use crate::gpu::params::ParamValue;

slotmap::new_key_type! {
    /// Unique identifier for any node, group, or filter in a [`Document`].
    /// Backed by a slotmap key (generational), so stale ids return `None` from
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

// A `LayerId` crosses the wire as the same packed `u64` it uses at the WASM/JS
// boundary. The slotmap key's internal repr is index+version, so this can't be
// a derive: it serializes *through* `to_ffi`/`from_ffi`, transparent as a
// single JSON number. This is the protocol's `LayerId` coercion (see the typed
// engine bridge): with it, handlers carry `LayerId` directly instead of
// shimming `u64` at every call site.
impl serde::Serialize for LayerId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(self.to_ffi())
    }
}

impl<'de> serde::Deserialize<'de> for LayerId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(LayerId::from_ffi(u64::deserialize(deserializer)?))
    }
}

// On the wire a `LayerId` is just its packed `u64`, so to the typed TS client it
// is a `number`, mirroring the serde impls above. The slotmap key has no fields
// to derive `TS` from, so this maps it to the same primitive `u64` exports as.
#[cfg(feature = "ts-export")]
impl ts_rs::TS for LayerId {
    type WithoutGenerics = Self;
    type OptionInnerType = Self;
    fn name(cfg: &ts_rs::Config) -> String {
        u64::name(cfg)
    }
    fn inline(cfg: &ts_rs::Config) -> String {
        u64::inline(cfg)
    }
}

/// Properties shared by every node in the tree: raster layers, groups, and
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
/// (raster layers and groups). Filters don't have one: masks structurally
/// have no opacity or blend mode.
///
/// `blend_mode` is a registry reference, not an enum: `type_id` is the
/// identity (used by the wire format, undo, and `set_blend_mode`), and
/// `gpu_value` is the integer the composite shader switches on. There is no
/// parallel enum representation: registry-resolved registrations are the
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
/// Every `PixelBuffer` is sampled independently: the blend shader computes UV
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
    /// layer panel: owners (the [`Document`]) supply a sequential
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
/// of storing them (see [`crate::gpu::void::Void`] for the trait + registry,
/// and the README's "Voids" section for the user-facing concept).
///
/// Void state is exactly: a [`crate::gpu::void::VoidRegistration::type_id`]
/// string identifying which procedural kind to run, plus the parameter
/// values for that kind. There is no pixel buffer: the compositor allocates
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
    /// (their output is purely procedural: replays from params). The
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

/// A filter layer: a non-destructive procedural *transform* in the layer
/// tree. Where a [`VoidLayer`] is a procedural *source* (it generates pixels),
/// a filter layer transforms the composite of everything below it in place
/// (the group accumulator), leaving the lower layers' own pixels untouched.
/// This is Krita's *adjustment layer*. Scope it to one layer by placing it in a
/// non-passthrough (isolated) group.
///
/// State is exactly: a `pipeline` id naming which
/// [`crate::gpu::filter::FilterPipelineRegistry`] transform to run (e.g.
/// `"invert"`), plus that transform's parameter values. There is no pixel
/// buffer: the compositor runs the shared filter pipeline over the running
/// accumulator each frame.
pub struct FilterLayer {
    pub id: LayerId,
    pub common: NodeCommon,
    pub blend: BlendProps,
    /// Stable `type_id` from [`crate::gpu::filter::FilterPipelineRegistry`]
    /// (e.g. `"invert"`). Named `pipeline` rather than `filter_type` because
    /// `filters` (below) already means the attached mask/selection list: two
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
/// `Alignment` at shape time and is the only alignment authority: the renderer
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

/// How a [`TextProps`] block lays out on the canvas. Modular by design so new
/// layout modes slot in additively: `Point` (auto width, grows with content)
/// and `Area` (fixed box, wraps + aligns within) today; a `Path` variant that
/// maps glyphs onto a `kurbo` curve is the designed-for future addition: one
/// new variant plus one shape/render arm, no rework of the readers here.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum TextLayout {
    /// Auto-width point text: lines break only at explicit newlines, alignment
    /// resolves within the block's own natural width.
    Point,
    /// Fixed area box in canvas pixels: lines wrap to `width`, `align` resolves
    /// within `width`, and the on-canvas frame is `width × height`.
    Area { width: f32, height: f32 },
}

impl TextLayout {
    /// The fixed area-box size in canvas pixels, or `None` for point text. The
    /// one accessor box readers use; no consumer matches the enum inline, so a
    /// future `Path` variant (which is also `None`-sized) needs no edits here.
    pub fn area_size(&self) -> Option<(f32, f32)> {
        match self {
            TextLayout::Area { width, height } => Some((*width, *height)),
            TextLayout::Point => None,
        }
    }
}

/// Editable text: the one bespoke vector source Darkly adds. Its persistent
/// state is a string plus a font selection, **not** glyph outlines: the layer
/// re-shapes (parley) and re-rasterizes (vello) whenever any field changes.
/// Everything else about a vector object is generic kurbo/peniko geometry.
///
/// Style is font-driven and open-ended: `variations` carries arbitrary
/// variable-font axes (weight is just the `wght` axis), `features` carries
/// OpenType features, and spacing/line-height are genuine fields, so a new
/// font capability is additive data, never a new hard-coded knob.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextProps {
    pub content: String,
    /// Family name resolved against the engine's font collection at shape time
    /// (e.g. `"Inter"`). A family the binary doesn't ship falls back to the
    /// collection default (see the font-portability open risk in the plan).
    pub font_family: String,
    /// Italic is a *face selection* (upright vs italic face), not a variation
    /// axis, so it stays a distinct field rather than living in `variations`.
    pub style: TextStyle,
    /// Variable-font axis values, keyed by the 4-char fvar tag (`"wght"`,
    /// `"wdth"`, `"opsz"`, …). An absent axis takes the font's default, so this
    /// stays empty for untouched/static fonts (clean serde, partial merges).
    pub variations: BTreeMap<String, f32>,
    /// OpenType feature values, keyed by the 4-char feature tag (`"liga"`,
    /// `"smcp"`, …). Modelled + shaped now; no UI yet.
    pub features: BTreeMap<String, u32>,
    /// Font size in canvas pixels (raster-first: this is the rasterization
    /// size, not a resolution-independent point size).
    pub size: f32,
    /// Multiplier on the font's natural line height (1.0 = natural).
    pub line_height: f32,
    /// Extra horizontal space between letters, in canvas pixels (0 = natural).
    pub letter_spacing: f32,
    /// Extra horizontal space between words, in canvas pixels (0 = natural).
    pub word_spacing: f32,
    pub align: TextAlign,
    /// Point vs fixed-area layout (see [`TextLayout`]).
    pub layout: TextLayout,
}

impl TextProps {
    pub fn new(content: String) -> Self {
        TextProps {
            content,
            font_family: "sans-serif".to_string(),
            style: TextStyle::Normal,
            variations: BTreeMap::new(),
            features: BTreeMap::new(),
            size: 48.0,
            line_height: 1.2,
            letter_spacing: 0.0,
            word_spacing: 0.0,
            align: TextAlign::Start,
            layout: TextLayout::Point,
        }
    }
}

/// What a [`VectorObject`] draws. Two variants and no growth axis: shapes are
/// plain [`BezPath`]s (rectangles/ellipses convert into one), and the only
/// editable, non-path source is [`TextProps`]. No trait, no registry: a third
/// kind would only ever be another bespoke editable source, added here.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ObjectSource {
    Path(BezPath),
    Text(TextProps),
}

/// Stable identity of a [`VectorObject`] within its owning [`VectorLayer`].
/// Scoped per layer (not globally) and minted monotonically by
/// [`VectorLayer::push_object`], never reused, so a delete-then-add can't
/// alias a stale reference. Object addressing uses this rather than a list
/// index, which would shift under reorder/insert/delete.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObjectId(pub u64);

impl ObjectId {
    /// The id stamped on an object that hasn't been pushed onto a layer yet
    /// (constructors leave this placeholder; [`VectorLayer::push_object`]
    /// overwrites it with the layer's next monotonic id).
    pub const UNASSIGNED: ObjectId = ObjectId(0);
}

/// One drawable object on a [`VectorLayer`]: geometry/text plus its local
/// transform and fill/stroke style. Geometry and style speak the standard
/// kurbo/peniko vocabulary every renderer in this space consumes, so objects
/// persist through their derived `serde` impls almost for free.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VectorObject {
    /// Stable per-layer identity. Stamped by [`VectorLayer::push_object`];
    /// constructors leave [`ObjectId::UNASSIGNED`] until then.
    pub id: ObjectId,
    /// Object-local affine, applied on top of the layer transform. For text
    /// this carries the placement (the caret position the tool clicked).
    pub transform: Affine,
    pub fill: Option<Brush>,
    pub stroke: Option<(Stroke, Brush)>,
    pub source: ObjectSource,
}

impl VectorObject {
    /// A text object filled with a solid color, placed by `transform`. The id
    /// is left [`ObjectId::UNASSIGNED`]: the owning layer stamps it on push.
    pub fn text(text: TextProps, transform: Affine, fill: Brush) -> Self {
        VectorObject {
            id: ObjectId::UNASSIGNED,
            transform,
            fill: Some(fill),
            stroke: None,
            source: ObjectSource::Text(text),
        }
    }
}

/// A vector-object layer: a layer owning an ordered list of [`VectorObject`]s
/// (text today; paths/shapes later, reusing this layer and renderer). The
/// objects are the authoritative, editable, serializable description; the GPU
/// texture is a rebuildable realization. Darkly is raster-first: the texture
/// regenerates only when objects/style/transform change, never on view zoom.
pub struct VectorLayer {
    pub id: LayerId,
    pub common: NodeCommon,
    pub blend: BlendProps,
    pub objects: Vec<VectorObject>,
    /// Next [`ObjectId`] to mint. Monotonic, never reused; survives
    /// serialization so reload can't re-issue a live id.
    pub next_object_id: u64,
    /// Layer-level user transform (gizmo-edited). Baked into the realized
    /// texture at rasterization: a change is an object change and re-rasters
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
            next_object_id: 0,
            transform: crate::transform::Transform::identity(),
            filters: Vec::new(),
        }
    }

    /// Append `obj`, stamping it with a fresh monotonic [`ObjectId`] and
    /// returning that id. The single entry point for adding objects: direct
    /// `objects.push` would leave the id unassigned.
    pub fn push_object(&mut self, mut obj: VectorObject) -> ObjectId {
        let id = ObjectId(self.next_object_id);
        self.next_object_id += 1;
        obj.id = id;
        self.objects.push(obj);
        id
    }

    /// Borrow the object with `id`, if present.
    pub fn object(&self, id: ObjectId) -> Option<&VectorObject> {
        self.objects.iter().find(|o| o.id == id)
    }

    /// Mutably borrow the object with `id`, if present.
    pub fn object_mut(&mut self, id: ObjectId) -> Option<&mut VectorObject> {
        self.objects.iter_mut().find(|o| o.id == id)
    }

    /// Index of the object with `id` in the draw-order list, if present.
    pub fn index_of(&self, id: ObjectId) -> Option<usize> {
        self.objects.iter().position(|o| o.id == id)
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
    /// [`RasterLayer::new`]: owners pass a sequential string.
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

/// A node in the layer tree: either a leaf layer or a group containing children.
/// Filters are NOT [`LayerNode`]s; they live on a host's `filters` list as
/// [`LayerId`] references and are resolved through the owning [`Document`].
///
/// [`Document`]: crate::document::Document
pub enum LayerNode {
    Layer(Layer),
    Group(LayerGroup),
}

/// Which of a node's two child lists an id lives in. A node owns both a
/// `filters` list (modifiers such as masks) and, for groups, a `children`
/// list of tree nodes; the two are disjoint. Detach reports the slot it found
/// an id in so reattach can put it back in the same one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildSlot {
    /// The host's `filters` list.
    Filter,
    /// A group's `children` list.
    Child,
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

    /// Group children, empty for a leaf layer. Pairs with
    /// [`LayerNode::children_mut`] so callers can treat "the node's children"
    /// uniformly without matching on the variant.
    pub fn children(&self) -> &[LayerId] {
        match self {
            LayerNode::Group(g) => &g.children,
            LayerNode::Layer(_) => &[],
        }
    }

    /// Mutable group children, `None` for a leaf layer: a layer has no
    /// children list to insert into, and that distinction is the node's to
    /// report rather than the caller's to test for.
    pub fn children_mut(&mut self) -> Option<&mut Vec<LayerId>> {
        match self {
            LayerNode::Group(g) => Some(&mut g.children),
            LayerNode::Layer(_) => None,
        }
    }

    /// Remove `child` from whichever of this node's two lists holds it, and
    /// report which one that was. Ids are unique across the document's
    /// slotmap, so a child can only be in one, which is what lets detach be
    /// kind-agnostic instead of asking the caller to know whether it holds a
    /// filter or a tree node.
    pub fn detach_child(&mut self, child: LayerId) -> Option<ChildSlot> {
        let filters = self.modifiers_mut();
        if let Some(i) = filters.iter().position(|c| *c == child) {
            filters.remove(i);
            return Some(ChildSlot::Filter);
        }
        let children = self.children_mut()?;
        let i = children.iter().position(|c| *c == child)?;
        children.remove(i);
        Some(ChildSlot::Child)
    }

    /// Insert `child` into the list named by `slot`, at `position` (clamped) or
    /// at the end. Returns false when the node has no such list: a leaf layer
    /// asked to take a tree child.
    pub fn attach_child(
        &mut self,
        child: LayerId,
        slot: ChildSlot,
        position: Option<usize>,
    ) -> bool {
        let list = match slot {
            ChildSlot::Filter => self.modifiers_mut(),
            ChildSlot::Child => match self.children_mut() {
                Some(list) => list,
                None => return false,
            },
        };
        let at = position.map_or(list.len(), |p| p.min(list.len()));
        list.insert(at, child);
        true
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

    /// The registration record for this node's kind: owns `type_id` (wire
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

    /// Convenience for the wire format / save file: just the stable `type_id`
    /// string from `kind()`.
    pub fn type_id(&self) -> &'static str {
        self.kind().type_id
    }

    /// Composite this node into its parent group's accumulators. The
    /// variant dispatch is owned by `LayerNode` so the compositor's child
    /// walk never re-introduces a centralised match on node kind. Each arm
    /// delegates back through `ctx` into a compositor-private method that
    /// owns the GPU work: variant *knows itself*, compositor *does the
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
    /// enumerating which kinds qualify: a new in-place kind is purely additive.
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

/// How a layer answers "can the user transform me, and how?", consumed by the
/// Transform tool to pick which binding drives the generic gizmo. This is the
/// layer describing *itself* (type-owned dispatch); the transform subsystem
/// never branches on layer kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransformCapability {
    /// Live, non-destructive, persistent transform property (e.g. a camera
    /// void). The gizmo edits the stored [`crate::transform::Transform`].
    Live,
    /// Destructive extract-and-commit (raster layers: today's floating).
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
    /// static, looked up from the registry by `void_type`, passed in because
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
            // A full-frame filter has no meaningful transform: there are no
            // pixels of its own to move.
            Layer::Filter(_) => TransformCapability::None,
            // Whole-layer vector transform is `None`: the transform gizmo drives
            // individual objects, not the layer as a unit. Per-object transform
            // is a different axis, routed by the object transform binding (see
            // `frontend/src/tools/transform.svelte.ts`) rather than this
            // layer-level capability, so a vector layer reports `None` here.
            Layer::Vector(_) => TransformCapability::None,
        }
    }

    /// Whether this layer participates in the standard blend pipeline, i.e.
    /// it has a node texture + blend uniforms in the compositor's
    /// `layer_cache`. Raster and void layers do; a filter layer transforms the
    /// running group accumulator instead of contributing a texture of its own,
    /// so it is realized by `compose_filter_arm`, not the content walk.
    pub fn is_blend_content(&self) -> bool {
        !matches!(self, Layer::Filter(_))
    }

    /// Whether this layer's own GPU texture holds data that cannot be
    /// reconstructed, and therefore has to be kept alive while the layer is
    /// undoably deleted and released once it isn't.
    ///
    /// Most non-raster layers say no because their texture is derived: a
    /// procedural void re-renders from its params, a vector layer re-rasterizes
    /// from its objects, a filter layer has no texture at all. The exception is
    /// a void holding an externally-sourced image (a placed photo or a
    /// captured frame), which exists nowhere else and can be large enough that
    /// leaking it matters.
    pub fn owns_disposable_texture(&self) -> bool {
        match self {
            Layer::Raster(_) => true,
            // Document-side fact, so this needs no GPU query.
            Layer::Void(v) => v.frame.is_some(),
            Layer::Filter(_) | Layer::Vector(_) => false,
        }
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
    /// have no authoritative pixels: a void regenerates from `params`, a
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

    /// Regenerable procedural state for void layers, as `(params, transform)`;
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
