//! Text shaping, font registry, and the document→renderer bridge.
//!
//! This is the only text-specific code in the engine. It owns parley's font
//! collection (fontique) and layout contexts, shapes a [`TextProps`] block into
//! positioned glyph runs, and builds a `vello::Scene` from a layer's vector
//! objects — text via `draw_glyphs`, paths via kurbo. Everything else about a
//! vector layer is generic kurbo/peniko geometry the renderer consumes directly.
//!
//! The same glyph runs would feed `vello_cpu`'s `glyph_run` unchanged, so the
//! shaping step is renderer-independent (see the text-tool plan, §6).
//!
//! Bundled font: Noto Sans (SIL Open Font License 1.1), © the Noto Project
//! Authors — <https://github.com/notofonts/latin-greek-cyrillic>.

use std::borrow::Cow;
use std::collections::HashMap;

use kurbo::Affine;
use parley::{
    Alignment, AlignmentOptions, FontContext, FontStack, FontStyle, FontWeight, Layout,
    LayoutContext, LineHeight, PositionedLayoutItem, StyleProperty,
};
use peniko::{Brush, Color, Fill};
use vello::Scene;

use crate::layer::{ObjectSource, TextAlign, TextProps, TextStyle, VectorObject};

/// Curated fonts embedded into the binary so text renders identically on every
/// platform (wasm has no system fonts). Family name → bytes. OS-font
/// enumeration (fontique does it for free on native) and user upload are
/// additive later behind this same registry.
const BUNDLED_FONTS: &[(&str, &[u8])] = &[(
    "Noto Sans",
    include_bytes!("../../resources/fonts/NotoSans-Regular.ttf"),
)];

/// The platform-agnostic font collection plus parley's reusable layout state.
/// Owned by the engine; the shaping/scene-build methods take `&mut self` because
/// parley caches shaping work across calls.
///
/// The layout brush parameter is `()` — we render glyphs with each object's own
/// peniko brush via `draw_glyphs`, so parley never needs to carry color.
pub struct FontRegistry {
    font_cx: FontContext,
    layout_cx: LayoutContext<()>,
    /// Display names of every registered family, surfaced to the UI font picker.
    families: Vec<String>,
    /// Content-addressed cache of the raw SFNT bytes behind every
    /// user-registered font, keyed by the hex content hash. One blob can
    /// register several families, so bytes are keyed by hash (not family) and
    /// deduped — registering the same bytes twice is free. The bundled fallback
    /// (Noto Sans) is deliberately absent: it's binary-resident and never
    /// embedded, and the *absence* of runtime bytes is exactly what excludes it
    /// from `.darkly` embedding.
    font_data: HashMap<String, Vec<u8>>,
    /// Family name → content hash of the blob that registered it, resolving a
    /// `font_family` back to its bytes at save time.
    family_hash: HashMap<String, String>,
}

