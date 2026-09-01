// @generated from RequestRegistry (ts-rs) — do not edit by hand.
// Regenerate: DARKLY_REGEN_TS=1 cargo test -p darkly --test protocol --features testing,ts-export

export type JsonValue =
    | string | number | boolean | null
    | JsonValue[]
    | { [key: string]: JsonValue };

export type AddFilterReq = { pipeline: string, params: JsonValue, anchor: number | null, };

export type AddGroupReq = { anchor: number | null, };

export type AddMaskReq = { id: number, };

export type AddRasterReq = { anchor: number | null, };

export type AddTextReq = { content: string, x: number, y: number, 
/**
 * RGBA 0–255. Defaults to opaque black.
 */
color?: [number, number, number, number], anchor: number, font_family?: string, size?: number, 
/**
 * Variable-font axis values (tag → value), including `wght`.
 */
variations?: { [key in string]: number }, 
/**
 * OpenType feature values (tag → value).
 */
features?: { [key in string]: number }, letter_spacing?: number, word_spacing?: number, line_height?: number, italic: boolean, align?: string, 
/**
 * `[w, h]` for a drag-created area-text box; `null`/absent → point text.
 */
box?: [number, number] | null, };

export type AddTextObjectReq = { id: number, content: string, x: number, y: number, 
/**
 * RGBA 0–255. Defaults to opaque black.
 */
color?: [number, number, number, number], font_family?: string, size?: number, 
/**
 * Variable-font axis values (tag → value), including `wght`.
 */
variations?: { [key in string]: number }, 
/**
 * OpenType feature values (tag → value).
 */
features?: { [key in string]: number }, letter_spacing?: number, word_spacing?: number, line_height?: number, italic: boolean, align?: string, 
/**
 * `[w, h]` for a drag-created area-text box; `null`/absent → point text.
 */
box?: [number, number] | null, };

export type AddVeilReq = { veil_type: string, params: JsonValue, };

export type AddVoidReq = { void_type: string, params: JsonValue, anchor: number | null, };

export type AlphaToSelectionReq = { id: number, };

export type ApplyFilterReq = { node_id: number, filter_type: string, params: JsonValue, };

export type ApplyMaskReq = { id: number, };

export type BeginStrokeReq = { id: number, };

export type BeginTransformReq = { id: number, };

export type BorderSelectionReq = { radius: number, };

export type BrushGraphCapabilities = { 
/**
 * Whether the graph's terminals honour erase mode — false iff any
 * terminal registers `supports_erase = false`. The brush-tool
 * options bar hides the erase toggle when false.
 */
supports_erase: boolean, 
/**
 * Iconify icon to show in the dab slot in place of a baked thumbnail,
 * contributed by the first node whose registration declares
 * `preview_staging` — content-dependent nodes (clone, blur, smudge,
 * liquify) whose still-dab bake renders blank.
 */
preview_fallback_icon: string | null, 
/**
 * Field the stroke preview is rendered over, from the same declaration
 * the icon comes from. [`PreviewBackdrop::Flat`] for a brush that deposits
 * pigment and so needs nothing staged under it.
 */
preview_backdrop: PreviewBackdrop, };

export type PreviewBackdrop = "Flat" | "Stripes";

export type BrushDabThumbnailReq = { name: string, };

export type BrushDeleteReq = { id: string, };

export type BrushExportYamlReq = { id: string, };

export type ExposedValue = { "kind": "scalar", 
/**
 * Current value in display-space.
 */
value: number, 
/**
 * Display-space minimum.
 */
min: number, 
/**
 * Display-space maximum.
 */
max: number, 
/**
 * Display-space default — what double-click reset returns to.
 * Sourced from the node-type registration, not the loaded brush.
 */
default: number, 
/**
 * Unit type for formatting and conversion.
 */
unitType: UnitType, } | { "kind": "bool", 
/**
 * Current value.
 */
value: boolean, } | { "kind": "enum", 
/**
 * Current selected index into `options`.
 */
value: number, 
/**
 * Dropdown labels in index order.
 */
options: Array<string>, };

export type UnitType = "Normalized" | "Percent" | "Degrees" | "Raw" | "Pixels";

export type ExposedPortInfo = { 
/**
 * `"<node_id>.<port_name>"` — the same string used to address the
 * entry in `Graph::exposed_ports`. Frontend passes it back to
 * `set_exposed_port_meta` / `reorder_exposed_port` without having
 * to reconstruct the format.
 */
key: string, nodeId: string, portName: string, label: string, icon: string, description: string, nodeDisplayName: string, data: ExposedValue, };

export type BrushGraphAddNodeReq = { type_id: string, };

export type BrushGraphAutoLayoutReq = { sizes: { [key in string]: [number, number] }, };

export type BrushGraphJsonReq = { json: string, };

export type BrushGraphConnectReq = { from_node: string, from_port: string, to_node: string, to_port: string, };

export type BrushGraphDisconnectReq = { from_node: string, from_port: string, to_node: string, to_port: string, };

export type BrushGraphExposePortReq = { node_id: string, port_name: string, };

export type BrushGraphYamlReq = { yaml: string, };

export type BrushGraphRemoveNodeReq = { node_id: string, };

export type BrushGraphReorderExposedPortReq = { key: string, new_index: number, };

export type BrushGraphSetExposedPortMetaReq = { key: string, label: string, description: string, icon: string, };

export type BrushGraphSetInputReq = { node_id: string, input_name: string, kind: string, value: JsonValue, };

export type BrushGraphSetNodeCommentReq = { node_id: string, comment: string, };

export type BrushGraphSetPortRangeReq = { node_id: string, port_name: string, display_min: number, display_max: number, };

export type BrushGraphUnexposePortReq = { node_id: string, port_name: string, };

export type BrushInfo = { 
/**
 * Opaque identity — what pack member lists and recents hold.
 */
id: string, 
/**
 * Display name, and the engine's public lookup key.
 */
name: string, author: string, description: string, tags: Array<string>, 
/**
 * Iconify icon shown in place of the baked dab/stroke thumbnails —
 * present when the graph contains a content-dependent node whose
 * preview bake renders blank (clone, blur, smudge, liquify). See
 * [`crate::brush::graph_capabilities`].
 */
icon: string | null, 
/**
 * Whether the painter may rename or delete this brush, so the UI can grey
 * out affordances it would otherwise offer. A hint, not the authority —
 * same contract as [`BrushPackInfo::can_edit_members`].
 */
can_edit: boolean, };

export type BrushLoadReq = { name: string, };

export type BrushNodePreviewReq = { node_id: string, };

export type PortDir = "Input" | "Output";

export type BrushWireType = "Scalar" | "Int" | "Bool" | "Vec2" | "Vec4" | "Enum" | "String" | "Curve";

export type InputValue = boolean | number | number | string | Array<[number, number]> | [number, number] | [number, number, number, number];

