//! Zip assembly for Darkly's containers.
//!
//! [`write_entries`] is the counterpart to
//! [`unzip_entries`](super::unzip::unzip_entries) and is production code: the
//! brush-pack archive is written through it.
//!
//! `.darkly` *document* saves remain a JS-side write (via `fflate`) to keep
//! slow encoders off the WASM main thread — [`assemble_zip`] exists only so
//! Rust-side tests can drive the full save→file→reload loop without crossing
//! the WASM/JS boundary, and is gated behind the `testing` feature
//! accordingly. A pack is a handful of small JSONs, so writing one in Rust
//! does not run into that constraint.

use std::io::{Cursor, Write};

use super::error::LoadError;

/// Write named entries into a zip.
///
/// `method` is a parameter rather than a constant because the two callers
/// genuinely differ: the `.darkly` test container is `Stored`, and a pack
/// archive of JSON compresses well enough to be worth `Deflated`. Hardcoding
/// either would silently change the other's output.
pub fn write_entries(
    entries: &[(&str, &[u8])],
    method: zip::CompressionMethod,
) -> Result<Vec<u8>, LoadError> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(method);

    for (path, bytes) in entries {
        zip.start_file(*path, options)
            .map_err(|e| LoadError::Zip(e.to_string()))?;
        zip.write_all(bytes)
            .map_err(|e| LoadError::Zip(e.to_string()))?;
    }

    let cursor = zip.finish().map_err(|e| LoadError::Zip(e.to_string()))?;
    Ok(cursor.into_inner())
}

#[cfg(any(test, feature = "testing"))]
pub use test_only::assemble_zip;

#[cfg(any(test, feature = "testing"))]
mod test_only {
    use std::io::Cursor;

    use super::write_entries;
    use crate::format::manifest::SaveBundle;

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
    /// Entries are `Stored`, which is what this container has always been.
    pub fn assemble_zip(bundle: &SaveBundle) -> Vec<u8> {
        let composite_png = encode_rgba_as_png(
            &bundle.composite_rgba,
            bundle.composite_width,
            bundle.composite_height,
        );

        let mut entries: Vec<(&str, &[u8])> = vec![
            (MANIFEST_PATH, &bundle.manifest_json),
            (COMPOSITE_PATH, &composite_png),
        ];
        for blob in &bundle.blobs {
            entries.push((blob.path.as_str(), &blob.bytes));
        }

        write_entries(&entries, zip::CompressionMethod::Stored).expect("assembling a test zip")
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::unzip::unzip_entries;

    #[test]
    fn write_entries_round_trips_through_unzip_entries() {
        // The shared writer and the production reader must agree, under either
        // compression method — the parameter exists precisely because both are
        // in use.
        for method in [
            zip::CompressionMethod::Stored,
            zip::CompressionMethod::Deflated,
        ] {
            let json = br#"{"format":"darkly-brush"}"#;
            let blob: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
            let bytes = write_entries(&[("pack.json", json), ("brushes/9f1c.json", &blob)], method)
                .unwrap();

            let entries = unzip_entries(&bytes).unwrap();
            assert_eq!(entries.len(), 2, "{method:?}");
            assert_eq!(
                entries.get("pack.json").unwrap().as_slice(),
                json,
                "{method:?}"
            );
            assert_eq!(
                entries.get("brushes/9f1c.json").unwrap(),
                &blob,
                "{method:?}"
            );
        }
    }

    #[test]
    fn writing_no_entries_yields_a_readable_empty_zip() {
        let bytes = write_entries(&[], zip::CompressionMethod::Deflated).unwrap();
        assert!(unzip_entries(&bytes).unwrap().is_empty());
    }
}
