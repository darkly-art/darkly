//! Krita `.kpp` preset parser.
//!
//! A `.kpp` is a PNG container with two text chunks: `version` (always "2.2"
//! or "5.0") and `preset` (an XML document describing the brush). For 5.0,
//! brush-tip and pattern resources are embedded inside the preset XML as
//! base64-encoded `<resource>` elements.

use serde::Serialize;

use super::resource::{sniff_resource_format, KritaResource, ResourceFormat};
use super::xml::{parse_preset_xml, parse_xml_node, ParsedPresetXml, RawParam, XmlNode};

#[derive(Debug, Serialize)]
pub struct KritaPreset {
    /// Krita preset format version. "2.2" (no embedded resources) or "5.0"
    /// (with embedded resources). Older or newer versions are passed through
    /// as-is so the inspector can still surface what it sees.
    pub format_version: String,

    /// `paintopid` attribute of the root `<Preset>` element. This is Krita's
    /// engine identifier (e.g. `"paintbrush"`, `"colorsmudge"`, `"hairy"`).
    pub paintop_id: String,

    /// Optional human-readable engine description for known paintop IDs.
    pub paintop_description: Option<&'static str>,

    /// `name` attribute of the root `<Preset>` element.
    pub preset_name: Option<String>,

    /// `embedded_resources` attribute on `<Preset>` (5.0+).
    pub embedded_resources_attr: Option<u32>,

    /// PNG-level info: chunk listing and thumbnail metadata.
    pub png: PngInfo,

    /// All `<param>` entries from the preset XML, in document order.
    pub params: Vec<KritaParam>,

    /// All `<resource>` entries embedded inside the preset XML.
    pub resources: Vec<KritaResource>,

    /// The preset XML body, with CDATA payloads of embedded resources elided
    /// (replaced by `[…N bytes elided…]`) so the inspector can show the raw
    /// XML without dragging the base64 blobs along.
    pub preset_xml_elided: String,
}

#[derive(Debug, Serialize)]
pub struct PngInfo {
    pub width: u32,
    pub height: u32,
    pub color_type: String,
    pub bit_depth: u8,
    pub chunks: Vec<PngChunkInfo>,
}

