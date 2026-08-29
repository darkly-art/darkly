//! Renders one animated preview per previewable registry entry, to disk.
//!
//! Sixteen blend modes have no icon anywhere, and ten veils deliberately have
//! none either because their picker renders a live thumbnail instead. For all
//! four effect catalogs the image *is* the documentation — and for most of them
//! a still is not enough, because what a control does only becomes legible when
//! it moves. This module renders that motion headlessly against a fixed subject
//! and writes it out as PNG frame sequences plus a small index.
//!
//! **This module renders nothing of its own.** A preview's motion belongs to
//! the entry that has it — `Veil::preview_at`, `Void::preview_at`, a filter
//! registration's `preview_at` — and the driver that runs it is
//! [`crate::gpu::preview`], the same one the editor's pickers go through. What
//! is left here is what only a headless documentation run needs: one fixed
//! subject instead of the user's canvas, a blocking capture sink instead of an
//! asynchronous one, PNGs on disk, and an index beside them.
//!
//! **Two leftover renderers.** A blend mode is a relation between two images
//! rather than an effect over one, so there is no `src → out` mechanism to open
//! for it; it is rendered through a real [`DarklyEngine`] document whose top
//! layer's opacity this module drives directly. A brush is a stroke driven
//! through the brush engine rather than an effect over one image, so it has no
//! mechanism either; it is rendered through the same `BrushStrokePreviewRenderer`
//! and the same framer the editor's picker goes through. Both are further
//! *callers* of the same `PreviewAnim`, not further preview systems.
//!
//! Everything in this module performs blocking GPU readbacks and is therefore
//! gated behind the `testing` feature exactly as `gpu::test_utils` is. Engine,
//! compositor and WASM-bridge code cannot name it in a production build.

pub mod subject;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::brush::pipeline::BrushPipelines;
use crate::brush::preview_renderer::BrushStrokePreviewRenderer;
use crate::catalog::preview_mechanisms;
use crate::engine::DarklyEngine;
use crate::gpu::context::{GpuContext, GpuDevice};
use crate::gpu::filter::FilterPipelineRegistry;
use crate::gpu::preview::{
    close_loop, drive, frame_t, swing, PreviewAnim, PreviewMechanism, PreviewRegistries,
    PreviewSequence, PreviewTarget, PreviewVariant, PREVIEW_FORMAT,
};
use crate::gpu::test_utils::{readback_texture, test_device};
use crate::gpu::veil::VeilRegistry;
use crate::gpu::void::VoidRegistry;
use crate::layer::LayerId;
use subject::{blend_source_rgba, subject_rgba, DOCS_SUBJECT_DIM};

/// The preview target always resamples its source into its own preview-sized
/// texture. Feeding it the subject at twice the output edge puts that resample
/// at the 2:1 ratio its shader was written for, where the four taps land on
/// input texel centres and tile the 2 × 2 block exactly — an area average rather
/// than the softening a 1:1 pass would apply to the very edges the blur,
/// pixelate, painting and aberration previews are read by.
const SUBJECT_SCALE: u32 = 2;

/// How far the blend-mode preview's top layer rises over the backdrop at `t`.
///
/// The one host-layer knob in the tree, and it lives here rather than in a
/// shared vocabulary because blend modes are the only catalog that needs one and
/// the only catalog rendered through a document. It returns to zero, which is
/// why [`crate::gpu::blend_mode::PREVIEW`] declares a closing loop.
pub fn blend_opacity_at(t: f32) -> f32 {
    swing(t)
}

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
    /// A caller named a catalog no registry produces.
    UnknownCatalog(String),
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
            Self::UnknownCatalog(id) => write!(f, "no catalog named `{id}`"),
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

/// What a consumer cannot derive from a directory listing: how big the frames
/// are, how many files, how fast to play them, whether the last frame hands back
/// to the first without a visible jump, and which frame stands for the whole.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Asset {
    pub dir: String,
    /// Frame dimensions. Per-asset rather than per-manifest: an effect is
    /// documented against the fixed square subject, but a brush stroke is a
    /// left-to-right line framed to the picker strip's own shape.
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub fps: u32,
    #[serde(rename = "loop")]
    pub loops: bool,
    /// Index of the poster frame — the one a consumer shows when it is not
    /// playing the sequence, and the same frame the editor's picker renders for
    /// [`PreviewVariant::Still`]. `{still:03}.png` in `dir`.
    pub still: u32,
}

