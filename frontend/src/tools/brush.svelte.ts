import type { Tool, ToolContext } from './registry';
import { app } from '../state/app.svelte';

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
        ca: (c.a / 255) * app.brushOpacity,
    };
}

// --- Gesture interpreter ---

export const brushTool: Tool = {
    id: 'brush',
    name: 'Brush',
    icon: 'B',
    hotkeyAction: 'brushTool',

    onPointerDown(ctx, e, cx, cy) {
        const layerId = app.activeLayerId;
        if (!layerId) return;

        ctx.handle.begin_stroke(layerId);
        ctx.handle.stroke_to('brush_stroke', brushStrokeParams(e, cx, cy));
    },

    onPointerMove(ctx, e, cx, cy) {
        if (!(e.buttons & 1)) return;
        ctx.handle.stroke_to('brush_stroke', brushStrokeParams(e, cx, cy));
    },

    onPointerUp(ctx) {
        ctx.handle.end_stroke();
    },
};
