import type { Tool, ToolContext } from './registry';
import { app } from '../state/app.svelte';

export const ERASER_HOTKEY = 'KeyE';

export const eraserTool: Tool = {
    id: 'eraser',
    name: 'Eraser',
    icon: 'E',
    hotkeyAction: 'eraserTool',

    onPointerDown(ctx, e, cx, cy) {
        const layerId = app.activeLayerId;
        if (!layerId) return;

        ctx.handle.begin_stroke(BigInt(layerId));

        ctx.handle.stroke_to('erase_circle', {
            x: cx, y: cy, radius: app.brushSize,
        });
    },

    onPointerMove(ctx, e, cx, cy) {
        if (!(e.buttons & 1)) return;
        ctx.handle.stroke_to('erase_circle', {
            x: cx, y: cy, radius: app.brushSize,
        });
    },

    onPointerUp(ctx) {
        ctx.handle.end_stroke();
    },
};
