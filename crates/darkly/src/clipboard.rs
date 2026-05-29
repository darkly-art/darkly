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
