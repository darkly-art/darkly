/**
 * Magic-byte file-type detection for the unified Open flow.
 *
 * The picker filter narrows by extension, but extensions lie — a
 * `.darkly` zip might have been renamed; a screenshot saved with the
 * wrong suffix; a drag-drop carry no extension at all. Sniffing the
 * first 12 bytes is cheap and definitive: a real zip can't masquerade
 * as a PNG. Returns `'unknown'` for anything we can't ingest, so the
 * caller can surface a precise toast rather than silently failing
 * deep inside a decoder.
 *
 * Kept tiny and dependency-free so unit tests run in isolation.
 */

export type FileKind = 'darkly' | 'png' | 'jpeg' | 'webp' | 'unknown';

/** Inspect the first few bytes of `bytes` and return the file kind.
 *  `.darkly` is the zip signature `50 4B 03 04` — Phase 3's
 *  `format::zip_io` writes a stock deflate zip, no funny prefix.
 *  PNG / JPEG / WebP follow their respective format signatures. */
export function detectKind(bytes: Uint8Array): FileKind {
    if (bytes.length >= 4
        && bytes[0] === 0x50 && bytes[1] === 0x4b
        && bytes[2] === 0x03 && bytes[3] === 0x04) {
        return 'darkly';
    }
    if (bytes.length >= 4
        && bytes[0] === 0x89 && bytes[1] === 0x50
        && bytes[2] === 0x4e && bytes[3] === 0x47) {
        return 'png';
    }
    if (bytes.length >= 3
        && bytes[0] === 0xff && bytes[1] === 0xd8 && bytes[2] === 0xff) {
        return 'jpeg';
    }
    // WebP: `RIFF` (52 49 46 46), 4 bytes file size, then `WEBP`.
    if (bytes.length >= 12
        && bytes[0] === 0x52 && bytes[1] === 0x49
        && bytes[2] === 0x46 && bytes[3] === 0x46
        && bytes[8] === 0x57 && bytes[9] === 0x45
        && bytes[10] === 0x42 && bytes[11] === 0x50) {
        return 'webp';
    }
    return 'unknown';
}

/** True when `kind` is an image format the browser can decode via
 *  `createImageBitmap` (the path the Open flow uses for raster files). */
export function isImageKind(kind: FileKind): kind is 'png' | 'jpeg' | 'webp' {
    return kind === 'png' || kind === 'jpeg' || kind === 'webp';
}
