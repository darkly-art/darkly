/**
 * Composite export: read back the current canvas composite via the async
 * export readback and encode it to an image Blob off the WASM main thread.
 *
 * `rgbaToBlob` (OffscreenCanvas `putImageData → convertToBlob`) is the single
 * home for RGBA→image encoding; `saveDocument.ts` reuses it for the `.darkly`
 * zip's internal `composite.png` too, so the encode core exists once.
 */

import { rgbaToBlob } from '../lib/rgba';
import type { DarklyInstance } from '../state/app.svelte';

export type ImageFormat = 'png' | 'jpeg' | 'webp';

const MIME: Record<ImageFormat, string> = {
    png: 'image/png',
    jpeg: 'image/jpeg',
    webp: 'image/webp',
};

// JPEG/WebP quality is fixed at 0.92: the historical export default; PNG is
// lossless and ignores it.
const QUALITY = 0.92;

/** Drive the async export readback for `instance` and encode the composite to
 *  an image Blob. Kicks `startExport` and awaits the one-shot readback the
 *  render loop polls to completion. */
export async function exportComposite(
    instance: DarklyInstance,
    format: ImageFormat,
): Promise<Blob> {
    const engine = instance.engine;
    if (!engine) throw new Error('no engine handle');
    // Fire-and-forget by protocol: `start_export` reports its own failures
    // through the transport's error path rather than returning a promise.
    engine.api.startExport();
    const result = await instance.awaitReadback('export', () => engine.api.pollExportResult());
    if (!result?.bytes) throw new Error('export produced no pixels');
    const quality = format === 'png' ? undefined : QUALITY;
    return rgbaToBlob(result.bytes, result.width, result.height, MIME[format], quality);
}
