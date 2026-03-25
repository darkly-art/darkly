import type { Tool, ToolContext } from './registry';
import { app } from '../state/app.svelte';
import { brushGraph } from '../state/brush_graph.svelte';

/** Convert sRGB 0-255 to linear 0-1. */
function srgbToLinear(c: number): number {
    const s = c / 255;
    return s <= 0.04045 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
}

/** Build brush_stroke params for eraser (white color, full opacity — only alpha matters for erase). */
function eraserStrokeParams(e: PointerEvent, cx: number, cy: number) {
    return {
        x: cx,
        y: cy,
        pressure: e.pressure,
        x_tilt: (e.tiltX ?? 0) / 90,
        y_tilt: (e.tiltY ?? 0) / 90,
        rotation: (e.twist ?? 0) / 360,
        tangential_pressure: (e as any).tangentialPressure ?? 0,
        time_ms: e.timeStamp,
        cr: 1.0,
        cg: 1.0,
        cb: 1.0,
        ca: app.brushOpacity,
    };
}

export const eraserTool: Tool = {
    id: 'eraser',
    name: 'Eraser',
    icon: 'E',
    hotkeyAction: 'eraserTool',

    onActivate(_ctx) {
        if (!brushGraph.graph && app.handle) {
            brushGraph.init();
        }
    },

    onPointerDown(ctx, e, cx, cy) {
        const layerId = app.activeLayerId;
        if (!layerId) return;

        ctx.handle.set_brush_blend_mode(1); // erase
        ctx.handle.begin_stroke(layerId);
        ctx.handle.stroke_to('brush_stroke', eraserStrokeParams(e, cx, cy));
    },

    onPointerMove(ctx, e, cx, cy) {
        if (!(e.buttons & 1)) return;
        ctx.handle.stroke_to('brush_stroke', eraserStrokeParams(e, cx, cy));
    },

    onPointerUp(ctx) {
        ctx.handle.end_stroke();
        ctx.handle.set_brush_blend_mode(0); // restore to paint
    },
};
