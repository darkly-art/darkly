//! Test-only zip assembly and extraction for `.darkly` containers.
//!
//! Production save assembles the zip in JS (via `fflate`) to keep slow
//! encoders off the WASM main thread. This module exists purely so the
//! Rust-side kitchen-sink test can drive the full save→file→reload loop
//! without crossing the WASM/JS boundary.
//!
//! Gated `#[cfg(test)]` at the module declaration in
//! [`super::mod`] — never reachable from engine or WASM code.

use std::collections::HashMap;
use std::io::{Cursor, Read, Write};

use super::manifest::SaveBundle;

/// Path inside the zip for the manifest JSON.
const MANIFEST_PATH: &str = "manifest.json";
/// Path inside the zip for the baked composite PNG. The save flow stores
/// raw RGBA in `SaveBundle::composite_rgba`; this helper PNG-encodes it
/// on the way into the zip so the extracted archive is consumable by any
/// standard tool (file managers, image viewers).
const COMPOSITE_PATH: &str = "composite.png";

/// Assemble a `SaveBundle` into the `.darkly` zip bytes used by the
/// kitchen-sink test. Mirrors what JS does in production via `fflate`:
///
/// 1. Write `manifest.json` verbatim from `bundle.manifest_json`.
/// 2. PNG-encode the composite RGBA and write to `composite.png`.
/// 3. Write each `blobs[i].path` → `blobs[i].bytes` verbatim.
///
/// Compression is Deflated — matches what the JS path will produce.
pub fn assemble_zip(bundle: &SaveBundle) -> Vec<u8> {
    let buf = Vec::new();
    let cursor = Cursor::new(buf);
    let mut zip = zip::ZipWriter::new(cursor);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    zip.start_file(MANIFEST_PATH, options).unwrap();
    zip.write_all(&bundle.manifest_json).unwrap();

    let composite_png = encode_rgba_as_png(
        &bundle.composite_rgba,
        bundle.composite_width,
        bundle.composite_height,
    );
    zip.start_file(COMPOSITE_PATH, options).unwrap();
    zip.write_all(&composite_png).unwrap();

    for blob in &bundle.blobs {
        zip.start_file(&blob.path, options).unwrap();
        zip.write_all(&blob.bytes).unwrap();
    }

    let cursor = zip.finish().unwrap();
    cursor.into_inner()
}

/// All entries extracted from a `.darkly` zip, keyed by zip path. Used
/// by the kitchen-sink test to feed bytes into the load path without
/// going through the production unzip code (which Phase 4 will own).
pub struct ZipEntries {
    pub entries: HashMap<String, Vec<u8>>,
}

impl ZipEntries {
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        self.entries.get(path).map(Vec::as_slice)
    }
}

/// Extract every entry in a `.darkly` zip into a map keyed by path.
pub fn extract_zip(bytes: &[u8]) -> ZipEntries {
    let cursor = Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor).unwrap();
    let mut entries = HashMap::with_capacity(archive.len());
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        let path = entry.name().to_string();
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes).unwrap();
        entries.insert(path, bytes);
    }
    ZipEntries { entries }
}

/// PNG-encode an RGBA8 buffer for the in-zip composite. Mirrors what JS
/// does in production via `OffscreenCanvas.convertToBlob`.
fn encode_rgba_as_png(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let cursor = Cursor::new(&mut out);
    use image::ImageEncoder;
    image::codecs::png::PngEncoder::new(cursor)
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
        .unwrap();
    out
}