export type PortDef = { name: string, dir: PortDir, wire_type: BrushWireType, 
/**
 * Slider min when the port is disconnected (UI metadata only).
 */
min: number, 
/**
 * Slider max when the port is disconnected (UI metadata only).
 */
max: number, 
/**
 * The authored value used when this input port is disconnected — the
 * full typed value (scalar slider value, enum-dropdown index, texture
 * name, curve points, color). Wired inputs ignore it and take the
 * upstream expression. For output ports it stays the neutral scalar
 * default and is unused. Replaces the old scalar-only `default: f32`;
 * numeric inputs carry [`InputValue::Scalar`].
 */
value: InputValue, 
/**
 * Enum-dropdown labels, in index order — non-empty only for
 * [`WireKind`]-`Enum` inputs (shape's `algorithm`, noise/image `space`,
 * random's `mode`). Empty for every other input kind.
 */
enum_options?: Array<string>, 
/**
 * Whether an upstream wire may drive this input per-dab. Computed from
 * `wire_type.is_wirable()` at construction and carried as data so the
 * frontend reads it directly rather than re-deriving the rule — the
 * single source of truth is [`WireKind::is_wirable`]. Every port built
 * from a registration (`PortDef::input`/`output`, and the clones in
 * `add_node` / portable import) sets it correctly; serde round-trips it.
 */
wirable: boolean, 
/**
 * Whether a user may *expose* this input as a brush-bar control.
 * Computed from `wire_type.is_user_exposable()` at construction and
 * carried as data so the frontend gates its expose affordance directly
 * off one value rather than re-deriving the type rule — the single
 * source of truth is [`WireKind::is_user_exposable`]. Orthogonal to
 * `wirable`: an enum is exposable but not wirable; a wired scalar is
 * wirable but (while connected) not user-scrubbable. `expose_port`
 * enforces it, so a control the brush bar can't render can never be
 * surfaced. Serde round-trips it.
 */
exposable: boolean, 
/**
 * Quantization step. `0.0` (the default) means continuous; any positive
 * value snaps the slider, scrub, and typed-value commits to multiples of
 * `step` from `min`. Used when the wire takes a value but only certain
 * quantized values produce well-defined behavior — e.g. the shape
 * node's `frequency`, where only integer values yield a seam-free
 * closed silhouette. Frontend honors the snap; the engine should still
 * defend by quantizing inputs in the node evaluator (a wired-in float
 * from a curve or pen-pressure modulator bypasses the slider).
 */
step: number, 
/**
 * Human-readable description shown as a tooltip in the node editor.
 */
description: string, 
/**
 * Display unit for numeric ports (controls UI conversion and suffix).
 */
unit_type: UnitType, 
/**
 * Iconify icon name (e.g. `"fa6-solid:circle"`), or empty.
 */
icon: string, 
/**
 * User-facing display label.  Falls back to `name` if empty.
 */
label: string, 
/**
 * Whether this port is exposed in the brush properties panel.
 */
exposed: boolean, 
/**
 * Value substituted for this port in every "brush identity"
 * render: the cursor-following dab overlay, the editor stroke
 * preview, and the library thumbnail bake. The brush WGSL
 * compiler clones the graph, drops incoming wires on flagged
 * ports, and replaces `default` with this constant — so all
 * previews read as a showcase of the brush regardless of the
 * user's working scrub. Real strokes still honour the
 * configured value.
 *
 * Use when the port is something the user actively scrubs but
 * the preview must stay at a canonical value (otherwise the
 * preview becomes a moving target as the user dials in their
 * brush). The picker dab tile uses a more aggressive
 * neutralizer (`reset_exposed_scrubs`) that targets every
 * exposed scrub regardless of `preview_value`.
 *
 * Canonical example: `brush_settings.size` (0.1, so a huge
 * brush's preview still fits the small cursor mask and the editor
 * preview doesn't redraw on every size scrub).
 */
preview_value: number | null, 
/**
 * Declares that scrubbing this port's value does **not** change
 * what the synthetic-stroke editor preview produces, so the
 * preview cache and version counter should not bump on its scrub.
 *
 * Used by ports whose value the preview *pipeline* (not the
 * shader) ignores. Canonical example: `pen_input.stabilize` —
 * the editor preview's stroke engine is hard-wired to use
 * `PassThrough` as the stabilizer (the path is pre-cooked), so
 * the live `stabilize` value never reaches it. Marking this
 * declaratively avoids re-rendering a full stroke every ~100 ms
 * while the user drags the slider for no visible effect.
 *
 * Distinct from [`PortDef::preview_value`]: that one substitutes
 * values into the *cursor overlay shader*; this one skips a
 * version bump on the *editor stroke preview*. A port can carry
 * either, both, or neither.
 */
preview_irrelevant_scrub: boolean, 
/**
 * Conditional visibility: the port is only shown in the UI when the
 * value of the named param is one of the listed integer values. The
 * param is referenced by its registration name (e.g. `"algorithm"`)
 * and is expected to be an `Int`/`Enum` param — those are the only
 * types where dispatch on a discrete value makes sense.
 *
 * When `None` (the default), the port is always visible. When set,
 * the frontend hides the port row whenever the named param's current
 * value is outside the allowed list. This is purely a UI affordance —
 * the engine still accepts and reads the port's value normally; it
 * just stops showing the user a control they wouldn't act on.
 * Used by the Shape node to hide algorithm-specific knobs (Perlin's
 * `seed`, Superformula's `n1`/`n2`/`n3`) under the wrong algorithm.
 */
visible_when: [string, Array<number>] | null, 
/**
 * Wire-side natural value range. When a connection's source and dest
 * ports both declare this, the runner remaps the scalar value at
 * slot-read time from source range to dest range (affine transform).
 * When either side is `None`, the value passes through raw.
 *
 * Distinct from `min`/`max`, which are slider/UI hints — `with_range`
 * stays "UI hint only, not enforced", and `with_natural_range` is the
 * separate, explicit opt-in for wire-boundary range mapping. Most
 * ports declare both with the same numbers; the two diverge for
 * over-drag sliders like `brush_settings.size`, where the range is
 * a hint but the wire-side semantics are passthrough.
 */
natural_range: [number, number] | null, 
/**
 * Mark this exposed port as part of the brush's *identity* so its
 * user-set value persists into the dab thumbnail render.
 *
 * By default `crate::brush::reset_exposed_scrubs` resets every
 * exposed input back to its registration default before rendering
 * the dab thumbnail — the icon represents brush shape/texture, not
 * the user's working size/opacity/flow knobs. That policy is wrong
 * for orientation knobs (rotation): a calligraphy nib at
 * 45° *is* a different-looking brush, and the icon should reflect
 * that.
 *
 * When this flag is set: (1) the reset skips this port, and (2)
 * scrubbing this port bumps the topology version so the dab
 * thumbnail re-renders, not just the editor preview.
 */
persist_in_thumbnail: boolean, 
/**
 * This output port emits a *spatial, per-fragment image* — a coverage
 * mask or colour field that varies across the dab — so a node carrying it
 * is worth a preview thumbnail (`circle.mask`, `image.color`,
 * `noise.color`, `stamp.dab`). Declared per port rather than inferred
 * from `wire_type`, because wire type can't tell a spatial field from a
 * per-dab constant: `random.value` and `paint_color.color` share the
 * `Scalar`/`Vec4` types with the real image outputs but render as flat
 * blobs. The node-preview builder wires the first port carrying this flag;
 * the brush-builder's preview gate reads it directly (like `wirable` /
 * `exposable`). Meaningless on inputs; only set on outputs.
 */
preview_image: boolean, 
/**
 * This input port is *also* a wire source: its resolved value (the
 * wired value if driven, else the authored default) is available for
 * other nodes to wire *from*, exactly like an output. Only meaningful
 * on `dir == Input`; ignored on outputs (which are sources anyway).
 *
 * The editor shows the source handle only while the input is not
 * itself wire-driven — a driven port's value is the driver's, so it
 * should be tapped there instead. Consumers that resolve "which port a
 * wire leaves from" must ask [`PortDef::is_source`], never
 * `dir == Output`, or a settable-source is treated as a second-class
 * source (skipped by wire-range remap, unreachable by `find_port`).
 */
source: boolean, };

export type PreviewStaging = { 
/**
 * Iconify glyph shown in the dab slot, where a single stationary sample
 * has no motion to make the effect visible at all.
 */
icon: string, 
/**
 * Field painted under the stroke preview, giving the node something to
 * transport.
 */
backdrop: PreviewBackdrop, };

export type NodeRegistration = { 
/**
 * Unique identifier (e.g. "pen_input", "multiply").
 */
type_id: string, 
/**
 * UI category for the add-node palette — describes what the node *does*,
 * not how it executes. Current values: "input", "math", "modulate",
 * "color", "shape", "texture", "output". Nothing filters on it; every
 * registered node appears in the palette and in the catalog.
 */
category: string, 
/**
 * Human-readable name (e.g. "Pen Input", "Multiply").
 */
display_name: string, 
/**
 * Short, single-sentence description of what this node does — shown as
 * the add-node menu tooltip. Should read as a noun-phrase or imperative
 * fragment in painter vocabulary (never engine-internal terms like
 * "scalar" or "fragment shader"); per-port detail goes on the ports
 * themselves via `PortDef::with_description`.
 */
description: string, 
/**
 * Port definitions for this node type — the node's single, unified
 * input/output list. Every input carries its own authored value and
 * widget metadata on the [`PortDef`]; there is no separate parameter
 * system.
 */
ports: Array<PortDef>, 
/**
 * Whether this node requires GPU execution.
 */
is_gpu: boolean, 
/**
 * True for output terminals whose upstream graph fuses into a
 * compiled WGSL fragment shader. The dispatch walk in the runner
 * skips every upstream GPU node when one of these is present —
 * their contribution lives inside the terminal's compiled shader,
 * only the terminal itself runs to queue dabs and flush.
 */
is_terminal: boolean, 
/**
 * Whether this terminal honours erase mode (paint vs. erase).
 * Defaults `true`; smear/displace terminals that sample existing
 * pixels (smudge, watercolor, liquify) override to `false` so the
 * brush-tool options bar hides the erase toggle.
 */
supports_erase: boolean, 
/**
 * How a preview of any brush containing this node must be staged. Set by
 * nodes whose output depends on existing canvas content — over a flat
 * preview background they render blank, so the stroke gets a field to
 * transport and the dab slot gets a glyph. `None` for a node that makes
 * its own marks, which is every node that does not sample the canvas.
 */
preview_staging: PreviewStaging | null, };

export type BrushRenameReq = { id: string, name: string, };

export type BrushSaveReq = { id: string, name: string, };

export type BrushSetExposedPortReq = { node_id: string, port_name: string, display_value: number, };

export type BrushThumbnailReq = { name: string, };

export type BrushUploadImageReq = { resource_name: string, width: number, height: number, };

export type CanFlattenNodeReq = { node_id: number, };

export type CanMergeDownReq = { source_id: number, };

export type CanvasDimensionsResp = { width: number, height: number, };

export type CanvasRectResp = { origin_x: number, origin_y: number, width: number, height: number, };

export type CatalogEntry = { type: string, displayName: string, 
/**
 * Iconify name, or `None` when the variant deliberately declares no icon
 * (veils render a live preview; raster layers always show a thumbnail).
 */
icon: string | null, description: string | null, 
/**
 * Grouping label within the catalog, for variants that group.
 */
category: string | null, 
/**
 * Action id this variant is bound to, for variants a hotkey can select.
 */
hotkeyAction: string | null, params: Array<ParamInfo>, 
/**
 * Whether this variant declares a
 * [`PreviewAnim`](crate::gpu::preview::PreviewAnim) — the one fact behind
 * "a rendered preview of it exists". False for the registries whose entries
 * are affordances rather than images.
 *
 * It does **not** promise a *picker* preview. A blend mode declares one and
 * has a documentation asset, but is a relation between two images rather
 * than an effect over one, so its catalog exports no preview mechanism and
 * `start_preview` no-ops for it exactly as it does for an unknown type.
 * Whether a catalog can be driven live is
 * [`preview_mechanisms`](crate::catalog::preview_mechanisms)' answer, not
 * this field's.
 */
supportsPreview: boolean, 
/**
 * Where this variant's pixels come from; voids only. `None` for every
 * other registry, whose entries are effects over an existing image rather
 * than sources of one.
 */
source: VoidSource | null, };