/// A thin index of what was written — not a second copy of the metadata export.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Manifest {
    /// The same value the metadata export stamps, and the only thing that lets
    /// a consumer check that a JSON artifact and an asset directory came from
    /// one build.
    pub version: String,
    /// Catalog id → entry type id → what was written for it.
    pub assets: BTreeMap<String, BTreeMap<String, Asset>>,
}

/// One RGBA8 buffer per frame, in playback order.
pub type Frames = Vec<Vec<u8>>;

/// One entry's rendered frames, with the playback facts its declaration
/// determines.
pub struct Rendered {
    pub frames: Frames,
    /// Dimensions of every frame in [`Self::frames`]. Carried alongside the
    /// pixels because the renderers do not all produce the same shape.
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub loops: bool,
    pub still: u32,
}

impl Rendered {
    /// Turn what a renderer produced into what a consumer receives.
    ///
    /// The three renderers differ in how they get their pixels and in nothing
    /// else, so the two facts that depend on *how the frames were asked for* —
    /// whether a one-way sequence needs its hand-back dissolved in, and where
    /// the poster sits — are settled once, here.
    fn assemble(
        variant: PreviewVariant,
        anim: PreviewAnim,
        frames: Frames,
        width: u32,
        height: u32,
    ) -> Self {
        // A still is its own poster, and one frame has no hand-back to close.
        let (frames, still) = match variant {
            PreviewVariant::Still => (frames, 0),
            PreviewVariant::Animated => (close_loop(anim, frames), anim.still_frame()),
        };
        Rendered {
            frames,
            width,
            height,
            fps: anim.fps,
            loops: anim.emits_a_loop(),
            still,
        }
    }
}

// ---------------------------------------------------------------------------
// Shared GPU state
// ---------------------------------------------------------------------------

/// One device, one preview target, and the one document the blend-mode renderer
/// needs, for the whole run.
///
/// Every `DarklyEngine` construction splices sixteen WGSL arms into the
/// composite shader and compiles it, which on a software rasterizer is real CPU
/// time — and blend modes differ from one another only by an in-place property
/// write, so one document serves the whole catalog. The offscreen catalogs need
/// no document at all.
pub struct Gpu {
    gpu: Arc<GpuDevice>,
    target: PreviewTarget,
    /// Kept alive for the target's loaded source; the downscale has already
    /// consumed it, but dropping the texture it was read from is still wrong.
    subject: Option<wgpu::Texture>,
    veils: VeilRegistry,
    voids: VoidRegistry,
    filters: FilterPipelineRegistry,
    blend_doc: Option<BlendDoc>,
    /// The brush engine's GPU pipelines and its stroke-preview scratch target.
    /// Both are reusable for the whole run and both are expensive to build, for
    /// the same reason `blend_doc` is kept.
    brush: Option<(BrushPipelines, BrushStrokePreviewRenderer)>,
}

