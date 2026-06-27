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

pub use super::effect::{EffectCache, EffectPipeline};
pub use super::params::{ParamDef, ParamValue};

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
    fn create_cache(
        &self,
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
    /// `(origin_x, origin_y, w, h)`. The transform gizmo draws its bbox around
    /// this rect (the engine lifts it to plane space by adding `canvas_origin`).
    ///
    /// Default is canvas-filling `(0, 0, canvas_w, canvas_h)` — correct for
    /// procedural voids like noise whose field fills the canvas. The camera
    /// overrides it with the cover-fit webcam rect, which extends *beyond* the
    /// canvas on the cropped axis, so the gizmo wraps the real image bounds.
    fn content_extent(&self, canvas_w: u32, canvas_h: u32) -> (f32, f32, f32, f32) {
        (0.0, 0.0, canvas_w as f32, canvas_h as f32)
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

    /// Restore a saved frame at load time. Called once per camera void at
    /// document open with the bytes read from the `.darkly` zip; the void
    /// (re)allocates its aux texture at `(width, height)`, rebuilds the
    /// bind group, and `queue.write_texture`s the pixels. Default no-op
    /// for procedural voids that never declared persistent state.
    fn restore_persistent_pixels(
        &mut self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _cache: &mut EffectCache,
        _width: u32,
        _height: u32,
        _bytes: &[u8],
    ) {
    }
}

/// How a void's per-frame external image is captured on the browser side.
/// Carried on [`VoidRegistration`] and surfaced to the frontend (serialized as
/// `"camera"` / `"display"`) so the app knows which `MediaDevices` API to call
/// for each void kind — `getUserMedia` for [`Self::Camera`], `getDisplayMedia`
/// for [`Self::Display`]. Purely procedural voids (noise) declare `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureKind {
    Camera,
    Display,
}

/// What each void module returns from its `register()` function.
pub struct VoidRegistration {
    pub type_id: &'static str,
    pub display_name: &'static str,
    pub params: &'static [ParamDef],
    /// Iconify icon name (e.g. `"tabler:galaxy"`). Always present — the layer
    /// panel renders it for void layers of this kind, and the picker falls back
    /// to it when the void declares no rendered preview.
    pub icon: &'static str,
    /// Whether this void can render a meaningful picker thumbnail. When true the
    /// "Add Void" picker shows a live rendered preview; when false it shows
    /// [`icon`](Self::icon) instead. (The camera void opts out — its aux texture
    /// is a 1×1 placeholder until a webcam frame arrives, so there's nothing to
    /// render at preview time.)
    pub supports_preview: bool,
    /// Whether this void exposes a live, user-editable transform (driven by the
    /// generic gizmo, stored on [`crate::layer::VoidLayer::transform`]). Voids
    /// that opt in implement [`Void::set_transform`]; the rest leave it false
    /// and the engine never hands them a transform.
    pub supports_live_transform: bool,
    /// How this void's external frames are captured in the browser, or `None`
    /// for procedural voids that consume no external input. The frontend reads
    /// this to pick `getUserMedia` vs `getDisplayMedia`.
    pub capture_kind: Option<CaptureKind>,
    /// The void's initial gizmo transform, given the canvas dimensions at
    /// creation time. Lets a kind seed a non-identity affine — the camera seeds
    /// a horizontal flip about the canvas center (selfie view); everything else
    /// returns identity. Stored on [`crate::layer::VoidLayer::transform`] at
    /// creation so it round-trips through save/load and undo like any edit.
    pub default_transform: fn(u32, u32) -> crate::transform::Transform,
    pub create_pipeline: fn(&wgpu::Device, wgpu::TextureFormat) -> EffectPipeline,
    pub from_params: fn(&[ParamValue], Arc<EffectPipeline>) -> Box<dyn Void>,
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
