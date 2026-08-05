//! Renders one animated preview per previewable registry entry, to disk.
//!
//! Sixteen blend modes have no icon anywhere, and ten veils deliberately have
//! none either because their picker renders a live thumbnail instead. For all
//! four effect catalogs the image *is* the documentation — and for most of them
//! a still is not enough, because what a control does only becomes legible when
//! it moves. This module renders that motion headlessly against a fixed subject
//! and writes it out as PNG frame sequences plus a small index.
//!
//! **How motion is specified.** Every previewable variant declares a
//! `PreviewRecipe` in its own file. A renderer evaluates the recipe's tracks
//! at `t = i / frames`, applies the resulting values, renders, and keeps the
//! pixels. That loop is identical for all four catalogs, so no renderer here
//! ever asks what kind of thing it is animating.
//!
//! **Two mechanisms, because the effect traits share no invocation contract.**
//! Filters and blend modes are driven through a real [`DarklyEngine`] document
//! and read back from the composite; veils and voids are driven through the
//! shipped offscreen preview renderers. Both are entry points that already
//! exist — nothing here adds a method to a production trait.
//!
//! Everything in this module performs blocking GPU readbacks and is therefore
//! gated behind the `testing` feature exactly as `gpu::test_utils` is. Engine,
//! compositor and WASM-bridge code cannot name it in a production build.

pub mod subject;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::engine::DarklyEngine;
use crate::gpu::context::{GpuContext, GpuDevice};
use crate::gpu::effect::EffectCache;
use crate::gpu::params::{ParamDef, ParamValue};
use crate::gpu::preview_recipe::PreviewSpec;
use crate::gpu::test_utils::{readback_texture, test_device};
use crate::gpu::veil::{Veil, VeilRegistry};
use crate::gpu::veil_preview::VeilPreviewRenderer;
use crate::gpu::void::VoidRegistry;
use crate::gpu::void_preview::VoidPreviewRenderer;
use crate::layer::LayerId;
use subject::{blend_source_rgba, subject_rgba, DOCS_SUBJECT_DIM};

/// Pixel format every asset is rendered and read back in. Matches the
/// compositor's accumulator and the void preview target, so a frame carries the
/// same kind of pixels the editor's own picker shows.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The veil preview renderer always resamples its source into its own
/// preview-sized ping-pong texture. Feeding it the subject at twice the output
/// edge puts that resample at the 2:1 ratio its shader was written for, where
/// the four taps land on input texel centres and tile the 2 × 2 block exactly —
/// an area average rather than the softening a 1:1 pass would apply to the very
/// edges the blur, pixelate, painting and aberration previews are read by.
const VEIL_SOURCE_SCALE: u32 = 2;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum DocsRenderError {
    /// A catalog holds previewable entries but no renderer knows how to draw
    /// them — what a new previewable registry looks like from here.
    NoRenderer {
        catalog: String,
        type_id: String,
    },
    /// An entry claims previewability but its registry hands out no recipe.
    NoRecipe {
        catalog: String,
        type_id: String,
    },
    /// A track named a host-layer property this catalog does not expose, or one
    /// no renderer knows how to apply.
    UnknownLayerKnob {
        catalog: String,
        type_id: String,
        knob: &'static str,
    },
    /// The filter registry refused a type id the catalog listed.
    UnknownFilter(String),
    Usage(String),
    Io(std::io::Error),
    Encode(image::ImageError),
}

impl std::fmt::Display for DocsRenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoRenderer { catalog, type_id } => write!(
                f,
                "`{catalog}/{type_id}` is previewable but `{catalog}` has no renderer"
            ),
            Self::NoRecipe { catalog, type_id } => {
                write!(f, "`{catalog}/{type_id}` declares no preview recipe")
            }
            Self::UnknownLayerKnob {
                catalog,
                type_id,
                knob,
            } => write!(
                f,
                "`{catalog}/{type_id}` drives layer knob `{knob}`, which `{catalog}` cannot apply"
            ),
            Self::UnknownFilter(t) => write!(f, "no filter registered as `{t}`"),
            Self::Usage(m) => write!(f, "{m}"),
            Self::Io(e) => write!(f, "{e}"),
            Self::Encode(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DocsRenderError {}

impl From<std::io::Error> for DocsRenderError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<image::ImageError> for DocsRenderError {
    fn from(e: image::ImageError) -> Self {
        Self::Encode(e)
    }
}

// ---------------------------------------------------------------------------
// The index written beside the frames
// ---------------------------------------------------------------------------

/// The three things a consumer cannot derive from a directory listing: how many
/// files, how fast to play them, and whether the last frame hands back to the
/// first without a visible jump.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Asset {
    pub dir: String,
    pub frames: u32,
    pub fps: u32,
    /// Computed from the recipe, never authored: true exactly when every track
    /// ends where it began.
    #[serde(rename = "loop")]
    pub loops: bool,
}

