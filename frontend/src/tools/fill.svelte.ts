import type { Tool, ToolContext } from './registry';
import { app } from '../state/app.svelte';

export const FILL_HOTKEY = 'KeyF';

export const fillTool: Tool = {
    id: 'fill',
    name: 'Fill',
    icon: 'F',
    hotkeyAction: 'fillTool',

    onPointerDown(ctx, e, cx, cy) {
        const layerId = app.activeLayerId;
        if (!layerId) return;

        const c = app.foreground;

        ctx.handle.begin_stroke(BigInt(layerId));
        ctx.handle.stroke_to('flood_fill', {
            x: cx, y: cy,
            r: c.r, g: c.g, b: c.b, a: c.a,
            tolerance: app.fillTolerance,
        });
        ctx.handle.end_stroke();
    },

    onPointerMove() {},

    onPointerUp() {},
};
