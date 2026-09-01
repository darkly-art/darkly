//! Void layer effects — procedural per-layer content.
//!
//! A void is a GPU effect that *generates* a layer's pixel content from a
//! shader (noise, screenshare, future portals), rather than storing static
//! pixels like a raster layer. Voids live inside the layer stack as a real
//! [`crate::layer::Layer::Void`] variant, participating in normal blending,
//! masking, and undo.
//!
//! A void [`Void::encode`] writes the layer's color content into `dst_view`
//! from scratch — there is no upstream input texture. The compositor then
//! composites the void's texture through the normal raster blend pipeline,
//! so opacity / blend mode / mask work uniformly with raster layers.
//!
//! Adding a new void type is one new file under [`super::voids`]: the
//! module's `register()` returns a [`VoidRegistration`] with everything the
//! engine needs — display name, parameter schema, pipeline constructor, and
//! a factory that builds the trait object from a parameter slice.

use std::collections::HashMap;
use std::sync::Arc;

use crate::coord::CanvasRect;

pub use super::effect::{EffectCache, EffectPipeline};
pub use super::params::{ParamDef, ParamValue};
use super::preview::{
    PreviewAnim, PreviewEntry, PreviewMechanism, PreviewRegistries, PreviewSession, PreviewTarget,
    PREVIEW_FORMAT,
};

/// External image source for [`Void::upload_external_image`]. Today the only
/// populated variant is `Web`, which wraps wgpu's WebGPU-only external-image
/// copy descriptor (HTMLVideoElement, ImageBitmap, etc.). On native targets
/// the enum is uninhabited — the trait method signature still exists for a
/// uniform API surface, but no caller can construct an argument.
#[derive(Debug)]
pub enum ExternalImageSource {
    /// Browser-side image source: video element, image bitmap, canvas, etc.
    /// The caller has already built a `CopyExternalImageSourceInfo` describing
    /// the source rect and y-flip. The void implementation owns the
    /// destination texture (in its [`EffectCache::aux_textures`]) and chooses
    /// the destination's color-space / premultiplication.
    #[cfg(target_arch = "wasm32")]
    Web(wgpu::CopyExternalImageSourceInfo),
}

impl ExternalImageSource {
    /// Pixel dimensions of the underlying source, used by voids to (re)size
    /// their destination aux texture.
    #[allow(clippy::needless_return, unreachable_code)]
    pub fn pixel_size(&self) -> (u32, u32) {
        #[cfg(target_arch = "wasm32")]
        match self {
            Self::Web(info) => return (info.source.width(), info.source.height()),
        }
        // Native: enum is uninhabited; method is unreachable.
        unreachable!("ExternalImageSource has no variants on this target")
    }
}

/// A void's content rectangle, in fractional pixels.
///
/// Voids draw content that need not align to the integer canvas grid — a
/// cover-fit webcam frame overhangs the canvas by a fractional amount, and a
/// placed image sits wherever its transform puts it — so this cannot be a
/// [`CanvasRect`], whose origin and size are integral.
///
/// **The rect is in plane space** (the absolute document frame that does not
/// move when the canvas is cropped). [`Self::to_window`] is the single
/// conversion into the window-local frame the shader indexes, so no caller
/// hand-writes `± canvas_origin` — see `docs/coordinate-systems.md`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ContentRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl ContentRect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        ContentRect {
            x,
            y,
            width,
            height,
        }
    }

    /// The whole canvas window, as a plane-space content rect. The answer for
    /// any void whose field fills the canvas.
    pub fn covering(canvas: CanvasRect) -> Self {
        ContentRect::new(
            canvas.origin.x as f32,
            canvas.origin.y as f32,
            canvas.width as f32,
            canvas.height as f32,
        )
    }

    /// Re-express this plane-space rect in the window-local frame whose origin
    /// is `canvas.origin`. The shader works in window-local coordinates
    /// (`FragCoord` is window-relative), so the uniform builder converts here
    /// and nowhere else.
    pub fn to_window(self, canvas: CanvasRect) -> Self {
        ContentRect::new(
            self.x - canvas.origin.x as f32,
            self.y - canvas.origin.y as f32,
            self.width,
            self.height,
        )
    }
}

