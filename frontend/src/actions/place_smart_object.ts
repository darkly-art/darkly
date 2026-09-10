//! Decoding images and placing them as smart objects.
//!
//! Split out of `actions/index.ts` so the modules that need only this (the
//! clipboard actions, the void picker) don't pull in the whole action
//! registry's dependency graph with it.

import { app } from '../state/app.svelte';
import { toast } from '../state/toast.svelte';

/** Largest source dimension we place. Above this the image is downscaled at
 *  decode time.
 *
 *  Two reasons, both real: the pixels cross the WASM boundary as a copy, so a
 *  huge source costs a proportional transfer (4096² RGBA is already 64 MB), and
 *  a smart object keeps its source resident in VRAM alongside a mip chain.
 *  4096 is comfortably above any logo or photograph an artist places and well
 *  inside every backend's `maxTextureDimension2D`. */
const MAX_SOURCE_DIM = 4096;

/** Decoded image pixels, straight (un-premultiplied) RGBA8. */
export type DecodedImage = { width: number; height: number; rgba: Uint8Array };

/** Decode any image blob to raw RGBA, downscaling anything past
 *  [`MAX_SOURCE_DIM`]. The single decode path: every caller that needs pixels
 *  out of a `Blob`/`File` goes through here rather than repeating the
 *  `createImageBitmap` → `OffscreenCanvas` → `getImageData` dance.
 *
 *  Returns `null` on decode failure; the caller owns the artist-facing message,
 *  since what to say depends on how the image arrived. */
export async function decodeToRgba(blob: Blob): Promise<DecodedImage | null> {
    let bitmap: ImageBitmap;
    try {
        bitmap = await createImageBitmap(blob);
    } catch (e) {
        console.error('[image] decode failed', e);
        return null;
    }

    // Resize during decode when oversized: `createImageBitmap`'s own
    // resampler is better than anything we'd do afterwards, and it avoids
    // ever materializing the full-size buffer.
    const longest = Math.max(bitmap.width, bitmap.height);
    if (longest > MAX_SOURCE_DIM) {
        const scale = MAX_SOURCE_DIM / longest;
        const resized = await createImageBitmap(bitmap, {
            resizeWidth: Math.max(1, Math.round(bitmap.width * scale)),
            resizeHeight: Math.max(1, Math.round(bitmap.height * scale)),
            resizeQuality: 'high',
        });
        bitmap.close();
        bitmap = resized;
        toast.show('info', `Image downscaled to ${MAX_SOURCE_DIM}px for placement.`);
    }

    const { width, height } = bitmap;
    const canvas = new OffscreenCanvas(width, height);
    const ctx = canvas.getContext('2d');
    if (!ctx) {
        bitmap.close();
        return null;
    }
    ctx.drawImage(bitmap, 0, 0);
    bitmap.close();
    return { width, height, rgba: new Uint8Array(ctx.getImageData(0, 0, width, height).data.buffer) };
}

/** Place an image blob as a smart object in the CURRENT document: a layer
 *  that holds the image at its own resolution and displays it through a stored
 *  transform, so resizing it stays lossless.
 *
 *  Returns the new layer id, or `-1` if the image couldn't be decoded or
 *  placed. */
export async function placeSmartObjectFromBlob(blob: Blob, label: string): Promise<number> {
    const engine = app.engine;
    if (!engine) return -1;
    const decoded = await decodeToRgba(blob);
    if (!decoded) {
        toast.show('error', `Failed to decode ${label}`);
        return -1;
    }
    try {
        const { id } = await engine.api.placeSmartObject(
            {
                width: decoded.width,
                height: decoded.height,
                active_layer_id: app.activeLayerId ?? -1,
            },
            decoded.rgba,
        );
        app.selectLayer(id);
        await app.refreshLayerTree();
        app.requestFrame();
        return id;
    } catch (e) {
        toast.show('error', `Failed to place ${label}`);
        console.error('[place] smart object failed', e);
        return -1;
    }
}
