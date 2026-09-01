//! The one kind of image transform Darkly has: an **effect**.
//!
//! An effect is a registered, parameterized transform of an image — a
//! `type_id`, a [`ParamDef`] schema, a WGSL pipeline and a preview. One
//! instance exists per place the effect is used; the pipeline behind it is
//! `Arc`'d by [`EffectRegistry`] and shared across every instance of its type
//! and target format.
//!
//! The same instance serves every path an effect can be invoked through —
//! a layer in the tree, the screen-space chain, a destructive apply over a
//! node region, a picker preview — because all four hand it a ping-pong pair
//! and ask it to write one half from the other. Masking is **not** the
//! effect's concern: it is applied outside, by the shared in-place apply pass,
//! which is what makes every effect maskable without declaring anything.
//!
//! Modules live in `gpu/effects/`, one file each, discovered by `build.rs`.

use std::collections::HashMap;
use std::sync::Arc;

pub use super::params::{ParamDef, ParamValue};
use super::preview::{
    PreviewAnim, PreviewEntry, PreviewMechanism, PreviewRegistries, PreviewSession, PreviewTarget,
    PREVIEW_FORMAT,
};
use crate::catalog::{Catalog, CatalogEntry};

/// Shared GPU pipeline for an effect type at one target format.
/// Arc-wrapped so multiple instances of the same effect share them.
pub struct EffectPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub bind_group_layout: wgpu::BindGroupLayout,
}

impl std::fmt::Debug for EffectPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EffectPipeline").finish_non_exhaustive()
    }
}

/// Cached GPU objects for an effect instance.
/// Created once at instance creation, never in the render loop.
pub struct EffectCache {
    /// One uniform buffer per pass.
    pub uniform_bufs: Vec<wgpu::Buffer>,
    /// One bind group per pass, per ping-pong direction.
    /// Indexed as bind_groups[pass_index][ping_pong_src].
    pub bind_groups: Vec<[wgpu::BindGroup; 2]>,
    /// Optional auxiliary textures (e.g., noise texture, intermediate render targets).
    pub aux_textures: Vec<wgpu::Texture>,
    pub aux_views: Vec<wgpu::TextureView>,
    /// Optional auxiliary pipelines (e.g. an internal blit an effect runs
    /// between its own passes).
    pub aux_pipelines: Vec<wgpu::RenderPipeline>,
}

impl EffectCache {
    /// A cache holding no resources — what an effect with nothing to cache
    /// returns from [`Effect::create_cache`], and the placeholder a consumer
    /// binds when an instance has not been realized yet.
    pub fn empty() -> Self {
        EffectCache {
            uniform_bufs: Vec::new(),
            bind_groups: Vec::new(),
            aux_textures: Vec::new(),
            aux_views: Vec::new(),
            aux_pipelines: Vec::new(),
        }
    }

    /// Rewrite the uniform buffer at `index` from freshly-packed bytes, or do
    /// nothing when this cache holds no such buffer.
    ///
    /// An effect's parameter state reaches the GPU through exactly two callers
    /// — the effect's own `create_cache` and whatever rewrites it afterwards —
    /// and the two must agree on a layout `bytemuck` will not check for them.
    /// Routing both through one method is what keeps the packing in one place.
    pub fn write_uniform(&self, queue: &wgpu::Queue, index: usize, bytes: &[u8]) {
        if let Some(buf) = self.uniform_bufs.get(index) {
            queue.write_buffer(buf, 0, bytes);
        }
    }
}

/// A fragment-visible bind-group entry kind. Fullscreen post-process pipelines
/// (every effect, plus the blit/downscale passes) are built from a short
/// ordered list of these; the builder assigns each its list position as its
/// binding index, matching the `@group(0) @binding(i)` numbering the shaders
/// use.
#[derive(Clone, Copy)]
pub enum Binding {
    /// Filterable 2D float texture (hardware-sampled). Input and aux textures.
    Texture,
    /// Filtering sampler.
    Sampler,
    /// Uniform buffer, no dynamic offset.
    Uniform,
}

impl Binding {
    fn layout_entry(self, binding: u32) -> wgpu::BindGroupLayoutEntry {
        let ty = match self {
            Binding::Texture => wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            Binding::Sampler => wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            Binding::Uniform => wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
        };
        wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty,
            count: None,
        }
    }
}

