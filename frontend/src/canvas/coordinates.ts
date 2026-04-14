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
 *
 * Pure-JS inverse of the forward transform above — avoids calling into WASM,
 * which would alias the RefCell borrow if a pointer event fires while
 * render() holds &mut self (WebGPU can synchronously pump the event queue).
 *
 * Inverse transform (from view.rs ViewTransform::from_pan_zoom_rotate):
 *   1. CSS → buffer pixels (* DPR, - element offset)
 *   2. Apply inverse view matrix: M^-1 * [buf_x, buf_y]
 *      where M^-1 = [cos/z, sin/z; -sin/z, cos/z] with translation
 */
export function screenToCanvas(
    clientX: number, clientY: number,
    canvasEl: HTMLCanvasElement,
): { x: number; y: number } {
    const dpr = window.devicePixelRatio || 1;
    const rect = canvasEl.getBoundingClientRect();
    const buf_x = (clientX - rect.left) * dpr;
    const buf_y = (clientY - rect.top) * dpr;

    const cos_r = Math.cos(app.rotation);
    const sin_r = Math.sin(app.rotation);
    const inv_zoom = 1.0 / app.zoom;

    const canvas_w = config.get('canvas.width') as number;
    const canvas_h = config.get('canvas.height') as number;
    const cx = canvas_w / 2;
    const cy = canvas_h / 2;

    // Screen center + pan in buffer pixels (matches Rust's sx, sy)
    const sx = canvasEl.width / 2 + app.panX * dpr;
    const sy = canvasEl.height / 2 + app.panY * dpr;

    // Inverse matrix coefficients (same as view.rs)
    const m00 = cos_r * inv_zoom;
    const m01 = sin_r * inv_zoom;
    const m10 = -sin_r * inv_zoom;
    const m11 = cos_r * inv_zoom;
    const tx = cx - m00 * sx - m10 * sy;
    const ty = cy - m01 * sx - m11 * sy;

    return {
        x: m00 * buf_x + m10 * buf_y + tx,
        y: m01 * buf_x + m11 * buf_y + ty,
    };
}
