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
import BrushOptions from '../ui/BrushOptions.svelte';
import BrushBuilderPanel from '../ui/BrushBuilderPanel.svelte';

/** Brush-tool session state. Persists across strokes within the session;
 *  resets on reload. The engine-side blend-mode mirror is pushed by
 *  `onActivate` / `onDeactivate` and by the toggleEraseMode action. */
class BrushSession {
    /** When true, strokes use destination-out (erase) instead of source-over. */
    eraseMode = $state(false);
}
export const brushSession = new BrushSession();

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

/** Pose for the on-canvas cursor preview. Tracks the live PointerEvent
 *  verbatim so the cursor circle reflects what a dab at this pose would
 *  actually look like — pressure-driven dynamics included. The resize
 *  scrub uses the same pose, keeping cursor and stroke in lockstep. */
export function cursorPose(e: PointerEvent): PenPose {
    return {
        pressure: e.pressure,
        tiltX: (e.tiltX ?? 0) / 90,
        tiltY: (e.tiltY ?? 0) / 90,
        twist: (e.twist ?? 0) / 360,
        tangentialPressure: (e as any).tangentialPressure ?? 0,
    };
}

/** Last hover pose+position pushed to the overlay. Cached so non-event
 *  callers (the `[` / `]` size hotkeys) can re-push at the same spot
 *  after mutating the graph — otherwise the on-canvas circle stays at
 *  the old size until the user wiggles the pointer. Cleared on stroke
 *  start, pointer-leave, and tool deactivate, so it only exists while
 *  a hover preview is actually visible. */
let lastHover: { cx: number; cy: number; pose: PenPose } | null = null;

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
        lastHover = null;
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
    lastHover = { cx, cy, pose };
}

/** Re-push the hover overlay at the last known hover position. No-op
 *  if the pointer isn't currently hovering the canvas (no cached
 *  pose). Used by hotkey-driven brush-param changes so the on-canvas
 *  preview reflects the new value without requiring pointer motion. */
export function refreshHoverOverlay(handle: any) {
    if (!lastHover) return;
    pushHoverOverlay(handle, lastHover.pose, lastHover.cx, lastHover.cy);
}

/** Drop the cached hover. Called whenever the overlay is cleared
 *  (stroke start, pointer leave, tool deactivate) so a stale position
 *  can't resurrect the preview. */
function clearHover() {
    lastHover = null;
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
    /** Icon swaps to the eraser glyph while `brushSession.eraseMode` is on,
     *  giving the toolbar button a visible mode indicator. Reactive because
     *  Svelte's template re-reads the getter when `brushSession.eraseMode`
     *  ($state) changes. */
    get faIcon() {
        return brushSession.eraseMode
            ? 'fa-solid fa-eraser'
            : 'fa-solid fa-paint-brush';
    },
    group: 'paint',
    hotkeyAction: 'brushTool',
    optionsComponent: BrushOptions,
    panelComponent: BrushBuilderPanel,

    onActivate(ctx) {
        // Initialize brush graph state from WASM on first activation.
        if (!brushGraph.graph && app.handle) {
            brushGraph.init();
        }
        // Sync session erase-mode flag to the engine. Other tools that
        // don't paint never read brush_blend_mode; brush tools that do
        // (color_output) will pick this up on the next stroke.
        ctx.handle.set_brush_blend_mode(brushSession.eraseMode ? 1 : 0);
        // Hide the native cursor only if a preview is available — otherwise
        // fall back to the default cursor so the user has *something* to see.
        const info = ctx.handle.get_brush_preview_info();
        app.toolCursor = info ? 'none' : null;
    },

    onDeactivate(ctx) {
        ctx.handle.clear_overlay();
        // Reset engine blend mode so a future paint-capable tool (or a
        // direct WASM call) doesn't inherit our erase state.
        ctx.handle.set_brush_blend_mode(0);
        app.toolCursor = null;
        clearHover();
    },

    onPointerDown(ctx, e, cx, cy) {
        const layerId = app.activeLayerId;
        if (!layerId) return;

        // Clear the hover overlay while painting — the stamp renders onto
        // the canvas directly; a ghost at the cursor would just clutter.
        ctx.handle.clear_overlay();
        ctx.handle.clear_brush_preview_pose();
        clearHover();
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
        clearHover();
    },
};
