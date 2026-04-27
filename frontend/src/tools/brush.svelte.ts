import type { Tool, ToolContext } from './registry';
import { app } from '../state/app.svelte';
import { brushGraph } from '../state/brush_graph.svelte';
import { srgbToLinear } from '../lib/color';
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

/** Pen pose passed to `refresh_brush_preview` — drives any pressure /
 *  tilt / twist dynamics wired into the brush graph. Components are in
 *  the normalised ranges WASM expects (pressure 0–1, tilt ±1, twist 0–1). */
export interface PenPose {
    pressure: number;
    tiltX: number;
    tiltY: number;
    twist: number;
    tangentialPressure: number;
}

/** Pose for the on-canvas cursor preview. Tilt / twist track the live event
 *  (so tilt-driven brushes like calligraphy rotate with the pen), but
 *  pressure is forced to 1.0 so the preview circle always shows the
 *  brush's *maximum* extent at the user's current size — pressure-induced
 *  shrinkage is a stroke-time effect, not something the cursor reflects. */
export function cursorPose(e: PointerEvent): PenPose {
    return {
        pressure: 1,
        tiltX: (e.tiltX ?? 0) / 90,
        tiltY: (e.tiltY ?? 0) / 90,
        twist: (e.twist ?? 0) / 360,
        tangentialPressure: (e as any).tangentialPressure ?? 0,
    };
}

/** Refresh the on-canvas brush cursor preview at `(cx, cy)` using the
 *  given pose. Exported so non-brush callers (e.g. the shift+drag size
 *  scrub, which uses `FULL_PRESS_POSE` so the circle shows the brush's
 *  max extent) can keep the preview in sync after mutating the graph. */
export function pushHoverOverlay(handle: any, pose: PenPose, cx: number, cy: number) {
    const info = handle.refresh_brush_preview(
        cx,
        cy,
        pose.pressure,
        pose.tiltX,
        pose.tiltY,
        pose.twist,
        pose.tangentialPressure,
    ) as BrushPreviewInfo | null;
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
        ctx.handle.clear_brush_preview_pose();
        ctx.handle.begin_stroke(layerId);
        ctx.handle.stroke_to('brush_stroke', brushStrokeParams(e, cx, cy));
    },

    onPointerMove(ctx, e, cx, cy) {
        if (e.buttons & 1) {
            ctx.handle.stroke_to('brush_stroke', brushStrokeParams(e, cx, cy));
            return;
        }
        // Hover: re-render the preview with live pen data + draw it.
        pushHoverOverlay(ctx.handle, cursorPose(e), cx, cy);
    },

    onPointerUp(ctx) {
        ctx.handle.end_stroke();
    },

    onPointerLeave(ctx) {
        // Pointer left the canvas: drop the hover ghost so it doesn't
        // linger at the last-seen edge position.
        ctx.handle.clear_overlay();
        ctx.handle.clear_brush_preview_pose();
    },
};
