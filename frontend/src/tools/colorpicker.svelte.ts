import type { Tool, ToolContext } from './registry';
import { app } from '../state/app.svelte';

export const colorPickerTool: Tool = {
    id: 'colorpicker',
    name: 'Color Picker',
    icon: 'P',
    hotkeyAction: 'colorPickerTool',

    onPointerDown(ctx, e, cx, cy) {
        const rgba = ctx.handle.pick_color(cx, cy);
        if (rgba && rgba.length >= 4) {
            app.foreground = { r: rgba[0], g: rgba[1], b: rgba[2], a: rgba[3] };
        }
    },

    onPointerMove(ctx, e, cx, cy) {
        if (!(e.buttons & 1)) return;
        const rgba = ctx.handle.pick_color(cx, cy);
        if (rgba && rgba.length >= 4) {
            app.foreground = { r: rgba[0], g: rgba[1], b: rgba[2], a: rgba[3] };
        }
    },

    onPointerUp() {},
};
