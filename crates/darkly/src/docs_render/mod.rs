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
//! is left here is what only a headless documentation run needs: a fixed
//! synthetic subject instead of the user's canvas, a blocking capture sink
//! instead of an asynchronous one, PNGs on disk, and an index beside them.
//!
//! **One leftover renderer.** A blend mode is a relation between two images
//! rather than an effect over one, so there is no `src → out` mechanism to open
//! for it; it is rendered through a real [`DarklyEngine`] document whose top
//! layer's opacity this module drives directly. That is a second *caller* of the
//! same `PreviewAnim`, not a second preview system.
//!
//! Everything in this module performs blocking GPU readbacks and is therefore
//! gated behind the `testing` feature exactly as `gpu::test_utils` is. Engine,
//! compositor and WASM-bridge code cannot name it in a production build.

pub mod subject;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::catalog::preview_mechanisms;
use crate::engine::DarklyEngine;
use crate::gpu::context::{GpuContext, GpuDevice};
use crate::gpu::filter::FilterPipelineRegistry;
use crate::gpu::preview::{
    drive, frame_t, swing, PreviewAnim, PreviewMechanism, PreviewRegistries, PreviewSequence,
    PreviewTarget, PREVIEW_FORMAT,
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

/// One entry's rendered frames, with the playback facts the recipe determines.
pub struct Rendered {
    pub frames: Frames,
    pub fps: u32,
    pub loops: bool,
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
}

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
            let mut seq = PreviewSequence::open(mech, regs, type_id).ok_or_else(no_recipe)?;
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
        Ok(Rendered {
            frames,
            fps: anim.fps,
            loops: anim.loops,
        })
    }

    /// Render one blend mode through a real document, driving the top layer's
    /// opacity — the one thing a consumer without a document cannot do, which is
    /// why this catalog has no offscreen mechanism.
    fn render_blend_mode(&mut self, type_id: &str) -> Result<Rendered, DocsRenderError> {
        let anim: PreviewAnim = crate::gpu::blend_mode::registry()
            .preview(type_id)
            .ok_or_else(|| DocsRenderError::NoRecipe {
                catalog: crate::gpu::blend_mode::CATALOG_ID.to_string(),
                type_id: type_id.to_string(),
            })?;
        let doc = self.blend_doc();
        doc.engine.set_blend_mode(doc.top, type_id);

        let mut frames = Vec::with_capacity(anim.frames as usize);
        for i in 0..anim.frames {
            let opacity = blend_opacity_at(frame_t(i, anim.frames));
            doc.engine.set_opacity(doc.top, opacity);
            frames.push(doc.engine.test_readback_canvas());
        }
        // The document is reused across every mode, and a mode only ever writes
        // the frames it renders — so leaving the last frame's opacity behind
        // would leak into the next entry's first frame.
        doc.engine.set_opacity(doc.top, 1.0);
        Ok(Rendered {
            frames,
            fps: anim.fps,
            loops: anim.loops,
        })
    }
}

/// Render one entry's whole sequence, plus the playback facts its animation
/// determines. The binary and the tests both come through here, so neither
/// re-implements the dispatch.
///
/// A catalog with an offscreen mechanism goes through the shared driver; the
/// one catalog without goes through its document. A catalog with neither is
/// [`DocsRenderError::NoRenderer`] — what a new previewable registry that has
/// not declared a mechanism looks like from here.
pub fn render_entry(
    gpu: &mut Gpu,
    catalog: &str,
    type_id: &str,
) -> Result<Rendered, DocsRenderError> {
    if let Some((_, mech)) = preview_mechanisms()
        .into_iter()
        .find(|(id, _)| *id == catalog)
    {
        return gpu.render_offscreen(mech, catalog, type_id);
    }
    if catalog == crate::gpu::blend_mode::CATALOG_ID {
        return gpu.render_blend_mode(type_id);
    }
    Err(DocsRenderError::NoRenderer {
        catalog: catalog.to_string(),
        type_id: type_id.to_string(),
    })
}

/// The source the offscreen path handed the effect, read back.
///
/// Test-only, and gated for the same reason `PreviewTarget::source_texture` is:
/// nothing in a run reads the source back, and `AGENTS.md` §No Blocking GPU
/// Readbacks keeps readback surface behind the gate. A value-pinned assertion
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
