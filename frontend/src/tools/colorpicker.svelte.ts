import { ToolBase, type ToolDescriptor } from './registry';
import type { DarklyInstance } from '../state/app.svelte';
import { startPick } from './color_pick_sync';
import {
    tickColorPickerCursor,
    setColorPickerPressed,
} from './colorpicker_cursor';
import ColorPickerOptions from '../ui/ColorPickerOptions.svelte';

class ColorPickerTool extends ToolBase {
    onActivate(): void {
        // Take ownership of `app.toolCursor` immediately — the tool-transition
        // effect resets it to null right before calling us, so we push the
        // picker cursor now rather than waiting for the next frame's
        // `tickColorPickerCursor`.
        tickColorPickerCursor();
    }

    onDeactivate(): void {
        // Reset pressed state for cleanliness. The cursor itself is taken over
        // by the next tool's onActivate (the transition effect nulls it first).
        setColorPickerPressed(false);
    }

    onPointerDown(_e: PointerEvent, cx: number, cy: number): void {
        const engine = this.engine;
        if (!engine) return;
        setColorPickerPressed(true);
        startPick(engine, cx, cy);
    }

    onPointerMove(e: PointerEvent, cx: number, cy: number): void {
        const engine = this.engine;
        if (engine && e.buttons & 1) {
            startPick(engine, cx, cy);
        }
    }

    onPointerUp(): void {
        setColorPickerPressed(false);
    }

    onPointerLeave(): void {
        setColorPickerPressed(false);
    }
    // No `onFrame` — `pollPick` runs unconditionally from the frame loop in
    // app.svelte.ts, and `tickColorPickerCursor` next to it keeps the cursor
    // in sync with foreground updates regardless of which tool is active.
}

export const colorPickerTool: ToolDescriptor = {
    id: 'colorpicker',
    icon: 'fa6-solid:eye-dropper',
    group: 'paint',
    hotkeyAction: 'colorPickerTool',
    optionsComponent: ColorPickerOptions,
    create: (inst: DarklyInstance) => new ColorPickerTool(inst),
};