/// Build a render pipeline from a passthrough blit shader.
/// Used by the reduced-resolution upscale and the final blit to surface.
pub fn create_blit_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    label: &str,
) -> EffectPipeline {
    create_effect_pipeline(
        device,
        format,
        label,
        &[Binding::Texture, Binding::Sampler],
        include_str!("../../shaders/blit.wgsl"),
        "fs_blit",
    )
}

/// Build a render pipeline for the multi-tap soft downscale shader.
/// Feeds a reduced-resolution effect a properly anti-aliased input —
/// single-tap bilinear (blit) aliases hard at any downscale ratio worse than
/// ~0.7 because it's a fixed 2×2 box filter regardless of the ratio.
pub fn create_downscale_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    label: &str,
) -> EffectPipeline {
    create_effect_pipeline(
        device,
        format,
        label,
        &[Binding::Texture, Binding::Sampler],
        include_str!("../../shaders/downscale.wgsl"),
        "fs_downscale",
    )
}

/// Build a fullscreen-triangle post-process pipeline: `vs_main` +
/// `fragment_entry`, one color target of `format`, no blend/depth/stencil. The
/// bind-group layout is `bindings` in order, numbered 0..n. The single home for
/// every effect's pipeline construction and the blit/downscale passes.
pub fn create_effect_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    label: &str,
    bindings: &[Binding],
    shader_source: &str,
    fragment_entry: &str,
) -> EffectPipeline {
    let entries: Vec<_> = bindings
        .iter()
        .enumerate()
        .map(|(i, b)| b.layout_entry(i as u32))
        .collect();
    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(&format!("{label}-bgl")),
        entries: &entries,
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(&format!("{label}-layout")),
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(&format!("{label}-shader")),
        source: wgpu::ShaderSource::Wgsl(shader_source.into()),
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some(fragment_entry),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    EffectPipeline {
        pipeline,
        bind_group_layout,
    }
}

/// Create a bind group for a simple blit (texture + sampler).
pub fn create_blit_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    source_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    label: &str,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(source_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

// ---------------------------------------------------------------------------
// The effect itself
// ---------------------------------------------------------------------------

/// A parameterized image transform, prepared against a ping-pong pair at a
/// known resolution.
///
/// One instance per place the effect is used — a layer in the tree, an entry in
/// the screen-space chain, a destructive apply, an open preview. The shared
/// pipeline behind it is `Arc`'d by [`EffectRegistry`], so an instance is cheap:
/// its own parameter values plus whatever GPU objects [`create_cache`] built for
/// the pair it was bound against.
///
/// Nothing here mentions masks. A mask confines *where* a transform lands,
/// which is a property of the site rather than of the transform, and it is
/// applied outside by the shared in-place apply pass.
///
/// [`create_cache`]: Effect::create_cache
pub trait Effect: std::fmt::Debug {
    fn type_id(&self) -> &'static str;
    fn clone_boxed(&self) -> Box<dyn Effect>;

    /// The current parameter values, in the same order as this type's
    /// [`ParamDef`] array on its [`EffectRegistration`].
    fn param_values(&self) -> Vec<ParamValue>;

    /// Create GPU resources for this instance against a ping-pong pair.
    ///
    /// `ping_pong_views` are the two halves the effect reads from and writes
    /// to; `render_width`/`render_height` are their dimensions. An effect never
    /// learns why it was given that size — a reduced-resolution screen-space
    /// run, a canvas-sized accumulator and a region-sized destructive scratch
    /// all look the same from here.
    ///
    /// Takes `&mut self` so an effect whose uniform folds in something it is
    /// only handed here — the render resolution, a decoded texture's aspect —
    /// can keep it and rewrite that uniform later from state alone.
    fn create_cache(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        ping_pong_views: &[wgpu::TextureView; 2],
        sampler: &wgpu::Sampler,
        render_width: u32,
        render_height: u32,
    ) -> EffectCache;

    /// Encode every pass this effect needs, reading `ping_pong[src_idx]`
    /// through the pre-built bind groups in `cache` and writing the final
    /// result to `dst_view`. Intermediate passes into `cache.aux_views` are the
    /// effect's own business.
    fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        cache: &EffectCache,
        src_idx: usize,
        dst_view: &wgpu::TextureView,
    );

    /// Per-effect resolution scale, applied on top of the global config scale.
    /// An effect whose per-pixel cost is too high at full resolution overrides
    /// this below 1.0; the caller renders it smaller and upscales the result.
    fn perf_scale_factor(&self) -> f32 {
        1.0
    }

    /// Whether this effect animates over time. When true (and visible), the
    /// compositor drives continuous re-rendering.
    fn needs_animation(&self) -> bool {
        false
    }

    /// Advance animated state by `dt` seconds and sync whatever GPU resources
    /// it feeds. A no-op for effects that do not animate. This is how the live
    /// path drives the clock; a preview positions it with [`seek`](Effect::seek).
    fn update_time(&mut self, _queue: &wgpu::Queue, _cache: &EffectCache, _dt: f32) {}

    /// Position animated state at normalized preview time `t ∈ [0, 1]`.
    ///
    /// Absolute, not incremental: `seek(0.5)` produces the same state whether
    /// it follows `seek(0.4)` or nothing at all, which is what lets a preview
    /// sequence be dropped and re-opened mid-run without replaying anything.
    ///
    /// Distinct from [`set_params`](Effect::set_params) because a clock is not
    /// a parameter. It is not in the schema, it does not serialize, and — since
    /// the time an effect is showing cannot change what its cache *is*, only
    /// what that cache holds — it never invalidates anything and so answers
    /// nothing. A parameter can do both, which is why the two are separate
    /// questions rather than one method that means whichever the caller
    /// intended.
    fn seek(&mut self, _queue: &wgpu::Queue, _cache: &EffectCache, _t: f32) {}

    /// Adopt a new parameter vector, answering whether `cache` still describes
    /// this instance.
    ///
    /// The default answers `false` — rebuild — which is always correct and is
    /// what the screen-space chain did for every slider drag before effects had
    /// this method. An effect whose cache *shape* is parameter-independent
    /// overrides it to rewrite its uniform in place and answer `true`, which is
    /// the difference between a slider drag reallocating textures and writing
    /// 32 bytes.
    fn set_params(
        &mut self,
        _queue: &wgpu::Queue,
        _cache: &EffectCache,
        _params: &[ParamValue],
    ) -> bool {
        false
    }
}

