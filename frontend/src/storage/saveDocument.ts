/**
 * Save orchestration. Consumes the Rust-side `SaveBundle`, PNG-encodes
 * composite + thumbnail via the browser's native `OffscreenCanvas`
 * (off-WASM-thread), assembles the zip via `fflate`, and writes through
 * the file handle abstraction.
 *
 * Rust never writes the zip in production — keeping PNG encoders off
 * the WASM main thread matches the architectural call in
 * [`crates/darkly/src/format/zip_io.rs`]. The kitchen-sink integration
 * test exercises the equivalent path on the Rust side.
 */

import { zip } from 'fflate';
import { app } from '../state/app.svelte';
import { toast } from '../state/toast.svelte';
import { canSave, pickSaveFile, writeToHandle } from './fileHandle';
import { sanitizeFilename } from './index';

/** Wire shape returned by `DarklyHandle.poll_save_result()`. Mirrors
 *  `crates/darkly/src/format/manifest.rs::SaveBundle`. */
export interface SaveBundle {
    manifestJson: Uint8Array;
    compositeWidth: number;
    compositeHeight: number;
    compositeRgba: Uint8Array;
    blobs: Array<{ path: string; bytes: Uint8Array }>;
}

const THUMBNAIL_MAX_DIM = 256;
const COMPOSITE_PATH = 'composite.png';
const THUMBNAIL_PATH = 'thumbnail.png';
const MANIFEST_PATH = 'manifest.json';

/**
 * Save the current document. Drives:
 *   1. Pick a handle (cached or via `showSaveFilePicker`).
 *   2. WASM `start_save_document` → poll → `SaveBundle`.
 *   3. PNG-encode composite + 256px thumbnail via `OffscreenCanvas`.
 *   4. Assemble zip with `fflate`.
 *   5. Write bytes via the file handle.
 *
 * `forceAs` skips the cached handle and always prompts (Ctrl+Shift+S).
 */
export async function saveDocument({ forceAs = false }: { forceAs?: boolean } = {}): Promise<void> {
    if (!app.handle) return;
    if (!canSave) {
        toast.show(
            'error',
            "Save isn't supported in this browser — try Chrome, Edge, or Safari.",
        );
        return;
    }

    const handle = await acquireHandle(forceAs);
    if (!handle) return; // user cancelled

    let bundle: SaveBundle;
    try {
        bundle = await runSaveBundle();
    } catch (e: unknown) {
        toast.show('error', `Save failed: ${errorMessage(e)}`);
        return;
    }

    try {
        const zipBytes = await assembleZip(bundle);
        await writeToHandle(handle, zipBytes);
        app.fileHandle = handle;
        toast.show('success', 'Saved');
    } catch (e: unknown) {
        toast.show('error', `Save failed: ${errorMessage(e)}`);
    }
}

/** Resolve the file handle for the active save. Re-uses the cached
 *  handle when one exists and `forceAs` is false; otherwise prompts via
 *  the picker and seeds `doc.name` from the chosen filename. */
async function acquireHandle(forceAs: boolean): Promise<FileSystemFileHandle | null> {
    if (!app.handle) return null;
    if (!forceAs && app.fileHandle) return app.fileHandle;

    const suggested =
        sanitizeFilename(app.handle.document_name()) || 'darkly-document';
    const handle = await pickSaveFile(`${suggested}.darkly`);
    if (!handle) return null;

    // Reflect the chosen filename in the doc's display name so the tab
    // strip and a subsequent Ctrl+S both pick it up.
    const baseName = handle.name.replace(/\.darkly$/i, '');
    if (baseName) app.handle.set_document_name(baseName);
    return handle;
}

/** Kick `start_save_document` and await the `poll_save_result` callback. */
function runSaveBundle(): Promise<SaveBundle> {
    return new Promise((resolve, reject) => {
        if (!app.handle) {
            reject(new Error('no engine handle'));
            return;
        }
        try {
            app.handle.start_save_document();
        } catch (e) {
            reject(e instanceof Error ? e : new Error(String(e)));
            return;
        }
        app.onSaveResult((bundle: SaveBundle) => resolve(bundle));
    });
}

/** Build the .darkly zip bytes from a SaveBundle. */
async function assembleZip(bundle: SaveBundle): Promise<Uint8Array> {
    const composite = await encodeRgbaPng(
        bundle.compositeRgba,
        bundle.compositeWidth,
        bundle.compositeHeight,
    );
    const thumbnail = await encodeThumbnailPng(
        bundle.compositeRgba,
        bundle.compositeWidth,
        bundle.compositeHeight,
    );

    const entries: Record<string, Uint8Array> = {
        [MANIFEST_PATH]: bundle.manifestJson,
        [COMPOSITE_PATH]: composite,
        [THUMBNAIL_PATH]: thumbnail,
    };
    for (const blob of bundle.blobs) {
        entries[blob.path] = blob.bytes;
    }

    return await new Promise((resolve, reject) => {
        zip(entries, { level: 6 }, (err, out) => {
            if (err) reject(err);
            else resolve(out);
        });
    });
}

/** Round-trip RGBA8 bytes through `OffscreenCanvas` to PNG. The
 *  browser's PNG encoder runs off the WASM main thread and reuses the
 *  same path `export-image` already uses. */
async function encodeRgbaPng(
    rgba: Uint8Array,
    width: number,
    height: number,
): Promise<Uint8Array> {
    const canvas = new OffscreenCanvas(width, height);
    const ctx = canvas.getContext('2d');
    if (!ctx) throw new Error('2d context unavailable');
    // ImageData rejects SharedArrayBuffer-backed Uint8ClampedArray
    // (which the WASM heap can be); copy into a fresh ArrayBuffer.
    const copy = new Uint8ClampedArray(rgba.length);
    copy.set(rgba);
    ctx.putImageData(new ImageData(copy, width, height), 0, 0);
    const blob = await canvas.convertToBlob({ type: 'image/png' });
    return new Uint8Array(await blob.arrayBuffer());
}

/** Downsample the composite to a ≤256px thumbnail and PNG-encode.
 *  Aspect-preserving — fits within a 256×256 square, never stretches. */
async function encodeThumbnailPng(
    rgba: Uint8Array,
    width: number,
    height: number,
): Promise<Uint8Array> {
    const scale = Math.min(1, THUMBNAIL_MAX_DIM / Math.max(width, height));
    const thumbW = Math.max(1, Math.round(width * scale));
    const thumbH = Math.max(1, Math.round(height * scale));

    const src = new OffscreenCanvas(width, height);
    const srcCtx = src.getContext('2d');
    if (!srcCtx) throw new Error('2d context unavailable');
    const copy = new Uint8ClampedArray(rgba.length);
    copy.set(rgba);
    srcCtx.putImageData(new ImageData(copy, width, height), 0, 0);

    const dst = new OffscreenCanvas(thumbW, thumbH);
    const dstCtx = dst.getContext('2d');
    if (!dstCtx) throw new Error('2d context unavailable');
    dstCtx.imageSmoothingEnabled = true;
    dstCtx.imageSmoothingQuality = 'high';
    dstCtx.drawImage(src, 0, 0, thumbW, thumbH);

    const blob = await dst.convertToBlob({ type: 'image/png' });
    return new Uint8Array(await blob.arrayBuffer());
}

function errorMessage(e: unknown): string {
    if (e instanceof Error) return e.message;
    return String(e);
}