/// Sticky "needs re-encode" flag every void embeds.
///
/// Implementations call [`Self::mark`] from inside the state-changing methods
/// (`update_params`, `update_time`, `upload_external_image`, …). The
/// compositor's render pass clears it via [`Self::take`] right before
/// invoking [`Void::encode`], so a void re-renders exactly once per state
/// change. Defaulting `true` at construction time means the first
/// compositor pass after `ensure_void_layer` always produces a fresh
/// texture, even if no param edits have happened yet.
#[derive(Debug)]
pub struct DirtyFlag(bool);

impl DirtyFlag {
    pub fn new_dirty() -> Self {
        DirtyFlag(true)
    }
    pub fn mark(&mut self) {
        self.0 = true;
    }
    /// Return whether the flag was set and clear it.
    pub fn take(&mut self) -> bool {
        std::mem::replace(&mut self.0, false)
    }
}

impl Default for DirtyFlag {
    fn default() -> Self {
        Self::new_dirty()
    }
}

/// Layer-level procedural-content effect ("void"). Renders the layer's
/// pixels from a shader instead of storing them.
///
/// Voids do not receive an upstream texture — they have no input. The
/// compositor allocates a per-void destination texture at canvas resolution
/// and the void writes its full output there in `encode()`. The compositor
/// then samples that texture through the existing blend pipeline, so every
/// raster-layer feature (blend modes, opacity, masks, group nesting) works
/// for voids without any per-kind branching.
pub trait Void: std::fmt::Debug {
    fn type_id(&self) -> &'static str;
    fn clone_boxed(&self) -> Box<dyn Void>;

    /// Return the current parameter values, in the same order as the
    /// type's [`ParamDef`] array in [`VoidRegistration`].
    fn param_values(&self) -> Vec<ParamValue>;

    /// Return whether the void needs re-encoding into its destination
    /// texture and clear the flag. The compositor calls this once per
    /// frame; voids embed a [`DirtyFlag`] and forward to it. Implementations
    /// must initialise the flag dirty so the first compositor pass after
    /// `ensure_void_layer` produces a fresh texture.
    fn take_dirty(&mut self) -> bool;

    /// Re-mark this void as needing a fresh encode. Voids call this from
    /// their own state-mutating methods (`update_params`, `update_time`,
    /// `upload_external_image`); rarely needed externally.
    fn mark_dirty(&mut self);

    /// Allocate per-instance GPU resources. The compositor passes the
    /// destination view (the void's own texture) so the void can build
    /// bind groups that target it directly — voids never sample from a
    /// ping-pong pair the way veils do.
    ///
    /// Takes `&mut self` so a void whose uniform struct folds in something it
    /// is only handed here — the render resolution, the render-target→canvas
    /// scale — can keep it, and rewrite that struct later from state alone.
    fn create_cache(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        dst_view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        render_width: u32,
        render_height: u32,
    ) -> EffectCache;

    /// Whether this void uses time-based animation. When true (and visible),
    /// the compositor calls [`Self::update_time`] each frame the
    /// `animation.void_divisor` master clock fires, and keeps the canvas
    /// re-presenting. The visibility half of that gate is enforced by the
    /// engine, not the void — voids don't store layer visibility (the doc
    /// does).
    fn needs_animation(&self) -> bool {
        false
    }

    /// Per-frame uniform update for animated voids. Default is a no-op.
    fn update_time(&mut self, _queue: &wgpu::Queue, _cache: &EffectCache, _dt: f32) {}

    /// Put this instance into the state its preview shows at normalized time
    /// `t ∈ [0, 1]`, and sync whatever GPU resources that state feeds.
    ///
    /// Absolute, not incremental: `preview_at(0.5)` produces the same state
    /// whether it follows `preview_at(0.4)` or nothing at all.
    ///
    /// Answers whether `cache` still describes this instance; a void whose
    /// cache shape is a function of its parameters would answer `false` and be
    /// rebuilt through [`create_cache`](Self::create_cache). The default is a
    /// no-op answering `true`, which renders a still at the instance's own
    /// parameters.
    ///
    /// See [`super::preview`] for the shape every body follows and the sweeps
    /// they share.
    fn preview_at(&mut self, _queue: &wgpu::Queue, _cache: &EffectCache, _t: f32) -> bool {
        true
    }

