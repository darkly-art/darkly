/**
 * Bidirectional coordinate transforms between canvas (plane) space and screen
 * space.
 *
 * Canvas/plane space = document pixels (window-local + `canvas_origin`).
 * Screen space = CSS pixels relative to the canvas element's bounding rect.
 *
 * These are pure matrix-vector products over `app.viewMatrices`: the screen↔
 * plane affines built by the single Rust source of truth (`compute_view_matrices`)
 * and cached reactively on `app`. The transform math lives only in Rust; this
 * file just applies the cached matrices and handles the DOM boundary (DPR +
 * element offset). Reading the cache is borrow-free, so it is safe inside a
 * pointer event (no RefCell aliasing with an in-flight `render()`).
 *
 * `app.viewMatrices` packs 12 floats: `[screen→plane (6), plane→screen (6)]`,
 * each row-major `[m00, m01, m02, m10, m11, m12]` with
 * `out_x = m00·x + m01·y + m02`, `out_y = m10·x + m11·y + m12`.
 */

import { app } from '../state/app.svelte';

/**
 * Convert canvas (plane) coordinates to screen CSS coordinates (relative to the
 * canvas element's bounding rect).
 */
export function canvasToScreen(
    cx: number, cy: number,
    _canvasEl: HTMLCanvasElement,
): { x: number; y: number } {
    const dpr = window.devicePixelRatio || 1;
    const m = app.viewMatrices; // plane→screen at offset 6, output in buffer px
    const buf_x = m[6] * cx + m[7] * cy + m[8];
    const buf_y = m[9] * cx + m[10] * cy + m[11];
    return { x: buf_x / dpr, y: buf_y / dpr };
}

/**
 * Convert screen CSS coordinates (clientX/clientY) to canvas (plane) coordinates.
 */
export function screenToCanvas(
    clientX: number, clientY: number,
    canvasEl: HTMLCanvasElement,
): { x: number; y: number } {
    const dpr = window.devicePixelRatio || 1;
    const rect = canvasEl.getBoundingClientRect();
    const buf_x = (clientX - rect.left) * dpr;
    const buf_y = (clientY - rect.top) * dpr;

    const m = app.viewMatrices; // screen→plane at offset 0
    return {
        x: m[0] * buf_x + m[1] * buf_y + m[2],
        y: m[3] * buf_x + m[4] * buf_y + m[5],
    };
}
