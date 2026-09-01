/**
 * Save orchestration. Consumes the Rust-side `SaveBundle`, PNG-encodes
 * composite + thumbnail via the browser's native `OffscreenCanvas`
 * (off-WASM-thread), assembles the zip via `fflate`, and writes through
 * the file handle abstraction.
 *
 * Rust never writes the zip in production: keeping PNG encoders off
 * the WASM main thread matches the architectural call in
 * [`crates/darkly/src/format/zip_io.rs`]. The kitchen-sink integration
 * test exercises the equivalent path on the Rust side.
 */

import { zip, type Zippable } from 'fflate';
import { getActiveInstance, type DarklyInstance } from '../state/app.svelte';
import { toast } from '../state/toast.svelte';
import { hasFilePicker, pickFileHandle, writeToHandle, type SaveAccept } from './fileHandle';
import { downloadBlob, sanitizeFilename } from './index';
import { exportComposite, rgbaToBlob } from './exportComposite';
import { saveModal } from '../state/saveModal.svelte';
import { removeSnapshot } from './recovery';
import { sessionId } from '../state/recoverySession';
import { processRecording } from '../recording/recorder.svelte';

/** Why a `.darkly` save is being produced: see the Rust `SavePurpose`.
 *  A `'snapshot'` autosave leaves the document dirty; a `'file'` save
 *  clears it. */
export type SavePurpose = 'file' | 'snapshot';

/** Wire shape reconstructed from the `poll_save_result` request (the engine
 *  packs it; `app.svelte.ts::unpackSaveBundle` slices it back out). Mirrors
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

/** A save destination format. `.darkly` is a true document save; the image
 *  formats are behind-the-scenes exports of the current canvas composite. */
export type Format = 'darkly' | 'png' | 'jpeg' | 'webp';

/** One row of the save format table. Each format owns everything about
 *  itself; consumers call `produce()` and read `isDocument`, never
 *  `switch(format)` (type-owned dispatch). */
interface SaveFormat {
    /** Filename extension (no dot). */
    ext: string;
    mime: string;
    /** File-picker filter entry. */
    accept: SaveAccept;
    /** True for a real `.darkly` document save; false for an export. */
    isDocument: boolean;
    /** Produce the bytes to write for this format. */
    produce(instance: DarklyInstance): Promise<Uint8Array | Blob>;
}

/** The single source of truth for what Darkly can save/export to. */
export const SAVE_FORMATS: Record<Format, SaveFormat> = {
    darkly: {
        ext: 'darkly',
        mime: 'application/x-darkly',
        accept: { description: 'Darkly Document', accept: { 'application/x-darkly': ['.darkly'] } },
        isDocument: true,
        produce: (i) => produceDarklyBytes(i, 'file'),
    },
    png: {
        ext: 'png',
        mime: 'image/png',
        accept: { description: 'PNG Image', accept: { 'image/png': ['.png'] } },
        isDocument: false,
        produce: (i) => exportComposite(i, 'png'),
    },
    jpeg: {
        ext: 'jpg',
        mime: 'image/jpeg',
        accept: { description: 'JPEG Image', accept: { 'image/jpeg': ['.jpg', '.jpeg'] } },
        isDocument: false,
        produce: (i) => exportComposite(i, 'jpeg'),
    },
    webp: {
        ext: 'webp',
        mime: 'image/webp',
        accept: { description: 'WebP Image', accept: { 'image/webp': ['.webp'] } },
        isDocument: false,
        produce: (i) => exportComposite(i, 'webp'),
    },
};

/** Format order for the native picker's "Save as type" dropdown and the
 *  fallback modal, `.darkly` first so it's the default. */
export const SAVE_FORMAT_ORDER: Format[] = ['darkly', 'png', 'jpeg', 'webp'];

/** Picker type list, in `SAVE_FORMAT_ORDER`. */
const SAVE_ACCEPTS: SaveAccept[] = SAVE_FORMAT_ORDER.map((f) => SAVE_FORMATS[f].accept);

const EXT_TO_FORMAT: Record<string, Format> = {
    darkly: 'darkly',
    png: 'png',
    jpg: 'jpeg',
    jpeg: 'jpeg',
    webp: 'webp',
};

