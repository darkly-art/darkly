/**
 * The dashed bounding box a drag-to-create gesture shows while in flight: the
 * one band the selection and text tools draw.
 */
import type { EngineApi } from '../engine/protocol_gen';
import { KIND_RECT, FLAG_CANVAS_SPACE, FLAG_INVERT_COLOR, prim } from './selection_helpers';

type OverlayEngine = { api: Pick<EngineApi, 'setOverlay' | 'clearOverlay'> };

/** Min canvas-px extent on each axis for a drag to count as a box (vs a click). */
export const DRAG_THRESHOLD = 4;

/** Draw the band between two canvas-space corners (any order). */
export function drawRubberBand(
    engine: OverlayEngine,
    x0: number,
    y0: number,
    x1: number,
    y1: number,
): void {
    const tl: [number, number] = [Math.min(x0, x1), Math.min(y0, y1)];
    const br: [number, number] = [Math.max(x0, x1), Math.max(y0, y1)];
    engine.api.setOverlay({
        primitives: [
            prim(KIND_RECT, FLAG_CANVAS_SPACE | FLAG_INVERT_COLOR, tl, br, { dashLen: 6, thickness: 1 }),
        ],
    });
}

export function clearRubberBand(engine: OverlayEngine): void {
    engine.api.clearOverlay();
}
