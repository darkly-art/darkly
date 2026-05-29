import { actions } from './registry';
import { app } from '../state/app.svelte';
import { startPick } from '../tools/color_pick_sync';
import { setColorPickerPressed } from '../tools/colorpicker_cursor';
import { screenToCanvas } from '../canvas/coordinates';

/** Register the modifier-held "sample color" chord. The actual binding
 *  comes from the YAML preset layers — Krita/GIMP ship `ctrl+drag`,
 *  Photoshop ships `alt+drag`; the action itself just declares the
 *  semantics. Tools can preempt this chord by returning `true` from
 *  `claimsPointer` (see `CanvasView.onPointerDown`'s dispatch order). */
export function registerSampleColorAction(): void {
    actions.register({
        id: 'sampleColor',
        displayName: 'Sample Color',
        category: 'colors',
        description:
            'Hold the modifier and drag on the canvas to sample a color into the foreground swatch.',
        type: 'hold',
        handler: (ctx) => {
            if (!app.handle) return;
            const cx = typeof ctx.x === 'number' ? ctx.x : 0;
            const cy = typeof ctx.y === 'number' ? ctx.y : 0;
            setColorPickerPressed(true);
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
            setColorPickerPressed(false);
        },
    });
}