/// A thin index of what was written — not a second copy of the metadata export.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Manifest {
    /// The same value the metadata export stamps, and the only thing that lets
    /// a consumer check that a JSON artifact and an asset directory came from
    /// one build.
    pub version: String,
    pub width: u32,
    pub height: u32,
    /// Catalog id → entry type id → what was written for it.
    pub assets: BTreeMap<String, BTreeMap<String, Asset>>,
}

/// One RGBA8 buffer per frame, in playback order.
pub type Frames = Vec<Vec<u8>>;

/// How a catalog renders one of its entries: evaluate the recipe at each frame's
/// `t`, apply, render, keep the pixels.
pub type RenderFn = fn(&mut Gpu, &str, PreviewSpec) -> Result<Frames, DocsRenderError>;

/// One entry's rendered frames, with the playback facts the recipe determines.
pub struct Rendered {
    pub frames: Frames,
    pub fps: u32,
    pub loops: bool,
}

// ---------------------------------------------------------------------------
// Shared GPU state
// ---------------------------------------------------------------------------

/// One device and, at most, two documents for the whole run.
///
/// Every `DarklyEngine` construction splices sixteen WGSL arms into the
/// composite shader and compiles it, which on a software rasterizer is real CPU
/// time. Filters differ from one another only by which filter layer sits in the
/// group, and blend modes only by an in-place property write, so one document
/// each serves every asset in its catalog. Each renderer restores its document
/// before the next entry.
pub struct Gpu {
    gpu: Arc<GpuDevice>,
    filter_doc: Option<FilterDoc>,
    blend_doc: Option<BlendDoc>,
    veil: Option<VeilCtx>,
    void: Option<VoidCtx>,
}

/// A backdrop with an isolated group stacked over it, the group holding a copy
/// of the same subject and a filter layer above it.
///
/// The group exists because a filter layer cannot fade: the compositor resolves
/// a filter's pipeline and ping-pongs the group accumulator without ever reading
/// the layer's opacity, so a filter at the root has no opacity knob at all.
/// Isolating it and driving the *group's* opacity crossfades unfiltered →
/// filtered, which is a better demonstration than a fade to transparent and the
/// reason the backdrop copy is there.
struct FilterDoc {
    engine: DarklyEngine,
    group: LayerId,
    filter: Option<LayerId>,
}

/// The subject with a second, differently-oriented field stacked over it — the
/// layer whose blend mode and opacity the recipe drives.
struct BlendDoc {
    engine: DarklyEngine,
    top: LayerId,
}

struct VeilCtx {
    renderer: VeilPreviewRenderer,
    registry: VeilRegistry,
    /// Kept alive for the renderer's loaded source; the downscale has already
    /// consumed it, but dropping the texture it was read from is still wrong.
    _source: wgpu::Texture,
}

struct VoidCtx {
    renderer: VoidPreviewRenderer,
    registry: VoidRegistry,
}

impl Default for Gpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Gpu {
    pub fn new() -> Self {
        let (device, queue) = test_device();
        Gpu {
            #[allow(clippy::arc_with_non_send_sync)] // see GpuDevice's own docs
            gpu: Arc::new(GpuDevice { device, queue }),
            filter_doc: None,
            blend_doc: None,
            veil: None,
            void: None,
        }
    }

    fn engine(&self) -> DarklyEngine {
        DarklyEngine::new(
            GpuContext::new_headless_shared(Arc::clone(&self.gpu)),
            DOCS_SUBJECT_DIM,
            DOCS_SUBJECT_DIM,
        )
    }

