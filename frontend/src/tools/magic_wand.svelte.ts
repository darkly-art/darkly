/**
 * Magic wand selection tool.
 * Click to select contiguous pixels with similar color on the active layer.
 * Tolerance controls how similar colors must be (0 = exact match, 255 = all).
 * Modifier keys control boolean mode:
 *   - No modifier: replace selection
 *   - Shift: add to selection
 *   - Alt: subtract from selection
 *   - Shift+Alt: intersect with selection
 * Escape clears the selection.
 */
import type { Tool, ToolContext } from './registry';
import { app } from '../state/app.svelte';
import { selectionMode } from './selection_helpers';

/** Default tolerance for color matching (0–255). */
const DEFAULT_TOLERANCE = 15;

export const magicWandTool: Tool = {
    id: 'magic_wand',
    name: 'Magic Wand',
    icon: '✦',
    hotkeyAction: 'magicWandTool',

    onPointerDown(_ctx, e, cx, cy) {
        if (!app.handle || app.activeLayerId == null) return;

        const mode = selectionMode(e);
        app.handle.select_magic_wand(
            BigInt(app.activeLayerId),
            Math.round(cx),
            Math.round(cy),
            DEFAULT_TOLERANCE,
            mode,
        );
    },
    onPointerMove() {},
    onPointerUp() {},

    onKeyDown(e) {
        if (e.key === 'Escape') {
            app.handle?.clear_selection();
            return true;
        }
        return false;
    },
};
