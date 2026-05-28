import type { Tool } from './registry';
import { startPick } from './color_pick_sync';
import {
    setEyedropperToolActive,
    setEyedropperPressed,
} from './eyedropper_cursor';
import ColorPickerOptions from '../ui/ColorPickerOptions.svelte';

export const colorPickerTool: Tool = {
    id: 'colorpicker',
    faIcon: 'fa-solid fa-eye-dropper',
    group: 'paint',
    hotkeyAction: 'colorPickerTool',
    optionsComponent: ColorPickerOptions,

    onActivate() {
        setEyedropperToolActive(true);
    },

    onDeactivate() {
        setEyedropperToolActive(false);
    },

    onPointerDown(ctx, _e, cx, cy) {
        setEyedropperPressed(true);
        startPick(ctx.handle, cx, cy);
    },

    onPointerMove(ctx, e, cx, cy) {
        if (e.buttons & 1) {
            startPick(ctx.handle, cx, cy);
        }
    },

    onPointerUp() {
        setEyedropperPressed(false);
    },

    onPointerLeave() {
        setEyedropperPressed(false);
    },
    // No `onFrame` — `pollPick` runs unconditionally from the frame loop in
    // app.svelte.ts, and `tickEyedropperCursor` next to it keeps the cursor
    // in sync with foreground updates regardless of which tool is active.
};
