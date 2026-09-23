/**
 * Raw RGBA8 pixels to the browser's image types.
 *
 * Every path out of the engine that ends in a picture (export, save thumbnail,
 * layer thumbnail, clipboard, brush resource preview) starts with the same
 * three lines, and the reason for the first of them is not guessable from
 * reading it: see {@link rgbaToImageData}. This module is the one place that
 * knows it.
 *
 * The ladder is `rgbaToImageData` at the bottom and everything else built on
 * it, so a caller picks its sink rather than restating the conversion.
 */

/**
 * RGBA8 pixels as they arrive from the engine.
 *
 * A `Uint8Array` when the payload rode the protocol's binary sidecar, and a
 * plain number array when it crossed inside the JSON value instead: `serde_json`
 * has no byte type, so a Rust `Vec<u8>` in a response body lands as an array of
 * numbers (the clipboard readback is the one that does). Every sink here copies
 * into a fresh buffer regardless, and `TypedArray.set` accepts either, so the
 * distinction stops at this module rather than spreading to the call sites.
 */
export type RgbaBytes = ArrayLike<number>;

/**
 * Wrap RGBA bytes in an `ImageData`, copying them first.
 *
 * The copy is load-bearing, not defensive. `ImageData` rejects a
 * `Uint8ClampedArray` backed by a `SharedArrayBuffer`, and a view into the WASM
 * heap can be exactly that when the engine is built with threads. Copying into
 * a fresh `ArrayBuffer` is what makes the bytes acceptable, and it also detaches
 * the result from a heap that the next engine call may resize under us.
 */
export function rgbaToImageData(rgba: RgbaBytes, width: number, height: number): ImageData {
    const copy = new Uint8ClampedArray(rgba.length);
    copy.set(rgba);
    return new ImageData(copy, width, height);
}

/** Paint RGBA bytes into an `OffscreenCanvas`, for the encode paths. */
export function rgbaToCanvas(rgba: RgbaBytes, width: number, height: number): OffscreenCanvas {
    const canvas = new OffscreenCanvas(width, height);
    const ctx = canvas.getContext('2d');
    if (!ctx) throw new Error('2d context unavailable');
    ctx.putImageData(rgbaToImageData(rgba, width, height), 0, 0);
    return canvas;
}

/** Encode RGBA bytes to an image Blob. The browser's encoder runs off the WASM
 *  main thread. `quality` is omitted for PNG, which is lossless and ignores it. */
export async function rgbaToBlob(
    rgba: RgbaBytes,
    width: number,
    height: number,
    mime: string,
    quality?: number,
): Promise<Blob> {
    const canvas = rgbaToCanvas(rgba, width, height);
    return quality === undefined
        ? await canvas.convertToBlob({ type: mime })
        : await canvas.convertToBlob({ type: mime, quality });
}

/**
 * Encode RGBA bytes to a data URL suitable for `<img src>`.
 *
 * An `HTMLCanvasElement` rather than the `OffscreenCanvas` the other sinks use:
 * `toDataURL` is synchronous and `convertToBlob` is not, and the layer panel
 * reads thumbnails inside a `$derived` that cannot await.
 */
export function rgbaToDataUrl(rgba: RgbaBytes, width: number, height: number): string {
    const canvas = document.createElement('canvas');
    canvas.width = width;
    canvas.height = height;
    const ctx = canvas.getContext('2d');
    if (!ctx) throw new Error('2d context unavailable');
    ctx.putImageData(rgbaToImageData(rgba, width, height), 0, 0);
    return canvas.toDataURL();
}

/** Decode RGBA bytes into an `ImageBitmap`, for GPU-side reuse. */
export function rgbaToBitmap(
    rgba: RgbaBytes,
    width: number,
    height: number,
): Promise<ImageBitmap> {
    return createImageBitmap(rgbaToImageData(rgba, width, height));
}
