import type { Tool } from './registry';
import { startPick } from './color_pick_sync';
import {
    tickColorPickerCursor,
    setColorPickerPressed,
} from './colorpicker_cursor';
import ColorPickerOptions from '../ui/ColorPickerOptions.svelte';

export const colorPickerTool: Tool = {
    id: 'colorpicker',
    faIcon: 'fa-solid fa-eye-dropper',
    group: 'paint',
    hotkeyAction: 'colorPickerTool',
    optionsComponent: ColorPickerOptions,

    onActivate() {
        // Take ownership of `app.toolCursor` immediately — CanvasView's
        // tool-switch $effect resets it to null right before calling us,
        // so we need to push the picker cursor now rather than waiting
        // for the next frame's `tickColorPickerCursor`.
        tickColorPickerCursor();
    },

    onDeactivate() {
        // Reset pressed state for cleanliness. The cursor itself is
        // taken over by the next tool's onActivate (CanvasView nulls
        // it before this runs).
        setColorPickerPressed(false);
    },

    onPointerDown(ctx, _e, cx, cy) {
        setColorPickerPressed(true);
        startPick(ctx.handle, cx, cy);
    },

    onPointerMove(ctx, e, cx, cy) {
        if (e.buttons & 1) {
            startPick(ctx.handle, cx, cy);
        }
    },

    onPointerUp() {
        setColorPickerPressed(false);
    },

    onPointerLeave() {
        setColorPickerPressed(false);
    },
    // No `onFrame` — `pollPick` runs unconditionally from the frame loop in
    // app.svelte.ts, and `tickColorPickerCursor` next to it keeps the cursor
    // in sync with foreground updates regardless of which tool is active.
};
