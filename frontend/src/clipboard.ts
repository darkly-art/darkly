/**
 * System clipboard integration — image encode/decode via Canvas API.
 *
 * All image decoding uses the browser's createImageBitmap + OffscreenCanvas,
 * which handles PNG, JPEG, WebP, GIF, BMP, SVG, AVIF, ICO — anything the
 * browser supports — and always outputs sRGB RGBA8. No prompts, no color
 * profile dialogs.
 */

/**
 * Encode RGBA bytes as PNG and write to the system clipboard.
 * Silently no-ops if the Clipboard API is unavailable or permission is denied.
 */
export async function copyToSystemClipboard(
    rgba: Uint8Array,
    width: number,
    height: number,
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
        await navigator.clipboard.write([new ClipboardItem({ 'image/png': blob })]);
    } catch (e) {
        // Permission denied or API unavailable — silently ignore.
        console.warn('Failed to write to system clipboard:', e);
    }
}

/**
 * Read an image from the system clipboard and decode to raw RGBA bytes.
 * Returns null if no image is found or the Clipboard API is unavailable.
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