    /// Replace this void's parameter values in place — update internal
    /// fields, rewrite the uniform buffer, but leave any stateful GPU
    /// resources (aux textures holding the camera's last received frame,
    /// future readback buffers, etc.) untouched. Required because the
    /// alternative — rebuilding the void from `from_params` — drops
    /// `EffectCache::aux_textures`, which is where the camera void stores
    /// the live webcam frame. Toggling any param (including `freeze`)
    /// would otherwise wipe the displayed image.
    fn update_params(&mut self, queue: &wgpu::Queue, cache: &EffectCache, params: &[ParamValue]);

    /// Apply a user transform (pan / scale / rotate) in place — the void
    /// *consuming* the generic transform helper's output. Like
    /// [`Self::update_params`], this rewrites the uniform buffer in place and
    /// must NOT rebuild via `from_params` (that drops the aux webcam texture).
    /// Default is a genuine no-op: voids that don't set
    /// [`VoidRegistration::supports_live_transform`] are never handed a
    /// transform by the engine, so they ignore this entirely.
    fn set_transform(
        &mut self,
        _queue: &wgpu::Queue,
        _cache: &EffectCache,
        _transform: &crate::transform::Transform,
    ) {
    }

    /// The void's natural content rectangle at the identity transform, in
    /// WINDOW-LOCAL coords (relative to the canvas-window top-left), as
    /// The void's content rectangle **in plane space**. The transform gizmo
    /// draws its bbox around this rect directly — no lifting at the boundary.
    ///
    /// Default is the canvas window itself — correct for procedural voids like
    /// noise whose field fills the canvas, and it moves with the window on a
    /// crop, which is what "fills the canvas" means. Voids anchored to the
    /// document rather than the window (a placed image) override it with a rect
    /// that does *not* move on crop; the camera overrides it with the cover-fit
    /// webcam rect, which extends beyond the canvas on the cropped axis so the
    /// gizmo wraps the real image bounds.
    fn content_extent(&self, canvas: CanvasRect) -> ContentRect {
        ContentRect::covering(canvas)
    }

    /// React to a canvas resize or crop. The compositor calls this on every
    /// void whenever the canvas rect changes, so a void that caches canvas
    /// geometry in its sampling uniform can rewrite it in place — the same
    /// shape as [`Self::set_transform`].
    ///
    /// Default no-op: a void that derives everything from `content_extent` at
    /// draw time has nothing cached to refresh.
    fn set_canvas_rect(&mut self, _queue: &wgpu::Queue, _cache: &EffectCache, _canvas: CanvasRect) {
    }

    /// Whether this void consumes per-frame external image input (webcam,
    /// screenshare, …). When true, the bridge plumbs frames through
    /// [`Self::upload_external_image`] each render. The default is false —
    /// procedural voids (noise, future portals) ignore this path entirely.
    fn wants_external_input(&self) -> bool {
        false
    }

    /// Receive an external image frame (browser-supplied video, bitmap, etc.)
    /// and copy it into the void's GPU input. Voids that own an input texture
    /// in their [`EffectCache::aux_textures`] use this hook to (re)allocate on
    /// dimension changes and dispatch a [`wgpu::Queue::copy_external_image_to_texture`].
    /// Default no-op so the noise void (and any future pure-procedural void)
    /// doesn't pay attention.
    fn upload_external_image(
        &mut self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _cache: &mut EffectCache,
        _source: ExternalImageSource,
    ) {
    }

