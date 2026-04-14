//! Clipboard system — typed internal clipboard with extensible content types.
//!
//! `ImageClip` stores a flat RGBA pixel buffer. All copy/paste goes through
//! GPU readback (copy) and write_texture (paste) — no CPU tile storage.

// ---------------------------------------------------------------------------
// Clipboard enum — extensible content container
// ---------------------------------------------------------------------------

/// Typed clipboard content. New variants can be added for future content types
/// (e.g. full layers, layer groups) without refactoring the clipboard system.
pub enum Clipboard {
    /// Flattened RGBA pixel region — used for canvas copy/paste and external interop.
    ImageData(ImageClip),
    // Future variants (not implemented):
    // Layer(LayerClip),        — full layer with mask, blend mode, opacity
    // LayerGroup(GroupClip),   — group with children
}

impl Clipboard {
    /// Extract an `ImageClip` reference, regardless of variant.
    /// Future layer/group variants would flatten themselves to pixels on demand.
    pub fn as_image(&self) -> Option<&ImageClip> {
        match self {
            Clipboard::ImageData(clip) => Some(clip),
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
    pub fn from_rgba(
        width: u32,
        height: u32,
        rgba: Vec<u8>,
        offset_x: i32,
        offset_y: i32,
    ) -> Self {
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
        (&self.data, self.width, self.height, self.offset_x, self.offset_y)
    }

    /// Returns true if this clip has no pixel data.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
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
            rgba[i * 4] = 255;     // R
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
        assert_eq!(out[1], 0);   // G
        assert_eq!(out[2], 0);   // B
        assert_eq!(out[3], 255); // A
    }

    #[test]
    fn empty_clip() {
        let clip = ImageClip::from_rgba(0, 0, vec![], 0, 0);
        assert!(clip.is_empty());
    }
}