    fn filter_doc(&mut self) -> &mut FilterDoc {
        if self.filter_doc.is_none() {
            let dim = DOCS_SUBJECT_DIM;
            let pixels = subject_rgba(dim);
            let mut engine = self.engine();
            let backdrop = engine.paste_image(dim, dim, &pixels, 0, 0, None);
            let group = engine.add_group(Some(backdrop));
            engine.set_group_passthrough(group, false);
            // A group anchor nests rather than siblings, so this lands inside
            // the group and the filter added against the same anchor lands
            // above it — giving the filter something to transform.
            engine.paste_image(dim, dim, &pixels, 0, 0, Some(group));
            self.filter_doc = Some(FilterDoc {
                engine,
                group,
                filter: None,
            });
        }
        self.filter_doc.as_mut().unwrap()
    }

    fn blend_doc(&mut self) -> &mut BlendDoc {
        if self.blend_doc.is_none() {
            let dim = DOCS_SUBJECT_DIM;
            let mut engine = self.engine();
            let backdrop = engine.paste_image(dim, dim, &subject_rgba(dim), 0, 0, None);
            let top = engine.paste_image(dim, dim, &blend_source_rgba(dim), 0, 0, Some(backdrop));
            self.blend_doc = Some(BlendDoc { engine, top });
        }
        self.blend_doc.as_mut().unwrap()
    }