/// What each module in `gpu/effects/` returns from its `register()` function.
pub struct EffectRegistration {
    pub type_id: &'static str,
    pub display_name: &'static str,
    /// One-sentence summary shown as a picker tooltip and folded into the
    /// destructive-apply action's description, where the command palette's
    /// substring search indexes it — include the terms users would search for.
    pub description: &'static str,
    /// Iconify name shown in the tree row and the menu action. Effects render
    /// live previews in the picker, so the icon is not what identifies them
    /// there.
    pub icon: &'static str,
    /// Which group of the picker this effect appears under. Presentational
    /// only: nothing in [`Effect`], the layer kind, the compositor or the save
    /// format reads it. Same field and same frontend grouping as
    /// [`BlendModeRegistration::category`](super::blend_mode::BlendModeRegistration).
    pub category: &'static str,
    /// Id of the action that applies this effect destructively to the active
    /// node. Bindings in `presets/*.yaml` name this string; declaring it here
    /// rather than deriving it from `type_id` is what gives those bindings a
    /// compile-time target.
    pub hotkey_action: &'static str,
    pub params: &'static [ParamDef],
    /// How long this effect's preview runs, or `None` for one with nothing
    /// worth showing. Declaring an animation is what makes an effect
    /// previewable — the two facts are one.
    pub preview: Option<PreviewAnim>,
    /// The parameter values this effect's preview shows at `t ∈ [0, 1]`, in
    /// `params` order. `None` is a still at the schema defaults, which is the
    /// honest answer for an effect with nothing to sweep.
    pub preview_at: Option<fn(f32) -> Vec<ParamValue>>,
    /// Target formats this effect's pipeline may be compiled against. A
    /// pipeline is compiled against exactly one format, so this is the list the
    /// registry will build on demand — declaring `R8Unorm` is what lets an
    /// effect run over a mask node.
    pub targets: &'static [wgpu::TextureFormat],
    pub create_pipeline: fn(&wgpu::Device, wgpu::TextureFormat) -> EffectPipeline,
    pub from_params: fn(&[ParamValue], Arc<EffectPipeline>) -> Box<dyn Effect>,
}

/// Every effect renders into the layer/accumulator format; the ones that also
/// declare [`MASK_TARGETS`] can run over an R8 mask node.
pub const COLOR_TARGETS: &[wgpu::TextureFormat] = &[wgpu::TextureFormat::Rgba8Unorm];

/// Color plus R8 — for an effect whose transform is meaningful on a
/// single-channel mask.
pub const MASK_TARGETS: &[wgpu::TextureFormat] = &[
    wgpu::TextureFormat::Rgba8Unorm,
    wgpu::TextureFormat::R8Unorm,
];

