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
import { ToolBase, type ToolDescriptor } from './registry';
import type { DarklyInstance } from '../state/app.svelte';
import { selectionMode } from './selection_helpers';
import MagicWandOptions from '../ui/MagicWandOptions.svelte';

/** Magic-wand session state: an app-global user preference. Persists within
 *  the session; resets on reload. */
class MagicWandSession {
    /** Color-distance threshold for the flood fill (0 = exact match, 255 = anything). */
    tolerance = $state(15);
}
export const magicWandSession = new MagicWandSession();

class MagicWandTool extends ToolBase {
    onPointerDown(e: PointerEvent, cx: number, cy: number): void {
        if (this.inst.activeLayerId == null) return;

        const mode = selectionMode(e);
        this.engine?.api.selectMagicWand({
            id: this.inst.activeLayerId,
            seed_canvas: { x: Math.round(cx), y: Math.round(cy) },
            tolerance: magicWandSession.tolerance,
            mode,
        });
    }

    onKeyDown(e: KeyboardEvent): boolean {
        if (e.key === 'Escape') {
            this.engine?.api.clearSelection();
            return true;
        }
        return false;
    }
}

export const magicWandTool: ToolDescriptor = {
    id: 'magic_wand',
    group: 'select',
    cluster: 'select',
    optionsComponent: MagicWandOptions,
    create: (inst: DarklyInstance) => new MagicWandTool(inst),
};