#[derive(Debug, Serialize)]
pub struct PngChunkInfo {
    /// 4-character chunk type: `IHDR`, `tEXt`, `iTXt`, `IDAT`, etc.
    pub chunk_type: String,
    /// Raw chunk payload size in bytes (not counting type/length/crc).
    pub byte_length: usize,
    /// For text chunks: the keyword (e.g. `version`, `preset`).
    pub text_keyword: Option<String>,
    /// For text chunks: decoded text length in bytes (post-decompression).
    pub text_length: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct KritaParam {
    pub name: String,
    /// `type` attribute on `<param>`. Krita writes `string`, `internal`,
    /// `bytearray`, or `color`; legacy presets omit it entirely.
    pub raw_type: Option<String>,
    /// Raw text content of the `<param>` element (the inner CDATA or text
    /// node).
    pub raw_value: String,
    /// Best-effort decoded interpretation of `raw_value`.
    pub decoded: ParamDecoded,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParamDecoded {
    /// Plain scalar — number, boolean, or short string.
    Plain { value: String },
    /// Parsed as a curve point list (`x1,y1;x2,y2;...`).
    Curve { points: Vec<(f32, f32)> },
    /// Parsed as a sensor-config XML blob (`<params id="pressure"/>` etc.).
    SensorXml {
        sensor_id: Option<String>,
        xml: String,
    },
    /// `type="bytearray"` — base64 decoded into raw bytes (length only;
    /// payload available via the WASM bridge if ever needed).
    Bytearray { byte_length: usize },
    /// CDATA payload parsed as a structured XML tree. This is how
    /// `brush_definition` (and similar nested-XML params) surface every
    /// element + attribute the engine consumes — diameter, fade, spikes,
    /// mask type, file references, etc.
    NestedXml { root: XmlNode },
    /// The param value is a base64-encoded image (typically PNG). Krita
    /// inlines pattern textures this way without declaring `type="bytearray"`,
    /// so the type tag alone isn't enough to spot them — we sniff the
    /// decoded magic bytes. Bytes are skipped from JSON; the WASM bridge
    /// hands them out via `param_image_bytes(index)`.
    EmbeddedImage {
        format: ResourceFormat,
        byte_length: usize,
        #[serde(skip)]
        bytes: Vec<u8>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("failed to decode PNG container: {0}")]
    Png(#[from] png::DecodingError),
    #[error("preset is missing the `version` text chunk")]
    MissingVersion,
    #[error("preset is missing the `preset` text chunk")]
    MissingPreset,
    #[error("preset XML parse failed: {0}")]
    Xml(String),
    #[error("preset XML is missing the root <Preset> element")]
    MissingRoot,
}

/// Parse a Krita `.kpp` preset from a byte slice.
pub fn parse_kpp(bytes: &[u8]) -> Result<KritaPreset, ParseError> {
    let png = read_png(bytes)?;

    let version = png
        .text_value("version")
        .ok_or(ParseError::MissingVersion)?;
    let preset_xml = png.text_value("preset").ok_or(ParseError::MissingPreset)?;

    let parsed = parse_preset_xml(&preset_xml).map_err(|e| ParseError::Xml(e.to_string()))?;
    let ParsedPresetXml {
        paintop_id,
        preset_name,
        embedded_resources_attr,
        params: raw_params,
        resources: raw_resources,
        elided_xml,
    } = parsed;

    let paintop_id = paintop_id.ok_or(ParseError::MissingRoot)?;
    let paintop_description = super::paintop::describe(&paintop_id);

    let params = raw_params.into_iter().map(decode_param).collect();

    let resources = raw_resources
        .into_iter()
        .map(|r| {
            let format = sniff_resource_format(&r.bytes);
            KritaResource {
                name: r.name,
                filename: r.filename,
                resource_type: r.resource_type,
                md5sum: r.md5sum,
                byte_length: r.bytes.len(),
                format,
                bytes: r.bytes,
            }
        })
        .collect();

    Ok(KritaPreset {
        format_version: version,
        paintop_id,
        paintop_description,
        preset_name,
        embedded_resources_attr,
        png: png.info,
        params,
        resources,
        preset_xml_elided: elided_xml,
    })
}

/// Best-effort decode of a single `<param>`. The fall-through is `Plain`,
/// which is always safe — the raw value is preserved in `KritaParam::raw_value`
/// regardless of decode outcome.
fn decode_param(raw: RawParam) -> KritaParam {
    let decoded = match raw.raw_type.as_deref() {
        Some("bytearray") => {
            use base64::Engine;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(raw.raw_value.trim().as_bytes())
                .unwrap_or_default();
            ParamDecoded::Bytearray {
                byte_length: bytes.len(),
            }
        }
        _ => decode_string_value(&raw.raw_value),
    };
    KritaParam {
        name: raw.name,
        raw_type: raw.raw_type,
        raw_value: raw.raw_value,
        decoded,
    }
}

fn decode_string_value(raw: &str) -> ParamDecoded {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return ParamDecoded::Plain {
            value: String::new(),
        };
    }
    if let Some(points) = super::sensor::parse_curve_points(trimmed) {
        return ParamDecoded::Curve { points };
    }
    if trimmed.starts_with('<') {
        if let Some(sensor_id) = super::sensor::extract_sensor_id(trimmed) {
            return ParamDecoded::SensorXml {
                sensor_id: Some(sensor_id),
                xml: trimmed.to_string(),
            };
        }
        if let Some(root) = parse_xml_node(trimmed) {
            return ParamDecoded::NestedXml { root };
        }
        // Fall through to Plain so we still surface the raw text rather than
        // dropping it on the floor when XML parsing fails.
    }
    if let Some(image) = try_decode_inline_image(trimmed) {
        return image;
    }
    ParamDecoded::Plain {
        value: trimmed.to_string(),
    }
}

/// Krita inlines pattern textures (and similar) as base64-encoded PNGs/JPEGs
/// in plain `<param>` values, often *without* declaring `type="bytearray"`.
/// This sniffs the value: if it's all-base64 of meaningful length and decodes
/// to something with a recognized image magic, return an `EmbeddedImage`.
fn try_decode_inline_image(raw: &str) -> Option<ParamDecoded> {
    // Cheap pre-filter: skip anything obviously not base64.
    if raw.len() < 64 {
        return None;
    }
    if !raw
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'\n' | b'\r'))
    {
        return None;
    }
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(raw.as_bytes())
        .ok()?;
    let format = sniff_resource_format(&bytes);
    if matches!(format, ResourceFormat::Unknown { .. }) {
        return None;
    }
    Some(ParamDecoded::EmbeddedImage {
        format,
        byte_length: bytes.len(),
        bytes,
    })
}