/// The theme every documentation brush stroke is rendered in — a white stroke
/// on black, which is also the engine's own default (`preview_theme_fg` /
/// `preview_theme_bg`). Named here rather than read off an engine so a headless
/// run depends on nothing ambient, and stated once so the staged backdrop's
/// tones and the stroke colour cannot drift apart.
const DOCS_STROKE_FG: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const DOCS_STROKE_BG: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// The subject with a second, differently-oriented field stacked over it — the
/// layer whose blend mode and opacity the preview drives.
struct BlendDoc {
    engine: DarklyEngine,
    top: LayerId,
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
            target: PreviewTarget::new(),
            subject: None,
            veils: VeilRegistry::new(),
            voids: VoidRegistry::new(),
            filters: FilterPipelineRegistry::new(),
            blend_doc: None,
            brush: None,
        }
    }

    fn blend_doc(&mut self) -> &mut BlendDoc {
        if self.blend_doc.is_none() {
            let dim = DOCS_SUBJECT_DIM;
            let mut engine = DarklyEngine::new(
                GpuContext::new_headless_shared(Arc::clone(&self.gpu)),
                dim,
                dim,
            );
            let backdrop = engine.paste_image(dim, dim, &subject_rgba(dim), 0, 0, None);
            let top = engine.paste_image(dim, dim, &blend_source_rgba(dim), 0, 0, Some(backdrop));
            self.blend_doc = Some(BlendDoc { engine, top });
        }
        self.blend_doc.as_mut().unwrap()
    }

    /// Fill the preview target with the fixed subject — the documentation run's
    /// one substitution for the editor's live canvas. Built once and reloaded
    /// per entry, because a mechanism that generates its own content wants the
    /// source cleared rather than loaded.
    fn load_subject(&mut self, reads_source: bool) {
        if !reads_source {
            self.target.clear_source(
                &self.gpu.device,
                &self.gpu.queue,
                DOCS_SUBJECT_DIM,
                DOCS_SUBJECT_DIM,
            );
            return;
        }
        let dim = DOCS_SUBJECT_DIM * SUBJECT_SCALE;
        let texture = self.subject.get_or_insert_with(|| {
            let texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("docs-subject"),
                size: wgpu::Extent3d {
                    width: dim,
                    height: dim,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: PREVIEW_FORMAT,
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
            texture
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.target
            .load_source(&self.gpu.device, &self.gpu.queue, &view, dim, dim);
    }
}

// ---------------------------------------------------------------------------
// The two consumers
// ---------------------------------------------------------------------------

impl Gpu {
    /// Render one entry through the shared driver with the blocking sink.
    ///
    /// The whole of what this module contributes: a subject, a `readback_texture`
    /// instead of a `ReadbackScheduler`, and no per-tick budget — the binary
    /// wants every frame now.
    fn render_offscreen(
        &mut self,
        mech: &'static dyn PreviewMechanism,
        catalog: &str,
        type_id: &str,
        variant: PreviewVariant,
    ) -> Result<Rendered, DocsRenderError> {
        let no_recipe = || DocsRenderError::NoRecipe {
            catalog: catalog.to_string(),
            type_id: type_id.to_string(),
        };
        let anim = mech.resolve(type_id).ok_or_else(no_recipe)?.anim;
        self.load_subject(mech.reads_source());

        let device = Arc::clone(&self.gpu);
        let (w, h) = self.target.size();
        let mut frames = Vec::with_capacity(anim.frames as usize);
        {
            let Gpu {
                target,
                veils,
                voids,
                filters,
                ..
            } = self;
            // The binary's counterpart of `Compositor::preview_registries`,
            // destructured here rather than behind a method so `target` stays
            // borrowable alongside it.
            let regs = PreviewRegistries {
                veils,
                voids,
                filters,
            };
            // An animated asset is every frame, with the poster recorded as an
            // index into it rather than written twice; a still asks the same
            // sequence for the one frame the entry nominates.
            let mut seq =
                PreviewSequence::open(mech, regs, type_id, variant).ok_or_else(no_recipe)?;
            drive(
                &mut seq,
                &device.device,
                &device.queue,
                target,
                |encoder, output, _, _| {
                    device.queue.submit([encoder.finish()]);
                    frames.push(readback_texture(
                        &device.device,
                        &device.queue,
                        output,
                        PREVIEW_FORMAT,
                        w,
                        h,
                    ));
                },
            );
        }
        Ok(Rendered::assemble(variant, anim, frames, w, h))
    }

    /// Render one blend mode through a real document, driving the top layer's
    /// opacity — the one thing a consumer without a document cannot do, which is
    /// why this catalog has no offscreen mechanism.
    fn render_blend_mode(
        &mut self,
        type_id: &str,
        variant: PreviewVariant,
    ) -> Result<Rendered, DocsRenderError> {
        let anim: PreviewAnim = crate::gpu::blend_mode::registry()
            .preview(type_id)
            .ok_or_else(|| DocsRenderError::NoRecipe {
                catalog: crate::gpu::blend_mode::CATALOG_ID.to_string(),
                type_id: type_id.to_string(),
            })?;
        let doc = self.blend_doc();
        doc.engine.set_blend_mode(doc.top, type_id);

        // The timeline this catalog is driven over — the whole of it, or the one
        // moment the entry nominates as standing for it.
        let timeline: Vec<f32> = match variant {
            PreviewVariant::Still => vec![anim.still_at],
            PreviewVariant::Animated => (0..anim.frames).map(|i| frame_t(i, anim.frames)).collect(),
        };
        let mut frames = Vec::with_capacity(timeline.len());
        for t in timeline {
            doc.engine.set_opacity(doc.top, blend_opacity_at(t));
            frames.push(doc.engine.test_readback_canvas());
        }
        // The document is reused across every mode, and a mode only ever writes
        // the frames it renders — so leaving the last frame's opacity behind
        // would leak into the next entry's first frame.
        doc.engine.set_opacity(doc.top, 1.0);
        Ok(Rendered::assemble(
            variant,
            anim,
            frames,
            DOCS_SUBJECT_DIM,
            DOCS_SUBJECT_DIM,
        ))
    }

    /// Render one brush's preview stroke — the same synthetic S-curve, through
    /// the same stroke engine and the same framer the editor's picker uses, so
    /// the documentation and the picker show one image of a brush rather than
    /// two.
    ///
    /// A brush is a stroke driven through the brush engine rather than an effect
    /// over one image, so like a blend mode it has no `src → out` mechanism to
    /// open — it is a second caller of the same `PreviewAnim`, not a second
    /// preview system.
    fn render_brush_stroke(
        &mut self,
        type_id: &str,
        variant: PreviewVariant,
    ) -> Result<Rendered, DocsRenderError> {
        let no_recipe = || DocsRenderError::NoRecipe {
            catalog: crate::brush::builtin_brushes::CATALOG_ID.to_string(),
            type_id: type_id.to_string(),
        };
        let anim = crate::brush::builtin_brushes::preview(type_id).ok_or_else(no_recipe)?;

        // Brushes are keyed by file stem in the catalog and by name in the
        // library, and `docs()` is what pairs the two.
        let position = crate::brush::builtin_brushes::docs()
            .iter()
            .position(|(stem, _)| *stem == type_id)
            .ok_or_else(no_recipe)?;
        let mut graph = crate::brush::builtin_brushes::all()
            .swap_remove(position)
            .metadata
            .graph;

        // The same two steps `request_stroke_preview_readback` takes, in the
        // same order, so the artifact and the picker render the same brush.
        graph.apply_preview_overrides();
        let backdrop = crate::brush::graph_capabilities(&graph).preview_backdrop;

        let (rw, rh) = crate::engine::brush_library::BRUSH_STROKE_RENDER_SIZE;
        let inset =
            rw.min(rh) as f32 * crate::engine::brush_library::BRUSH_STROKE_PATH_INSET_FRACTION;
        let path =
            crate::brush::preview_renderer::synthesize_stroke_path(rw as f32, rh as f32, 30, inset);

        let (device, queue) = (&self.gpu.device, &self.gpu.queue);
        let (pipelines, renderer) = self.brush.get_or_insert_with(|| {
            (
                BrushPipelines::new(
                    device,
                    queue,
                    &crate::gpu::selection::selection_mask_bgl(device),
                ),
                BrushStrokePreviewRenderer::new(),
            )
        });
        let texture = renderer
            .render_stroke(
                device,
                queue,
                pipelines,
                &graph,
                &path,
                DOCS_STROKE_FG,
                DOCS_STROKE_BG,
                backdrop,
                rw,
                rh,
                None,
            )
            .ok_or_else(no_recipe)?;
        let pixels = readback_texture(device, queue, texture, PREVIEW_FORMAT, rw, rh);

        let (tw, th) = crate::engine::brush_library::BRUSH_THUMBNAIL_SIZE;
        let framed = crate::engine::rendering::frame_stroke_thumbnail(
            &pixels,
            rw,
            rh,
            tw,
            th,
            backdrop,
            DOCS_STROKE_FG,
            DOCS_STROKE_BG,
        );
        // A brush stroke is one frame either way, so both variants and the
        // closing pass are no-ops here — routed through the shared assembly
        // anyway so no arm of this module is the one that decides for itself
        // what a declaration means.
        Ok(Rendered::assemble(variant, anim, vec![framed], tw, th))
    }
}

/// Render one entry's whole sequence, plus the playback facts its animation
/// determines. The binary and the tests both come through here, so neither
/// re-implements the dispatch.
///
/// A catalog with an offscreen mechanism goes through the shared driver; the
/// two catalogs without go through a document and through the brush engine
/// respectively. A catalog with none of the three is
/// [`DocsRenderError::NoRenderer`] — what a new previewable registry that has
/// not declared a mechanism looks like from here.
pub fn render_entry(
    gpu: &mut Gpu,
    catalog: &str,
    type_id: &str,
    variant: PreviewVariant,
) -> Result<Rendered, DocsRenderError> {
    if let Some((_, mech)) = preview_mechanisms()
        .into_iter()
        .find(|(id, _)| *id == catalog)
    {
        return gpu.render_offscreen(mech, catalog, type_id, variant);
    }
    if catalog == crate::gpu::blend_mode::CATALOG_ID {
        return gpu.render_blend_mode(type_id, variant);
    }
    if catalog == crate::brush::builtin_brushes::CATALOG_ID {
        return gpu.render_brush_stroke(type_id, variant);
    }
    Err(DocsRenderError::NoRenderer {
        catalog: catalog.to_string(),
        type_id: type_id.to_string(),
    })
}

/// The source the offscreen path handed the effect, read back.
///
/// Test-only, and gated for the same reason `PreviewTarget::source_texture` is:
/// nothing in a run reads the source back, and `CONTRIBUTING.md` §No Blocking
/// GPU Readbacks keeps readback surface behind the gate. A value-pinned assertion
/// about what a filter *did* has to compare against what it was *given* — the
/// 2:1 area average of the subject, not the subject itself.
#[cfg(any(test, feature = "testing"))]
pub fn test_source_pixels(gpu: &mut Gpu) -> Vec<u8> {
    gpu.load_subject(true);
    let (w, h) = gpu.target.size();
    readback_texture(
        &gpu.gpu.device,
        &gpu.gpu.queue,
        gpu.target.source_texture(),
        PREVIEW_FORMAT,
        w,
        h,
    )
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

/// Write one frame as JPEG, dropping the alpha channel.
///
/// The stills are photographs of the documentation subject with an effect on
/// them, embedded in markdown at a fraction of their rendered size and committed
/// to this repository — which is the case JPEG is for. As PNG the same images
/// are several hundred kilobytes each, and a repository pays that on every
/// re-render forever. The frames themselves stay lossless; this is the last step
/// before a reader sees them.
///
/// Quality 90 rather than the encoder's default: these are 256 px squares read
/// at 120, where the ringing a lower setting leaves around a pixelate veil's
/// hard block edges is visible.
fn write_jpeg(path: &Path, pixels: &[u8], w: u32, h: u32) -> Result<(), DocsRenderError> {
    let rgb: Vec<u8> = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::Cursor::new(&mut out), 90).encode(
        &rgb,
        w,
        h,
        image::ExtendedColorType::Rgb8,
    )?;
    std::fs::write(path, out)?;
    Ok(())
}

/// Write one catalog's poster frames as JPEG — what markdown in this repository
/// embeds, since a table cannot play a video.
///
/// One frame per entry, not a sequence: [`PreviewVariant::Still`] renders at the
/// moment the entry itself nominates, so this is the same picture the editor's
/// picker shows at rest and the same one the release artifact's poster carries.
///
/// The pixels come from whichever adapter runs this, so re-rendering an
/// unchanged catalog on a different GPU than the last committed run can produce
/// a diff with no visible change. That is why this is a deliberate command
/// rather than a build step: it is run when a catalog gains or loses an entry.
pub fn render_stills(out: &Path, catalog_id: &str) -> Result<Vec<PathBuf>, DocsRenderError> {
    let catalog = crate::catalog::catalogs()
        .into_iter()
        .find(|c| c.id == catalog_id)
        .ok_or_else(|| DocsRenderError::UnknownCatalog(catalog_id.to_string()))?;

    let mut gpu = Gpu::new();
    let mut written = Vec::new();
    for entry in catalog.entries.iter().filter(|e| e.supports_preview) {
        let rendered = render_entry(&mut gpu, catalog.id, entry.type_id, PreviewVariant::Still)?;
        let path = out.join(catalog.id).join(format!("{}.jpg", entry.type_id));
        write_jpeg(
            &path,
            &rendered.frames[rendered.still as usize],
            rendered.width,
            rendered.height,
        )?;
        written.push(path);
    }
    Ok(written)
}

/// Render every previewable catalog entry into `out` and write the index
/// beside them.
///
/// Directory names are the catalog and entry ids themselves — no id literal
/// appears in the layout — so the asset directory and the metadata artifact
/// cannot disagree about what a thing is called.
pub fn render_all(out: &Path) -> Result<Manifest, DocsRenderError> {
    let mut gpu = Gpu::new();
    let mut assets: BTreeMap<String, BTreeMap<String, Asset>> = BTreeMap::new();

    for catalog in crate::catalog::catalogs() {
        for entry in catalog.entries.iter().filter(|e| e.supports_preview) {
            let rendered = render_entry(
                &mut gpu,
                catalog.id,
                entry.type_id,
                PreviewVariant::Animated,
            )?;
            let rel = PathBuf::from(catalog.id).join(entry.type_id);
            write_frames(
                &out.join(&rel),
                &rendered.frames,
                rendered.width,
                rendered.height,
            )?;
            assets.entry(catalog.id.to_string()).or_default().insert(
                entry.type_id.to_string(),
                Asset {
                    dir: rel.to_string_lossy().replace('\\', "/"),
                    width: rendered.width,
                    height: rendered.height,
                    frames: rendered.frames.len() as u32,
                    fps: rendered.fps,
                    loops: rendered.loops,
                    still: rendered.still,
                },
            );
        }
    }

    let manifest = Manifest {
        version: crate::VERSION.to_string(),
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
render_docs — render previews for the registry entries that declare one

USAGE:
    render_docs --out <dir>
    render_docs --stills --catalog <id> [--out <dir>]

OPTIONS:
    --out <dir>      Where to write. Frame sequences and assets.json by
                     default; with --stills, defaults to this repository's own
                     preview directory.
    --stills         Write one JPEG poster per entry instead of a sequence —
                     what markdown in this repository embeds.
    --catalog <id>   Which catalog to render stills for. Required by --stills.
    --help           Print this message
";

/// What a command line asked for.
pub enum Command {
    /// `--help` was asked for; there is no work to do.
    Help,
    /// Every previewable entry, as PNG frame sequences plus an index — the
    /// release artifact.
    Frames { out: PathBuf },
    /// One catalog's poster frames, as JPEG — what this repository's markdown
    /// embeds.
    Stills { out: PathBuf, catalog: String },
}

/// Parse the command line. This lives here rather than in the binary because
/// coverage tooling runs test targets and never executes a `[[bin]]` — anything
/// left inside `fn main` is untestable by construction.
pub fn parse_args(argv: impl Iterator<Item = String>) -> Result<Command, DocsRenderError> {
    let mut out: Option<PathBuf> = None;
    let mut catalog: Option<String> = None;
    let mut stills = false;
    let mut argv = argv.peekable();
    while let Some(arg) = argv.next() {
        let mut value = |what: &str| {
            argv.next()
                .ok_or_else(|| DocsRenderError::Usage(format!("{what} needs a value")))
        };
        match arg.as_str() {
            "--help" | "-h" => return Ok(Command::Help),
            "--stills" => stills = true,
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--catalog" => catalog = Some(value("--catalog")?),
            other => {
                return Err(DocsRenderError::Usage(format!(
                    "unrecognized argument `{other}`"
                )))
            }
        }
    }

    if !stills {
        if catalog.is_some() {
            return Err(DocsRenderError::Usage(
                "--catalog only applies to --stills".into(),
            ));
        }
        return Ok(Command::Frames {
            out: out.ok_or_else(|| DocsRenderError::Usage("--out is required".into()))?,
        });
    }
    Ok(Command::Stills {
        // The default destination is the one the markdown fragments link to, so
        // the writer and the readers cannot disagree about where stills live.
        out: out.unwrap_or_else(|| crate::docs_md::repo_root().join(crate::docs_md::STILLS_DIR)),
        catalog: catalog
            .ok_or_else(|| DocsRenderError::Usage("--stills needs --catalog <id>".into()))?,
    })
}