export type ParamValue = boolean | number | number | string | Array<[number, number]> | [number, number, number, number, number] | [number, number, number] | [number, number] | Array<{ [key in string]: ParamValue }>;

export type ParamDisplay = { min: string | null, max: string | null, default: string | null, 
/**
 * The unit suffix alone, for a column header. Empty for unitless values.
 */
unit: string, };

export type ParamInfo = { kind: string, name: string, 
/**
 * Display label. `None` → the UI title-cases `name`.
 */
label: string | null, description: string | null, 
/**
 * How to render this parameter's editor. One closed set, which both
 * `ParamKind` and the settings schema's `WidgetHint` map into:
 * `"auto"`, `"numberInput"`, `"icon"`, `"hotkey"`, `"color"`, `"hidden"`.
 */
widget: string, unit: UnitType, min: number | null, max: number | null, default: ParamValue, value: ParamValue | null, 
/**
 * Enum: `["Label1", "Label2", ...]`.
 * Icon: `[["fa6-solid:icon-name", "Label"], ...]`.
 */
options: JsonValue | null, display: ParamDisplay, };

export type CaptureKind = "camera" | "display" | "stream";

export type VoidSource = { "kind": "procedural" } | { "kind": "capture", capture: CaptureKind, } | { "kind": "image" };

export type Catalog = { id: string, title: string, description: string | null, icon: string | null, 
/**
 * Presentation order, for catalogs that declare one. Registry catalogs do
 * not; settings sections do.
 */
order: number | null, entries: Array<CatalogEntry>, };

export type ClearSelectionContentsReq = { id: number, };

export type CommitFilterPreviewReq = { node_id: number, filter_type: string, params: JsonValue, };

export type CopyReq = { id: number, };

export type ClipboardExport = { rgba: Array<number>, width: number, height: number, offset_x: number, offset_y: number, };

export type CopyLayerRichReq = { id: number, };

export type CutReq = { id: number, };

export type DuplicateNodeReq = { source_id: number, };

export type DuplicateNodesReq = { ids: Array<number>, };

export type FeatherSelectionReq = { radius: number, };

export type FillBackgroundReq = { id: number, };

export type FillBackgroundColorReq = { id: number, rgba: [number, number, number, number], };

export type FlattenNodeReq = { node_id: number, };

export type FlipCanvasReq = { axis: FlipAxis, };

export type FlipAxis = "h" | "v";

export type FlipNodeReq = { node_id: number, xform: OrthoXform, };

export type OrthoXform = "flip_h" | "flip_v" | "rot180" | "rot90_cw" | "rot90_ccw";

export type FloatingInfoResp = { ox: number, oy: number, w: number, h: number, mode: number, matrix: Array<number>, };

export type FontAxesReq = { family: string, };

export type BrushCursorPreviewInfoResp = { halfExtent: [number, number], };

export type GroupLayersReq = { ids: Array<number>, };

export type GrowSelectionReq = { radius: number, };

export type HistogramReq = { id: number, };

export type HitTestVectorObjectReq = { id: number, x: number, y: number, };

export type LayerTransformCapabilityReq = { id: number, };

export type LayerInfo = { "type": "raster", id: number, name: string, visible: boolean, locked: boolean, 
/**
 * Effective editability — `false` when this node *or any ancestor*
 * carries `locked = true`. Mirrors `Document::is_node_editable`;
 * the UI consumes this directly to grey out controls so the
 * inheritance rule lives in one place (the document predicate)
 * rather than being recomputed by every Svelte component.
 */
editable: boolean, 
/**
 * Whether paint ops have somewhere to land on this node — mirrors
 * `DarklyEngine::is_node_paintable`. False for kinds whose pixels are
 * generated (void, filter, vector) and for groups; the panel reads it
 * to offer "Rasterize" instead of branching on `type`.
 */
paintable: boolean, canHaveMask: boolean, canRename: boolean, hasThumbnail: boolean, icon: string, kindName: string, opacity: number, 
/**
 * Stable `type_id` from the blend-mode registry (snake_case, e.g.
 * `"normal"`, `"color_burn"`). Resolve to a display label via the
 * blend-mode registry, not a sibling field on this struct.
 */
blendMode: string, 
/**
 * Filters attached to this layer (today: at most one mask).
 */
modifiers: Array<ModifierInfo>, 
/**
 * Pixel-space bounds of the layer's GPU texture in canvas coords.
 */
bounds: { origin: { x: number, y: number }, width: number, height: number }, } | { "type": "void", id: number, name: string, visible: boolean, locked: boolean, editable: boolean, 
/**
 * Whether paint ops have somewhere to land on this node — mirrors
 * `DarklyEngine::is_node_paintable`. False for kinds whose pixels are
 * generated (void, filter, vector) and for groups; the panel reads it
 * to offer "Rasterize" instead of branching on `type`.
 */
paintable: boolean, canHaveMask: boolean, canRename: boolean, hasThumbnail: boolean, 
/**
 * Iconify icon for this void kind (e.g. `"tabler:galaxy"`), resolved
 * per-subtype from the void's registration. The layer panel renders
 * it as the void layer's thumbnail.
 */
icon: string, kindName: string, opacity: number, blendMode: string, modifiers: Array<ModifierInfo>, 
/**
 * Stable `type_id` from the void registry — UI resolves to a
 * display label via `void_types()`.
 */
voidType: string, 
/**
 * Param schema + current values, in the order the void's
 * `ParamDef` slice declares them. Same shape the veil panel uses.
 */
params: Array<ParamInfo>, } | { "type": "filter", id: number, name: string, visible: boolean, locked: boolean, editable: boolean, 
/**
 * Whether paint ops have somewhere to land on this node — mirrors
 * `DarklyEngine::is_node_paintable`. False for kinds whose pixels are
 * generated (void, filter, vector) and for groups; the panel reads it
 * to offer "Rasterize" instead of branching on `type`.
 */
paintable: boolean, canHaveMask: boolean, canRename: boolean, hasThumbnail: boolean, icon: string, kindName: string, opacity: number, blendMode: string, modifiers: Array<ModifierInfo>, 
/**
 * Stable filter `type_id` (e.g. `"invert"`) — UI resolves to a
 * display label via `filter_types()`.
 */
pipeline: string, 
/**
 * Param schema + current values, in the order the filter's `ParamDef`
 * slice declares them. Empty for parameter-free filters (invert);
 * carries the five tone curves for `curves`. Same shape the void panel
 * uses.
 */
params: Array<ParamInfo>, } | { "type": "vector", id: number, name: string, visible: boolean, locked: boolean, editable: boolean, 
/**
 * Whether paint ops have somewhere to land on this node — mirrors
 * `DarklyEngine::is_node_paintable`. False for kinds whose pixels are
 * generated (void, filter, vector) and for groups; the panel reads it
 * to offer "Rasterize" instead of branching on `type`.
 */
paintable: boolean, canHaveMask: boolean, canRename: boolean, hasThumbnail: boolean, icon: string, kindName: string, opacity: number, blendMode: string, modifiers: Array<ModifierInfo>, } | { "type": "group", id: number, name: string, visible: boolean, locked: boolean, editable: boolean, 
/**
 * Whether paint ops have somewhere to land on this node — mirrors
 * `DarklyEngine::is_node_paintable`. False for kinds whose pixels are
 * generated (void, filter, vector) and for groups; the panel reads it
 * to offer "Rasterize" instead of branching on `type`.
 */
paintable: boolean, canHaveMask: boolean, canRename: boolean, hasThumbnail: boolean, icon: string, kindName: string, collapsed: boolean, passthrough: boolean, opacity: number, blendMode: string, modifiers: Array<ModifierInfo>, children: Array<LayerInfo>, };

export type ModifierInfo = { id: number, kind: string, name: string, visible: boolean, locked: boolean, 
/**
 * Whether this modifier participates in transforms with its host.
 */
linkedToHost: boolean, 
/**
 * See [`LayerInfo::Raster::editable`] — a modifier is editable when
 * neither it nor its host (nor any ancestor of the host) is locked.
 */
editable: boolean, };

export type LibrarySnapshot = { brushes: Array<BrushInfo>, packs: Array<BrushPackInfo>, };

export type PackPalette = { 
/**
 * The pack's own hue at full vividness — the color you would name it by.
 */
chroma: string, 
/**
 * The same light bent: a near neighbour in hue, equally vivid. Drawn with
 * `chroma` as a gradient, never alone.
 */
refraction: string, 
/**
 * The body the glass sits on — light or dark, the pack's own choice, low in
 * saturation. Carries value, not color. Alpha here lets the background
 * behind it show through.
 */
surface: string, };

export type BrushPackInfo = { id: string, name: string, description: string, icon: string, palette: PackPalette, 
/**
 * Member brush ids, in the pack's order. The authority on membership —
 * nothing on [`BrushInfo`] repeats it.
 */
members: Array<string>, 
/**
 * What the painter may change, so the UI can grey out affordances it
 * would otherwise offer. A hint, not the authority — the engine rejects a
 * forbidden edit regardless of what the UI believed.
 */
can_edit_members: boolean, can_edit_identity: boolean, };

