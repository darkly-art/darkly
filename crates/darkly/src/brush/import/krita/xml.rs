//! Preset XML reader.
//!
//! Walks the `<Preset>` document, extracting the root attributes, all
//! `<param>` children, and any `<resources>/<resource>` entries. Returns
//! a typed struct ready for higher-level decoding in [`super::kpp`].

use base64::Engine;
use quick_xml::events::Event;
use quick_xml::Reader;
use serde::Serialize;

/// A node in a parsed XML tree. Used to surface structured contents of
/// nested-XML param values (e.g. `brush_definition`) so the inspector can
/// show every element/attribute the engine consumes.
///
/// Attributes are kept in document order — Krita's XML doesn't guarantee
/// any order but reading them as written keeps diffs across presets stable.
#[derive(Debug, Serialize)]
pub struct XmlNode {
    pub tag: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<XmlNode>,
    /// Trimmed text content, if any (rare for the structures we encounter).
    pub text: Option<String>,
}

/// Parse a snippet of XML into an [`XmlNode`] tree rooted at the first
/// element. Doctype declarations, comments, and processing instructions are
/// skipped. Returns `None` if no element is found or parsing fails — the
/// caller falls back to displaying the raw string.
pub fn parse_xml_node(xml: &str) -> Option<XmlNode> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut stack: Vec<XmlNode> = Vec::new();
    let mut buf = Vec::new();
    let mut root: Option<XmlNode> = None;

    loop {
        match reader.read_event_into(&mut buf).ok()? {
            Event::Eof => break,
            Event::Start(e) => {
                let node = node_from_start(&e)?;
                stack.push(node);
            }
            Event::Empty(e) => {
                let node = node_from_start(&e)?;
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                } else if root.is_none() {
                    root = Some(node);
                }
            }
            Event::End(_) => {
                if let Some(node) = stack.pop() {
                    if let Some(parent) = stack.last_mut() {
                        parent.children.push(node);
                    } else if root.is_none() {
                        root = Some(node);
                    }
                }
            }
            Event::Text(t) => {
                if let Some(parent) = stack.last_mut() {
                    let s = t.unescape().ok()?.trim().to_string();
                    if !s.is_empty() {
                        parent.text = match parent.text.take() {
                            Some(prev) => Some(format!("{prev}{s}")),
                            None => Some(s),
                        };
                    }
                }
            }
            Event::CData(t) => {
                if let Some(parent) = stack.last_mut() {
                    let s = std::str::from_utf8(t.as_ref()).ok()?.trim().to_string();
                    if !s.is_empty() {
                        parent.text = match parent.text.take() {
                            Some(prev) => Some(format!("{prev}{s}")),
                            None => Some(s),
                        };
                    }
                }
            }
            _ => {}
        }
        buf.clear();
    }

    root
}

fn node_from_start(e: &quick_xml::events::BytesStart<'_>) -> Option<XmlNode> {
    let tag = std::str::from_utf8(e.name().as_ref()).ok()?.to_string();
    let mut attrs = Vec::new();
    for attr in e.attributes() {
        let attr = attr.ok()?;
        let key = std::str::from_utf8(attr.key.as_ref()).ok()?.to_string();
        let val = attr.unescape_value().ok()?.into_owned();
        attrs.push((key, val));
    }
    Some(XmlNode {
        tag,
        attrs,
        children: Vec::new(),
        text: None,
    })
}

#[derive(Debug)]
pub struct ParsedPresetXml {
    pub paintop_id: Option<String>,
    pub preset_name: Option<String>,
    pub embedded_resources_attr: Option<u32>,
    pub params: Vec<RawParam>,
    pub resources: Vec<RawResource>,
    /// The original preset XML with embedded `<resource>` CDATA payloads
    /// replaced by `[…N bytes elided…]`. Lets the inspector show the
    /// document structure without the multi-MB base64 blobs.
    pub elided_xml: String,
}

#[derive(Debug)]
pub struct RawParam {
    pub name: String,
    pub raw_type: Option<String>,
    pub raw_value: String,
}

