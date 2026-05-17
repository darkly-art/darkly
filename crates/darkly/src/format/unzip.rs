//! Rust-side unzip + entry extraction for `.darkly` load.
//!
//! Load isn't the hot path — files are small (a typical doc is a few
//! tens of MB at most) and the user is already waiting on a file
//! picker — so we keep unzip on the Rust side rather than round-trip
//! bytes through JS twice. The save path is JS-side (`fflate` via
//! `OffscreenCanvas`) to keep slow PNG encoders off the WASM main
//! thread; load doesn't have that constraint.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use super::error::LoadError;

/// Extract every entry from a `.darkly` zip into a `path → bytes` map.
/// Zip-level errors (missing magic, truncated central directory,
/// corrupt entry headers) surface as [`LoadError::Zip`]; per-entry I/O
/// errors as [`LoadError::Io`].
pub fn unzip_entries(bytes: &[u8]) -> Result<HashMap<String, Vec<u8>>, LoadError> {
    let cursor = Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| LoadError::Zip(e.to_string()))?;
    let mut entries = HashMap::with_capacity(archive.len());
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| LoadError::Zip(e.to_string()))?;
        let path = entry.name().to_string();
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry
            .read_to_end(&mut buf)
            .map_err(|e| LoadError::Zip(e.to_string()))?;
        entries.insert(path, buf);
    }
    Ok(entries)
}
