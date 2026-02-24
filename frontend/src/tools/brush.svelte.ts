import type { Tool, ToolContext } from './registry';
import { app } from '../state/app.svelte';

// --- Constants & hotkey (hotkey imported by schema.ts) ---

export const MIN_SIZE = 1;
export const MAX_SIZE = 500;
export const SIZE_STEP = 4;
export const INITIAL_SIZE = 24;
export const INITIAL_OPACITY = 1.0;

export const BRUSH_HOTKEY = 'KeyB';

// --- Gesture interpreter ---

export const brushTool: Tool = {
    id: 'brush',
    name: 'Brush',
    icon: 'B',
    hotkeyAction: 'brushTool',

    onPointerDown(ctx, e, cx, cy) {
        const layerId = app.activeLayerId;
        if (!layerId) return;

        ctx.handle.begin_stroke(BigInt(layerId));

        const c = app.foreground;
        const alpha = Math.round(c.a * app.brushOpacity);
        ctx.handle.stroke_to('paint_circle', {
            x: cx, y: cy, radius: app.brushSize,
            r: c.r, g: c.g, b: c.b, a: alpha,
        });
    },

    onPointerMove(ctx, e, cx, cy) {
        if (!(e.buttons & 1)) return;
        const c = app.foreground;
        const alpha = Math.round(c.a * app.brushOpacity);
        ctx.handle.stroke_to('paint_circle', {
            x: cx, y: cy, radius: app.brushSize,
            r: c.r, g: c.g, b: c.b, a: alpha,
        });
    },

    onPointerUp(ctx) {
        ctx.handle.end_stroke();
    },
};