#[derive(Debug)]
pub struct RawResource {
    pub name: String,
    pub filename: String,
    pub resource_type: String,
    pub md5sum: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum XmlError {
    #[error("xml read error: {0}")]
    Read(#[from] quick_xml::Error),
    #[error("attribute error: {0}")]
    Attr(#[from] quick_xml::events::attributes::AttrError),
    #[error("invalid utf-8 in preset xml: {0}")]
    Utf8(#[from] std::str::Utf8Error),
}

/// Parse a preset XML document into a typed representation.
pub fn parse_preset_xml(xml: &str) -> Result<ParsedPresetXml, XmlError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);

    let mut paintop_id: Option<String> = None;
    let mut preset_name: Option<String> = None;
    let mut embedded_resources_attr: Option<u32> = None;
    let mut params: Vec<RawParam> = Vec::new();
    let mut resources: Vec<RawResource> = Vec::new();

    let mut in_resources = false;
    let mut current_resource: Option<RawResource> = None;
    let mut current_param: Option<RawParam> = None;
    let mut text_buf = String::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) => {
                let name = e.name();
                let tag = std::str::from_utf8(name.as_ref())?.to_string();
                match tag.as_str() {
                    "Preset" => {
                        for attr in e.attributes() {
                            let attr = attr?;
                            let key = std::str::from_utf8(attr.key.as_ref())?.to_string();
                            let val = attr.unescape_value()?.into_owned();
                            match key.as_str() {
                                "paintopid" => paintop_id = Some(val),
                                "name" => preset_name = Some(val),
                                "embedded_resources" => {
                                    embedded_resources_attr = val.parse().ok();
                                }
                                _ => {}
                            }
                        }
                    }
                    "resources" => {
                        in_resources = true;
                    }
                    "resource" if in_resources => {
                        let mut name = String::new();
                        let mut filename = String::new();
                        let mut resource_type = String::new();
                        let mut md5sum = String::new();
                        for attr in e.attributes() {
                            let attr = attr?;
                            let key = std::str::from_utf8(attr.key.as_ref())?.to_string();
                            let val = attr.unescape_value()?.into_owned();
                            match key.as_str() {
                                "name" => name = val,
                                "filename" => filename = val,
                                "type" => resource_type = val,
                                "md5sum" => md5sum = val,
                                _ => {}
                            }
                        }
                        current_resource = Some(RawResource {
                            name,
                            filename,
                            resource_type,
                            md5sum,
                            bytes: Vec::new(),
                        });
                        text_buf.clear();
                    }
                    "param" => {
                        let mut name = String::new();
                        let mut raw_type: Option<String> = None;
                        for attr in e.attributes() {
                            let attr = attr?;
                            let key = std::str::from_utf8(attr.key.as_ref())?.to_string();
                            let val = attr.unescape_value()?.into_owned();
                            match key.as_str() {
                                "name" => name = val,
                                "type" => raw_type = Some(val),
                                _ => {}
                            }
                        }
                        current_param = Some(RawParam {
                            name,
                            raw_type,
                            raw_value: String::new(),
                        });
                        text_buf.clear();
                    }
                    _ => {}
                }
            }
            Event::End(e) => {
                let tag = std::str::from_utf8(e.name().as_ref())?.to_string();
                match tag.as_str() {
                    "resources" => in_resources = false,
                    "resource" => {
                        if let Some(mut r) = current_resource.take() {
                            r.bytes = base64::engine::general_purpose::STANDARD
                                .decode(text_buf.split_whitespace().collect::<String>().as_bytes())
                                .unwrap_or_default();
                            resources.push(r);
                        }
                        text_buf.clear();
                    }
                    "param" => {
                        if let Some(mut p) = current_param.take() {
                            p.raw_value = std::mem::take(&mut text_buf);
                            params.push(p);
                        }
                    }
                    _ => {}
                }
            }
            Event::Text(t) => {
                let s = t.unescape()?;
                text_buf.push_str(&s);
            }
            Event::CData(t) => {
                // CDATA preserves bytes verbatim — no entity unescaping.
                let s = std::str::from_utf8(t.as_ref())?;
                text_buf.push_str(s);
            }
            _ => {}
        }
        buf.clear();
    }

    let elided = elide_resource_payloads(xml);
    let pretty = pretty_print_xml(&elided).unwrap_or(elided);

    Ok(ParsedPresetXml {
        paintop_id,
        preset_name,
        embedded_resources_attr,
        params,
        resources,
        elided_xml: pretty,
    })
}

/// Re-emit `xml` with two-space indentation so the inspector's raw-XML view
/// is actually readable. Krita writes the entire preset as a single line, so
/// without this the document scrolls hundreds of pixels off the right edge.
/// Returns `None` (and the caller falls back to the original) if the input
/// doesn't parse cleanly — better to show ugly XML than swallowed XML.
fn pretty_print_xml(xml: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut writer = quick_xml::Writer::new_with_indent(Vec::new(), b' ', 2);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf).ok()? {
            Event::Eof => break,
            e => writer.write_event(e).ok()?,
        }
        buf.clear();
    }
    String::from_utf8(writer.into_inner()).ok()
}

/// Replace each `<resource ...><![CDATA[...]]></resource>` payload with a
/// short placeholder so the inspector can show the XML structure without
/// the embedded base64 blobs (which can run to hundreds of KB).
fn elide_resource_payloads(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(idx) = rest.find("<resource ") {
        out.push_str(&rest[..idx]);
        let after_open = &rest[idx..];
        let cdata_start = after_open.find("<![CDATA[");
        let cdata_end = after_open.find("]]>");
        let res_close = after_open.find("</resource>");
        match (cdata_start, cdata_end, res_close) {
            (Some(cs), Some(ce), Some(rc)) if cs < ce && ce < rc => {
                out.push_str(&after_open[..cs + "<![CDATA[".len()]);
                let payload_len = ce - (cs + "<![CDATA[".len());
                out.push_str(&format!("[…{payload_len} bytes elided…]"));
                out.push_str(&after_open[ce..rc + "</resource>".len()]);
                rest = &after_open[rc + "</resource>".len()..];
            }
            _ => {
                out.push_str(after_open);
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}