/// Id of the catalog this registry projects into.
pub const CATALOG_ID: &str = "effects";

impl EffectRegistration {
    pub fn catalog_entry(&self) -> CatalogEntry {
        CatalogEntry::new(self.type_id, self.display_name)
            .with_icon(self.icon)
            .with_description(self.description)
            .with_category(self.category)
            .with_hotkey_action(self.hotkey_action)
            .with_params(self.params)
            .with_supports_preview(self.preview.is_some())
    }
}

/// The effect catalog, sorted by `(category, display_name)` so a consumer can
/// run-length group it into its category headings without bucketing — the same
/// shape the blend-mode dropdown consumes.
pub fn catalog() -> Catalog {
    let registry = EffectRegistry::new();
    let mut regs = registry.registrations();
    regs.sort_by_key(|reg| (reg.category, reg.display_name));
    Catalog::new(
        CATALOG_ID,
        "Effects",
        regs.iter().map(|reg| reg.catalog_entry()).collect(),
    )
    .with_description("Non-destructive transforms of the image beneath them.")
}

/// Auto-discovered effect registry with lazy per-`(type, format)` pipeline
/// caching. A pipeline is compiled against one target format, so the same
/// effect over a layer and over a mask are two pipelines behind one entry.
pub struct EffectRegistry {
    entries: HashMap<&'static str, EffectRegistration>,
    pipelines: HashMap<(&'static str, wgpu::TextureFormat), Arc<EffectPipeline>>,
}

impl Default for EffectRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl EffectRegistry {
    pub fn new() -> Self {
        let entries = super::effects::registrations()
            .into_iter()
            .map(|reg| (reg.type_id, reg))
            .collect();
        EffectRegistry {
            entries,
            pipelines: HashMap::new(),
        }
    }

    /// Every registration, sorted by `type_id` for a deterministic order.
    /// Callers read whatever fields they need off the registration — a new
    /// field is free here.
    pub fn registrations(&self) -> Vec<&EffectRegistration> {
        let mut regs: Vec<&EffectRegistration> = self.entries.values().collect();
        regs.sort_by_key(|reg| reg.type_id);
        regs
    }

    pub fn get(&self, type_id: &str) -> Option<&EffectRegistration> {
        self.entries.get(type_id)
    }

    /// True when this registry knows the given `type_id`. Used by the `.darkly`
    /// load pre-check to refuse files naming effects the binary doesn't ship —
    /// see [`crate::format::error::LoadError`].
    pub fn has(&self, type_id: &str) -> bool {
        self.entries.contains_key(type_id)
    }

    /// Parameter schema for an effect type, or an empty slice for an unknown
    /// type (or a parameter-free effect).
    pub fn params(&self, type_id: &str) -> &'static [ParamDef] {
        self.entries.get(type_id).map(|e| e.params).unwrap_or(&[])
    }

