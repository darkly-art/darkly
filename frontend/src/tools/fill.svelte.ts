import type { Tool, ToolContext } from './registry';
import { app } from '../state/app.svelte';

/** Fill-tool session state. Persists within the session; resets on reload. */
class FillSession {
    /** Color-distance threshold for the flood fill (0 = exact match, 255 = anything). */
    tolerance = $state(32);
}
export const fillSession = new FillSession();

export const fillTool: Tool = {
    id: 'fill',
    faIcon: 'fa-solid fa-fill-drip',
    group: 'paint',
    hotkeyAction: 'fillTool',

    onPointerDown(ctx, e, cx, cy) {
        const layerId = app.activeLayerId;
        if (!layerId) return;

        const c = app.foreground;

        ctx.handle.begin_stroke(layerId);
        ctx.handle.stroke_to('flood_fill', {
            x: cx, y: cy,
            r: c.r, g: c.g, b: c.b, a: c.a,
            tolerance: fillSession.tolerance,
        });
        ctx.handle.end_stroke();
    },

    onPointerMove() {},

    onPointerUp() {},
};