export type MaskToSelectionReq = { id: number, };

export type MergeDownReq = { source_id: number, };

export type MergeLayersReq = { ids: Array<number>, };

export type MoveLayerReq = { id: number, target: MoveTarget, };

export type MoveTarget = { "target_type": "before", "target_id": number } | { "target_type": "after", "target_id": number } | { "target_type": "into_top", "target_id": number } | { "target_type": "into_bottom", "target_id": number };

export type MoveLayersReq = { ids: Array<number>, target: MoveTarget, };

export type MoveVeilReq = { from: number, to: number, };

export type NodeThumbnailReq = { node_id: number, width: number, height: number, };

export type OverlayHitTestReq = { screen_x: number, screen_y: number, };

export type PackAddBrushReq = { pack: string, brush: string, };

export type PackCreateReq = { id: string, name: string, description: string, icon: string, palette: PackPalette, };

export type PackDeleteReq = { id: string, };

export type PackEditReq = { id: string, name: string, description: string, icon: string, palette: PackPalette, };

export type PackExportReq = { id: string, };

export type PackImportReq = { id: string, };

export type PackRemoveBrushReq = { pack: string, brush: string, };

export type PackReorderBrushReq = { pack: string, brush: string, index: number, };

export type PasteImageReq = { width: number, height: number, offset_x: number, offset_y: number, active_layer_id: number, };

export type PasteResultResp = { id: number, };

export type PasteInPlaceReq = { active_layer_id: number, };

export type PasteInPlaceFloatingReq = { id: number, };

export type PasteLayerRichReq = { json: string, active_layer_id: number, };

export type PickColorReq = { x: number, y: number, id: number, };

export type PlaceSmartObjectReq = { width: number, height: number, active_layer_id: number, };

export type PreviewReq = { catalog: string, type: string, variant: PreviewVariant, };

export type PreviewVariant = "still" | "animated";

export type PreviewFilterReq = { node_id: number, filter_type: string, params: JsonValue, };

export type RefreshBrushCursorPreviewReq = { x: number, y: number, pressure: number, tilt_x: number, tilt_y: number, rotation: number, tangential_pressure: number, };

export type RemoveLayerReq = { id: number, };

export type RemoveLayersReq = { ids: Array<number>, };

export type RemoveMaskReq = { id: number, };

export type RemoveVeilReq = { index: number, };

export type RescaleImageReq = { new_width: number, new_height: number, };

export type ResizeReq = { width: number, height: number, };

export type ResizeCanvasRectReq = { origin_x: number, origin_y: number, w: number, h: number, };

export type RotateCanvasReq = { dir: RotateDir, };

export type RotateDir = "cw" | "ccw" | "180";

export type SelectEllipseReq = { x: number, y: number, w: number, h: number, mode: SelectionMode, antialias: boolean, feather: number, };

export type SelectionMode = "replace" | "add" | "subtract" | "intersect";

export type SelectLassoReq = { verts: Array<[number, number]>, mode: SelectionMode, antialias: boolean, feather: number, };

export type SelectMagicWandReq = { id: number, seed_canvas: { x: number, y: number }, tolerance: number, mode: SelectionMode, };

export type SelectRectReq = { x: number, y: number, w: number, h: number, mode: SelectionMode, antialias: boolean, feather: number, };

export type SelectionToMaskReq = { id: number, };

export type SetBlendModeReq = { id: number, type_id: string, };

export type SetBrushBlendModeReq = { mode: number, };

export type SetOverlayReq = { primitives: Array<PrimIn>, };

export type PrimIn = { kind: number, flags: number, p0: [number, number], p1: [number, number], color: [number, number, number, number], thickness: number, dashLen: number, dashOffset: number, cornerRadius: number, modeParam: number, rotation: number, };

export type SetCloneSourceReq = { x: number, y: number, layer: number | null, };

export type SetDocumentNameReq = { name: string, };

export type SetFilterParamsReq = { id: number, params: JsonValue, };

export type SetGroupCollapsedReq = { id: number, collapsed: boolean, };

export type SetGroupPassthroughReq = { id: number, passthrough: boolean, };

export type SetIsolatedNodeReq = { id: number | null, };

export type SetLayerNameReq = { id: number, name: string, };

export type SetLayerVisibleReq = { id: number, visible: boolean, };

export type SetMaskLinkedToHostReq = { id: number, linked: boolean, };

export type SetNodeLockedReq = { id: number, locked: boolean, };

export type SetOpacityReq = { id: number, opacity: number, };

export type SetOverlayMaskReq = { width: number, height: number, rgba: Array<number>, };

export type SetPixelFilterReq = { mode: string, };

export type SetPreviewThemeReq = { fg: [number, number, number, number], bg: [number, number, number, number], };

export type SetRecordingParamsReq = { enabled: boolean, minIntervalSecs: number, width: number, height: number, baseWidth: number, baseHeight: number, };

export type SetTextBoxReq = { id: number, object: number, 
/**
 * Full canvas affine `G` (row-major) for the box's moved origin; the engine
 * strips the layer transform.
 */
matrix: [number, number, number, number, number, number], 
/**
 * Box size `[w, h]` in canvas pixels.
 */
box: [number, number], };

export type SetTextContentReq = { id: number, object: number, content: string, };

export type SetTextStyleReq = { id: number, object: number, font_family?: string, size?: number, 
/**
 * Axis values to **merge** (tag → value); the rest are kept.
 */
variations?: { [key in string]: number }, features?: { [key in string]: number }, letter_spacing?: number, word_spacing?: number, line_height?: number, italic?: boolean, align?: string, color?: [number, number, number, number], };

export type SetVeilVisibleReq = { index: number, visible: boolean, };

export type SetViewTransformReq = { pan_x: number, pan_y: number, zoom: number, rotation: number, mirror_h: boolean, screen_w: number, screen_h: number, };

export type SetViewportBgReq = { bg: [number, number, number, number], };

export type SetVoidParamsReq = { id: number, params: JsonValue, };

export type ShrinkSelectionReq = { radius: number, };

export type SmoothSelectionReq = { radius: number, };

export type StartSaveDocumentReq = { snapshot: boolean, };

export type StrokeToReq = { op: StrokeOp, };

export type StrokeOp = { "op": "flood_fill", x: number, y: number, r: number, g: number, b: number, a: number, tolerance: number, } | { "op": "linear_gradient", x0: number, y0: number, x1: number, y1: number, r0: number, g0: number, b0: number, a0: number, r1: number, g1: number, b1: number, a1: number, } | { "op": "brush_stroke", x: number, y: number, pressure: number, x_tilt: number, y_tilt: number, rotation: number, tangential_pressure: number, time_ms: number, 
/**
 * Foreground color as raw sRGB RGBA floats (0-1), as picked — the
 * compositor is display-referred, so no gamma conversion is applied.
 */
cr: number, cg: number, cb: number, ca: number, };

export type PixelTransformOperation = "destructive_transform";

export type TransformCapabilityError = { endpoint: number, operation: PixelTransformOperation, };

export type LayerIdReq = { id: number, };

export type UpdateFloatingMatrixReq = { transform: Transform, };

export type Transform = { "mode": "Basic", "data": [number, number, number, number, number, number] } | { "mode": "Perspective", "data": [number, number, number, number, number, number, number, number, number] };

export type UpdateVectorObjectTransformReq = { id: number, object: number, payload: Array<number>, };

export type UpdateVeilReq = { index: number, params: JsonValue, };

export type UpdateVoidTransformReq = { id: number, transform: Transform, };

export type ObjectRefReq = { id: number, object: number, };

export type VeilInfo = { type: string, visible: boolean, index: number, params: Array<ParamInfo>, };

export type VoidTransformInfoReq = { id: number, };

export type VoidTransformInfoResp = { ox: number, oy: number, w: number, h: number, mode: number, matrix: Array<number>, };

