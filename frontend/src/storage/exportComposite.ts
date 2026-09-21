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
 *  an image Blob. Kicks `startExport` and awaits the one-shot `onExportResult`
 *  callback the render loop resolves. */
export function exportComposite(
    instance: DarklyInstance,
    format: ImageFormat,
): Promise<Blob> {
    return new Promise((resolve, reject) => {
        const engine = instance.engine;
        if (!engine) {
            reject(new Error('no engine handle'));
            return;
        }
        instance.onExportResult(async (result) => {
            try {
                if (!result?.rgba) {
                    reject(new Error('export produced no pixels'));
                    return;
                }
                const quality = format === 'png' ? undefined : QUALITY;
                resolve(
                    await rgbaToBlob(result.rgba, result.width, result.height, MIME[format], quality),
                );
            } catch (e) {
                reject(e instanceof Error ? e : new Error(String(e)));
            }
        });
        // `startExport` rejects on error; surface that instead of hanging on a
        // callback that will never fire.
        Promise.resolve(engine.api.startExport()).catch((e) =>
            reject(e instanceof Error ? e : new Error(String(e))),
        );
    });
}