/// Stable 64-bit FNV-1a content hash of `bytes`, hex-encoded. Used to address
/// font blobs by content: identical bytes always hash the same (dedup), and the
/// hex string is both the `font_data` key and the `fonts/<hash>.ttf` container
/// path. Deterministic across runs — no random seeding — so saved files are
/// reproducible.
fn content_hash(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

impl Default for FontRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl FontRegistry {
    pub fn new() -> Self {
        let mut font_cx = FontContext::new();
        let mut families = Vec::new();
        for (name, bytes) in BUNDLED_FONTS {
            font_cx
                .collection
                .register_fonts(peniko::Blob::from(bytes.to_vec()), None);
            families.push((*name).to_string());
        }
        FontRegistry {
            font_cx,
            layout_cx: LayoutContext::new(),
            families,
            font_data: HashMap::new(),
            family_hash: HashMap::new(),
        }
    }

    /// Family names available to the font picker. Bundled families today; OS
    /// fonts and uploads extend this list later.
    pub fn list_fonts(&self) -> &[String] {
        &self.families
    }

    /// Register a user-supplied font (uploaded `.ttf`/`.otf`, a Google import,
    /// or a font embedded in an opened `.darkly`). Caches the raw bytes under
    /// their content hash so the families they provide can be re-embedded on
    /// save, and returns the family names it contributed so the picker can
    /// refresh. Registering identical bytes twice is free — the byte cache
    /// dedups on the content hash.
    pub fn register_font(&mut self, bytes: Vec<u8>) -> Vec<String> {
        let hash = content_hash(&bytes);
        let registered = self
            .font_cx
            .collection
            .register_fonts(peniko::Blob::from(bytes.clone()), None);
        let mut names = Vec::new();
        for (family_id, _) in registered {
            if let Some(name) = self.font_cx.collection.family_name(family_id) {
                let name = name.to_string();
                if !self.families.contains(&name) {
                    self.families.push(name.clone());
                }
                self.family_hash.insert(name.clone(), hash.clone());
                names.push(name);
            }
        }
        // Only cache the bytes if the blob actually registered a family — a blob
        // parley rejected contributes nothing to embed.
        if !names.is_empty() {
            self.font_data.entry(hash).or_insert(bytes);
        }
        names
    }

    /// Raw SFNT bytes backing `family`, if it was registered from bytes (upload
    /// / Google / embedded). `None` for the binary-resident fallback and generic
    /// families (`sans-serif`) — they have no runtime bytes, which is exactly
    /// what keeps them out of `.darkly` embedding.
    pub fn font_bytes(&self, family: &str) -> Option<&[u8]> {
        let hash = self.family_hash.get(family)?;
        self.font_data.get(hash).map(Vec::as_slice)
    }

    /// The content hash + raw bytes backing `family`, for the save path to emit
    /// one `fonts/<hash>.ttf` blob per unique hash (multiple families may share
    /// one blob → same hash → deduped). `None` when the family has no runtime
    /// bytes (fallback / generic), so it's skipped from embedding naturally.
    pub fn font_blob(&self, family: &str) -> Option<(&str, &[u8])> {
        let hash = self.family_hash.get(family)?;
        let bytes = self.font_data.get(hash)?;
        Some((hash.as_str(), bytes.as_slice()))
    }

    /// Build a `vello::Scene` realizing every object on a vector layer. Text is
    /// shaped here (parley) and emitted as glyph runs; paths are filled/stroked
    /// directly. `layer_transform` is the layer-level gizmo affine, baked in so
    /// the rasterization is crisp at canvas resolution (raster-first).
    pub fn build_scene(&mut self, objects: &[VectorObject], layer_transform: Affine) -> Scene {
        let mut scene = Scene::new();
        for obj in objects {
            let transform = layer_transform * obj.transform;
            match &obj.source {
                ObjectSource::Path(path) => {
                    if let Some(brush) = &obj.fill {
                        scene.fill(Fill::NonZero, transform, brush, None, path);
                    }
                    if let Some((stroke, brush)) = &obj.stroke {
                        scene.stroke(stroke, transform, brush, None, path);
                    }
                }
                ObjectSource::Text(text) => {
                    let fill = obj
                        .fill
                        .clone()
                        .unwrap_or(Brush::Solid(Color::from_rgba8(0, 0, 0, 255)));
                    self.draw_text(&mut scene, text, transform, &fill);
                }
            }
        }
        scene
    }

    /// Shape `text` and emit its glyph runs into `scene` at `transform`.
    fn draw_text(&mut self, scene: &mut Scene, text: &TextProps, transform: Affine, fill: &Brush) {
        let layout = self.shape(text);
        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                let run = glyph_run.run();
                scene
                    .draw_glyphs(run.font())
                    .font_size(run.font_size())
                    .normalized_coords(run.normalized_coords())
                    .brush(fill)
                    .transform(transform)
                    .draw(
                        Fill::NonZero,
                        glyph_run.positioned_glyphs().map(|g| vello::Glyph {
                            id: g.id,
                            x: g.x,
                            y: g.y,
                        }),
                    );
            }
        }
    }

    /// Shape + lay out a [`TextProps`] block into a positioned [`Layout`]. The
    /// requested family falls back to the bundled Noto Sans then the generic
    /// sans-serif, so a family the binary doesn't ship still renders.
    pub fn shape(&mut self, text: &TextProps) -> Layout<()> {
        let stack = format!("{}, Noto Sans, sans-serif", text.font_family);
        let mut builder =
            self.layout_cx
                .ranged_builder(&mut self.font_cx, &text.content, 1.0, true);
        builder.push_default(StyleProperty::FontStack(FontStack::Source(Cow::Owned(
            stack,
        ))));
        builder.push_default(StyleProperty::FontSize(text.size));
        builder.push_default(StyleProperty::LineHeight(LineHeight::FontSizeRelative(
            text.line_height,
        )));
        builder.push_default(StyleProperty::FontWeight(FontWeight::new(text.weight)));
        builder.push_default(StyleProperty::FontStyle(match text.style {
            TextStyle::Normal => FontStyle::Normal,
            TextStyle::Italic => FontStyle::Italic,
        }));

        let mut layout: Layout<()> = builder.build(&text.content);
        // Area text wraps to the box width; point text breaks only at explicit
        // newlines (natural width). Either way, alignment resolves within a
        // width — the box width for area text, the block's own natural width for
        // point text — so the Align control is never inert (a single line still
        // has nothing to shift, which is correct).
        let max_adv = text.box_size.map(|(w, _)| w);
        layout.break_all_lines(max_adv);
        let align_width = max_adv.unwrap_or_else(|| layout.width());
        layout.align(
            Some(align_width),
            to_parley_align(text.align),
            AlignmentOptions::default(),
        );
        layout
    }
}

