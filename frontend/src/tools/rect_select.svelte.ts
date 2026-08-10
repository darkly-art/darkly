/**
 * Rectangle select tool.
 * Drag to create a rectangular selection. Modifier keys control boolean mode:
 *   - No modifier: replace selection
 *   - Shift: add to selection
 *   - Alt: subtract from selection
 *   - Shift+Alt: intersect with selection
 * Escape clears the selection.
 */
import { ToolBase, type ToolDescriptor } from './registry';
import type { DarklyInstance } from '../state/app.svelte';
import { KIND_RECT, FLAG_CANVAS_SPACE, FLAG_INVERT_COLOR, prim, selectionMode } from './selection_helpers';

class RectSelectTool extends ToolBase {
    private dragStart: [number, number] | null = null;
    private dragEnd: [number, number] | null = null;

    // Krita-style integer-pixel snapping: rectangle selections always commit to
    // pixel-aligned bounds (mirrors `QRectF::toRect()` in Krita's
    // `KisToolSelectRectangular::finishRect`). The preview overlay snaps too so
    // what the user sees during the drag matches what they get on release.
    private pushPreviewOverlay(): void {
        const engine = this.engine;
        if (!engine || !this.dragStart || !this.dragEnd) return;
        const [x0, y0] = this.dragStart;
        const [x1, y1] = this.dragEnd;
        const sx0 = Math.round(x0);
        const sy0 = Math.round(y0);
        const sx1 = Math.round(x1);
        const sy1 = Math.round(y1);
        const tl: [number, number] = [Math.min(sx0, sx1), Math.min(sy0, sy1)];
        const br: [number, number] = [Math.max(sx0, sx1), Math.max(sy0, sy1)];
        engine.api.setOverlay({
            primitives: [
                prim(KIND_RECT, FLAG_CANVAS_SPACE | FLAG_INVERT_COLOR, tl, br, { dashLen: 6, thickness: 1 }),
            ],
        });
    }

    private clearPreviewOverlay(): void {
        this.dragStart = null;
        this.dragEnd = null;
        this.engine?.api.clearOverlay();
    }

    onDeactivate(): void {
        this.clearPreviewOverlay();
    }

    onPointerDown(_e: PointerEvent, cx: number, cy: number): void {
        this.dragStart = [cx, cy];
        this.dragEnd = [cx, cy];
        this.pushPreviewOverlay();
    }

    onPointerMove(_e: PointerEvent, cx: number, cy: number): void {
        if (!this.dragStart) return;
        this.dragEnd = [cx, cy];
        this.pushPreviewOverlay();
    }

    onPointerUp(e: PointerEvent): void {
        if (!this.dragStart || !this.dragEnd) {
            this.clearPreviewOverlay();
            return;
        }

        const [x0, y0] = this.dragStart;
        const [x1, y1] = this.dragEnd;
        const sx0 = Math.round(x0);
        const sy0 = Math.round(y0);
        const sx1 = Math.round(x1);
        const sy1 = Math.round(y1);
        const x = Math.min(sx0, sx1);
        const y = Math.min(sy0, sy1);
        const w = Math.abs(sx1 - sx0);
        const h = Math.abs(sy1 - sy0);

        // Only commit if the snapped rect has meaningful size. `antialias`
        // is off — pixel-aligned bounds need no SDF smoothing and the result
        // is a crisp 1-bit mask.
        if (w > 0 && h > 0) {
            const mode = selectionMode(e);
            this.engine?.api.selectRect({ x, y, w, h, mode, antialias: false, feather: 0 });
        } else if (selectionMode(e) === 'replace') {
            // Click without drag = deselect (only in replace mode)
            this.engine?.api.clearSelection();
        }

        this.clearPreviewOverlay();
    }

    onKeyDown(e: KeyboardEvent): boolean {
        if (e.key === 'Escape') {
            this.engine?.api.clearSelection();
            return true;
        }
        return false;
    }
}

export const rectSelectTool: ToolDescriptor = {
    id: 'rect_select',
    group: 'select',
    cluster: 'select',
    create: (inst: DarklyInstance) => new RectSelectTool(inst),
};
