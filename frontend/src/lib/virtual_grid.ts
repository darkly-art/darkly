/**
 * Pure geometry for a virtualized, fixed-tile responsive grid: given the
 * container/scroll measurements, work out how many columns fit, the full scroll
 * height, and which slice of items is in (or near) the viewport. Kept DOM-free
 * so it unit-tests without layout.
 *
 * Prior art: Graphite's font menu virtual scrolling
 * (`frontend/src/components/floating-menus/MenuList.svelte`) — only the visible
 * window of entries is rendered, with a spacer reserving the full height so the
 * scrollbar is correct and per-entry font previews load lazily.
 */
export interface GridMetrics {
    /** Total number of items in the (filtered) list. */
    count: number;
    /** Inner width available to the grid, in px. */
    containerWidth: number;
    /** The scroll container's current `scrollTop`, in px. */
    scrollTop: number;
    /** Offset of the grid's top within the scroll content (content above it). */
    offsetTop: number;
    /** Visible height of the scroll container, in px. */
    viewportH: number;
    /** Minimum tile width (the grid's min column), in px. */
    tileMinWidth: number;
    /** Fixed tile height, in px. */
    tileHeight: number;
    /** Gap between tiles (both axes), in px. */
    gap: number;
    /** Extra rows rendered above/below the viewport to cover fast scrolls. */
    rowBuffer: number;
}

export interface GridWindow {
    /** Columns that fit at the current width (>= 1). */
    columns: number;
    /** Total rows the full list occupies. */
    rowCount: number;
    /** Full scroll height the grid reserves, in px. */
    gridHeight: number;
    /** First rendered row (buffer-extended, clamped to 0). */
    firstRow: number;
    /** One past the last rendered row (clamped to `rowCount`). */
    lastRow: number;
    /** Y offset of the rendered window within `gridHeight`, in px. */
    windowTop: number;
    /** Inclusive start index into the item list. */
    sliceStart: number;
    /** Exclusive end index into the item list. */
    sliceEnd: number;
}

/** Compute the visible window for a virtualized fixed-tile grid. */
export function virtualGridWindow(m: GridMetrics): GridWindow {
    const stride = m.tileHeight + m.gap;
    const columns = Math.max(1, Math.floor((m.containerWidth + m.gap) / (m.tileMinWidth + m.gap)));
    const rowCount = Math.ceil(m.count / columns);
    const firstRow = Math.max(0, Math.floor((m.scrollTop - m.offsetTop) / stride) - m.rowBuffer);
    const lastRow = Math.min(rowCount, firstRow + Math.ceil(m.viewportH / stride) + m.rowBuffer * 2);
    return {
        columns,
        rowCount,
        gridHeight: rowCount * stride,
        firstRow,
        lastRow,
        windowTop: firstRow * stride,
        sliceStart: firstRow * columns,
        sliceEnd: lastRow * columns,
    };
}
