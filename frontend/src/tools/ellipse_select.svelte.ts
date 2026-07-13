/**
 * Ellipse select tool.
 * Drag to create an elliptical selection (inscribed in the drag rectangle).
 * Modifier keys control boolean mode:
 *   - No modifier: replace selection
 *   - Shift: add to selection
 *   - Alt: subtract from selection
 *   - Shift+Alt: intersect with selection
 * Escape clears the selection.
 */
import { ToolBase, type ToolDescriptor } from './registry';
import type { DarklyInstance } from '../state/app.svelte';
import { KIND_ELLIPSE, FLAG_CANVAS_SPACE, FLAG_INVERT_COLOR, prim, selectionMode } from './selection_helpers';

class EllipseSelectTool extends ToolBase {
    private dragStart: [number, number] | null = null;
    private dragEnd: [number, number] | null = null;

    // Krita-style integer-pixel snapping of the bounding rect (see
    // `kis_tool_select_elliptical.cc`). The ellipse boundary itself is curved,
    // so antialiasing stays on at commit time — only the bbox is snapped.
    private pushPreviewOverlay(): void {
        const engine = this.engine;
        if (!engine || !this.dragStart || !this.dragEnd) return;
        const [x0, y0] = this.dragStart;
        const [x1, y1] = this.dragEnd;
        const sx0 = Math.round(x0);
        const sy0 = Math.round(y0);
        const sx1 = Math.round(x1);
        const sy1 = Math.round(y1);
        const cx = (sx0 + sx1) / 2;
        const cy = (sy0 + sy1) / 2;
        const rx = Math.abs(sx1 - sx0) / 2;
        const ry = Math.abs(sy1 - sy0) / 2;
        engine.api.setOverlay({
            primitives: [
                prim(KIND_ELLIPSE, FLAG_CANVAS_SPACE | FLAG_INVERT_COLOR, [cx, cy], [rx, ry], { dashLen: 6, thickness: 1 }),
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

        // Only commit if the snapped bbox has meaningful size.
        if (w > 0 && h > 0) {
            const mode = selectionMode(e);
            this.engine?.api.selectEllipse({ x, y, w, h, mode, antialias: true, feather: 0 });
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

export const ellipseSelectTool: ToolDescriptor = {
    id: 'ellipse_select',
    icon: 'lucide:circle-dashed',
    group: 'select',
    cluster: 'select',
    hotkeyAction: 'ellipseSelectTool',
    create: (inst: DarklyInstance) => new EllipseSelectTool(inst),
};