    /// Render the void's content into `dst_view`. Called once per frame
    /// while the void is visible. Re-rendered eagerly on parameter changes
    /// via [`Self::needs_render`].
    fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        cache: &EffectCache,
        dst_view: &wgpu::TextureView,
    );

    /// Persistent input-texture size, if this void stores its last received
    /// frame as document state. Returns `Some((w, h))` for input-consuming
    /// voids (camera, future screenshare) that have actually received a
    /// frame; `None` for purely procedural voids and for input voids that
    /// haven't seen their first frame yet. The engine reads this after
    /// every `upload_external_image` to keep the doc-side
    /// [`crate::layer::VoidLayer::frame`] in sync, so save sees the right
    /// dimensions for the readback.
    fn persistent_frame_size(&self) -> Option<(u32, u32)> {
        None
    }

    /// Install `bytes` as this void's source image. The void (re)allocates its
    /// aux texture at `(width, height)`, rebuilds the bind group, uploads, and
    /// regenerates any mip chain. Default no-op for procedural voids that
    /// never declared a source.
    ///
    /// Two callers: document load (restoring a saved frame from the `.darkly`
    /// zip) and placement (installing a user-supplied image). `bytes` are
    /// **premultiplied** RGBA8 in both cases — the convention every sampled
    /// void source uses, so linear filtering doesn't bleed colour out of
    /// transparent texels.
    fn set_source_pixels(
        &mut self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _cache: &mut EffectCache,
        _width: u32,
        _height: u32,
        _bytes: &[u8],
    ) {
    }

    /// Reallocate this void's source texture at `width × height` and rebuild
    /// its bind group, without supplying any texels.
    ///
    /// The allocation half of [`Self::set_source_pixels`], split out so a
    /// caller that already holds the pixels **on the GPU** can size the
    /// destination and then blit into it, instead of routing a full-size buffer
    /// through the CPU just to make the void allocate.
    ///
    /// The fresh texture is zero-initialised by wgpu, so the void reads as
    /// fully transparent between this call and the blit that fills it. It
    /// already reports `persistent_frame_size()` and a natural-size content
    /// rect in that window — harmless for the synchronous allocate-then-blit
    /// sequences that use it, but the reason this is not a public two-phase
    /// API.
    ///
    /// Takes a `queue` because adopting a source changes what the sampling
    /// uniform has to say: the void stops covering the canvas and starts
    /// drawing at the source's own size. An implementation that resizes without
    /// republishing that uniform leaves the layer rendering through the
    /// placeholder's extent.
    fn allocate_source(
        &mut self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _cache: &mut EffectCache,
        _width: u32,
        _height: u32,
    ) {
    }
}

/// How the frontend acquires a void's per-frame external image. Carried on
/// [`VoidRegistration`] and surfaced to the frontend (serialized as `"camera"` /
/// `"display"` / `"stream"`) so the app knows how to source each void kind's
/// frames — `getUserMedia` for [`Self::Camera`], `getDisplayMedia` for
/// [`Self::Display`], and an HTTP frame stream for [`Self::Stream`] (the Blender
/// void connects to a localhost server serving length-prefixed WebP frames).
/// Purely procedural voids (noise) declare `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub enum CaptureKind {
    Camera,
    Display,
    /// Frames pulled from an HTTP stream the frontend `fetch`es (see the
    /// `blender` void). The stream URL is a document-persisted `url` param the
    /// frontend reads; the Rust void treats it like any browser-supplied frame.
    Stream,
}

/// Where a void's pixels come from — the one fact that decides how the
/// frontend offers the kind, whether the engine feeds it frames, and whether
/// its source is one-shot or continuous.
///
/// Consumers match on this once instead of consulting several parallel flags:
/// a picker that wants to know "does choosing this open a file dialog, request
/// a camera, or just add a layer?" reads exactly this field, and a new ingress
/// (video file, remote image, PDF page) is a new variant that every consumer's
/// match must then handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
pub enum VoidSource {
    /// Generated from parameters alone — noise and friends. No external input,
    /// no stored pixels.
    Procedural,
    /// A continuous stream of browser-supplied frames. The payload tells the
    /// frontend which capture API to open.
    Capture { capture: CaptureKind },
    /// A single still image the user supplies once, held at its native
    /// resolution and resampled through the layer's transform. The frontend
    /// opens a file picker rather than adding an empty layer.
    Image,
}

impl VoidSource {
    /// Whether this source delivers frames continuously. Drives both the
    /// external-input plumbing and the animation clock: a one-shot source must
    /// not keep the render loop awake.
    pub fn is_streaming(self) -> bool {
        matches!(self, VoidSource::Capture { .. })
    }

