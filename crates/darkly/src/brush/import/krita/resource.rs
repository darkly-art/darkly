//! Embedded resource representation and magic-byte format detection.
//!
//! Krita's preset XML embeds brush tips and patterns as base64-encoded blobs.
//! The inner format is one of: PNG, JPEG, SVG (XML), GBR (GIMP brush), GIH
//! (GIMP image hose), or ABR (Adobe brush). We don't decode any of those
//! here — we just sniff the magic bytes so the inspector can pick the right
//! display strategy (native `<img>` for browser-supported formats, fallback
//! hex view for the rest).

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct KritaResource {
    pub name: String,
    pub filename: String,
    /// Krita's resource type ID — e.g. `brushes`, `patterns`, `paintoppresets`.
    pub resource_type: String,
    pub md5sum: String,
    pub byte_length: usize,
    pub format: ResourceFormat,
    /// Decoded resource bytes. Skipped from JSON serialization; the WASM
    /// bridge exposes them via a separate `resource_bytes(idx)` call so the
    /// frontend gets a `Uint8Array` directly without base64 round-tripping.
    #[serde(skip)]
    pub bytes: Vec<u8>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResourceFormat {
    /// PNG image. Width/height parsed from IHDR if intact.
    Png {
        width: Option<u32>,
        height: Option<u32>,
    },
    /// JPEG image (rare for brush tips, common for patterns).
    Jpeg,
    /// SVG document (vector brush tip).
    Svg,
    /// GIMP brush — single grayscale or RGBA stamp.
    Gbr,
    /// GIMP image hose — animated/sequenced brush.
    Gih,
    /// Adobe brush bundle.
    Abr,
    /// Format not recognized. Includes a short hex dump of the leading bytes
    /// so the inspector can show *something* to the user.
    Unknown { magic_hex: String },
}

/// Inspect the leading bytes of a resource blob and return a best-guess
/// format label. This is intentionally narrow — we only care about
/// distinguishing formats the frontend will treat differently.
pub fn sniff_resource_format(bytes: &[u8]) -> ResourceFormat {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let (w, h) = read_png_dimensions(bytes);
        return ResourceFormat::Png {
            width: w,
            height: h,
        };
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return ResourceFormat::Jpeg;
    }
    if looks_like_svg(bytes) {
        return ResourceFormat::Svg;
    }
    // GIH header is "GIMP" magic followed by ASCII metadata; GBR has no magic
    // string but starts with a 4-byte big-endian header_size field followed by
    // a version (1, 2, or 3). Distinguish them by the presence of "GIMP" in
    // the GIH header at offset ~28.
    if bytes.len() > 4 {
        // GIH starts with an ASCII name line then a parameter line. Look for
        // "GIMP" anywhere in the first 256 bytes as a heuristic.
        let head = &bytes[..bytes.len().min(256)];
        if memchr_window(head, b"GIMP") {
            return ResourceFormat::Gih;
        }
    }
    if looks_like_gbr(bytes) {
        return ResourceFormat::Gbr;
    }
    if looks_like_abr(bytes) {
        return ResourceFormat::Abr;
    }
    let n = bytes.len().min(16);
    let hex = bytes[..n]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    ResourceFormat::Unknown { magic_hex: hex }
}

fn read_png_dimensions(bytes: &[u8]) -> (Option<u32>, Option<u32>) {
    // PNG signature is 8 bytes, then IHDR chunk: length(4) "IHDR"(4) width(4) height(4) ...
    if bytes.len() < 24 || &bytes[12..16] != b"IHDR" {
        return (None, None);
    }
    let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    (Some(w), Some(h))
}

fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(512)];
    let s = std::str::from_utf8(head).unwrap_or("");
    let lower = s.to_ascii_lowercase();
    lower.contains("<svg") || (lower.contains("<?xml") && lower.contains("svg"))
}

fn looks_like_gbr(bytes: &[u8]) -> bool {
    // GBR v1/2/3: header_size (>=20), version (1..=3), width, height, bytes_per_pixel,
    // magic_number = 'G' 'I' 'M' 'P' at offset 20 for v2+.
    if bytes.len() < 28 {
        return false;
    }
    let header_size = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let version = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    if !(1..=3).contains(&version) || header_size < 20 || header_size as usize > bytes.len() {
        return false;
    }
    if version >= 2 && &bytes[20..24] == b"GIMP" {
        return true;
    }
    version == 1
}

fn looks_like_abr(bytes: &[u8]) -> bool {
    // ABR v1/2: 16-bit big-endian version (1 or 2) followed by 16-bit count.
    // ABR v6+: starts with version=8BIM "8BPS"-like markers; in practice the
    // first 4 bytes are "8BIM" or version=6 layout. We do a loose check.
    if bytes.len() < 4 {
        return false;
    }
    let v = u16::from_be_bytes([bytes[0], bytes[1]]);
    if matches!(v, 1 | 2 | 6 | 10) {
        return true;
    }
    bytes.starts_with(b"8BIM") || bytes.starts_with(b"8BPS")
}

fn memchr_window(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_png_with_dimensions() {
        let mut bytes = Vec::from(&b"\x89PNG\r\n\x1a\n"[..]);
        bytes.extend_from_slice(&13u32.to_be_bytes()); // IHDR length
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&64u32.to_be_bytes()); // width
        bytes.extend_from_slice(&32u32.to_be_bytes()); // height
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]); // bit depth, color type, etc.
        match sniff_resource_format(&bytes) {
            ResourceFormat::Png { width, height } => {
                assert_eq!(width, Some(64));
                assert_eq!(height, Some(32));
            }
            other => panic!("expected png, got {other:?}"),
        }
    }

    #[test]
    fn detects_jpeg() {
        assert!(matches!(
            sniff_resource_format(&[0xff, 0xd8, 0xff, 0xe0, 0, 0]),
            ResourceFormat::Jpeg
        ));
    }

    #[test]
    fn detects_svg() {
        let svg = b"<?xml version=\"1.0\"?><svg xmlns=\"...\"></svg>";
        assert!(matches!(sniff_resource_format(svg), ResourceFormat::Svg));
    }

    #[test]
    fn unknown_format_includes_hex() {
        match sniff_resource_format(&[0xde, 0xad, 0xbe, 0xef]) {
            ResourceFormat::Unknown { magic_hex } => {
                assert!(magic_hex.contains("de"));
                assert!(magic_hex.contains("ad"));
            }
            other => panic!("expected unknown, got {other:?}"),
        }
    }
}
