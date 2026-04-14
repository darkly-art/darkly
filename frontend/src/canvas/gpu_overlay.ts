/**
 * High-level GPU overlay builder.
 *
 * Why GPU instead of SVG/DOM?  Not because the CPU can't draw 14 circles —
 * it can.  The problem is that DOM-based overlays (SVG, HTML) go through the
 * browser's rendering pipeline (Svelte reactivity → DOM diff → style →
 * layout → paint → composite) on the **main thread**, on every pointer
 * move, competing with WASM/WebGPU work for the same thread.  The GPU
 * overlay avoids this entirely: `set_overlay()` is a one-way push into a
 * buffer that the GPU renders as part of the present pass it's already
 * doing.  Zero main-thread rendering work per frame.
 *
 * Tools describe overlays declaratively — lines and interactive handles in
 * canvas space — and the builder converts them to low-level GPU primitives,
 * handles DPR scaling and coordinate conversion, and provides hit-testing.
 *
 * All positions are in canvas space (document pixels).
 * Visual sizes (radius, thickness) are in CSS pixels.
 * Colors are hex strings ('#4af', '#ffffff') or [r,g,b,a] float arrays.
 */

import { canvasToScreen } from './coordinates';
import {
    KIND_LINE, KIND_CIRCLE, KIND_DASHED_LINE, KIND_FILLED_CIRCLE,
    FLAG_CANVAS_SPACE, prim,
    type GpuPrim,
} from '../tools/selection_helpers';
import type { DarklyHandle } from '../../wasm/pkg/darkly_wasm';

// ---------------------------------------------------------------------------
// Color conversion
// ---------------------------------------------------------------------------

type Color = string | [number, number, number, number];

const colorCache = new Map<string, [number, number, number, number]>();

/** Convert a hex color string to [r, g, b, a] floats in 0–1. */
function hexToRgba(hex: string): [number, number, number, number] {
    const cached = colorCache.get(hex);
    if (cached) return cached;

    let h = hex.startsWith('#') ? hex.slice(1) : hex;
    // Expand shorthand: #4af → #44aaff
    if (h.length === 3) h = h[0] + h[0] + h[1] + h[1] + h[2] + h[2];
    if (h.length === 4) h = h[0] + h[0] + h[1] + h[1] + h[2] + h[2] + h[3] + h[3];

    const r = parseInt(h.slice(0, 2), 16) / 255;
    const g = parseInt(h.slice(2, 4), 16) / 255;
    const b = parseInt(h.slice(4, 6), 16) / 255;
    const a = h.length >= 8 ? parseInt(h.slice(6, 8), 16) / 255 : 1;
    const result: [number, number, number, number] = [r, g, b, a];

    colorCache.set(hex, result);
    return result;
}

function toRgba(c: Color): [number, number, number, number] {
    return typeof c === 'string' ? hexToRgba(c) : c;
}

// ---------------------------------------------------------------------------
// Option types
// ---------------------------------------------------------------------------

export interface LineOpts {
    color?: Color;
    thickness?: number;   // CSS pixels, default 1
    dash?: number;        // dash length, 0 = solid, default 0
}

export interface HandleOpts {
    id?: any;             // tool-defined identifier, returned by hitTest
    cursor?: string;      // CSS cursor, default 'default'
    radius?: number;      // CSS pixels, default 5
    fill?: Color;         // default '#fff'
    stroke?: Color;       // default '#4af'
    strokeWidth?: number; // CSS pixels, default 1.5
}

// ---------------------------------------------------------------------------
// Internal storage
// ---------------------------------------------------------------------------

interface HandleEntry {
    canvasPos: [number, number];
    radius: number;       // CSS pixels
    id: any;
    cursor: string;
    fill: [number, number, number, number];
    stroke: [number, number, number, number];
    strokeWidth: number;
}

interface LineEntry {
    from: [number, number];
    to: [number, number];
    color: [number, number, number, number];
    thickness: number;
    dash: number;
}

// ---------------------------------------------------------------------------
// OverlayBuilder
// ---------------------------------------------------------------------------

export class OverlayBuilder {
    private canvasEl: HTMLCanvasElement;
    private lines: LineEntry[] = [];
    private handles: HandleEntry[] = [];

    constructor(canvasEl: HTMLCanvasElement) {
        this.canvasEl = canvasEl;
    }

    /** Add a line in canvas space. */
    line(from: [number, number], to: [number, number], opts?: LineOpts): this {
        this.lines.push({
            from, to,
            color: toRgba(opts?.color ?? '#fff'),
            thickness: opts?.thickness ?? 1,
            dash: opts?.dash ?? 0,
        });
        return this;
    }

    /** Add an interactive handle at a canvas-space position. */
    handle(pos: [number, number], opts?: HandleOpts): this {
        this.handles.push({
            canvasPos: pos,
            radius: opts?.radius ?? 5,
            id: opts?.id ?? null,
            cursor: opts?.cursor ?? 'default',
            fill: toRgba(opts?.fill ?? '#fff'),
            stroke: toRgba(opts?.stroke ?? '#4af'),
            strokeWidth: opts?.strokeWidth ?? 1.5,
        });
        return this;
    }

    /** Convert to GPU primitives and push to the overlay system. */
    push(wasmHandle: DarklyHandle): void {
        const dpr = window.devicePixelRatio || 1;
        const prims: GpuPrim[] = [];

        // Lines — canvas space, transformed by GPU shader
        for (const l of this.lines) {
            const kind = l.dash > 0 ? KIND_DASHED_LINE : KIND_LINE;
            prims.push(prim(kind, FLAG_CANVAS_SPACE, l.from, l.to, {
                color: l.color,
                thickness: l.thickness,
                dashLen: l.dash,
            }));
        }

        // Handles — screen space (constant pixel size)
        for (const h of this.handles) {
            const sp = canvasToScreen(h.canvasPos[0], h.canvasPos[1], this.canvasEl);
            const center: [number, number] = [sp.x * dpr, sp.y * dpr];
            const r: [number, number] = [h.radius * dpr, 0];

            prims.push(prim(KIND_FILLED_CIRCLE, 0, center, r, {
                color: h.fill,
            }));
            prims.push(prim(KIND_CIRCLE, 0, center, r, {
                color: h.stroke,
                thickness: h.strokeWidth * dpr,
            }));
        }

        wasmHandle.set_overlay(prims);
    }

    /** Clear the GPU overlay. */
    clear(wasmHandle: DarklyHandle): void {
        wasmHandle.clear_overlay();
    }

    /**
     * Hit-test against handles. Returns the nearest handle within its
     * hit radius, or null. Coordinates are in canvas space.
     */
    hitTest(canvasX: number, canvasY: number): { id: any; cursor: string } | null {
        const sp = canvasToScreen(canvasX, canvasY, this.canvasEl);
        const margin = 4; // extra CSS pixels beyond the handle radius

        let bestDist = Infinity;
        let bestHandle: HandleEntry | null = null;

        for (const h of this.handles) {
            const hp = canvasToScreen(h.canvasPos[0], h.canvasPos[1], this.canvasEl);
            const dx = sp.x - hp.x;
            const dy = sp.y - hp.y;
            const dist = Math.sqrt(dx * dx + dy * dy);
            const threshold = h.radius + margin;
            if (dist < threshold && dist < bestDist) {
                bestDist = dist;
                bestHandle = h;
            }
        }

        return bestHandle ? { id: bestHandle.id, cursor: bestHandle.cursor } : null;
    }
}