    /// The capture API to open, for sources that stream from one.
    pub fn capture_kind(self) -> Option<CaptureKind> {
        match self {
            VoidSource::Capture { capture } => Some(capture),
            _ => None,
        }
    }
}

/// What each void module returns from its `register()` function.
pub struct VoidRegistration {
    pub type_id: &'static str,
    pub display_name: &'static str,
    /// One-sentence summary shown as a tooltip in the Add Void picker —
    /// include the terms users would search for.
    pub description: &'static str,
    pub params: &'static [ParamDef],
    /// Iconify icon name (e.g. `"tabler:galaxy"`). Always present — the layer
    /// panel renders it for void layers of this kind, and the picker falls back
    /// to it when the void declares no rendered preview.
    pub icon: &'static str,
    /// How long this void's preview runs, or `None` for a void with nothing to
    /// show. Declaring an animation is what makes a void previewable — the "Add
    /// Void" picker renders a live thumbnail for the ones that do and falls
    /// back to [`icon`](Self::icon) for the ones that don't. (The stream voids
    /// opt out: their aux texture is a 1×1 placeholder until a browser frame
    /// arrives, so there is nothing to render and nothing to animate.) What the
    /// preview *does* over that span is [`Void::preview_at`].
    pub preview: Option<PreviewAnim>,
    /// Whether this void exposes a live, user-editable transform (driven by the
    /// generic gizmo, stored on [`crate::layer::VoidLayer::transform`]). Voids
    /// that opt in implement [`Void::set_transform`]; the rest leave it false
    /// and the engine never hands them a transform.
    pub supports_live_transform: bool,
    /// Where this void's pixels come from. The frontend reads it to decide how
    /// choosing the kind behaves (open a camera, open a file picker, or just
    /// add the layer); the engine reads it to decide whether to feed frames and
    /// whether the source is one-shot.
    pub source: VoidSource,
    /// The void's initial gizmo transform, given the canvas dimensions at
    /// creation time. Lets a kind seed a non-identity affine — the camera seeds
    /// a horizontal flip about the canvas center (selfie view); everything else
    /// returns identity. Stored on [`crate::layer::VoidLayer::transform`] at
    /// creation so it round-trips through save/load and undo like any edit.
    pub default_transform: fn(u32, u32) -> crate::transform::Transform,
    pub create_pipeline: fn(&wgpu::Device, wgpu::TextureFormat) -> EffectPipeline,
    pub from_params: fn(&[ParamValue], Arc<EffectPipeline>) -> Box<dyn Void>,
}

/// Id of the catalog this registry projects into.
pub const CATALOG_ID: &str = "voids";

impl VoidRegistration {
    pub fn catalog_entry(&self) -> crate::catalog::CatalogEntry {
        crate::catalog::CatalogEntry::new(self.type_id, self.display_name)
            .with_icon(self.icon)
            .with_description(self.description)
            .with_params(self.params)
            .with_supports_preview(self.preview.is_some())
            .with_source(self.source)
    }
}

/// The void catalog — every registered void, sorted by `type_id`.
pub fn catalog() -> crate::catalog::Catalog {
    crate::catalog::Catalog::new(
        CATALOG_ID,
        "Voids",
        VoidRegistry::new()
            .types()
            .into_iter()
            .map(VoidRegistration::catalog_entry)
            .collect(),
    )
    .with_description("Sources that generate a layer's pixels instead of storing them.")
}

/// Auto-discovered void registry with lazy pipeline caching. Each void
/// kind contributes one [`VoidRegistration`] via its module's `register()`;
/// `build.rs` collects them into [`super::voids::registrations`] and the
/// registry pulls them in at startup. Pipelines build on first use and are
/// shared via [`Arc`] across instances of the same kind.
pub struct VoidRegistry {
    entries: HashMap<&'static str, RegistryEntry>,
}

struct RegistryEntry {
    /// The full registration this entry was built from. All metadata accessors
    /// read straight off this, so a new `VoidRegistration` field is exposed
    /// without widening any tuple or touching the registry.
    reg: VoidRegistration,
    cached_pipeline: Option<Arc<EffectPipeline>>,
}

