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
    icon: 'fa6-solid:fill-drip',
    group: 'paint',
    cluster: 'fill',
    hotkeyAction: 'fillTool',

    onPointerDown(ctx, e, cx, cy) {
        const layerId = app.activeLayerId;
        if (!layerId) return;

        const c = app.foreground;

        ctx.engine.api.beginStroke({ id: layerId });
        ctx.engine.api.strokeTo({
            op: {
                op: 'flood_fill',
                x: cx, y: cy,
                r: c.r, g: c.g, b: c.b, a: c.a,
                tolerance: fillSession.tolerance,
            },
        });
        ctx.engine.api.endStroke();
    },

    onPointerMove() {},

    onPointerUp() {},
};
