//! Void layer effects — the procedural counterpart to [`Veil`].
//!
//! A void is a GPU effect that *generates* a layer's pixel content from a
//! shader (noise, screenshare, future portals), rather than storing static
//! pixels like a raster layer. Voids live inside the layer stack as a real
//! [`crate::layer::Layer::Void`] variant, participating in normal blending,
//! masking, and undo.
//!
//! The trait shape mirrors [`super::veil::Veil`] but the output contract is
//! different: a void [`Void::encode`] takes no ping-pong source view, because
//! a void has no upstream input — it writes the layer's color content into
//! `dst_view` from scratch. The compositor then composites the void's texture
//! through the normal raster blend pipeline, so opacity / blend mode / mask
//! work uniformly with raster layers.
//!
//! Adding a new void type is one new file under [`super::voids`]: the
//! module's `register()` returns a [`VoidRegistration`] with everything the
//! engine needs — display name, parameter schema, pipeline constructor, and
//! a factory that builds the trait object from a parameter slice.
//!
//! [`Veil`]: super::veil::Veil

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
    /// re-presenting.
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

/// What each void module returns from its `register()` function.
pub struct VoidRegistration {
    pub type_id: &'static str,
    pub display_name: &'static str,
    pub params: &'static [ParamDef],
    pub create_pipeline: fn(&wgpu::Device, wgpu::TextureFormat) -> EffectPipeline,
    pub from_params: fn(&[ParamValue], Arc<EffectPipeline>) -> Box<dyn Void>,
}

/// Auto-discovered void registry with lazy pipeline caching. Modeled on
/// [`super::veil::VeilRegistry`]; the two could in principle share a
/// generic registry, but keeping them separate avoids stamping out an
/// extra layer of generics for the modest amount of code involved.
pub struct VoidRegistry {
    entries: HashMap<&'static str, RegistryEntry>,
}

struct RegistryEntry {
    display_name: &'static str,
    create_pipeline: fn(&wgpu::Device, wgpu::TextureFormat) -> EffectPipeline,
    params: &'static [ParamDef],
    from_params: fn(&[ParamValue], Arc<EffectPipeline>) -> Box<dyn Void>,
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
                    display_name: reg.display_name,
                    create_pipeline: reg.create_pipeline,
                    params: reg.params,
                    from_params: reg.from_params,
                    cached_pipeline: None,
                },
            );
        }
        VoidRegistry { entries }
    }

    /// Return all registered void types with display name and parameter
    /// definitions. Sorted by `type_id` for deterministic UI ordering.
    pub fn types(&self) -> Vec<(&'static str, &'static str, &'static [ParamDef])> {
        let mut types: Vec<_> = self
            .entries
            .iter()
            .map(|(&id, e)| (id, e.display_name, e.params))
            .collect();
        types.sort_by_key(|(id, _, _)| *id);
        types
    }

    pub fn param_defs(&self, type_id: &str) -> &'static [ParamDef] {
        self.entries.get(type_id).map(|e| e.params).unwrap_or(&[])
    }

    pub fn has(&self, type_id: &str) -> bool {
        self.entries.contains_key(type_id)
    }

    pub fn display_name(&self, type_id: &str) -> &'static str {
        self.entries
            .get(type_id)
            .map(|e| e.display_name)
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
            .get_or_insert_with(|| Arc::new((entry.create_pipeline)(device, format)))
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
            .get_or_insert_with(|| Arc::new((entry.create_pipeline)(device, format)))
            .clone();
        (entry.from_params)(params, pipeline)
    }
}