impl Default for VoidRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl VoidRegistry {
    pub fn new() -> Self {
        let mut entries = HashMap::new();
        for reg in super::voids::registrations() {
            entries.insert(
                reg.type_id,
                RegistryEntry {
                    reg,
                    cached_pipeline: None,
                },
            );
        }
        VoidRegistry { entries }
    }

    /// Return every registered void's full [`VoidRegistration`], sorted by
    /// `type_id` for deterministic UI ordering. Callers read whatever fields
    /// they need off the registration — a new field is free here.
    pub fn types(&self) -> Vec<&VoidRegistration> {
        let mut types: Vec<&VoidRegistration> = self.entries.values().map(|e| &e.reg).collect();
        types.sort_by_key(|reg| reg.type_id);
        types
    }

    pub fn param_defs(&self, type_id: &str) -> &'static [ParamDef] {
        self.entries
            .get(type_id)
            .map(|e| e.reg.params)
            .unwrap_or(&[])
    }

    /// How long a void type's preview runs. `None` for an unknown type or one
    /// that declares no preview.
    pub fn preview(&self, type_id: &str) -> Option<PreviewAnim> {
        self.entries.get(type_id)?.reg.preview
    }

    /// The iconify icon name for a void kind (layer-panel icon + picker
    /// fallback). Empty for unknown types.
    pub fn icon(&self, type_id: &str) -> &'static str {
        self.entries.get(type_id).map(|e| e.reg.icon).unwrap_or("")
    }

    /// The initial gizmo transform for a freshly-created void of this kind at
    /// the given canvas size. Identity for unknown types.
    pub fn default_transform(
        &self,
        type_id: &str,
        canvas_w: u32,
        canvas_h: u32,
    ) -> crate::transform::Transform {
        self.entries
            .get(type_id)
            .map(|e| (e.reg.default_transform)(canvas_w, canvas_h))
            .unwrap_or_else(crate::transform::Transform::identity)
    }

    /// Resolve a runtime `&str` type id to the registry's `&'static str` key,
    /// or `None` if the type is unknown. Callers keying long-lived state by
    /// type id (the preview cache + its readback context) use this to obtain a
    /// `'static` id without leaking. Mirrors `VeilRegistry::static_type_id`.
    pub fn static_type_id(&self, type_id: &str) -> Option<&'static str> {
        self.entries.get_key_value(type_id).map(|(k, _)| *k)
    }

    /// Whether the named void kind exposes a live, user-editable transform.
    /// Consumed by [`crate::layer::Layer::transform_capability`]. Unknown
    /// types return false.
    pub fn supports_live_transform(&self, type_id: &str) -> bool {
        self.entries
            .get(type_id)
            .map(|e| e.reg.supports_live_transform)
            .unwrap_or(false)
    }

    /// Where a registered kind's pixels come from, or `None` for an unknown
    /// type id. Lets consumers ask about provenance without reaching for the
    /// whole registration.
    pub fn source(&self, type_id: &str) -> Option<VoidSource> {
        self.entries.get(type_id).map(|e| e.reg.source)
    }

    pub fn has(&self, type_id: &str) -> bool {
        self.entries.contains_key(type_id)
    }

    pub fn display_name(&self, type_id: &str) -> &'static str {
        self.entries
            .get(type_id)
            .map(|e| e.reg.display_name)
            .unwrap_or("")
    }

    /// Get or create the shared pipeline for a void type. Pipelines are
    /// shared across all instances of the same type (Arc-wrapped) since
    /// the bind-group layout and shader are identical; only the per-
    /// instance uniform values differ.
    pub fn pipeline(
        &mut self,
        type_id: &str,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> Arc<EffectPipeline> {
        let entry = self
            .entries
            .get_mut(type_id)
            .unwrap_or_else(|| panic!("Unknown void type: {type_id}"));
        entry
            .cached_pipeline
            .get_or_insert_with(|| Arc::new((entry.reg.create_pipeline)(device, format)))
            .clone()
    }

    /// Create a void instance from a type string and parameter values.
    pub fn create_void(
        &mut self,
        type_id: &str,
        params: &[ParamValue],
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> Box<dyn Void> {
        let entry = self
            .entries
            .get_mut(type_id)
            .unwrap_or_else(|| panic!("Unknown void type: {type_id}"));
        let pipeline = entry
            .cached_pipeline
            .get_or_insert_with(|| Arc::new((entry.reg.create_pipeline)(device, format)))
            .clone();
        (entry.reg.from_params)(params, pipeline)
    }
}

// ---------------------------------------------------------------------------
// Preview mechanism
// ---------------------------------------------------------------------------

/// This catalog's answer to [`PreviewMechanism`]. Exported by name so
/// `build.rs` finds it while scanning this module's source and emits a
/// `preview_mechanisms()` row for `voids`.
pub fn preview_mechanism() -> &'static dyn PreviewMechanism {
    &VoidMechanism
}

