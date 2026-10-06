/**
 * Pure helpers for the shape tools: the drag-box geometry and the
 * `begin_shape` request that starts a live placement. Kept free of Svelte and
 * the engine so the gesture rules are unit-testable on their own.
 */
import type { BeginShapeReq, StrokeWire } from '../engine/protocol_gen';
import type { ParamValue } from '../ui/params/paramSchema';

export type Rgba = [number, number, number, number];

/** A placed box in canvas (plane) coordinates, top-left plus size. */
export interface Box {
    x: number;
    y: number;
    w: number;
    h: number;
}

/** Modifiers that reshape the drag, re-read on every move. */
export interface DragModifiers {
    /** Constrain to a square (Shift). */
    square: boolean;
    /** Grow from the press point as centre (Alt). */
    fromCenter: boolean;
}

/** The box a drag from `(sx, sy)` to `(cx, cy)` describes under `mods`. A
 *  square takes the longer side and keeps the drag's direction; from-centre
 *  mirrors the drag about the press point. */
export function dragBox(sx: number, sy: number, cx: number, cy: number, mods: DragModifiers): Box {
    let dx = cx - sx;
    let dy = cy - sy;
    if (mods.square) {
        const side = Math.max(Math.abs(dx), Math.abs(dy));
        dx = (dx < 0 ? -1 : 1) * side;
        dy = (dy < 0 ? -1 : 1) * side;
    }
    if (mods.fromCenter) {
        const hw = Math.abs(dx);
        const hh = Math.abs(dy);
        return { x: sx - hw, y: sy - hh, w: 2 * hw, h: 2 * hh };
    }
    return { x: Math.min(sx, sx + dx), y: Math.min(sy, sy + dy), w: Math.abs(dx), h: Math.abs(dy) };
}

/** The creation style a placed shape takes. */
export interface ShapeStyle {
    params: Record<string, ParamValue>;
    fill: Rgba;
    border: StrokeWire;
}

/** The `begin_shape` request that starts placing `family` in `box`. */
export function buildBeginShape(box: Box, family: string, style: ShapeStyle, anchor: number | null): BeginShapeReq {
    return {
        family,
        params: { ...style.params },
        x: box.x,
        y: box.y,
        width: box.w,
        height: box.h,
        fill: style.fill,
        stroke: style.border,
        anchor,
    };
}
