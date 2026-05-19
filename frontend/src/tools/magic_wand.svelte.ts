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
import MagicWandOptions from '../ui/MagicWandOptions.svelte';

/** Magic-wand session state. Persists within the session; resets on reload. */
class MagicWandSession {
    /** Color-distance threshold for the flood fill (0 = exact match, 255 = anything). */
    tolerance = $state(15);
}
export const magicWandSession = new MagicWandSession();

export const magicWandTool: Tool = {
    id: 'magic_wand',
    faIcon: 'fa-solid fa-wand-magic-sparkles',
    group: 'select',
    cluster: 'select',
    hotkeyAction: 'magicWandTool',
    optionsComponent: MagicWandOptions,

    onPointerDown(_ctx, e, cx, cy) {
        if (!app.handle || app.activeLayerId == null) return;

        const mode = selectionMode(e);
        app.handle.select_magic_wand(
            BigInt(app.activeLayerId),
            Math.round(cx),
            Math.round(cy),
            magicWandSession.tolerance,
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
