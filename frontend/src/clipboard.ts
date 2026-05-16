/**
 * System clipboard integration — image encode/decode via Canvas API, plus
 * a custom `web application/x-darkly-layer` MIME for cross-tab paste with
 * full blend mode + opacity + name preservation.
 *
 * All image decoding uses the browser's createImageBitmap + OffscreenCanvas,
 * which handles PNG, JPEG, WebP, GIF, BMP, SVG, AVIF, ICO — anything the
 * browser supports — and always outputs sRGB RGBA8. No prompts, no color
 * profile dialogs.
 *
 * The custom MIME ("web custom" types — Chrome 104+, Edge, Safari 17.4+)
 * lets two Darkly tabs round-trip a layer's full state (including blend
 * mode and opacity) through the system clipboard, while still leaving a
 * standard PNG fallback for paste into other apps.
 */

/** MIME type for our rich-layer JSON envelope. The `web ` prefix is
 *  required by the Web Custom Formats spec — without it, browsers refuse
 *  to write or read the type. */
export const LAYER_CLIPBOARD_MIME = 'web application/x-darkly-layer';

/**
 * Write a copied layer to the system clipboard.
 *
 * Writes BOTH a standard `image/png` (so paste into any other app works)
 * AND the custom Darkly layer JSON. When the same Darkly tab — or another
 * Darkly tab — pastes, the rich path picks up the JSON first; other apps
 * see the PNG.
 *
 * `richJson` is optional — pass `undefined` for the existing pixels-only
 * path (e.g. when the rich payload isn't ready yet).
 */
export async function copyToSystemClipboard(
    rgba: Uint8Array,
    width: number,
    height: number,
    richJson?: string,
): Promise<void> {
    try {
        const canvas = new OffscreenCanvas(width, height);
        const ctx = canvas.getContext('2d')!;
        // Copy into a fresh ArrayBuffer to satisfy ImageData's type requirement
        // (Uint8ClampedArray from WASM memory may have SharedArrayBuffer backing).
        const copy = new Uint8ClampedArray(rgba.length);
        copy.set(rgba);
        const imageData = new ImageData(copy, width, height);
        ctx.putImageData(imageData, 0, 0);
        const blob = await canvas.convertToBlob({ type: 'image/png' });

        const items: Record<string, Blob | Promise<Blob>> = { 'image/png': blob };
        if (richJson) {
            items[LAYER_CLIPBOARD_MIME] = new Blob([richJson], { type: LAYER_CLIPBOARD_MIME });
        }
        await navigator.clipboard.write([new ClipboardItem(items)]);
    } catch (e) {
        // Permission denied or API unavailable — silently ignore.
        console.warn('Failed to write to system clipboard:', e);
    }
}

/**
 * Read the rich layer JSON from the system clipboard, if present. Returns
 * `null` when no Darkly tab put a layer there (e.g. content was pasted
 * from another app, or the clipboard is empty).
 *
 * Always returns synchronously-resolvable strings — no pixel decoding here;
 * the JSON itself carries base64-encoded RGBA which the engine decodes.
 */
export async function readLayerFromClipboard(): Promise<string | null> {
    try {
        const items = await navigator.clipboard.read();
        for (const item of items) {
            if (item.types.includes(LAYER_CLIPBOARD_MIME)) {
                const blob = await item.getType(LAYER_CLIPBOARD_MIME);
                return await blob.text();
            }
        }
    } catch (e) {
        console.warn('Failed to read rich layer from clipboard:', e);
    }
    return null;
}

/**
 * Read an image from the system clipboard and decode to raw RGBA bytes.
 * Returns null if no image is found or the Clipboard API is unavailable.
 *
 * Used as the fallback when the rich layer MIME isn't present — content
 * came from another app, or from a Darkly version that didn't write JSON.
 */
export async function readImageFromClipboard(): Promise<{
    rgba: Uint8Array;
    width: number;
    height: number;
} | null> {
    try {
        const items = await navigator.clipboard.read();
        for (const item of items) {
            for (const type of item.types) {
                if (type.startsWith('image/')) {
                    const blob = await item.getType(type);
                    const bitmap = await createImageBitmap(blob);
                    const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
                    const ctx = canvas.getContext('2d')!;
                    ctx.drawImage(bitmap, 0, 0);
                    bitmap.close();
                    const imageData = ctx.getImageData(0, 0, canvas.width, canvas.height);
                    return {
                        rgba: new Uint8Array(imageData.data.buffer),
                        width: canvas.width,
                        height: canvas.height,
                    };
                }
            }
        }
    } catch (e) {
        // Permission denied, API unavailable, or no image content.
        console.warn('Failed to read from system clipboard:', e);
    }
    return null;
}