    /// Resolve a runtime `&str` type id to the registry's `&'static str` key.
    /// Callers keying long-lived state by type id use this to obtain a
    /// `'static` id without leaking.
    pub fn static_type_id(&self, type_id: &str) -> Option<&'static str> {
        self.entries.get_key_value(type_id).map(|(k, _)| *k)
    }

    /// How long an effect type's preview runs. `None` for an unknown type or
    /// one that declares no preview.
    pub fn preview(&self, type_id: &str) -> Option<PreviewAnim> {
        self.entries.get(type_id)?.preview
    }

    /// The parameter values an effect type's preview shows at `t`, in schema
    /// order. Falls back to the schema defaults for an effect that declares no
    /// sweep, so a caller never has to ask whether one exists.
    pub fn preview_params(&self, type_id: &str, t: f32) -> Vec<ParamValue> {
        let Some(reg) = self.entries.get(type_id) else {
            return Vec::new();
        };
        match reg.preview_at {
            Some(at) => at(t),
            None => reg.params.iter().map(ParamDef::default_value).collect(),
        }
    }

    /// Human-friendly display name, falling back to the empty string when the
    /// type is unknown.
    pub fn display_name(&self, type_id: &str) -> &'static str {
        self.entries
            .get(type_id)
            .map(|e| e.display_name)
            .unwrap_or("")
    }

    /// Iconify name, falling back to the empty string when the type is unknown
    /// (callers substitute the generic layer-kind icon).
    pub fn icon(&self, type_id: &str) -> &'static str {
        self.entries.get(type_id).map(|e| e.icon).unwrap_or("")
    }

    /// Whether this effect declares `format` among its targets — the one
    /// question a caller asks before offering it over a mask node.
    pub fn supports_target(&self, type_id: &str, format: wgpu::TextureFormat) -> bool {
        self.entries
            .get(type_id)
            .is_some_and(|reg| reg.targets.contains(&format))
    }

    /// Get or compile the shared pipeline for an effect type at one target
    /// format. `None` for an unknown type, or for a format the effect does not
    /// declare — a caller handed an arbitrary string decides how to fail rather
    /// than tripping pipeline validation mid-frame.
    pub fn pipeline(
        &mut self,
        type_id: &str,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> Option<Arc<EffectPipeline>> {
        let reg = self.entries.get(type_id)?;
        if !reg.targets.contains(&format) {
            return None;
        }
        let key = (reg.type_id, format);
        if let Some(p) = self.pipelines.get(&key) {
            return Some(p.clone());
        }
        let built = Arc::new((reg.create_pipeline)(device, format));
        self.pipelines.insert(key, built.clone());
        Some(built)
    }

    /// Create an effect instance from a type string and parameter values.
    /// `None` on an unknown type or an undeclared target format.
    pub fn instance(
        &mut self,
        type_id: &str,
        params: &[ParamValue],
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> Option<Box<dyn Effect>> {
        let pipeline = self.pipeline(type_id, device, format)?;
        let from_params = self.entries.get(type_id)?.from_params;
        Some(from_params(params, pipeline))
    }
}

// ---------------------------------------------------------------------------
// Preview mechanism
// ---------------------------------------------------------------------------

/// This catalog's answer to [`PreviewMechanism`]. Exported by name so
/// `build.rs` finds it while scanning this module's source and emits a
/// `preview_mechanisms()` row for `effects`.
pub fn preview_mechanism() -> &'static dyn PreviewMechanism {
    &EffectMechanism
}

struct EffectMechanism;

impl PreviewMechanism for EffectMechanism {
    fn resolve(&self, type_id: &str) -> Option<PreviewEntry> {
        let registry = EffectRegistry::new();
        Some(PreviewEntry {
            type_id: registry.static_type_id(type_id)?,
            anim: registry.preview(type_id)?,
        })
    }

    fn reads_source(&self) -> bool {
        true
    }

    fn open<'a>(
        &self,
        regs: PreviewRegistries<'a>,
        type_id: &str,
    ) -> Option<Box<dyn PreviewSession + 'a>> {
        let type_id = regs.effects.static_type_id(type_id)?;
        Some(Box::new(EffectSession {
            registry: regs.effects,
            type_id,
            instance: None,
        }))
    }
}

/// One open effect preview: the instance and the cache it was built against.
///
/// Rebuilding is a normal outcome rather than a failure mode — [`Effect::set_params`]
/// answers `false` when the state it just entered no longer fits the cache, and
/// a rebuilt instance at `t` is fully described by `t`.
struct EffectSession<'a> {
    registry: &'a mut EffectRegistry,
    type_id: &'static str,
    instance: Option<(Box<dyn Effect>, EffectCache)>,
}

impl<'a> EffectSession<'a> {
    /// The one place a preview's cache is built against its target, so the two
    /// callers — the first build and a `set_params` that invalidated its cache
    /// — cannot disagree about what it is built from.
    fn build_cache(
        effect: &mut dyn Effect,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &PreviewTarget,
    ) -> EffectCache {
        let (w, h) = target.size();
        effect.create_cache(device, queue, target.views(), target.sampler(), w, h)
    }
}

impl<'a> PreviewSession for EffectSession<'a> {
    fn set_t(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &PreviewTarget,
        t: f32,
    ) {
        let params = self.registry.preview_params(self.type_id, t);
        if self.instance.is_none() {
            let Some(mut effect) =
                self.registry
                    .instance(self.type_id, &params, device, PREVIEW_FORMAT)
            else {
                return;
            };
            let cache = Self::build_cache(&mut *effect, device, queue, target);
            effect.seek(queue, &cache, t);
            self.instance = Some((effect, cache));
            return;
        }
        let (effect, cache) = self.instance.as_mut().expect("built above");
        if !effect.set_params(queue, cache, &params) {
            *cache = Self::build_cache(&mut **effect, device, queue, target);
        }
        effect.seek(queue, cache, t);
    }

    fn encode(
        &mut self,
        _device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &PreviewTarget,
    ) {
        let Some((effect, cache)) = self.instance.as_ref() else {
            return;
        };
        effect.encode(encoder, cache, 0, target.output_view());
    }
}
