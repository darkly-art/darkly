export interface AnchorRect {
    left: number;
    top: number;
    right: number;
    bottom: number;
}

export interface ViewportSize {
    width: number;
    height: number;
}

export interface BrushPickerPlacement {
    left: number;
    top: number | null;
    bottom: number | null;
}

/** Place the picker on the side of its trigger with the most viewport room. */
export function brushPickerPlacement(
    anchor: AnchorRect,
    viewport: ViewportSize,
    pickerWidth: number,
): BrushPickerPlacement {
    const margin = 8;
    const gap = 6;
    const left = Math.max(
        margin,
        Math.min(anchor.left, viewport.width - pickerWidth - margin),
    );

    const roomAbove = anchor.top;
    const roomBelow = viewport.height - anchor.bottom;
    if (roomBelow >= roomAbove) {
        return { left, top: anchor.bottom + gap, bottom: null };
    }
    return { left, top: null, bottom: viewport.height - anchor.top + gap };
}
