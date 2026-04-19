import type { Tool, ToolContext } from './registry';
import { app } from '../state/app.svelte';
import { brushGraph } from '../state/brush_graph.svelte';
import {
    KIND_MASKED_STAMP,
    FLAG_CANVAS_SPACE,
    FLAG_SOFT_CONTRAST,
    prim,
} from './selection_helpers';

/** Soft-contrast strength for big brushes. Tuned by eye. */
const BASE_STRENGTH = 0.22;
/** Strength at or below the "small" threshold — compensates for the
 *  stamp covering fewer screen pixels by amping contrast. */
const MAX_STRENGTH = 0.65;
/** Half-extent in *on-screen* pixels where MAX_STRENGTH applies. */
const SMALL_ON_SCREEN = 6;
/** Half-extent in *on-screen* pixels at/above which BASE_STRENGTH applies. */
const LARGE_ON_SCREEN = 40;

interface BrushPreviewInfo {
    halfExtent: [number, number];
    rotation: number;
}

/** Scale strength with on-screen stamp size: tiny stamps get more contrast
 *  so they remain readable; big stamps stay subtle. Smooth ramp. */
function previewStrength(halfExtent: [number, number]): number {
    const minHE = Math.min(halfExtent[0], halfExtent[1]) * app.zoom;
    const t = Math.max(0, Math.min(1,
        (minHE - SMALL_ON_SCREEN) / (LARGE_ON_SCREEN - SMALL_ON_SCREEN)));
    const smooth = t * t * (3 - 2 * t);  // smoothstep
    return MAX_STRENGTH + (BASE_STRENGTH - MAX_STRENGTH) * smooth;
}

/** Push the masked-stamp overlay primitive at the cursor, if a preview is
 *  available. Also toggles the native cursor: hidden when the ghost stamp
 *  is driving, visible as a fallback when the graph has no preview sink. */
function pushHoverOverlay(handle: any, cx: number, cy: number) {
    const info = handle.get_brush_preview_info() as BrushPreviewInfo | null;
    if (!info) {
        handle.clear_overlay();
        app.toolCursor = null;
        return;
    }
    app.toolCursor = 'none';
    handle.set_overlay([
        prim(
            KIND_MASKED_STAMP,
            FLAG_CANVAS_SPACE | FLAG_SOFT_CONTRAST,
            [cx, cy],
            info.halfExtent,
            { modeParam: previewStrength(info.halfExtent), rotation: info.rotation },
        ),
    ]);
}

export const MIN_SIZE = 1;
export const MAX_SIZE = 500;
export const SIZE_STEP = 4;
export const INITIAL_SIZE = 24;
export const INITIAL_OPACITY = 1.0;

/** Convert sRGB 0-255 to linear 0-1. */
function srgbToLinear(c: number): number {
    const s = c / 255;
    return s <= 0.04045 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
}

/** Build a brush_stroke params object from a PointerEvent. */
function brushStrokeParams(e: PointerEvent, cx: number, cy: number) {
    const c = app.foreground;
    return {
        x: cx,
        y: cy,
        pressure: e.pressure,
        x_tilt: (e.tiltX ?? 0) / 90, // normalize -90..90 → -1..1
        y_tilt: (e.tiltY ?? 0) / 90,
        rotation: (e.twist ?? 0) / 360, // normalize 0..359 → 0..1
        tangential_pressure: (e as any).tangentialPressure ?? 0,
        time_ms: e.timeStamp,
        cr: srgbToLinear(c.r),
        cg: srgbToLinear(c.g),
        cb: srgbToLinear(c.b),
        ca: c.a / 255,
    };
}

// --- Gesture interpreter ---

export const brushTool: Tool = {
    id: 'brush',
    name: 'Brush',
    faIcon: 'fa-solid fa-paintbrush',
    group: 'paint',
    hotkeyAction: 'brushTool',

    onActivate(ctx) {
        // Initialize brush graph state from WASM on first activation.
        if (!brushGraph.graph && app.handle) {
            brushGraph.init();
        }
        // Hide the native cursor only if a preview is available — otherwise
        // fall back to the default cursor so the user has *something* to see.
        const info = ctx.handle.get_brush_preview_info();
        app.toolCursor = info ? 'none' : null;
    },

    onDeactivate(ctx) {
        ctx.handle.clear_overlay();
        app.toolCursor = null;
    },

    onPointerDown(ctx, e, cx, cy) {
        const layerId = app.activeLayerId;
        if (!layerId) return;

        // Clear the hover overlay while painting — the stamp renders onto
        // the canvas directly; a ghost at the cursor would just clutter.
        ctx.handle.clear_overlay();
        ctx.handle.begin_stroke(layerId);
        ctx.handle.stroke_to('brush_stroke', brushStrokeParams(e, cx, cy));
    },

    onPointerMove(ctx, e, cx, cy) {
        if (e.buttons & 1) {
            ctx.handle.stroke_to('brush_stroke', brushStrokeParams(e, cx, cy));
            return;
        }
        // Hover: draw a soft masked-stamp preview at the cursor.
        pushHoverOverlay(ctx.handle, cx, cy);
    },

    onPointerUp(ctx) {
        ctx.handle.end_stroke();
    },

    onPointerLeave(ctx) {
        // Pointer left the canvas: drop the hover ghost so it doesn't
        // linger at the last-seen edge position.
        ctx.handle.clear_overlay();
    },
};
