import type { Tool } from './registry';
import { app } from '../state/app.svelte';

// pick_color queues an async GPU readback and returns the *previous* cached
// result synchronously. Consuming the sync return would apply the prior pick's
// color, making every click feel one step behind. Instead, flag that a pick is
// in flight and commit the real color in onFrame once the readback lands.
let waitingForPick = false;

export const colorPickerTool: Tool = {
    id: 'colorpicker',
    faIcon: 'fa-solid fa-eye-dropper',
    group: 'paint',
    hotkeyAction: 'colorPickerTool',

    onPointerDown(ctx, _e, cx, cy) {
        ctx.handle.pick_color(cx, cy);
        waitingForPick = true;
    },

    onPointerMove(ctx, e, cx, cy) {
        if (!(e.buttons & 1)) return;
        ctx.handle.pick_color(cx, cy);
        waitingForPick = true;
    },

    onPointerUp() {},

    onFrame() {
        if (!waitingForPick || !app.handle) return;
        if (app.handle.has_pending_color_pick()) return;
        const rgba = app.handle.last_picked_color();
        if (rgba.length >= 4) {
            app.foreground = { r: rgba[0], g: rgba[1], b: rgba[2], a: rgba[3] };
        }
        waitingForPick = false;
    },
};
