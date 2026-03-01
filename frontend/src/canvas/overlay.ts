/**
 * Overlay primitives for tool visualizations.
 * All positions are in CANVAS space (document pixels).
 * Visual sizes (radius, strokeWidth) are in CSS pixels (screen space)
 * so they remain constant regardless of zoom.
 */

export interface OverlayHandle {
    /** Unique id within this tool's overlay, used for keying and drag tracking */
    id: string;
    /** Canvas X coordinate */
    x: number;
    /** Canvas Y coordinate */
    y: number;
    /** CSS cursor when hovering this handle */
    cursor?: string;
    /** Radius in CSS pixels (default 6) */
    radius?: number;
    /** Fill color */
    fill?: string;
    /** Stroke color */
    stroke?: string;
    /** Called continuously during drag with canvas coordinates */
    onDrag?: (canvasX: number, canvasY: number) => void;
    /** Called when drag ends */
    onDragEnd?: () => void;
}

export interface OverlayLine {
    x1: number; y1: number;
    x2: number; y2: number;
    /** Stroke color (default 'white') */
    stroke?: string;
    /** Stroke width in CSS pixels (default 1) */
    strokeWidth?: number;
    /** Dash pattern in CSS pixels (default '6 3') */
    dashArray?: string;
}

export interface ToolOverlayData {
    handles?: OverlayHandle[];
    lines?: OverlayLine[];
}
