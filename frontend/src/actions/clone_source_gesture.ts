import { actions } from './registry';
import { app } from '../state/app.svelte';
import { screenToCanvas } from '../canvas/coordinates';
import { setCloneSourceAnchor } from '../tools/clone_source_cursor';

/** Register the brush-scoped "set clone source" gesture. Like
 *  `sampleColor`, the actual binding comes from the YAML preset layers;
 *  it ships brush-scoped (`canvas@paint@clone:$mod+drag`) so it only
 *  fires while the Clone brush is active, out-ranking the group-scoped
 *  color sampler that shares the same modifier (see
 *  `hotkey_resolve.ts`'s specificity + `resolveChord`).
 *
 *  The gesture records the source anchor in the engine as plane / canvas
 *  pixels; the anchor persists across strokes until re-set. Dragging
 *  repositions it, so the release point is the final source. */
export function registerCloneSourceAction(): void {
    actions.register({
        id: 'setCloneSource',
        type: 'hold',
        handler: (ctx) => {
            const cx = typeof ctx.x === 'number' ? ctx.x : 0;
            const cy = typeof ctx.y === 'number' ? ctx.y : 0;
            setCloneSourceAnchor(cx, cy);
        },
        onMove: (_ctx, e) => {
            if (!app.canvasEl) return;
            const { x, y } = screenToCanvas(e.clientX, e.clientY, app.canvasEl);
            setCloneSourceAnchor(x, y);
        },
    });
}
