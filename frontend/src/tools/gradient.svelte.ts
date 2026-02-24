import type { Tool, ToolContext } from './registry';
import { app } from '../state/app.svelte';

export const GRADIENT_HOTKEY = 'KeyG';

let startX = 0;
let startY = 0;

export const gradientTool: Tool = {
    id: 'gradient',
    name: 'Gradient',
    icon: 'G',
    hotkeyAction: 'gradientTool',

    onPointerDown(ctx, e, cx, cy) {
        const layerId = app.activeLayerId;
        if (!layerId) return;

        ctx.handle.begin_stroke(BigInt(layerId));
        startX = cx;
        startY = cy;
    },

    onPointerMove() {},

    onPointerUp(ctx, e) {
        const layerId = app.activeLayerId;
        if (!layerId) return;

        const pos = ctx.screenToCanvas(e.clientX, e.clientY);
        const c = app.foreground;
        const bg = app.background;

        ctx.handle.stroke_to('linear_gradient', {
            x0: startX, y0: startY,
            x1: pos.x, y1: pos.y,
            r0: c.r, g0: c.g, b0: c.b, a0: c.a,
            r1: bg.r, g1: bg.g, b1: bg.b, a1: bg.a,
        });
        ctx.handle.end_stroke();
    },
};