// ---------------------------------------------------------------------------
// PNG container reading
// ---------------------------------------------------------------------------

struct ReadPng {
    info: PngInfo,
    text_chunks: Vec<(String, String)>,
}

impl ReadPng {
    fn text_value(&self, keyword: &str) -> Option<String> {
        self.text_chunks
            .iter()
            .find(|(k, _)| k == keyword)
            .map(|(_, v)| v.clone())
    }
}

fn read_png(bytes: &[u8]) -> Result<ReadPng, ParseError> {
    // We need both the PNG-crate decoder (for IHDR + decompressed text) and a
    // raw chunk walk (for the chunk listing — including chunks the decoder
    // ignores). Both reads are cheap on these tiny preset PNGs.
    let chunks = walk_chunks(bytes);

    let decoder = png::Decoder::new(bytes);
    let reader = decoder.read_info()?;
    let info = reader.info();

    let mut text_chunks: Vec<(String, String)> = Vec::new();
    for t in &info.uncompressed_latin1_text {
        text_chunks.push((t.keyword.clone(), t.text.clone()));
    }
    for t in &info.compressed_latin1_text {
        if let Ok(text) = t.get_text() {
            text_chunks.push((t.keyword.clone(), text));
        }
    }
    for t in &info.utf8_text {
        if let Ok(text) = t.get_text() {
            text_chunks.push((t.keyword.clone(), text));
        }
    }

    let chunks_with_text = chunks
        .into_iter()
        .map(|c| {
            let keyword = if matches!(c.chunk_type.as_str(), "tEXt" | "zTXt" | "iTXt") {
                read_text_chunk_keyword(&bytes[c.payload_offset..c.payload_offset + c.byte_length])
            } else {
                None
            };
            let text_length = keyword.as_ref().and_then(|kw| {
                text_chunks
                    .iter()
                    .find(|(k, _)| k == kw)
                    .map(|(_, v)| v.len())
            });
            PngChunkInfo {
                chunk_type: c.chunk_type,
                byte_length: c.byte_length,
                text_keyword: keyword,
                text_length,
            }
        })
        .collect();

    let color_type = format!("{:?}", info.color_type);
    let png_info = PngInfo {
        width: info.width,
        height: info.height,
        color_type,
        bit_depth: info.bit_depth as u8,
        chunks: chunks_with_text,
    };

    Ok(ReadPng {
        info: png_info,
        text_chunks,
    })
}

struct RawChunk {
    chunk_type: String,
    byte_length: usize,
    payload_offset: usize,
}

fn walk_chunks(bytes: &[u8]) -> Vec<RawChunk> {
    let mut out = Vec::new();
    if bytes.len() < 8 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return out;
    }
    let mut i = 8;
    while i + 8 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;
        let ctype = &bytes[i + 4..i + 8];
        let chunk_type = String::from_utf8_lossy(ctype).into_owned();
        let payload_offset = i + 8;
        if payload_offset + len + 4 > bytes.len() {
            break;
        }
        out.push(RawChunk {
            chunk_type: chunk_type.clone(),
            byte_length: len,
            payload_offset,
        });
        i = payload_offset + len + 4; // skip payload + CRC
        if chunk_type == "IEND" {
            break;
        }
    }
    out
}

