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
    /// Display names of the bundled families, surfaced to the UI font picker.
    families: Vec<String>,
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
        }
    }

    /// Family names available to the font picker. Bundled families today; OS
    /// fonts and uploads extend this list later.
    pub fn list_fonts(&self) -> &[String] {
        &self.families
    }

    /// Register a user-supplied font (e.g. uploaded `.ttf`/`.otf`). Returns the
    /// family names it contributed so the picker can refresh.
    pub fn register_font(&mut self, bytes: Vec<u8>) -> Vec<String> {
        let registered = self
            .font_cx
            .collection
            .register_fonts(peniko::Blob::from(bytes), None);
        let mut names = Vec::new();
        for (family_id, _) in registered {
            if let Some(name) = self.font_cx.collection.family_name(family_id) {
                let name = name.to_string();
                if !self.families.contains(&name) {
                    self.families.push(name.clone());
                }
                names.push(name);
            }
        }
        names
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
        layout.break_all_lines(None);
        layout.align(
            None,
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
}