struct VoidMechanism;

impl PreviewMechanism for VoidMechanism {
    fn resolve(&self, type_id: &str) -> Option<PreviewEntry> {
        let registry = VoidRegistry::new();
        Some(PreviewEntry {
            type_id: registry.static_type_id(type_id)?,
            anim: registry.preview(type_id)?,
        })
    }

    /// A void generates its content from a shader with no input, so the
    /// target's source texture is cleared rather than loaded.
    fn reads_source(&self) -> bool {
        false
    }

    fn open<'a>(
        &self,
        regs: PreviewRegistries<'a>,
        type_id: &str,
    ) -> Option<Box<dyn PreviewSession + 'a>> {
        let type_id = regs.voids.static_type_id(type_id)?;
        Some(Box::new(VoidSession {
            registry: regs.voids,
            type_id,
            instance: None,
        }))
    }
}

/// One open void preview: the instance and the cache it renders through.
struct VoidSession<'a> {
    registry: &'a mut VoidRegistry,
    type_id: &'static str,
    instance: Option<(Box<dyn Void>, EffectCache)>,
}

impl<'a> PreviewSession for VoidSession<'a> {
    fn set_t(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &PreviewTarget,
        t: f32,
    ) {
        if self.instance.is_none() {
            let defaults: Vec<ParamValue> = self
                .registry
                .param_defs(self.type_id)
                .iter()
                .map(ParamDef::default_value)
                .collect();
            let mut void =
                self.registry
                    .create_void(self.type_id, &defaults, device, PREVIEW_FORMAT);
            let cache = build_cache(&mut *void, device, queue, target);
            self.instance = Some((void, cache));
        }
        let (void, cache) = self.instance.as_mut().expect("built above");
        if !void.preview_at(queue, cache, t) {
            *cache = build_cache(&mut **void, device, queue, target);
        }
    }

    fn encode(
        &mut self,
        _device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &PreviewTarget,
    ) {
        let Some((void, cache)) = self.instance.as_ref() else {
            return;
        };
        void.encode(encoder, cache, target.output_view());
    }
}

/// The one place a void's cache is built against a preview target, so the two
/// callers — the first build and a `preview_at` that invalidated its cache —
/// cannot disagree about what it is built from. A void writes straight into the
/// output view, so that is what its bind groups target.
fn build_cache(
    void: &mut dyn Void,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    target: &PreviewTarget,
) -> EffectCache {
    let (w, h) = target.size();
    void.create_cache(device, queue, target.output_view(), target.sampler(), w, h)
}

#[cfg(test)]
mod dirty_flag_tests {
    use super::DirtyFlag;

    #[test]
    fn starts_dirty_so_first_encode_fires() {
        let mut flag = DirtyFlag::new_dirty();
        assert!(flag.take(), "fresh flag must report dirty on first take");
    }

    #[test]
    fn take_clears() {
        let mut flag = DirtyFlag::new_dirty();
        assert!(flag.take());
        assert!(!flag.take(), "take must clear the flag");
    }

    #[test]
    fn mark_re_arms() {
        let mut flag = DirtyFlag::new_dirty();
        flag.take();
        flag.mark();
        assert!(flag.take(), "mark after clear must re-arm");
    }
}
