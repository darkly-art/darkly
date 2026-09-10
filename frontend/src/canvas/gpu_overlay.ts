/**
 * High-level GPU overlay builder.
 *
 * Why GPU instead of SVG/DOM?  Not because the CPU can't draw 14 circles;
 * it can.  The problem is that DOM-based overlays (SVG, HTML) go through the
 * browser's rendering pipeline (Svelte reactivity → DOM diff → style →
 * layout → paint → composite) on the **main thread**, on every pointer
 * move, competing with WASM/WebGPU work for the same thread.  The GPU
 * overlay avoids this entirely: `set_overlay()` is a one-way push into a
 * buffer that the GPU renders as part of the present pass it's already
 * doing.  Zero main-thread rendering work per frame.
 *
 * Tools describe overlays declaratively (lines and interactive handles in
 * canvas space) and the builder converts them to low-level GPU primitives,
 * handles DPR scaling and coordinate conversion, and provides hit-testing.
 *
 * All positions are in canvas space (document pixels).
 * Visual sizes (radius, thickness) are in CSS pixels.
 * Colors are hex strings ('#4af', '#ffffff') or [r,g,b,a] float arrays.
 */

import { canvasToScreen } from './coordinates';
import {
    KIND_LINE, KIND_CIRCLE, KIND_DASHED_LINE, KIND_FILLED_CIRCLE,
    FLAG_CANVAS_SPACE, FLAG_INVERT_COLOR, prim,
    type GpuPrim,
} from '../tools/selection_helpers';
import type { EngineRequests } from '../engine/protocol';
import { app } from '../state/app.svelte';

// ---------------------------------------------------------------------------
// Color conversion
// ---------------------------------------------------------------------------

type Color = string | [number, number, number, number];

/** Which engine overlay channel an `OverlayBuilder` pushes to. Mirrors the
 *  Rust `OverlayChannel` split: `'tool'` is churned every hover move, so
 *  persistent markers (the clone source crosshair) use `'clone'`. */
export type OverlayChannel = 'tool' | 'clone';

const colorCache = new Map<string, [number, number, number, number]>();

/** Convert a hex color string to [r, g, b, a] floats in 0-1. */
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

export interface CrosshairOpts {
    color?: Color;        // default '#fff'
    size?: number;        // arm half-length, CSS pixels, default 8
    thickness?: number;   // CSS pixels, default 1.5
    gap?: number;         // half-gap left blank at the centre, CSS px, default 0
    /** Render via the snapshot-invert overlay path (white on dark, black on
     *  light, same as the selection marching ants). Supersedes `color`'s
     *  rgb; alpha still applies. Default false. */
    invert?: boolean;
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

interface CrosshairEntry {
    canvasPos: [number, number];
    size: number;         // CSS pixels
    thickness: number;    // CSS pixels
    gap: number;          // CSS pixels
    color: [number, number, number, number];
    invert: boolean;
}

// ---------------------------------------------------------------------------
// OverlayBuilder
// ---------------------------------------------------------------------------

export class OverlayBuilder {
    private canvasEl: HTMLCanvasElement;
    private lines: LineEntry[] = [];
    private handles: HandleEntry[] = [];
    private crosshairs: CrosshairEntry[] = [];

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

    /** Add a screen-space crosshair (constant pixel size) at a canvas-space
     *  position. Used for the clone-brush source marker, matching Krita /
     *  GIMP's source-tool cross. */
    crosshair(pos: [number, number], opts?: CrosshairOpts): this {
        this.crosshairs.push({
            canvasPos: pos,
            size: opts?.size ?? 8,
            thickness: opts?.thickness ?? 1.5,
            gap: opts?.gap ?? 0,
            color: toRgba(opts?.color ?? '#fff'),
            invert: opts?.invert ?? false,
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

    /** Convert to GPU primitives and push to the given overlay channel.
     *  `'tool'` (default) is the transient active-tool channel, replaced
     *  every hover move; `'clone'` is the clone-brush source marker channel,
     *  which persists across the dab preview's `'tool'` churn. */
    push(engine: EngineRequests, channel: OverlayChannel = 'tool'): void {
        const dpr = window.devicePixelRatio || 1;
        const prims: GpuPrim[] = [];

        // Lines: canvas space, transformed by GPU shader
        for (const l of this.lines) {
            const kind = l.dash > 0 ? KIND_DASHED_LINE : KIND_LINE;
            prims.push(prim(kind, FLAG_CANVAS_SPACE, l.from, l.to, {
                color: l.color,
                thickness: l.thickness,
                dashLen: l.dash,
            }));
        }

        // Handles: screen space (constant pixel size)
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

        // Crosshairs: screen space (constant pixel size), same frame as
        // handles. Each arm is a separate line so an optional centre gap
        // leaves the exact source pixel visible.
        for (const c of this.crosshairs) {
            const sp = canvasToScreen(c.canvasPos[0], c.canvasPos[1], this.canvasEl);
            const cx = sp.x * dpr;
            const cy = sp.y * dpr;
            const size = c.size * dpr;
            const gap = c.gap * dpr;
            const thickness = c.thickness * dpr;
            const flags = c.invert ? FLAG_INVERT_COLOR : 0;
            const arm = (from: [number, number], to: [number, number]) =>
                prims.push(prim(KIND_LINE, flags, from, to, { color: c.color, thickness }));
            arm([cx - size, cy], [cx - gap, cy]);
            arm([cx + gap, cy], [cx + size, cy]);
            arm([cx, cy - size], [cx, cy - gap]);
            arm([cx, cy + gap], [cx, cy + size]);
        }

        if (channel === 'clone') {
            engine.api.setCloneOverlay({ primitives: prims });
        } else {
            engine.api.setOverlay({ primitives: prims });
        }
        // Overlay updates may originate outside a pointer event (e.g. async
        // GPU readback completion in a tool's onFrame hook). The frame loop
        // has already decided whether to continue based on render()'s return
        // value, so nothing would otherwise present these new primitives.
        app.requestFrame();
    }

    /** Clear the given overlay channel (`'tool'` default | `'clone'`). */
    clear(engine: EngineRequests, channel: OverlayChannel = 'tool'): void {
        if (channel === 'clone') {
            engine.api.clearCloneOverlay();
        } else {
            engine.api.clearOverlay();
        }
        app.requestFrame();
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
