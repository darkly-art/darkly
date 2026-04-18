/**
 * Shared helpers for selection tools — overlay constants, primitive builder,
 * and modifier-key → selection mode mapping.
 */

// GPU overlay constants (must match Rust/WGSL)
export const KIND_LINE           = 0;
export const KIND_CIRCLE         = 1;
export const KIND_RECT           = 2;
export const KIND_DASHED_LINE    = 3;
export const KIND_FILLED_RECT    = 4;
export const KIND_FILLED_CIRCLE  = 5;
export const KIND_ELLIPSE        = 6;
export const KIND_FILLED_ELLIPSE = 7;
export const KIND_MASKED_STAMP   = 8;

export const FLAG_CANVAS_SPACE   = 1;
export const FLAG_INVERT_COLOR   = 2;
export const FLAG_SOFT_CONTRAST  = 4;

export interface GpuPrim {
    kind: number;
    flags: number;
    p0: [number, number];
    p1: [number, number];
    color: [number, number, number, number];
    thickness: number;
    dashLen: number;
    dashOffset: number;
    cornerRadius: number;
    modeParam: number;
    rotation: number;
}

export function prim(
    kind: number,
    flags: number,
    p0: [number, number],
    p1: [number, number],
    opts?: Partial<GpuPrim>,
): GpuPrim {
    return {
        kind, flags, p0, p1,
        color: opts?.color ?? [1, 1, 1, 1],
        thickness: opts?.thickness ?? 1,
        dashLen: opts?.dashLen ?? 0,
        dashOffset: opts?.dashOffset ?? 0,
        cornerRadius: opts?.cornerRadius ?? 0,
        modeParam: opts?.modeParam ?? 0,
        rotation: opts?.rotation ?? 0,
    };
}

/** Map modifier keys to selection boolean mode string. */
export function selectionMode(e: PointerEvent | MouseEvent): string {
    if (e.shiftKey && e.altKey) return 'intersect';
    if (e.shiftKey) return 'add';
    if (e.altKey) return 'subtract';
    return 'replace';
}