    fn veil_ctx(&mut self) -> &mut VeilCtx {
        if self.veil.is_none() {
            let dim = DOCS_SUBJECT_DIM * VEIL_SOURCE_SCALE;
            let texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("docs-veil-source"),
                size: wgpu::Extent3d {
                    width: dim,
                    height: dim,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.gpu.queue.write_texture(
                texture.as_image_copy(),
                &subject_rgba(dim),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(dim * 4),
                    rows_per_image: Some(dim),
                },
                wgpu::Extent3d {
                    width: dim,
                    height: dim,
                    depth_or_array_layers: 1,
                },
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let mut renderer = VeilPreviewRenderer::new();
            renderer.load_source(&self.gpu.device, &self.gpu.queue, &view, dim, dim, FORMAT);
            self.veil = Some(VeilCtx {
                renderer,
                registry: VeilRegistry::new(),
                _source: texture,
            });
        }
        self.veil.as_mut().unwrap()
    }

    fn void_ctx(&mut self) -> &mut VoidCtx {
        if self.void.is_none() {
            self.void = Some(VoidCtx {
                renderer: VoidPreviewRenderer::new(),
                registry: VoidRegistry::new(),
            });
        }
        self.void.as_mut().unwrap()
    }
}

// ---------------------------------------------------------------------------
// The recipe loop, four times
// ---------------------------------------------------------------------------

/// Normalized timeline position of frame `i` of `frames`. Frame `frames` itself
/// is `t == 1.0` — the frame *after* the last, which is where a looping recipe
/// hands back to frame 0.
pub fn frame_t(i: u32, frames: u32) -> f32 {
    i as f32 / frames.max(1) as f32
}

/// Write one host-layer property. The only place a knob name meets the document,
/// so a knob that is declared but not wired here is a loud error rather than a
/// value that quietly does nothing.
fn apply_knob(
    engine: &mut DarklyEngine,
    host: LayerId,
    name: &'static str,
    value: ParamValue,
    catalog: &str,
    type_id: &str,
) -> Result<(), DocsRenderError> {
    match (name, value) {
        ("opacity", ParamValue::Float(v)) => engine.set_opacity(host, v),
        (other, _) => {
            return Err(DocsRenderError::UnknownLayerKnob {
                catalog: catalog.to_string(),
                type_id: type_id.to_string(),
                knob: other,
            })
        }
    }
    Ok(())
}

/// Return every host-layer knob to its schema default.
///
/// The documents are reused across every entry in their catalog, and a recipe
/// only writes the knobs it drives — so without this an entry would inherit
/// whatever its predecessor's last frame happened to leave behind. Driven off
/// the catalog's own knob set rather than a list here, so a second knob is
/// restored by the same code that introduced it.
fn reset_layer_knobs(
    engine: &mut DarklyEngine,
    host: LayerId,
    spec: PreviewSpec,
    catalog: &str,
    type_id: &str,
) -> Result<(), DocsRenderError> {
    for def in spec.layer_knobs {
        apply_knob(
            engine,
            host,
            def.name,
            def.default_value(),
            catalog,
            type_id,
        )?;
    }
    Ok(())
}

/// Push every host-layer knob the recipe drives at `t` at the document. The one
/// thing that differs between the two document renderers is which layer is the
/// host, and that is a local binding rather than a branch.
fn apply_layer_knobs(
    engine: &mut DarklyEngine,
    host: LayerId,
    spec: PreviewSpec,
    t: f32,
    catalog: &str,
    type_id: &str,
) -> Result<(), DocsRenderError> {
    let knobs = spec.recipe.layer_at(spec.layer_knobs, t).map_err(|e| {
        DocsRenderError::UnknownLayerKnob {
            catalog: catalog.to_string(),
            type_id: type_id.to_string(),
            knob: e.0,
        }
    })?;
    for (name, value) in knobs {
        apply_knob(engine, host, name, value, catalog, type_id)?;
    }
    Ok(())
}

fn render_filter(
    gpu: &mut Gpu,
    type_id: &str,
    spec: PreviewSpec,
) -> Result<Frames, DocsRenderError> {
    let recipe = spec.recipe;
    let doc = gpu.filter_doc();
    if let Some(prev) = doc.filter.take() {
        let _ = doc.engine.remove_layer(prev);
    }
    let defs = doc.engine.filter_param_defs(type_id);
    let id = doc
        .engine
        .add_filter_layer(
            type_id,
            defs.iter().map(ParamDef::default_value).collect(),
            Some(doc.group),
        )
        .ok_or_else(|| DocsRenderError::UnknownFilter(type_id.to_string()))?;
    doc.filter = Some(id);
    reset_layer_knobs(
        &mut doc.engine,
        doc.group,
        spec,
        crate::gpu::filter::CATALOG_ID,
        type_id,
    )?;

    let mut frames = Vec::with_capacity(recipe.frames as usize);
    for i in 0..recipe.frames {
        let t = frame_t(i, recipe.frames);
        doc.engine
            .update_filter_params(id, recipe.params_at(defs, t));
        apply_layer_knobs(
            &mut doc.engine,
            doc.group,
            spec,
            t,
            crate::gpu::filter::CATALOG_ID,
            type_id,
        )?;
        frames.push(doc.engine.test_readback_canvas());
    }
    Ok(frames)
}

fn render_blend_mode(
    gpu: &mut Gpu,
    type_id: &str,
    spec: PreviewSpec,
) -> Result<Frames, DocsRenderError> {
    let recipe = spec.recipe;
    let doc = gpu.blend_doc();
    doc.engine.set_blend_mode(doc.top, type_id);
    reset_layer_knobs(
        &mut doc.engine,
        doc.top,
        spec,
        crate::gpu::blend_mode::CATALOG_ID,
        type_id,
    )?;

    let mut frames = Vec::with_capacity(recipe.frames as usize);
    for i in 0..recipe.frames {
        let t = frame_t(i, recipe.frames);
        apply_layer_knobs(
            &mut doc.engine,
            doc.top,
            spec,
            t,
            crate::gpu::blend_mode::CATALOG_ID,
            type_id,
        )?;
        frames.push(doc.engine.test_readback_canvas());
    }
    Ok(frames)
}

fn render_veil(gpu: &mut Gpu, type_id: &str, spec: PreviewSpec) -> Result<Frames, DocsRenderError> {
    let recipe = spec.recipe;
    let device = Arc::clone(&gpu.gpu);
    let ctx = gpu.veil_ctx();
    let defs = ctx.registry.param_defs(type_id);
    let (w, h) = ctx.renderer.preview_size();

    // A veil has no in-place parameter update — the shipped path rebuilds — so
    // a frame whose evaluated parameters differ from the previous frame's
    // rebuilds through the registry, reusing the renderer's own ping-pong
    // textures. Rebuilding also resets the instance's clock, which is why a
    // veil recipe may not combine a time track with parameter tracks.
    let mut instance: Option<(Box<dyn Veil>, EffectCache)> = None;
    let mut applied: Option<Vec<ParamValue>> = None;
    let mut frames = Vec::with_capacity(recipe.frames as usize);

    for i in 0..recipe.frames {
        let t = frame_t(i, recipe.frames);
        let params = recipe.params_at(defs, t);
        if applied.as_ref() != Some(&params) {
            instance = Some(ctx.renderer.build_veil(
                &device.device,
                &device.queue,
                &mut ctx.registry,
                type_id,
                &params,
                FORMAT,
            ));
            applied = Some(params);
        }
        let (veil, cache) = instance.as_mut().expect("built on the first frame");

        if i > 0 {
            let dt = recipe.time_at(t) - recipe.time_at(frame_t(i - 1, recipe.frames));
            veil.update_time(&device.queue, cache, dt);
        }

        let mut encoder = device
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("docs-veil-frame"),
            });
        ctx.renderer
            .encode_frame(&mut encoder, veil.as_ref(), cache);
        device.queue.submit([encoder.finish()]);
        frames.push(readback_texture(
            &device.device,
            &device.queue,
            ctx.renderer.output_texture(),
            FORMAT,
            w,
            h,
        ));
    }
    Ok(frames)
}