fn to_parley_align(align: TextAlign) -> Alignment {
    match align {
        TextAlign::Start => Alignment::Start,
        TextAlign::Center => Alignment::Center,
        TextAlign::End => Alignment::End,
        TextAlign::Justified => Alignment::Justify,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_known_string_into_glyphs() {
        let mut reg = FontRegistry::new();
        let text = TextProps::new("Hello".to_string());
        let layout = reg.shape(&text);
        let mut glyph_count = 0usize;
        for line in layout.lines() {
            for item in line.items() {
                if let PositionedLayoutItem::GlyphRun(gr) = item {
                    glyph_count += gr.positioned_glyphs().count();
                }
            }
        }
        // "Hello" is five glyphs in any sane Latin font.
        assert_eq!(glyph_count, 5, "expected one glyph per character");
        assert!(layout.width() > 0.0, "shaped text has positive advance");
    }

    #[test]
    fn lists_the_bundled_family() {
        let reg = FontRegistry::new();
        assert!(reg.list_fonts().iter().any(|f| f == "Noto Sans"));
    }

    /// The `wght`-axis normalized coordinates of the first glyph run when
    /// `text` is shaped — empty for a non-variable face (parley emits no
    /// variation coords). Used by the variable-axis spike to prove a weight
    /// scrub reaches the face as a real variation.
    fn first_run_coords(reg: &mut FontRegistry, text: &TextProps) -> Vec<i16> {
        let layout = reg.shape(text);
        for line in layout.lines() {
            for item in line.items() {
                if let PositionedLayoutItem::GlyphRun(gr) = item {
                    return gr.run().normalized_coords().to_vec();
                }
            }
        }
        Vec::new()
    }

    /// Phase-0 gate for the font-import strategy: parley must honor a `weight`
    /// scrub against a **variable** face as a real axis variation, not snap to a
    /// named static instance. Register one variable family (Cantarell-VF, a CFF2
    /// font with a `wght` 100–800 axis), shape the same string at weight 300 vs
    /// 700, and assert the run's `normalized_coords` (the `wght` axis coord)
    /// actually differ. Passing means a single variable file per family covers
    /// every weight (`css2?family=X:wght@100..900`); failing would force discrete
    /// static-weight imports. See `polished-booping-tulip.md` §Phase-0.
    #[test]
    fn variable_weight_scrub_varies_normalized_coords() {
        let mut reg = FontRegistry::new();
        let bytes = include_bytes!("../../tests/fixtures/fonts/Cantarell-VF.otf").to_vec();
        let families = reg.register_font(bytes);
        let family = families
            .first()
            .expect("Cantarell-VF registers at least one family")
            .clone();

        let mut text = TextProps::new("Weight".to_string());
        text.font_family = family;
        text.size = 48.0;

        text.weight = 300.0;
        let light = first_run_coords(&mut reg, &text);
        text.weight = 700.0;
        let bold = first_run_coords(&mut reg, &text);

        assert!(
            !light.is_empty() && !bold.is_empty(),
            "the variable face must emit variation coords (light={light:?}, bold={bold:?})"
        );
        assert_ne!(
            light, bold,
            "a weight scrub against a variable face must change the wght axis coord \
             — if equal, parley is snapping to a static instance and imports must \
             fetch discrete weights instead"
        );
    }

    fn line_count(layout: &Layout<()>) -> usize {
        layout.lines().count()
    }

    /// x of the first positioned glyph — its alignment offset within the box.
    fn first_glyph_x(layout: &Layout<()>) -> f32 {
        for line in layout.lines() {
            for item in line.items() {
                if let PositionedLayoutItem::GlyphRun(gr) = item {
                    if let Some(g) = gr.positioned_glyphs().next() {
                        return g.x;
                    }
                }
            }
        }
        0.0
    }

    #[test]
    fn area_text_wraps_to_the_box_width() {
        let mut reg = FontRegistry::new();
        let mut text = TextProps::new("the quick brown fox jumps over the lazy dog".to_string());
        text.size = 32.0;

        // Point text: one line (no explicit newlines, no wrap box).
        text.box_size = None;
        assert_eq!(line_count(&reg.shape(&text)), 1, "point text doesn't wrap");

        // A narrow box wraps the same string onto multiple lines.
        text.box_size = Some((120.0, 400.0));
        assert!(
            line_count(&reg.shape(&text)) >= 2,
            "a narrow box wraps the text onto multiple lines",
        );
    }

    #[test]
    fn center_align_offsets_within_a_width() {
        let mut reg = FontRegistry::new();
        let mut text = TextProps::new("hi".to_string());
        text.size = 32.0;
        // A box much wider than the word: centering pushes the first glyph in.
        text.box_size = Some((400.0, 80.0));

        text.align = TextAlign::Start;
        let start_x = first_glyph_x(&reg.shape(&text));

        text.align = TextAlign::Center;
        let center_x = first_glyph_x(&reg.shape(&text));

        assert!(
            center_x > start_x + 1.0,
            "center alignment shifts the first glyph right of start (start={start_x}, center={center_x})",
        );
    }

    #[test]
    fn multiline_point_text_aligns_within_its_natural_width() {
        // Even without a box, alignment resolves within the block's own width:
        // a short second line is centered relative to the long first line.
        let mut reg = FontRegistry::new();
        let mut text = TextProps::new("wwwwwwwwww\ni".to_string());
        text.size = 32.0;
        text.box_size = None;

        text.align = TextAlign::Start;
        let start = reg.shape(&text);

        text.align = TextAlign::Center;
        let center = reg.shape(&text);

        // The short line's glyph (second line) moves right under centering.
        let short_glyph = |layout: &Layout<()>| -> f32 {
            let mut last = 0.0;
            for line in layout.lines() {
                for item in line.items() {
                    if let PositionedLayoutItem::GlyphRun(gr) = item {
                        if let Some(g) = gr.positioned_glyphs().next() {
                            last = g.x;
                        }
                    }
                }
            }
            last
        };
        assert!(
            short_glyph(&center) > short_glyph(&start) + 1.0,
            "the short line centers within the block's natural width",
        );
    }
}