export type RequestKind =
    | 'active_brush_needs_source'
    | 'add_filter'
    | 'add_group'
    | 'add_mask'
    | 'add_raster'
    | 'add_text'
    | 'add_text_object'
    | 'add_veil'
    | 'add_void'
    | 'alpha_to_selection'
    | 'antialias_selection'
    | 'apply_filter'
    | 'apply_mask'
    | 'begin_stroke'
    | 'begin_transform'
    | 'border_selection'
    | 'brush_active_capabilities'
    | 'brush_active_dab_preview'
    | 'brush_dab_thumbnail'
    | 'brush_delete'
    | 'brush_export_yaml'
    | 'brush_exposed_ports'
    | 'brush_graph_active'
    | 'brush_graph_add_node'
    | 'brush_graph_auto_layout'
    | 'brush_graph_compile'
    | 'brush_graph_connect'
    | 'brush_graph_default'
    | 'brush_graph_disconnect'
    | 'brush_graph_export_yaml'
    | 'brush_graph_expose_port'
    | 'brush_graph_import_yaml'
    | 'brush_graph_remove_node'
    | 'brush_graph_reorder_exposed_port'
    | 'brush_graph_reset'
    | 'brush_graph_set_exposed_port_meta'
    | 'brush_graph_set_input'
    | 'brush_graph_set_node_comment'
    | 'brush_graph_set_port_range'
    | 'brush_graph_unexpose_port'
    | 'brush_graph_validate'
    | 'brush_list'
    | 'brush_load'
    | 'brush_node_preview'
    | 'brush_node_types'
    | 'brush_rename'
    | 'brush_save'
    | 'brush_set_exposed_port'
    | 'brush_stroke_preview'
    | 'brush_thumbnail'
    | 'brush_topology_version'
    | 'brush_upload_image'
    | 'can_flatten'
    | 'can_flatten_node'
    | 'can_merge_down'
    | 'cancel_filter_preview'
    | 'cancel_floating'
    | 'canvas_dimensions'
    | 'canvas_rect'
    | 'catalogs'
    | 'clear_brush_cursor_preview_pose'
    | 'clear_clone_overlay'
    | 'clear_overlay'
    | 'clear_overlay_mask'
    | 'clear_selection'
    | 'clear_selection_contents'
    | 'clear_veils'
    | 'clone_source_anchored'
    | 'commit_filter_preview'
    | 'commit_floating'
    | 'copy'
    | 'copy_layer_rich'
    | 'crop_to_selection'
    | 'cut'
    | 'document_name'
    | 'duplicate_node'
    | 'duplicate_nodes'
    | 'end_stroke'
    | 'feather_selection'
    | 'fill_background'
    | 'fill_background_color'
    | 'flatten_image'
    | 'flatten_node'
    | 'flip_canvas'
    | 'flip_node'
    | 'floating_info'
    | 'floating_target_layer'
    | 'font_axes'
    | 'get_brush_cursor_preview_info'
    | 'group_layers'
    | 'grow_selection'
    | 'has_floating'
    | 'has_pending_color_pick'
    | 'has_selection'
    | 'histogram_result'
    | 'hit_test_vector_object'
    | 'invert_selection'
    | 'is_dirty'
    | 'last_picked_color'
    | 'layer_transform_capability'
    | 'layer_tree'
    | 'library_list'
    | 'list_fonts'
    | 'mark_dirty'
    | 'mask_to_selection'
    | 'merge_down'
    | 'merge_layers'
    | 'move_layer'
    | 'move_layers'
    | 'move_veil'
    | 'node_thumbnail'
    | 'open_document'
    | 'overlay_hit_test'
    | 'pack_add_brush'
    | 'pack_create'
    | 'pack_delete'
    | 'pack_edit'
    | 'pack_export'
    | 'pack_import'
    | 'pack_remove_brush'
    | 'pack_reorder_brush'
    | 'paste_image'
    | 'paste_image_floating'
    | 'paste_in_place'
    | 'paste_in_place_floating'
    | 'paste_layer_rich'
    | 'pick_color'
    | 'place_smart_object'
    | 'poll_copy_result'
    | 'poll_copy_rich_result'
    | 'poll_export_result'
    | 'poll_preview'
    | 'poll_recording_frame'
    | 'poll_save_result'
    | 'preview_filter'
    | 'redo'
    | 'refresh_brush_cursor_preview'
    | 'register_font'
    | 'remove_layer'
    | 'remove_layers'
    | 'remove_mask'
    | 'remove_veil'
    | 'request_histogram'
    | 'request_node_histogram'
    | 'request_recording_capture'
    | 'rescale_image'
    | 'resize'
    | 'resize_canvas_rect'
    | 'rotate_canvas'
    | 'select_all'
    | 'select_ellipse'
    | 'select_lasso'
    | 'select_magic_wand'
    | 'select_rect'
    | 'selection_to_mask'
    | 'set_blend_mode'
    | 'set_brush_blend_mode'
    | 'set_clone_overlay'
    | 'set_clone_source'
    | 'set_document_name'
    | 'set_filter_params'
    | 'set_group_collapsed'
    | 'set_group_passthrough'
    | 'set_isolated_node'
    | 'set_layer_name'
    | 'set_layer_visible'
    | 'set_mask_linked_to_host'
    | 'set_node_locked'
    | 'set_opacity'
    | 'set_overlay'
    | 'set_overlay_mask'
    | 'set_pixel_filter'
    | 'set_preview_theme'
    | 'set_recording_params'
    | 'set_text_box'
    | 'set_text_content'
    | 'set_text_style'
    | 'set_veil_visible'
    | 'set_view_transform'
    | 'set_viewport_bg'
    | 'set_void_params'
    | 'shrink_selection'
    | 'smooth_selection'
    | 'start_export'
    | 'start_preview'
    | 'start_save_document'
    | 'stroke_to'
    | 'take_transform_setup_error'
    | 'text_objects'
    | 'undo'
    | 'update_floating_matrix'
    | 'update_vector_object_transform'
    | 'update_veil'
    | 'update_void_transform'
    | 'vector_object_info'
    | 'veil_list'
    | 'void_transform_info'
    | 'warm_vector_renderer'
    ;

export const REQUEST_KINDS: readonly RequestKind[] = [
    'active_brush_needs_source',
    'add_filter',
    'add_group',
    'add_mask',
    'add_raster',
    'add_text',
    'add_text_object',
    'add_veil',
    'add_void',
    'alpha_to_selection',
    'antialias_selection',
    'apply_filter',
    'apply_mask',
    'begin_stroke',
    'begin_transform',
    'border_selection',
    'brush_active_capabilities',
    'brush_active_dab_preview',
    'brush_dab_thumbnail',
    'brush_delete',
    'brush_export_yaml',
    'brush_exposed_ports',
    'brush_graph_active',
    'brush_graph_add_node',
    'brush_graph_auto_layout',
    'brush_graph_compile',
    'brush_graph_connect',
    'brush_graph_default',
    'brush_graph_disconnect',
    'brush_graph_export_yaml',
    'brush_graph_expose_port',
    'brush_graph_import_yaml',
    'brush_graph_remove_node',
    'brush_graph_reorder_exposed_port',
    'brush_graph_reset',
    'brush_graph_set_exposed_port_meta',
    'brush_graph_set_input',
    'brush_graph_set_node_comment',
    'brush_graph_set_port_range',
    'brush_graph_unexpose_port',
    'brush_graph_validate',
    'brush_list',
    'brush_load',
    'brush_node_preview',
    'brush_node_types',
    'brush_rename',
    'brush_save',
    'brush_set_exposed_port',
    'brush_stroke_preview',
    'brush_thumbnail',
    'brush_topology_version',
    'brush_upload_image',
    'can_flatten',
    'can_flatten_node',
    'can_merge_down',
    'cancel_filter_preview',
    'cancel_floating',
    'canvas_dimensions',
    'canvas_rect',
    'catalogs',
    'clear_brush_cursor_preview_pose',
    'clear_clone_overlay',
    'clear_overlay',
    'clear_overlay_mask',
    'clear_selection',
    'clear_selection_contents',
    'clear_veils',
    'clone_source_anchored',
    'commit_filter_preview',
    'commit_floating',
    'copy',
    'copy_layer_rich',
    'crop_to_selection',
    'cut',
    'document_name',
    'duplicate_node',
    'duplicate_nodes',
    'end_stroke',
    'feather_selection',
    'fill_background',
    'fill_background_color',
    'flatten_image',
    'flatten_node',
    'flip_canvas',
    'flip_node',
    'floating_info',
    'floating_target_layer',
    'font_axes',
    'get_brush_cursor_preview_info',
    'group_layers',
    'grow_selection',
    'has_floating',
    'has_pending_color_pick',
    'has_selection',
    'histogram_result',
    'hit_test_vector_object',
    'invert_selection',
    'is_dirty',
    'last_picked_color',
    'layer_transform_capability',
    'layer_tree',
    'library_list',
    'list_fonts',
    'mark_dirty',
    'mask_to_selection',
    'merge_down',
    'merge_layers',
    'move_layer',
    'move_layers',
    'move_veil',
    'node_thumbnail',
    'open_document',
    'overlay_hit_test',
    'pack_add_brush',
    'pack_create',
    'pack_delete',
    'pack_edit',
    'pack_export',
    'pack_import',
    'pack_remove_brush',
    'pack_reorder_brush',
    'paste_image',
    'paste_image_floating',
    'paste_in_place',
    'paste_in_place_floating',
    'paste_layer_rich',
    'pick_color',
    'place_smart_object',
    'poll_copy_result',
    'poll_copy_rich_result',
    'poll_export_result',
    'poll_preview',
    'poll_recording_frame',
    'poll_save_result',
    'preview_filter',
    'redo',
    'refresh_brush_cursor_preview',
    'register_font',
    'remove_layer',
    'remove_layers',
    'remove_mask',
    'remove_veil',
    'request_histogram',
    'request_node_histogram',
    'request_recording_capture',
    'rescale_image',
    'resize',
    'resize_canvas_rect',
    'rotate_canvas',
    'select_all',
    'select_ellipse',
    'select_lasso',
    'select_magic_wand',
    'select_rect',
    'selection_to_mask',
    'set_blend_mode',
    'set_brush_blend_mode',
    'set_clone_overlay',
    'set_clone_source',
    'set_document_name',
    'set_filter_params',
    'set_group_collapsed',
    'set_group_passthrough',
    'set_isolated_node',
    'set_layer_name',
    'set_layer_visible',
    'set_mask_linked_to_host',
    'set_node_locked',
    'set_opacity',
    'set_overlay',
    'set_overlay_mask',
    'set_pixel_filter',
    'set_preview_theme',
    'set_recording_params',
    'set_text_box',
    'set_text_content',
    'set_text_style',
    'set_veil_visible',
    'set_view_transform',
    'set_viewport_bg',
    'set_void_params',
    'shrink_selection',
    'smooth_selection',
    'start_export',
    'start_preview',
    'start_save_document',
    'stroke_to',
    'take_transform_setup_error',
    'text_objects',
    'undo',
    'update_floating_matrix',
    'update_vector_object_transform',
    'update_veil',
    'update_void_transform',
    'vector_object_info',
    'veil_list',
    'void_transform_info',
    'warm_vector_renderer',
] as const;