fn render_void(gpu: &mut Gpu, type_id: &str, spec: PreviewSpec) -> Result<Frames, DocsRenderError> {
    let recipe = spec.recipe;
    let dim = DOCS_SUBJECT_DIM;
    let device = Arc::clone(&gpu.gpu);
    let ctx = gpu.void_ctx();
    let defs = ctx.registry.param_defs(type_id);

    // Voids generate their own content, so there is no source and no resample —
    // and unlike a veil, a void updates its parameters in place, which keeps its
    // cache and its clock alive across the whole sequence.
    let (mut void, cache) = ctx.renderer.build_void(
        &device.device,
        &device.queue,
        &mut ctx.registry,
        type_id,
        &recipe.params_at(defs, 0.0),
        dim,
        dim,
        FORMAT,
    );
    let (w, h) = ctx.renderer.preview_size();

    let mut frames = Vec::with_capacity(recipe.frames as usize);
    for i in 0..recipe.frames {
        let t = frame_t(i, recipe.frames);
        void.update_params(&device.queue, &cache, &recipe.params_at(defs, t));
        if i > 0 {
            let dt = recipe.time_at(t) - recipe.time_at(frame_t(i - 1, recipe.frames));
            void.update_time(&device.queue, &cache, dt);
        }

        let mut encoder = device
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("docs-void-frame"),
            });
        ctx.renderer
            .encode_frame(&mut encoder, void.as_ref(), &cache);
        device.queue.submit([encoder.finish()]);
        frames.push(readback_texture(
            &device.device,
            &device.queue,
            ctx.renderer.output_texture(),
            FORMAT,
            w,
            h,
        ));
    }
    Ok(frames)
}

// ---------------------------------------------------------------------------
// The walk
// ---------------------------------------------------------------------------

/// One previewable registry: the catalog id it publishes under, the lookups that
/// answer "does this entry declare a recipe, what knobs may it name, and what is
/// its parameter schema?", and the mechanism that renders its entries.
///
/// One row per registry — the same granularity the catalog projection itself
/// accepts — and the single place the walk, the four renderers and the
/// recipe-validity tests all read, so none of them carries a list of its own.
/// Adding a filter, veil, void or blend mode touches nothing here.
pub struct CatalogRenderer {
    pub id: &'static str,
    pub spec: fn(&str) -> Option<PreviewSpec>,
    pub defs: fn(&str) -> &'static [ParamDef],
    pub render: RenderFn,
}

pub const CATALOG_RENDERERS: &[CatalogRenderer] = &[
    CatalogRenderer {
        id: crate::gpu::filter::CATALOG_ID,
        spec: |t| crate::gpu::filter::FilterPipelineRegistry::new().preview(t),
        defs: |t| crate::gpu::filter::FilterPipelineRegistry::new().params(t),
        render: render_filter,
    },
    CatalogRenderer {
        id: crate::gpu::veil::CATALOG_ID,
        spec: |t| VeilRegistry::new().preview(t),
        defs: |t| VeilRegistry::new().param_defs(t),
        render: render_veil,
    },
    CatalogRenderer {
        id: crate::gpu::void::CATALOG_ID,
        spec: |t| VoidRegistry::new().preview(t),
        defs: |t| VoidRegistry::new().param_defs(t),
        render: render_void,
    },
    CatalogRenderer {
        id: crate::gpu::blend_mode::CATALOG_ID,
        spec: |t| crate::gpu::blend_mode::registry().preview(t),
        defs: |_| &[],
        render: render_blend_mode,
    },
];

fn renderer_for(catalog: &str, type_id: &str) -> Result<&'static CatalogRenderer, DocsRenderError> {
    CATALOG_RENDERERS
        .iter()
        .find(|c| c.id == catalog)
        .ok_or_else(|| DocsRenderError::NoRenderer {
            catalog: catalog.to_string(),
            type_id: type_id.to_string(),
        })
}

