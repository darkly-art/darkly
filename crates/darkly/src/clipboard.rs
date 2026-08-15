//! Clipboard system — typed internal clipboard with extensible content types.
//!
//! Two flavours of clipboard payload:
//! - [`ImageClip`] — flat RGBA pixel buffer. The cross-application interop
//!   path: a copied layer round-trips through a PNG on the system clipboard.
//! - [`LayerClipboard`] — full layer with blend mode, opacity, name, and
//!   pixel data. The cross-tab interop path: the multi-tab editor writes
//!   this alongside the PNG via a `web application/x-darkly-layer` custom
//!   MIME type so paste into another Darkly tab restores blend mode +
//!   opacity that PNG can't carry.
//!
//! Both go through the same async GPU readback pipeline. Mask pixel data
//! (R8) is not yet captured in `LayerClipboard` v1 — it requires a second
//! readback in parallel and lands in v2. The schema-version field exists
//! so the deserializer can warn loudly when it sees a future version.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Clipboard enum — extensible content container
// ---------------------------------------------------------------------------

/// Typed clipboard content. New variants can be added for future content types
/// (e.g. layer groups) without refactoring the clipboard system.
pub enum Clipboard {
    /// Flattened RGBA pixel region — used for canvas copy/paste and external interop.
    ImageData(ImageClip),
    /// Layer-with-metadata — used for cross-tab paste in the multi-tab
    /// editor. Carries blend mode + opacity + name + pixels so the
    /// receiving tab can recreate the source layer faithfully.
    Layer(LayerClipboard),
    // Future variants (not implemented):
    // LayerGroup(GroupClip),   — group with children
}

impl Clipboard {
    /// Extract an `ImageClip` reference. Returns `None` for richer variants —
    /// callers that want pixels-only fall back to the system PNG path.
    pub fn as_image(&self) -> Option<&ImageClip> {
        match self {
            Clipboard::ImageData(clip) => Some(clip),
            Clipboard::Layer(_) => None,
        }
    }

    pub fn as_layer(&self) -> Option<&LayerClipboard> {
        match self {
            Clipboard::Layer(l) => Some(l),
            _ => None,
        }
    }

    /// The clip's pixels as `(rgba, width, height, offset_x, offset_y)`,
    /// regardless of variant. A flat `ImageData` clip returns its buffer
    /// directly; a rich `Layer` clip decodes its base64 pixels. Used by the
    /// paste-in-place floating path so it works for the `Layer` clip a normal
    /// copy produces, not just flat image clips.
    ///
    /// Trimmed to the pasted object — see [`trim_to_content`]. What was copied
    /// is the region the user swept, but what is *pasted* is the thing inside
    /// it, and the floating session draws its bounding box from these
    /// dimensions: untrimmed, a select-all copy hands the transform gizmo a
    /// canvas-sized box around a small stroke.
    ///
    /// Returns `None` if a rich clip's pixels are malformed, or if the clip is
    /// entirely transparent and so has nothing to paste.
    pub fn paste_pixels(&self) -> Option<(Vec<u8>, u32, u32, i32, i32)> {
        let (rgba, width, height, x, y) = match self {
            Clipboard::ImageData(c) => (c.data.clone(), c.width, c.height, c.offset_x, c.offset_y),
            Clipboard::Layer(l) => {
                let pixels = l.decode_pixels().ok()?;
                if pixels.len() != (l.bounds.width * l.bounds.height * 4) as usize {
                    return None;
                }
                (
                    pixels,
                    l.bounds.width,
                    l.bounds.height,
                    l.bounds.x,
                    l.bounds.y,
                )
            }
        };
        trim_to_content(&rgba, width, height, x, y)
    }
}