/** The request boundary the generated client closes over. `request`
 *  awaits a typed response; `postFF` fires and forgets. */
export interface Transport {
    request(kind: RequestKind, payload?: object, bytes?: Uint8Array): Promise<any>;
    postFF(kind: RequestKind, payload?: object, bytes?: Uint8Array): void;
}

/** Typed, per-kind engine surface. */
export interface EngineApi {
    activeBrushNeedsSource(): Promise<boolean>;
    addFilter(req: AddFilterReq): Promise<number | null>;
    addGroup(req: AddGroupReq): Promise<number>;
    addMask(req: AddMaskReq): void;
    addRaster(req: AddRasterReq): Promise<number>;
    addText(req: AddTextReq): Promise<{ id: number, object: number }>;
    addTextObject(req: AddTextObjectReq): Promise<{ object: number }>;
    addVeil(req: AddVeilReq): void;
    addVoid(req: AddVoidReq): Promise<number | null>;
    alphaToSelection(req: AlphaToSelectionReq): void;
    antialiasSelection(): void;
    applyFilter(req: ApplyFilterReq): Promise<boolean>;
    applyMask(req: ApplyMaskReq): void;
    beginStroke(req: BeginStrokeReq): Promise<null>;
    beginTransform(req: BeginTransformReq): Promise<boolean>;
    borderSelection(req: BorderSelectionReq): void;
    brushActiveCapabilities(): Promise<BrushGraphCapabilities>;
    brushActiveDabPreview(): Promise<{ bytes: Uint8Array }>;
    brushDabThumbnail(req: BrushDabThumbnailReq): Promise<{ bytes: Uint8Array }>;
    brushDelete(req: BrushDeleteReq): Promise<null>;
    brushExportYaml(req: BrushExportYamlReq): Promise<string>;
    brushExposedPorts(): Promise<Array<ExposedPortInfo>>;
    brushGraphActive(): Promise<JsonValue>;
    brushGraphAddNode(req: BrushGraphAddNodeReq): Promise<{ graph: JsonValue, added_node_id: string } | { error: string }>;
    brushGraphAutoLayout(req: BrushGraphAutoLayoutReq): Promise<Record<string, [number, number]>>;
    brushGraphCompile(req: BrushGraphJsonReq): Promise<null | { error: string }>;
    brushGraphConnect(req: BrushGraphConnectReq): Promise<{ graph: JsonValue } | { error: string }>;
    brushGraphDefault(): Promise<JsonValue>;
    brushGraphDisconnect(req: BrushGraphDisconnectReq): Promise<{ graph: JsonValue } | { error: string }>;
    brushGraphExportYaml(): Promise<{ yaml: string }>;
    brushGraphExposePort(req: BrushGraphExposePortReq): Promise<{ graph: JsonValue } | { error: string }>;
    brushGraphImportYaml(req: BrushGraphYamlReq): Promise<null | { error: string }>;
    brushGraphRemoveNode(req: BrushGraphRemoveNodeReq): Promise<{ graph: JsonValue } | { error: string }>;
    brushGraphReorderExposedPort(req: BrushGraphReorderExposedPortReq): Promise<{ graph: JsonValue } | { error: string }>;
    brushGraphReset(): void;
    brushGraphSetExposedPortMeta(req: BrushGraphSetExposedPortMetaReq): Promise<{ graph: JsonValue } | { error: string }>;
    brushGraphSetInput(req: BrushGraphSetInputReq): Promise<{ graph: JsonValue } | { error: string }>;
    brushGraphSetNodeComment(req: BrushGraphSetNodeCommentReq): Promise<{ graph: JsonValue } | { error: string }>;
    brushGraphSetPortRange(req: BrushGraphSetPortRangeReq): Promise<{ graph: JsonValue } | { error: string }>;
    brushGraphUnexposePort(req: BrushGraphUnexposePortReq): Promise<{ graph: JsonValue } | { error: string }>;
    brushGraphValidate(req: BrushGraphJsonReq): Promise<null | { error: string }>;
    brushList(): Promise<Array<BrushInfo>>;
    brushLoad(req: BrushLoadReq): Promise<null>;
    brushNodePreview(req: BrushNodePreviewReq): Promise<{ bytes: Uint8Array }>;
    brushNodeTypes(): Promise<Array<NodeRegistration>>;
    brushRename(req: BrushRenameReq): Promise<null>;
    brushSave(req: BrushSaveReq): Promise<null>;
    brushSetExposedPort(req: BrushSetExposedPortReq): Promise<{ graph: JsonValue } | { error: string }>;
    brushStrokePreview(): Promise<{ bytes: Uint8Array }>;
    brushThumbnail(req: BrushThumbnailReq): Promise<{ bytes: Uint8Array }>;
    brushTopologyVersion(): Promise<{ value: number }>;
    brushUploadImage(req: BrushUploadImageReq, bytes: Uint8Array): Promise<void>;
    canFlatten(): Promise<boolean>;
    canFlattenNode(req: CanFlattenNodeReq): Promise<boolean>;
    canMergeDown(req: CanMergeDownReq): Promise<boolean>;
    cancelFilterPreview(): void;
    cancelFloating(): void;
    canvasDimensions(): Promise<CanvasDimensionsResp>;
    canvasRect(): Promise<CanvasRectResp>;
    catalogs(): Promise<Array<Catalog>>;
    clearBrushCursorPreviewPose(): void;
    clearCloneOverlay(): void;
    clearOverlay(): void;
    clearOverlayMask(): void;
    clearSelection(): void;
    clearSelectionContents(req: ClearSelectionContentsReq): void;
    clearVeils(): void;
    cloneSourceAnchored(): Promise<boolean>;
    commitFilterPreview(req: CommitFilterPreviewReq): Promise<boolean>;
    commitFloating(): void;
    copy(req: CopyReq): Promise<ClipboardExport | null>;
    copyLayerRich(req: CopyLayerRichReq): void;
    cropToSelection(): void;
    cut(req: CutReq): Promise<ClipboardExport | null>;
    documentName(): Promise<string>;
    duplicateNode(req: DuplicateNodeReq): Promise<number | null>;
    duplicateNodes(req: DuplicateNodesReq): Promise<Array<number>>;
    endStroke(): void;
    featherSelection(req: FeatherSelectionReq): void;
    fillBackground(req: FillBackgroundReq): void;
    fillBackgroundColor(req: FillBackgroundColorReq): void;
    flattenImage(): Promise<number>;
    flattenNode(req: FlattenNodeReq): Promise<number>;
    flipCanvas(req: FlipCanvasReq): void;
    flipNode(req: FlipNodeReq): Promise<boolean>;
    floatingInfo(): Promise<FloatingInfoResp | null>;
    floatingTargetLayer(): Promise<number | null>;
    fontAxes(req: FontAxesReq): Promise<{ italic: boolean, axes: Array<{ tag: string, min: number, default: number, max: number }> }>;
    getBrushCursorPreviewInfo(): Promise<BrushCursorPreviewInfoResp | null>;
    groupLayers(req: GroupLayersReq): Promise<number>;
    growSelection(req: GrowSelectionReq): void;
    hasFloating(): Promise<boolean>;
    hasPendingColorPick(): Promise<boolean>;
    hasSelection(): Promise<boolean>;
    histogramResult(req: HistogramReq): Promise<{ bytes: Uint8Array }>;
    hitTestVectorObject(req: HitTestVectorObjectReq): Promise<{ object: number }>;
    invertSelection(): void;
    isDirty(): Promise<boolean>;
    lastPickedColor(): Promise<{ bytes: Uint8Array }>;
    layerTransformCapability(req: LayerTransformCapabilityReq): Promise<string>;
    layerTree(): Promise<Array<LayerInfo>>;
    libraryList(): Promise<LibrarySnapshot>;
    listFonts(): Promise<{ fonts: string[] }>;
    markDirty(): void;
    maskToSelection(req: MaskToSelectionReq): void;
    mergeDown(req: MergeDownReq): Promise<number>;
    mergeLayers(req: MergeLayersReq): Promise<number>;
    moveLayer(req: MoveLayerReq): void;
    moveLayers(req: MoveLayersReq): Promise<number>;
    moveVeil(req: MoveVeilReq): void;
    nodeThumbnail(req: NodeThumbnailReq): Promise<{ bytes: Uint8Array }>;
    openDocument(bytes: Uint8Array): Promise<void>;
    overlayHitTest(req: OverlayHitTestReq): Promise<number | null>;
    packAddBrush(req: PackAddBrushReq): Promise<null>;
    packCreate(req: PackCreateReq): Promise<null>;
    packDelete(req: PackDeleteReq): Promise<null>;
    packEdit(req: PackEditReq): Promise<null>;
    packExport(req: PackExportReq): Promise<{ bytes: Uint8Array }>;
    packImport(req: PackImportReq, bytes: Uint8Array): Promise<string>;
    packRemoveBrush(req: PackRemoveBrushReq): Promise<null>;
    packReorderBrush(req: PackReorderBrushReq): Promise<null>;
    pasteImage(req: PasteImageReq, bytes: Uint8Array): Promise<PasteResultResp>;
    pasteImageFloating(req: PasteImageReq, bytes: Uint8Array): Promise<PasteResultResp>;
    pasteInPlace(req: PasteInPlaceReq): Promise<PasteResultResp>;
    pasteInPlaceFloating(req: PasteInPlaceFloatingReq): Promise<boolean>;
    pasteLayerRich(req: PasteLayerRichReq): Promise<PasteResultResp>;
    pickColor(req: PickColorReq): void;
    placeSmartObject(req: PlaceSmartObjectReq, bytes: Uint8Array): Promise<{ id: number }>;
    pollCopyResult(): Promise<ClipboardExport | null>;
    pollCopyRichResult(): Promise<string | null>;
    pollExportResult(): Promise<{ width: number, height: number, bytes: Uint8Array } | null>;
    pollPreview(req: PreviewReq): Promise<{ width: number, height: number, fps: number, frameCount: number, bytes: Uint8Array } | null>;
    pollRecordingFrame(): Promise<{ canvasWidth: number, canvasHeight: number, frame: { width: number, height: number, frameIndex: number } | null, bytes?: Uint8Array }>;
    pollSaveResult(): Promise<{ manifestLen: number, compositeWidth: number, compositeHeight: number, compositeLen: number, blobs: { path: string, len: number }[], bytes: Uint8Array } | null>;
    previewFilter(req: PreviewFilterReq): Promise<boolean>;
    redo(): void;
    refreshBrushCursorPreview(req: RefreshBrushCursorPreviewReq): Promise<BrushCursorPreviewInfoResp | null>;
    registerFont(bytes: Uint8Array): Promise<{ families: string[] }>;
    removeLayer(req: RemoveLayerReq): Promise<null>;
    removeLayers(req: RemoveLayersReq): Promise<number>;
    removeMask(req: RemoveMaskReq): void;
    removeVeil(req: RemoveVeilReq): void;
    requestHistogram(req: HistogramReq): void;
    requestNodeHistogram(req: HistogramReq): void;
    requestRecordingCapture(): void;
    rescaleImage(req: RescaleImageReq): void;
    resize(req: ResizeReq): void;
    resizeCanvasRect(req: ResizeCanvasRectReq): void;
    rotateCanvas(req: RotateCanvasReq): void;
    selectAll(): void;
    selectEllipse(req: SelectEllipseReq): void;
    selectLasso(req: SelectLassoReq): void;
    selectMagicWand(req: SelectMagicWandReq): void;
    selectRect(req: SelectRectReq): void;
    selectionToMask(req: SelectionToMaskReq): void;
    setBlendMode(req: SetBlendModeReq): void;
    setBrushBlendMode(req: SetBrushBlendModeReq): void;
    setCloneOverlay(req: SetOverlayReq): void;
    setCloneSource(req: SetCloneSourceReq): void;
    setDocumentName(req: SetDocumentNameReq): void;
    setFilterParams(req: SetFilterParamsReq): void;
    setGroupCollapsed(req: SetGroupCollapsedReq): void;
    setGroupPassthrough(req: SetGroupPassthroughReq): void;
    setIsolatedNode(req: SetIsolatedNodeReq): Promise<number | null>;
    setLayerName(req: SetLayerNameReq): void;
    setLayerVisible(req: SetLayerVisibleReq): void;
    setMaskLinkedToHost(req: SetMaskLinkedToHostReq): void;
    setNodeLocked(req: SetNodeLockedReq): void;
    setOpacity(req: SetOpacityReq): void;
    setOverlay(req: SetOverlayReq): void;
    setOverlayMask(req: SetOverlayMaskReq): void;
    setPixelFilter(req: SetPixelFilterReq): void;
    setPreviewTheme(req: SetPreviewThemeReq): void;
    setRecordingParams(req: SetRecordingParamsReq): void;
    setTextBox(req: SetTextBoxReq): void;
    setTextContent(req: SetTextContentReq): void;
    setTextStyle(req: SetTextStyleReq): void;
    setVeilVisible(req: SetVeilVisibleReq): void;
    setViewTransform(req: SetViewTransformReq): void;
    setViewportBg(req: SetViewportBgReq): void;
    setVoidParams(req: SetVoidParamsReq): void;
    shrinkSelection(req: ShrinkSelectionReq): void;
    smoothSelection(req: SmoothSelectionReq): void;
    startExport(): void;
    startPreview(req: PreviewReq): void;
    startSaveDocument(req: StartSaveDocumentReq): Promise<void>;
    strokeTo(req: StrokeToReq): void;
    takeTransformSetupError(): Promise<TransformCapabilityError | null>;
    textObjects(req: LayerIdReq): Promise<{ objects: Array<{ object: number, content: string, font_family: string, size: number, variations: Record<string, number>, features: Record<string, number>, letter_spacing: number, word_spacing: number, line_height: number, italic: boolean, align: string, color: [number, number, number, number], box: [number, number] | null }> }>;
    undo(): void;
    updateFloatingMatrix(req: UpdateFloatingMatrixReq): void;
    updateVectorObjectTransform(req: UpdateVectorObjectTransformReq): void;
    updateVeil(req: UpdateVeilReq): void;
    updateVoidTransform(req: UpdateVoidTransformReq): void;
    vectorObjectInfo(req: ObjectRefReq): Promise<{ ox: number, oy: number, w: number, h: number, mode: number, matrix: number[] } | null>;
    veilList(): Promise<Array<VeilInfo>>;
    voidTransformInfo(req: VoidTransformInfoReq): Promise<VoidTransformInfoResp | null>;
    warmVectorRenderer(): void;
}