/// Render one entry's whole sequence, plus the playback facts its recipe
/// determines. The binary and the tests both come through here, so neither
/// re-implements the dispatch.
pub fn render_entry(
    gpu: &mut Gpu,
    catalog: &str,
    type_id: &str,
) -> Result<Rendered, DocsRenderError> {
    let cr = renderer_for(catalog, type_id)?;
    let spec = (cr.spec)(type_id).ok_or_else(|| DocsRenderError::NoRecipe {
        catalog: catalog.to_string(),
        type_id: type_id.to_string(),
    })?;
    Ok(Rendered {
        frames: (cr.render)(gpu, type_id, spec)?,
        fps: spec.recipe.fps,
        loops: spec.recipe.loops(),
    })
}

/// Write one entry's frames as zero-padded PNGs, creating `dir`.
fn write_frames(dir: &Path, frames: &[Vec<u8>], w: u32, h: u32) -> Result<(), DocsRenderError> {
    use image::ImageEncoder;
    std::fs::create_dir_all(dir)?;
    for (i, pixels) in frames.iter().enumerate() {
        let mut out = Vec::new();
        image::codecs::png::PngEncoder::new(std::io::Cursor::new(&mut out)).write_image(
            pixels,
            w,
            h,
            image::ExtendedColorType::Rgba8,
        )?;
        std::fs::write(dir.join(format!("{i:03}.png")), out)?;
    }
    Ok(())
}

/// Render every previewable catalog entry into `out` and write the index
/// beside them.
///
/// Directory names are the catalog and entry ids themselves — no id literal
/// appears in the layout — so the asset directory and the metadata artifact
/// cannot disagree about what a thing is called.
pub fn render_all(out: &Path) -> Result<Manifest, DocsRenderError> {
    let dim = DOCS_SUBJECT_DIM;
    let mut gpu = Gpu::new();
    let mut assets: BTreeMap<String, BTreeMap<String, Asset>> = BTreeMap::new();

    for catalog in crate::catalog::catalogs() {
        for entry in catalog.entries.iter().filter(|e| e.supports_preview) {
            let rendered = render_entry(&mut gpu, catalog.id, entry.type_id)?;
            let rel = PathBuf::from(catalog.id).join(entry.type_id);
            write_frames(&out.join(&rel), &rendered.frames, dim, dim)?;
            assets.entry(catalog.id.to_string()).or_default().insert(
                entry.type_id.to_string(),
                Asset {
                    dir: rel.to_string_lossy().replace('\\', "/"),
                    frames: rendered.frames.len() as u32,
                    fps: rendered.fps,
                    loops: rendered.loops,
                },
            );
        }
    }

    let manifest = Manifest {
        version: crate::VERSION.to_string(),
        width: dim,
        height: dim,
        assets,
    };
    std::fs::create_dir_all(out)?;
    std::fs::write(
        out.join("assets.json"),
        serde_json::to_string_pretty(&manifest).expect("the manifest is plain data"),
    )?;
    Ok(manifest)
}

// ---------------------------------------------------------------------------
// Arguments
// ---------------------------------------------------------------------------

pub const USAGE: &str = "\
render_docs — render an animated preview for every previewable registry entry

USAGE:
    render_docs --out <dir>

OPTIONS:
    --out <dir>    Directory to write frame sequences and assets.json into
    --help         Print this message
";

pub struct Args {
    /// `None` when `--help` was asked for and there is no work to do.
    pub out: Option<PathBuf>,
}

/// Parse the command line. This lives here rather than in the binary because
/// coverage tooling runs test targets and never executes a `[[bin]]` — anything
/// left inside `fn main` is untestable by construction.
pub fn parse_args(argv: impl Iterator<Item = String>) -> Result<Args, DocsRenderError> {
    let mut out = None;
    let mut argv = argv.peekable();
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(Args { out: None }),
            "--out" => {
                out = Some(PathBuf::from(argv.next().ok_or_else(|| {
                    DocsRenderError::Usage("--out needs a directory".into())
                })?))
            }
            other => {
                return Err(DocsRenderError::Usage(format!(
                    "unrecognized argument `{other}`"
                )))
            }
        }
    }
    Ok(Args {
        out: Some(out.ok_or_else(|| DocsRenderError::Usage("--out is required".into()))?),
    })
}
