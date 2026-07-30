/**
 * Lasso (freehand polygon) select tool.
 * Click and drag to draw a freehand selection boundary. The polygon is
 * automatically closed on mouse up and rasterized via SDF.
 * Modifier keys control boolean mode:
 *   - No modifier: replace selection
 *   - Shift: add to selection
 *   - Alt: subtract from selection
 *   - Shift+Alt: intersect with selection
 * Escape clears the selection.
 */
import { ToolBase, type ToolDescriptor } from './registry';
import type { DarklyInstance } from '../state/app.svelte';
import { KIND_LINE, FLAG_CANVAS_SPACE, FLAG_INVERT_COLOR, prim, selectionMode } from './selection_helpers';

/** Minimum squared distance between consecutive points to avoid redundancy. */
const MIN_DIST_SQ = 4;

class LassoSelectTool extends ToolBase {
    private points: [number, number][] = [];

    private pushPreviewOverlay(): void {
        const engine = this.engine;
        if (!engine || this.points.length < 2) return;
        const prims = [];
        for (let i = 1; i < this.points.length; i++) {
            prims.push(prim(KIND_LINE, FLAG_CANVAS_SPACE | FLAG_INVERT_COLOR, this.points[i - 1], this.points[i], { thickness: 1 }));
        }
        // Closing line back to start
        prims.push(prim(KIND_LINE, FLAG_CANVAS_SPACE | FLAG_INVERT_COLOR, this.points[this.points.length - 1], this.points[0], { dashLen: 4, thickness: 1 }));
        engine.api.setOverlay({ primitives: prims });
    }

    private clearPreviewOverlay(): void {
        this.points = [];
        this.engine?.api.clearOverlay();
    }

    onDeactivate(): void {
        this.clearPreviewOverlay();
    }

    onPointerDown(_e: PointerEvent, cx: number, cy: number): void {
        this.points = [[cx, cy]];
        this.pushPreviewOverlay();
    }

    onPointerMove(_e: PointerEvent, cx: number, cy: number): void {
        if (this.points.length === 0) return;
        const last = this.points[this.points.length - 1];
        const dx = cx - last[0];
        const dy = cy - last[1];
        if (dx * dx + dy * dy >= MIN_DIST_SQ) {
            this.points.push([cx, cy]);
            this.pushPreviewOverlay();
        }
    }

    onPointerUp(e: PointerEvent): void {
        if (this.points.length < 3) {
            if (selectionMode(e) === 'replace') {
                this.engine?.api.clearSelection();
            }
            this.clearPreviewOverlay();
            return;
        }

        const mode = selectionMode(e);
        this.engine?.api.selectLasso({ verts: this.points, mode, antialias: true, feather: 0 });
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

export const lassoSelectTool: ToolDescriptor = {
    id: 'lasso_select',
    icon: 'tabler:lasso',
    group: 'select',
    cluster: 'select',
    hotkeyAction: 'lassoSelectTool',
    create: (inst: DarklyInstance) => new LassoSelectTool(inst),
};
