import { describe, it, expect } from 'vitest';
import { virtualGridWindow, type GridMetrics } from '../virtual_grid';

// A 150px-min tile, 74px tall, 8px gap: the font browser's real footprint.
const BASE: GridMetrics = {
    count: 1000,
    containerWidth: 640,
    scrollTop: 0,
    offsetTop: 0,
    viewportH: 500,
    tileMinWidth: 150,
    tileHeight: 74,
    gap: 8,
    rowBuffer: 2,
};

describe('virtualGridWindow', () => {
    it('fits as many min-width columns as the width allows', () => {
        // (640 + 8) / (150 + 8) = 4.1 → 4 columns.
        expect(virtualGridWindow(BASE).columns).toBe(4);
        // Narrower: (320 + 8) / 158 = 2.07 → 2 columns.
        expect(virtualGridWindow({ ...BASE, containerWidth: 320 }).columns).toBe(2);
        // Always at least one column, even at zero width (pre-measurement).
        expect(virtualGridWindow({ ...BASE, containerWidth: 0 }).columns).toBe(1);
    });

    it('reserves the full scroll height for the whole list', () => {
        // 1000 items / 4 cols = 250 rows; stride 82 → 20500px.
        const w = virtualGridWindow(BASE);
        expect(w.rowCount).toBe(250);
        expect(w.gridHeight).toBe(250 * 82);
    });

    it('renders only the viewport window (plus buffer) at the top', () => {
        const w = virtualGridWindow(BASE);
        // scrollTop 0 → firstRow clamps to 0 (buffer can't go negative).
        expect(w.firstRow).toBe(0);
        expect(w.windowTop).toBe(0);
        // ceil(500/82) = 7 viewport rows + 2*2 buffer = 11 rows → 44 items.
        expect(w.lastRow).toBe(11);
        expect(w.sliceStart).toBe(0);
        expect(w.sliceEnd).toBe(44);
    });

    it('slides the window down as the artist scrolls', () => {
        // Scrolled 100 rows down: 100 * 82 = 8200px.
        const w = virtualGridWindow({ ...BASE, scrollTop: 8200 });
        // floor(8200/82) - 2 buffer = 98.
        expect(w.firstRow).toBe(98);
        expect(w.windowTop).toBe(98 * 82);
        expect(w.sliceStart).toBe(98 * 4);
        // The rendered slice tracks the viewport, not the whole list above it.
        expect(w.sliceEnd - w.sliceStart).toBeLessThan(60);
    });

    it('accounts for content above the grid via offsetTop', () => {
        // The grid sits 200px down (an Installed section above it); a scrollTop
        // of 200 is still the grid's row 0.
        const w = virtualGridWindow({ ...BASE, scrollTop: 200, offsetTop: 200 });
        expect(w.firstRow).toBe(0);
    });

    it('clamps the last row to the end of the list', () => {
        const w = virtualGridWindow({ ...BASE, count: 10, scrollTop: 0 });
        // 10 items / 4 cols = 3 rows total; the window never exceeds it.
        expect(w.rowCount).toBe(3);
        expect(w.lastRow).toBe(3);
        expect(w.sliceEnd).toBe(12); // 3 rows * 4 cols (last row partial)
    });
});