/** Map a picker-chosen filename to a save format. Unknown or missing
 *  extensions fall back to a true `.darkly` save (the default). */
export function formatFromName(name: string): Format {
    const ext = name.slice(name.lastIndexOf('.') + 1).toLowerCase();
    return EXT_TO_FORMAT[ext] ?? 'darkly';
}

/** Shared `showSaveFilePicker` id: one id across save and export so the
 *  picker reopens in the same last-used directory. */
const PICKER_ID = 'darkly-file';

/**
 * Save the current document. One flow for `.darkly` saves and canvas exports:
 *   - Cached handle + not `forceAs` → silent re-save to the same `.darkly`.
 *   - Native picker present (Chromium) → multi-type picker; the chosen file
 *     type decides `.darkly` save vs image export; write to the handle.
 *   - No picker (Firefox/Safari) → the in-app Save modal drives the download.
 *
 * `forceAs` skips the cached handle and always prompts (Ctrl+Shift+S). The
 * returned promise resolves only once the save is fully done (including the
 * async fallback modal), so `closeGuard` can await it.
 */
export async function saveDocument({ forceAs = false }: { forceAs?: boolean } = {}): Promise<void> {
    const instance = getActiveInstance();
    if (!instance?.engine) return;

    // Silent re-save to the cached handle (Chromium, after a first Save As or
    // opening a `.darkly`).
    if (!forceAs && instance.fileHandle) {
        try {
            const bytes = await produceDarklyBytes(instance, 'file');
            await writeToHandle(instance.fileHandle, bytes);
            await afterSaved(instance, 'darkly');
        } catch (e: unknown) {
            toast.show('error', `Save failed: ${errorMessage(e)}`);
        }
        return;
    }

    const suggested =
        sanitizeFilename(await instance.engine.api.documentName()) || 'darkly-document';

    // Firefox / Safari: no native picker; route to the in-app Save modal,
    // which drives produce + download and resolves when the user is done.
    if (!hasFilePicker) {
        await saveModal.request(suggested);
        return;
    }

    // Chromium: native multi-type picker (activation required here; bytes are
    // produced *after* so the picker isn't blocked). Only the picker needs
    // transient activation; `writeToHandle` on the returned handle does not.
    try {
        const handle = await pickFileHandle(`${suggested}.darkly`, SAVE_ACCEPTS, PICKER_ID);
        if (!handle) return; // user cancelled
        const format = formatFromName(handle.name);
        const data = await SAVE_FORMATS[format].produce(instance);
        await writeToHandle(handle, data);
        if (SAVE_FORMATS[format].isDocument) {
            // A true save adopts this file: cache the handle for silent Ctrl+S
            // re-saves and reflect the chosen filename in the doc name.
            instance.fileHandle = handle;
            const baseName = handle.name.replace(/\.[^./]+$/, '');
            if (baseName) instance.engine.api.setDocumentName({ name: baseName });
        }
        await afterSaved(instance, format);
    } catch (e: unknown) {
        toast.show('error', `Save failed: ${errorMessage(e)}`);
    }
}

/**
 * Produce `format` bytes for `instance` and save them via a browser download
 * (the Firefox/Safari path), shared with the fallback Save modal so the format
 * table and encode live in exactly one place.
 */
export async function saveViaDownload(
    instance: DarklyInstance,
    format: Format,
    baseName: string,
): Promise<void> {
    const fmt = SAVE_FORMATS[format];
    const data = await fmt.produce(instance);
    const blob =
        data instanceof Blob
            ? data
            : new Blob([data as Uint8Array<ArrayBuffer>], { type: fmt.mime });
    downloadBlob(blob, `${sanitizeFilename(baseName) || 'darkly-document'}.${fmt.ext}`);
    await afterSaved(instance, format);
}

/**
 * Post-save bookkeeping. A true `.darkly` save clears crash-recovery state and
 * toasts "Saved"; an image export leaves the document dirty (its recovery
 * snapshot must survive a later crash) and toasts "Exported".
 */