/** Build the typed client over a transport (in-process today, Tauri later). */
export function makeApi(t: Transport): EngineApi {
    return {
        activeBrushNeedsSource: () => t.request('active_brush_needs_source'),
        addFilter: (req) => t.request('add_filter', req),
        addGroup: (req) => t.request('add_group', req),
        addMask: (req) => t.postFF('add_mask', req),
        addRaster: (req) => t.request('add_raster', req),
        addText: (req) => t.request('add_text', req),
        addTextObject: (req) => t.request('add_text_object', req),
        addVeil: (req) => t.postFF('add_veil', req),
        addVoid: (req) => t.request('add_void', req),
        alphaToSelection: (req) => t.postFF('alpha_to_selection', req),
        antialiasSelection: () => t.postFF('antialias_selection'),
        applyFilter: (req) => t.request('apply_filter', req),
        applyMask: (req) => t.postFF('apply_mask', req),
        beginStroke: (req) => t.request('begin_stroke', req),
        beginTransform: (req) => t.request('begin_transform', req),
        borderSelection: (req) => t.postFF('border_selection', req),
        brushActiveCapabilities: () => t.request('brush_active_capabilities'),
        brushActiveDabPreview: () => t.request('brush_active_dab_preview'),
        brushDabThumbnail: (req) => t.request('brush_dab_thumbnail', req),
        brushDelete: (req) => t.request('brush_delete', req),
        brushExportYaml: (req) => t.request('brush_export_yaml', req),
        brushExposedPorts: () => t.request('brush_exposed_ports'),
        brushGraphActive: () => t.request('brush_graph_active'),
        brushGraphAddNode: (req) => t.request('brush_graph_add_node', req),
        brushGraphAutoLayout: (req) => t.request('brush_graph_auto_layout', req),
        brushGraphCompile: (req) => t.request('brush_graph_compile', req),
        brushGraphConnect: (req) => t.request('brush_graph_connect', req),
        brushGraphDefault: () => t.request('brush_graph_default'),
        brushGraphDisconnect: (req) => t.request('brush_graph_disconnect', req),
        brushGraphExportYaml: () => t.request('brush_graph_export_yaml'),
        brushGraphExposePort: (req) => t.request('brush_graph_expose_port', req),
        brushGraphImportYaml: (req) => t.request('brush_graph_import_yaml', req),
        brushGraphRemoveNode: (req) => t.request('brush_graph_remove_node', req),
        brushGraphReorderExposedPort: (req) => t.request('brush_graph_reorder_exposed_port', req),
        brushGraphReset: () => t.postFF('brush_graph_reset'),
        brushGraphSetExposedPortMeta: (req) => t.request('brush_graph_set_exposed_port_meta', req),
        brushGraphSetInput: (req) => t.request('brush_graph_set_input', req),
        brushGraphSetNodeComment: (req) => t.request('brush_graph_set_node_comment', req),
        brushGraphSetPortRange: (req) => t.request('brush_graph_set_port_range', req),
        brushGraphUnexposePort: (req) => t.request('brush_graph_unexpose_port', req),
        brushGraphValidate: (req) => t.request('brush_graph_validate', req),
        brushList: () => t.request('brush_list'),
        brushLoad: (req) => t.request('brush_load', req),
        brushNodePreview: (req) => t.request('brush_node_preview', req),
        brushNodeTypes: () => t.request('brush_node_types'),
        brushRename: (req) => t.request('brush_rename', req),
        brushSave: (req) => t.request('brush_save', req),
        brushSetExposedPort: (req) => t.request('brush_set_exposed_port', req),
        brushStrokePreview: () => t.request('brush_stroke_preview'),
        brushThumbnail: (req) => t.request('brush_thumbnail', req),
        brushTopologyVersion: () => t.request('brush_topology_version'),
        brushUploadImage: (req, bytes) => t.request('brush_upload_image', req, bytes),
        canFlatten: () => t.request('can_flatten'),
        canFlattenNode: (req) => t.request('can_flatten_node', req),
        canMergeDown: (req) => t.request('can_merge_down', req),
        cancelFilterPreview: () => t.postFF('cancel_filter_preview'),
        cancelFloating: () => t.postFF('cancel_floating'),
        canvasDimensions: () => t.request('canvas_dimensions'),
        canvasRect: () => t.request('canvas_rect'),
        catalogs: () => t.request('catalogs'),
        clearBrushCursorPreviewPose: () => t.postFF('clear_brush_cursor_preview_pose'),
        clearCloneOverlay: () => t.postFF('clear_clone_overlay'),
        clearOverlay: () => t.postFF('clear_overlay'),
        clearOverlayMask: () => t.postFF('clear_overlay_mask'),
        clearSelection: () => t.postFF('clear_selection'),
        clearSelectionContents: (req) => t.postFF('clear_selection_contents', req),
        clearVeils: () => t.postFF('clear_veils'),
        cloneSourceAnchored: () => t.request('clone_source_anchored'),
        commitFilterPreview: (req) => t.request('commit_filter_preview', req),
        commitFloating: () => t.postFF('commit_floating'),
        copy: (req) => t.request('copy', req),
        copyLayerRich: (req) => t.postFF('copy_layer_rich', req),
        cropToSelection: () => t.postFF('crop_to_selection'),
        cut: (req) => t.request('cut', req),
        documentName: () => t.request('document_name'),
        duplicateNode: (req) => t.request('duplicate_node', req),
        duplicateNodes: (req) => t.request('duplicate_nodes', req),
        endStroke: () => t.postFF('end_stroke'),
        featherSelection: (req) => t.postFF('feather_selection', req),
        fillBackground: (req) => t.postFF('fill_background', req),
        fillBackgroundColor: (req) => t.postFF('fill_background_color', req),
        flattenImage: () => t.request('flatten_image'),
        flattenNode: (req) => t.request('flatten_node', req),
        flipCanvas: (req) => t.postFF('flip_canvas', req),
        flipNode: (req) => t.request('flip_node', req),
        floatingInfo: () => t.request('floating_info'),
        floatingTargetLayer: () => t.request('floating_target_layer'),
        fontAxes: (req) => t.request('font_axes', req),
        getBrushCursorPreviewInfo: () => t.request('get_brush_cursor_preview_info'),
        groupLayers: (req) => t.request('group_layers', req),
        growSelection: (req) => t.postFF('grow_selection', req),
        hasFloating: () => t.request('has_floating'),
        hasPendingColorPick: () => t.request('has_pending_color_pick'),
        hasSelection: () => t.request('has_selection'),
        histogramResult: (req) => t.request('histogram_result', req),
        hitTestVectorObject: (req) => t.request('hit_test_vector_object', req),
        invertSelection: () => t.postFF('invert_selection'),
        isDirty: () => t.request('is_dirty'),
        lastPickedColor: () => t.request('last_picked_color'),
        layerTransformCapability: (req) => t.request('layer_transform_capability', req),
        layerTree: () => t.request('layer_tree'),
        libraryList: () => t.request('library_list'),
        listFonts: () => t.request('list_fonts'),
        markDirty: () => t.postFF('mark_dirty'),
        maskToSelection: (req) => t.postFF('mask_to_selection', req),
        mergeDown: (req) => t.request('merge_down', req),
        mergeLayers: (req) => t.request('merge_layers', req),
        moveLayer: (req) => t.postFF('move_layer', req),
        moveLayers: (req) => t.request('move_layers', req),
        moveVeil: (req) => t.postFF('move_veil', req),
        nodeThumbnail: (req) => t.request('node_thumbnail', req),
        openDocument: (bytes) => t.request('open_document', {}, bytes),
        overlayHitTest: (req) => t.request('overlay_hit_test', req),
        packAddBrush: (req) => t.request('pack_add_brush', req),
        packCreate: (req) => t.request('pack_create', req),
        packDelete: (req) => t.request('pack_delete', req),
        packEdit: (req) => t.request('pack_edit', req),
        packExport: (req) => t.request('pack_export', req),
        packImport: (req, bytes) => t.request('pack_import', req, bytes),
        packRemoveBrush: (req) => t.request('pack_remove_brush', req),
        packReorderBrush: (req) => t.request('pack_reorder_brush', req),
        pasteImage: (req, bytes) => t.request('paste_image', req, bytes),
        pasteImageFloating: (req, bytes) => t.request('paste_image_floating', req, bytes),
        pasteInPlace: (req) => t.request('paste_in_place', req),
        pasteInPlaceFloating: (req) => t.request('paste_in_place_floating', req),
        pasteLayerRich: (req) => t.request('paste_layer_rich', req),
        pickColor: (req) => t.postFF('pick_color', req),
        placeSmartObject: (req, bytes) => t.request('place_smart_object', req, bytes),
        pollCopyResult: () => t.request('poll_copy_result'),
        pollCopyRichResult: () => t.request('poll_copy_rich_result'),
        pollExportResult: () => t.request('poll_export_result'),
        pollPreview: (req) => t.request('poll_preview', req),
        pollRecordingFrame: () => t.request('poll_recording_frame'),
        pollSaveResult: () => t.request('poll_save_result'),
        previewFilter: (req) => t.request('preview_filter', req),
        redo: () => t.postFF('redo'),
        refreshBrushCursorPreview: (req) => t.request('refresh_brush_cursor_preview', req),
        registerFont: (bytes) => t.request('register_font', {}, bytes),
        removeLayer: (req) => t.request('remove_layer', req),
        removeLayers: (req) => t.request('remove_layers', req),
        removeMask: (req) => t.postFF('remove_mask', req),
        removeVeil: (req) => t.postFF('remove_veil', req),
        requestHistogram: (req) => t.postFF('request_histogram', req),
        requestNodeHistogram: (req) => t.postFF('request_node_histogram', req),
        requestRecordingCapture: () => t.postFF('request_recording_capture'),
        rescaleImage: (req) => t.postFF('rescale_image', req),
        resize: (req) => t.postFF('resize', req),
        resizeCanvasRect: (req) => t.postFF('resize_canvas_rect', req),
        rotateCanvas: (req) => t.postFF('rotate_canvas', req),
        selectAll: () => t.postFF('select_all'),
        selectEllipse: (req) => t.postFF('select_ellipse', req),
        selectLasso: (req) => t.postFF('select_lasso', req),
        selectMagicWand: (req) => t.postFF('select_magic_wand', req),
        selectRect: (req) => t.postFF('select_rect', req),
        selectionToMask: (req) => t.postFF('selection_to_mask', req),
        setBlendMode: (req) => t.postFF('set_blend_mode', req),
        setBrushBlendMode: (req) => t.postFF('set_brush_blend_mode', req),
        setCloneOverlay: (req) => t.postFF('set_clone_overlay', req),
        setCloneSource: (req) => t.postFF('set_clone_source', req),
        setDocumentName: (req) => t.postFF('set_document_name', req),
        setFilterParams: (req) => t.postFF('set_filter_params', req),
        setGroupCollapsed: (req) => t.postFF('set_group_collapsed', req),
        setGroupPassthrough: (req) => t.postFF('set_group_passthrough', req),
        setIsolatedNode: (req) => t.request('set_isolated_node', req),
        setLayerName: (req) => t.postFF('set_layer_name', req),
        setLayerVisible: (req) => t.postFF('set_layer_visible', req),
        setMaskLinkedToHost: (req) => t.postFF('set_mask_linked_to_host', req),
        setNodeLocked: (req) => t.postFF('set_node_locked', req),
        setOpacity: (req) => t.postFF('set_opacity', req),
        setOverlay: (req) => t.postFF('set_overlay', req),
        setOverlayMask: (req) => t.postFF('set_overlay_mask', req),
        setPixelFilter: (req) => t.postFF('set_pixel_filter', req),
        setPreviewTheme: (req) => t.postFF('set_preview_theme', req),
        setRecordingParams: (req) => t.postFF('set_recording_params', req),
        setTextBox: (req) => t.postFF('set_text_box', req),
        setTextContent: (req) => t.postFF('set_text_content', req),
        setTextStyle: (req) => t.postFF('set_text_style', req),
        setVeilVisible: (req) => t.postFF('set_veil_visible', req),
        setViewTransform: (req) => t.postFF('set_view_transform', req),
        setViewportBg: (req) => t.postFF('set_viewport_bg', req),
        setVoidParams: (req) => t.postFF('set_void_params', req),
        shrinkSelection: (req) => t.postFF('shrink_selection', req),
        smoothSelection: (req) => t.postFF('smooth_selection', req),
        startExport: () => t.postFF('start_export'),
        startPreview: (req) => t.postFF('start_preview', req),
        startSaveDocument: (req) => t.request('start_save_document', req),
        strokeTo: (req) => t.postFF('stroke_to', req),
        takeTransformSetupError: () => t.request('take_transform_setup_error'),
        textObjects: (req) => t.request('text_objects', req),
        undo: () => t.postFF('undo'),
        updateFloatingMatrix: (req) => t.postFF('update_floating_matrix', req),
        updateVectorObjectTransform: (req) => t.postFF('update_vector_object_transform', req),
        updateVeil: (req) => t.postFF('update_veil', req),
        updateVoidTransform: (req) => t.postFF('update_void_transform', req),
        vectorObjectInfo: (req) => t.request('vector_object_info', req),
        veilList: () => t.request('veil_list'),
        voidTransformInfo: (req) => t.request('void_transform_info', req),
        warmVectorRenderer: () => t.postFF('warm_vector_renderer'),
    };
}