/// Shrink a straight-alpha RGBA region to the bounding box of its
/// non-transparent pixels, moving the origin so the content keeps its position.
/// `None` when every pixel is transparent.
pub fn trim_to_content(
    rgba: &[u8],
    width: u32,
    height: u32,
    offset_x: i32,
    offset_y: i32,
) -> Option<(Vec<u8>, u32, u32, i32, i32)> {
    let (w, h) = (width as usize, height as usize);
    if rgba.len() < w * h * 4 {
        return None;
    }
    let (mut min_x, mut min_y) = (w, h);
    let (mut max_x, mut max_y) = (0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            if rgba[(y * w + x) * 4 + 3] != 0 {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if min_x > max_x || min_y > max_y {
        return None;
    }
    if (min_x, min_y, max_x, max_y) == (0, 0, w - 1, h - 1) {
        return Some((rgba.to_vec(), width, height, offset_x, offset_y));
    }

    let (tw, th) = (max_x - min_x + 1, max_y - min_y + 1);
    let mut out = Vec::with_capacity(tw * th * 4);
    for y in min_y..=max_y {
        let row = (y * w + min_x) * 4;
        out.extend_from_slice(&rgba[row..row + tw * 4]);
    }
    Some((
        out,
        tw as u32,
        th as u32,
        offset_x + min_x as i32,
        offset_y + min_y as i32,
    ))
}

// ---------------------------------------------------------------------------
// ImageClip — flattened RGBA pixel region
// ---------------------------------------------------------------------------

/// A rectangular region of RGBA pixels stored as a flat buffer.
/// Created by GPU readback (copy), consumed by write_texture (paste).
pub struct ImageClip {
    /// Flat RGBA pixel data, row-major, width * height * 4 bytes.
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub offset_x: i32,
    pub offset_y: i32,
}

impl ImageClip {
    /// Create an `ImageClip` from raw RGBA bytes (e.g. from GPU readback or external paste).
    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>, offset_x: i32, offset_y: i32) -> Self {
        debug_assert_eq!(rgba.len(), (width * height * 4) as usize);
        ImageClip {
            data: rgba,
            width,
            height,
            offset_x,
            offset_y,
        }
    }

    /// Export the clip to a contiguous RGBA byte buffer for JS-side PNG encoding.
    ///
    /// Returns `(bytes, width, height, offset_x, offset_y)`.
    pub fn to_rgba(&self) -> (&[u8], u32, u32, i32, i32) {
        (
            &self.data,
            self.width,
            self.height,
            self.offset_x,
            self.offset_y,
        )
    }

    /// Returns true if this clip has no pixel data.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

// ---------------------------------------------------------------------------
// LayerClipboard — layer with blend mode, opacity, name, and pixels
// ---------------------------------------------------------------------------

/// Bumped on any breaking change to the on-the-wire representation. Cross-tab
/// paste between mismatched Darkly versions is best-effort — pre-release we
/// just accept that and refuse anything we don't understand.
pub const LAYER_CLIPBOARD_SCHEMA_VERSION: u32 = 1;

/// Rich clipboard payload for a single raster layer. The cross-tab paste
/// path round-trips this through the system clipboard's `web application/
/// x-darkly-layer` custom MIME, alongside a standard `image/png` so paste
/// into other apps still works.
///
/// Pixel data is base64-encoded inline. That inflates payload size by ~33%
/// vs. raw bytes, but keeps the JSON envelope self-contained and trivially
/// pumpable through `navigator.clipboard.write`/`read`. A 1024×1024 RGBA
/// layer is ~5.5 MiB after base64 — acceptable for clipboards.
#[derive(Serialize, Deserialize, Clone)]
pub struct LayerClipboard {
    pub schema_version: u32,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub opacity: f32,
    /// Stable [`crate::gpu::blend_mode`] type-id (e.g. `"normal"`, `"multiply"`).
    pub blend_mode: String,
    pub bounds: ClipboardRect,
    /// Base64-encoded raw RGBA8 pixels, row-major, `width * height * 4`
    /// bytes after decode. Straight alpha (Darkly never premultiplies — see
    /// `docs/lessons-learned/compositing-lessons-learned.md §1`).
    pub pixels_b64: String,
    /// Mask metadata if the source had one. Pixel data is **not** captured
    /// in v1 — restoring rebuilds an empty (fully opaque) mask with the
    /// recorded bounds. v2 will add R8 pixels via a parallel readback.
    pub mask: Option<MaskClipboard>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct MaskClipboard {
    pub name: String,
    pub visible: bool,
    pub bounds: ClipboardRect,
    /// Reserved for v2. Empty in v1 payloads.
    #[serde(default)]
    pub pixels_b64: String,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
pub struct ClipboardRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl LayerClipboard {
    /// Decode the base64 pixel payload back to raw RGBA bytes.
    pub fn decode_pixels(&self) -> Result<Vec<u8>, base64::DecodeError> {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        STANDARD.decode(&self.pixels_b64)
    }

    /// Serialize to JSON for transport over the system clipboard.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("LayerClipboard serializes infallibly")
    }

    /// Parse a JSON envelope produced by [`Self::to_json`]. Rejects payloads
    /// from a future schema version — pre-release we don't carry forward
    /// shims for formats we haven't shipped yet.
    pub fn from_json(s: &str) -> Result<Self, String> {
        let parsed: LayerClipboard = serde_json::from_str(s).map_err(|e| e.to_string())?;
        if parsed.schema_version > LAYER_CLIPBOARD_SCHEMA_VERSION {
            return Err(format!(
                "LayerClipboard schema_version {} is newer than this build's {}",
                parsed.schema_version, LAYER_CLIPBOARD_SCHEMA_VERSION
            ));
        }
        Ok(parsed)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// What a paste puts down is the object, not the region the user swept to
    /// copy it — a select-all copy of one small dab must not paste a
    /// canvas-sized rect, because the floating session takes its bounding box
    /// from these dimensions.
    #[test]
    fn trim_to_content_shrinks_to_opaque_pixels_and_shifts_the_origin() {
        // 4×4, transparent but for one opaque texel at (2, 1).
        let mut rgba = vec![0u8; 4 * 4 * 4];
        let i = (1 * 4 + 2) * 4;
        rgba[i..i + 4].copy_from_slice(&[10, 20, 30, 255]);

        let (out, w, h, x, y) = trim_to_content(&rgba, 4, 4, 100, 200).expect("has content");
        assert_eq!((w, h), (1, 1));
        // Origin moves by the trimmed margin so the pixel keeps its position.
        assert_eq!((x, y), (102, 201));
        assert_eq!(out, vec![10, 20, 30, 255]);
    }

    #[test]
    fn trim_to_content_keeps_a_full_bleed_clip_intact() {
        let rgba = vec![255u8; 3 * 2 * 4];
        let (out, w, h, x, y) = trim_to_content(&rgba, 3, 2, -5, 7).expect("has content");
        assert_eq!((w, h, x, y), (3, 2, -5, 7));
        assert_eq!(out.len(), rgba.len());
    }

    /// Nothing opaque means nothing to paste — the caller treats `None` as "no
    /// paste happened" rather than floating an empty rect.
    #[test]
    fn trim_to_content_rejects_a_fully_transparent_clip() {
        assert!(trim_to_content(&vec![0u8; 2 * 2 * 4], 2, 2, 0, 0).is_none());
    }

    #[test]
    fn round_trip_rgba() {
        let w = 4u32;
        let h = 4u32;
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for i in 0..16 {
            rgba[i * 4] = 255; // R
            rgba[i * 4 + 3] = 255; // A
        }

        let clip = ImageClip::from_rgba(w, h, rgba.clone(), 10, 20);
        assert_eq!(clip.width, 4);
        assert_eq!(clip.height, 4);
        assert_eq!(clip.offset_x, 10);
        assert_eq!(clip.offset_y, 20);

        let (out, ow, oh, ox, oy) = clip.to_rgba();
        assert_eq!((ow, oh), (4, 4));
        assert_eq!((ox, oy), (10, 20));
        assert_eq!(out[0], 255); // R
        assert_eq!(out[1], 0); // G
        assert_eq!(out[2], 0); // B
        assert_eq!(out[3], 255); // A
    }

    #[test]
    fn empty_clip() {
        let clip = ImageClip::from_rgba(0, 0, vec![], 0, 0);
        assert!(clip.is_empty());
    }

    #[test]
    fn layer_clipboard_roundtrips_through_json() {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let clip = LayerClipboard {
            schema_version: LAYER_CLIPBOARD_SCHEMA_VERSION,
            name: "Painted layer".into(),
            visible: true,
            locked: false,
            opacity: 0.65,
            blend_mode: "multiply".into(),
            bounds: ClipboardRect {
                x: 12,
                y: -4,
                width: 8,
                height: 4,
            },
            pixels_b64: STANDARD.encode([0xAA; 8 * 4 * 4]),
            mask: Some(MaskClipboard {
                name: "Mask".into(),
                visible: true,
                bounds: ClipboardRect {
                    x: 12,
                    y: -4,
                    width: 8,
                    height: 4,
                },
                pixels_b64: String::new(),
            }),
        };

        let json = clip.to_json();
        let back = LayerClipboard::from_json(&json).unwrap();

        assert_eq!(back.schema_version, LAYER_CLIPBOARD_SCHEMA_VERSION);
        assert_eq!(back.name, "Painted layer");
        assert!((back.opacity - 0.65).abs() < 1e-6);
        assert_eq!(back.blend_mode, "multiply");
        assert_eq!(back.bounds.width, 8);
        assert_eq!(back.bounds.x, 12);
        assert_eq!(back.decode_pixels().unwrap().len(), 8 * 4 * 4);
        assert!(back.mask.is_some());
        assert_eq!(back.mask.unwrap().name, "Mask");
    }

    #[test]
    fn rejects_future_schema_version() {
        let json = serde_json::json!({
            "schema_version": LAYER_CLIPBOARD_SCHEMA_VERSION + 1,
            "name": "x", "visible": true, "locked": false,
            "opacity": 1.0, "blend_mode": "normal",
            "bounds": {"x":0, "y":0, "width":1, "height":1},
            "pixels_b64": "",
            "mask": null,
        })
        .to_string();
        assert!(LayerClipboard::from_json(&json).is_err());
    }
}