fn read_text_chunk_keyword(payload: &[u8]) -> Option<String> {
    let null = payload.iter().position(|&b| b == 0)?;
    Some(String::from_utf8_lossy(&payload[..null]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::super::resource::ResourceFormat;
    use super::*;

    const SOFTBRUSH_30PX: &[u8] = include_bytes!("testdata/softbrush_30px.kpp");
    const EMBEDDED_5_0: &[u8] = include_bytes!("testdata/test-embedded-resources-5.0.kpp");
    const COLORSMUDGE_DEFAULT: &[u8] = include_bytes!("testdata/colorsmudge.kpp");

    #[test]
    fn parses_softbrush_2_2() {
        let p = parse_kpp(SOFTBRUSH_30PX).unwrap();
        assert_eq!(p.format_version, "2.2");
        assert_eq!(p.paintop_id, "paintbrush");
        assert_eq!(p.preset_name.as_deref(), Some("softbrush_30px"));
        assert!(p.resources.is_empty());
        assert!(!p.params.is_empty(), "expected params");
        let curve = p
            .params
            .iter()
            .find(|p| p.name == "CurveSize")
            .expect("CurveSize param");
        match &curve.decoded {
            ParamDecoded::Curve { points } => {
                assert_eq!(points, &vec![(0.0, 0.0), (1.0, 1.0)]);
            }
            other => panic!("expected curve points, got {other:?}"),
        }
    }

    #[test]
    fn parses_5_0_with_embedded_resources() {
        let p = parse_kpp(EMBEDDED_5_0).unwrap();
        assert_eq!(p.format_version, "5.0");
        assert_eq!(p.paintop_id, "paintbrush");
        assert_eq!(p.embedded_resources_attr, Some(2));
        assert_eq!(p.resources.len(), 2);
        assert_eq!(p.resources[0].resource_type, "brushes");
        assert_eq!(p.resources[1].resource_type, "patterns");
        for r in &p.resources {
            assert!(matches!(r.format, ResourceFormat::Png { .. }));
            assert!(!r.bytes.is_empty());
        }
    }

    #[test]
    fn detects_sensor_xml_param() {
        let p = parse_kpp(EMBEDDED_5_0).unwrap();
        let sensor = p
            .params
            .iter()
            .find(|p| p.name == "DarkenSensor")
            .expect("DarkenSensor param");
        match &sensor.decoded {
            ParamDecoded::SensorXml { sensor_id, .. } => {
                assert_eq!(sensor_id.as_deref(), Some("pressure"));
            }
            other => panic!("expected sensor xml, got {other:?}"),
        }
    }

    #[test]
    fn brush_definition_decodes_to_xml_tree() {
        let p = parse_kpp(SOFTBRUSH_30PX).unwrap();
        let bd = p
            .params
            .iter()
            .find(|p| p.name == "brush_definition")
            .expect("brush_definition param");
        match &bd.decoded {
            ParamDecoded::NestedXml { root } => {
                assert_eq!(root.tag, "Brush");
                let brush_type = root
                    .attrs
                    .iter()
                    .find(|(k, _)| k == "type")
                    .map(|(_, v)| v.as_str());
                assert_eq!(brush_type, Some("auto_brush"));
                let mask = root
                    .children
                    .iter()
                    .find(|c| c.tag == "MaskGenerator")
                    .expect("MaskGenerator child");
                let mask_type = mask
                    .attrs
                    .iter()
                    .find(|(k, _)| k == "type")
                    .map(|(_, v)| v.as_str());
                assert_eq!(mask_type, Some("circle"));
                let diameter = mask
                    .attrs
                    .iter()
                    .find(|(k, _)| k == "diameter")
                    .map(|(_, v)| v.as_str());
                assert_eq!(diameter, Some("30"));
            }
            other => panic!("expected NestedXml tree, got {other:?}"),
        }
    }

    #[test]
    fn detects_inline_base64_png_pattern() {
        let p = parse_kpp(COLORSMUDGE_DEFAULT).unwrap();
        let pat = p
            .params
            .iter()
            .find(|p| p.name == "Texture/Pattern/Pattern")
            .expect("Texture/Pattern/Pattern param");
        match &pat.decoded {
            ParamDecoded::EmbeddedImage {
                format,
                byte_length,
                bytes,
            } => {
                assert!(matches!(format, ResourceFormat::Png { .. }));
                assert!(*byte_length > 0);
                assert_eq!(bytes.len(), *byte_length);
                assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
            }
            other => panic!("expected EmbeddedImage, got {other:?}"),
        }
    }

    #[test]
    fn lists_all_png_chunks() {
        let p = parse_kpp(EMBEDDED_5_0).unwrap();
        let kinds: Vec<_> = p.png.chunks.iter().map(|c| c.chunk_type.as_str()).collect();
        assert!(kinds.contains(&"IHDR"));
        assert!(kinds.contains(&"iTXt"));
        assert!(kinds.contains(&"IDAT"));
        assert!(kinds.contains(&"IEND"));
    }
}
