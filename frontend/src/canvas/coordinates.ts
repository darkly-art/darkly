/**
 * Bidirectional coordinate transforms between canvas space and screen space.
 *
 * Canvas space = document pixels (e.g. 900×1600).
 * Screen space = CSS pixels relative to the canvas element's top-left.
 *
 * The forward transform (canvas→screen) is the mathematical inverse of the
 * screen→canvas transform in gpu/view.rs. We compute it in JS from the same
 * inputs (panX, panY, zoom, rotation, doc dimensions, element dimensions, DPR).
 */

import { app } from '../state/app.svelte';
import { config } from '../config/store.svelte';

/**
 * Convert canvas coordinates to screen CSS coordinates (relative to the
 * canvas element's bounding rect).
 *
 * Forward transform pipeline (from view.rs docs):
 *   1. Translate by -canvas_center
 *   2. Scale by zoom
 *   3. Rotate by R(-rotation) = [cos, sin; -sin, cos]
 *   4. Translate by screen_center + pan (buffer pixels)
 *   5. Convert buffer pixels to CSS pixels (÷ DPR)
 */
export function canvasToScreen(
    cx: number, cy: number,
    canvasEl: HTMLCanvasElement,
): { x: number; y: number } {
    const dpr = window.devicePixelRatio || 1;
    const cos_r = Math.cos(app.rotation);
    const sin_r = Math.sin(app.rotation);

    const dx = cx - (config.get('canvas.width') as number) / 2;
    const dy = cy - (config.get('canvas.height') as number) / 2;

    const buf_x = app.zoom * (cos_r * dx + sin_r * dy)
                  + canvasEl.width / 2 + app.panX * dpr;
    const buf_y = app.zoom * (-sin_r * dx + cos_r * dy)
                  + canvasEl.height / 2 + app.panY * dpr;

    return { x: buf_x / dpr, y: buf_y / dpr };
}

/**
 * Convert screen CSS coordinates (clientX/clientY) to canvas coordinates.
 * Wraps the WASM screen_to_canvas with CSS→buffer conversion.
 */
export function screenToCanvas(
    clientX: number, clientY: number,
    canvasEl: HTMLCanvasElement,
): { x: number; y: number } {
    const dpr = window.devicePixelRatio || 1;
    const rect = canvasEl.getBoundingClientRect();
    const buf_x = (clientX - rect.left) * dpr;
    const buf_y = (clientY - rect.top) * dpr;
    const result = app.handle!.screen_to_canvas(buf_x, buf_y);
    return { x: result[0], y: result[1] };
}
