import { app } from '../../state/app.svelte';

const THUMB_SIZE = 36;

/** Convert RGBA byte array to a data URL suitable for <img src>. */
export function rgbaToDataUrl(rgba: Uint8Array, width: number, height: number): string {
    const tmpCanvas = document.createElement('canvas');
    tmpCanvas.width = width;
    tmpCanvas.height = height;
    const tmpCtx = tmpCanvas.getContext('2d')!;
    // Copy into a Uint8ClampedArray backed by a fresh ArrayBuffer to satisfy ImageData
    const clamped = new Uint8ClampedArray(rgba.length);
    clamped.set(rgba);
    const imageData = new ImageData(clamped, width, height);
    tmpCtx.putImageData(imageData, 0, 0);
    return tmpCanvas.toDataURL();
}

/** Get a layer content thumbnail as a data URL. Returns empty string for non-raster layers. */
export function getLayerThumbnail(layerId: number): string {
    if (!app.handle) return '';
    const rgba = app.handle.layer_thumbnail(layerId, THUMB_SIZE, THUMB_SIZE);
    if (!rgba || rgba.length === 0) return '';
    return rgbaToDataUrl(rgba, THUMB_SIZE, THUMB_SIZE);
}

/** Get a mask thumbnail as a data URL. Returns empty string if no mask. */
export function getMaskThumbnail(layerId: number): string {
    if (!app.handle) return '';
    const rgba = app.handle.mask_thumbnail(layerId, THUMB_SIZE, THUMB_SIZE);
    if (!rgba || rgba.length === 0) return '';
    return rgbaToDataUrl(rgba, THUMB_SIZE, THUMB_SIZE);
}

export { THUMB_SIZE };
