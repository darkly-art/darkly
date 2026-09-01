import { ToolBase, type ToolDescriptor } from './registry';
import type { DarklyInstance } from '../state/app.svelte';
import { beginPaintStroke } from './paint_stroke';

/** Fill-tool session state: an app-global user preference. Persists within the
 *  session; resets on reload. */
class FillSession {
    /** Color-distance threshold for the flood fill (0 = exact match, 255 = anything). */
    tolerance = $state(32);
}
export const fillSession = new FillSession();

class FillTool extends ToolBase {
    onPointerDown(_e: PointerEvent, cx: number, cy: number): void {
        const engine = this.engine;
        const layerId = this.inst.activeLayerId;
        if (!layerId || !engine) return;

        const c = this.inst.consumeForeground();

        beginPaintStroke(engine, layerId);
        engine.api.strokeTo({
            op: {
                op: 'flood_fill',
                x: cx, y: cy,
                r: c.r, g: c.g, b: c.b, a: c.a,
                tolerance: fillSession.tolerance,
            },
        });
        engine.api.endStroke();
    }
}

export const fillTool: ToolDescriptor = {
    id: 'fill',
    group: 'paint',
    cluster: 'fill',
    create: (inst: DarklyInstance) => new FillTool(inst),
};