async function afterSaved(instance: DarklyInstance, format: Format): Promise<void> {
    if (SAVE_FORMATS[format].isDocument) {
        await removeSnapshot(sessionId, instance.recoveryId).catch(() => {});
        toast.show('success', 'Saved');
    } else {
        toast.show('success', 'Exported');
    }
}

/**
 * Drive a `.darkly` save for `instance` to completion and return the
 * assembled zip bytes, the destination-agnostic core shared by file-save
 * (above) and autosave snapshots. It kicks `start_save_document` over the
 * async transport and awaits the `poll_save_result` callback, which the
 * instance's render loop drives to completion (`onSaveResult` keeps that
 * loop alive even for a backgrounded tab; the Rust `poll_save_result`
 * drains the readback scheduler itself).
 *
 * Rejects if a save is already in flight on the engine
 * (`SaveError::InProgress`); autosave catches this and skips the tick so
 * a manual Ctrl+S always wins the single save slot.
 */
export async function produceDarklyBytes(
    instance: DarklyInstance,
    purpose: SavePurpose,
): Promise<Uint8Array> {
    const bundle = await runSaveBundle(instance, purpose === 'snapshot');
    // The process recording is embedded in real file saves only. Autosave
    // snapshots skip it: the OPFS scratch is already crash-safe on its own,
    // so re-embedding it per snapshot would be pure write amplification.
    const recording =
        purpose === 'file' ? await processRecording.collectZipEntries(instance) : [];
    return assembleZip(bundle, recording);
}

/** Kick `start_save_document` on `instance` and await the
 *  `poll_save_result` callback. `snapshot` marks an autosave save (which
 *  must not clear the document's dirty flag; see the Rust `SavePurpose`).
 *  The instance's render loop polls `poll_save_result` until the bundle
 *  lands; `onSaveResult` keeps that loop alive even for a backgrounded tab. */
function runSaveBundle(instance: DarklyInstance, snapshot: boolean): Promise<SaveBundle> {
    return new Promise((resolve, reject) => {
        const engine = instance.engine;
        if (!engine) {
            reject(new Error('no engine handle'));
            return;
        }
        instance.onSaveResult((bundle: SaveBundle) => resolve(bundle));
        // `start_save_document` rejects on error; surface that as the save
        // failure rather than waiting forever for a callback that won't fire.
        engine
            .api.startSaveDocument({ snapshot })
            .catch((e) => reject(e instanceof Error ? e : new Error(String(e))));
    });
}

/** Build the .darkly zip bytes from a SaveBundle, plus any embedded
 *  process-recording entries (already-compressed video, stored raw). */
async function assembleZip(
    bundle: SaveBundle,
    recording: Array<{ path: string; bytes: Uint8Array }> = [],
): Promise<Uint8Array> {
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

    const entries: Zippable = {
        [MANIFEST_PATH]: bundle.manifestJson,
        [COMPOSITE_PATH]: composite,
        [THUMBNAIL_PATH]: thumbnail,
    };
    for (const blob of bundle.blobs) {
        entries[blob.path] = blob.bytes;
    }
    for (const entry of recording) {
        // Encoded video doesn't deflate, so don't burn CPU trying.
        entries[entry.path] = entry.path.endsWith('.bin')
            ? [entry.bytes, { level: 0 }]
            : entry.bytes;
    }

    return await new Promise((resolve, reject) => {
        zip(entries, { level: 6 }, (err, out) => {
            if (err) reject(err);
            else resolve(out);
        });
    });
}

/** PNG-encode RGBA8 bytes for the zip's internal `composite.png`, sharing the
 *  same `OffscreenCanvas` encode core as `export-image` (`rgbaToBlob`). */
async function encodeRgbaPng(
    rgba: Uint8Array,
    width: number,
    height: number,
): Promise<Uint8Array> {
    const blob = await rgbaToBlob(rgba, width, height, 'image/png');
    return new Uint8Array(await blob.arrayBuffer());
}

/** Downsample the composite to a ≤256px thumbnail and PNG-encode.
 *  Aspect-preserving: fits within a 256×256 square, never stretches. */
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
