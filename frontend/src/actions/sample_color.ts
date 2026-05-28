import { actions } from './registry';
import { app } from '../state/app.svelte';
import { startPick } from '../tools/color_pick_sync';
import { setEyedropperPressed } from '../tools/eyedropper_cursor';
import { screenToCanvas } from '../canvas/coordinates';

/** Register the modifier-held "sample color" chord. The action is bound to
 *  `canvas@paint:ctrl+drag` by default (Krita's binding) — the Photoshop
 *  preset overrides to `alt+drag`, GIMP inherits Ctrl. Tools can preempt
 *  this chord by returning `true` from `claimsPointer` (see
 *  `CanvasView.onPointerDown`'s dispatch order). */
export function registerSampleColorAction(): void {
    actions.register({
        id: 'sampleColor',
        displayName: 'Sample Color',
        category: 'colors',
        description:
            'Hold the modifier and drag on the canvas to sample a color into the foreground swatch.',
        type: 'hold',
        // `@paint` scope so this only fires when a paint-group tool is active.
        // Selection tools have their own Ctrl-modified gestures (subtract /
        // intersect); without the scope we'd steal those.
        defaultMouseClick: 'canvas@paint:ctrl+drag',
        handler: (ctx) => {
            if (!app.handle) return;
            const cx = typeof ctx.x === 'number' ? ctx.x : 0;
            const cy = typeof ctx.y === 'number' ? ctx.y : 0;
            setEyedropperPressed(true);
            startPick(app.handle, cx, cy);
        },
        onMove: (_ctx, e) => {
            if (!app.handle || !app.canvasEl) return;
            const { x, y } = screenToCanvas(e.clientX, e.clientY, app.canvasEl);
            startPick(app.handle, x, y);
        },
        // `deactivate` only flips the cursor back to the idle indicator —
        // the picked color itself sticks (no need to undo any foreground
        // write). Without this the cursor would freeze on the "pressed"
        // variant after pointerup.
        deactivate: () => {
            setEyedropperPressed(false);
        },
    });
}
