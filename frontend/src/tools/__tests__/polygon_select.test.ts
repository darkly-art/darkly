import { describe, it, expect, vi, beforeEach } from 'vitest';

// Mock the app module before importing the tool, so the tool's
// `import { app } from '../state/app.svelte'` resolves to our fake.
// Avoids pulling in the Svelte runtime ($state runes) for unit tests.
//
// `vi.mock` is hoisted above any top-level `const` — `vi.hoisted` is the
// supported escape hatch to declare spies that the mock factory and the
// tests can both reference.
const { handle, fakeApp } = vi.hoisted(() => {
    const handle = {
        select_lasso: vi.fn(),
        clear_selection: vi.fn(),
        set_overlay: vi.fn(),
        clear_overlay: vi.fn(),
    };
    return { handle, fakeApp: { handle, zoom: 1.0 } };
});
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));

// Module under test — imported after the mock is registered.
import { polygonSelectTool } from '../polygon_select.svelte';

// Plain-object event fakes — vitest's default node env has no DOM globals
// (`PointerEvent` / `KeyboardEvent`), and we only read a handful of fields.
let clock = 0;
function pointerDown(_x: number, _y: number, dtMs = 1000): PointerEvent {
    // Manual timestamp control — the tool detects double-click via
    // `e.timeStamp` deltas, not `e.detail` (which the canvas's
    // preventDefault suppresses).
    clock += dtMs;
    return { timeStamp: clock, shiftKey: false, altKey: false } as unknown as PointerEvent;
}
function keyEvent(key: string, mods: { shiftKey?: boolean; altKey?: boolean } = {}): KeyboardEvent {
    return { key, shiftKey: !!mods.shiftKey, altKey: !!mods.altKey } as unknown as KeyboardEvent;
}
// Minimal ToolContext stub — the polygon tool never reads any of these.
const ctx = {} as any;

function reset() {
    Object.values(handle).forEach(fn => fn.mockClear());
    // Escape clears any in-progress polygon without committing — guarantees
    // each test starts with an empty module-level vertex buffer.
    polygonSelectTool.onKeyDown?.(keyEvent('Escape'));
    // Then run an explicit Escape again to clear the now-empty buffer state
    // (the first Escape may have committed to clear_selection if the buffer
    // was already empty), and zero out the spies one more time.
    handle.clear_selection.mockClear();
    handle.clear_overlay.mockClear();
    clock = 0;
}

describe('polygonSelectTool', () => {
    beforeEach(reset);

    it('single click adds a vertex and draws a preview overlay', () => {
        polygonSelectTool.onPointerDown(ctx, pointerDown(10, 20), 10, 20);
        expect(handle.select_lasso).not.toHaveBeenCalled();
        expect(handle.set_overlay).toHaveBeenCalled();
    });

    it('does not commit before three vertices are placed', () => {
        polygonSelectTool.onPointerDown(ctx, pointerDown(0, 0), 0, 0);
        polygonSelectTool.onPointerDown(ctx, pointerDown(10, 0), 10, 0);
        polygonSelectTool.onKeyDown?.(keyEvent('Enter'));
        expect(handle.select_lasso).not.toHaveBeenCalled();
    });

    it('Enter closes the polygon and commits all placed vertices', () => {
        polygonSelectTool.onPointerDown(ctx, pointerDown(0, 0), 0, 0);
        polygonSelectTool.onPointerDown(ctx, pointerDown(10, 0), 10, 0);
        polygonSelectTool.onPointerDown(ctx, pointerDown(10, 10), 10, 10);
        polygonSelectTool.onKeyDown?.(keyEvent('Enter'));
        expect(handle.select_lasso).toHaveBeenCalledTimes(1);
        const [verts, mode] = handle.select_lasso.mock.calls[0];
        expect(verts).toEqual([[0, 0], [10, 0], [10, 10]]);
        expect(mode).toBe('replace');
    });

    it('double-click closes without adding a duplicate vertex', () => {
        polygonSelectTool.onPointerDown(ctx, pointerDown(0, 0), 0, 0);
        polygonSelectTool.onPointerDown(ctx, pointerDown(10, 0), 10, 0);
        polygonSelectTool.onPointerDown(ctx, pointerDown(10, 10), 10, 10);
        // Second click of a double-click — small dt, same position.
        polygonSelectTool.onPointerDown(ctx, pointerDown(10, 10, 50), 10, 10);
        expect(handle.select_lasso).toHaveBeenCalledTimes(1);
        expect(handle.select_lasso.mock.calls[0][0]).toEqual([[0, 0], [10, 0], [10, 10]]);
    });

    it('clicking inside the first-vertex snap zone closes', () => {
        // zoom=1 → snap radius = 10 canvas-px; (3,4) is at distance 5 from origin.
        polygonSelectTool.onPointerDown(ctx, pointerDown(0, 0), 0, 0);
        polygonSelectTool.onPointerDown(ctx, pointerDown(100, 0), 100, 0);
        polygonSelectTool.onPointerDown(ctx, pointerDown(100, 100), 100, 100);
        // Move into snap zone first so the snap indicator becomes active.
        polygonSelectTool.onPointerMove(ctx, pointerDown(3, 4), 3, 4);
        polygonSelectTool.onPointerDown(ctx, pointerDown(3, 4), 3, 4);
        expect(handle.select_lasso).toHaveBeenCalledTimes(1);
        // Snap-click should NOT add the snap point as a new vertex.
        expect(handle.select_lasso.mock.calls[0][0]).toEqual([[0, 0], [100, 0], [100, 100]]);
    });

    it('Backspace removes the last placed vertex', () => {
        polygonSelectTool.onPointerDown(ctx, pointerDown(0, 0), 0, 0);
        polygonSelectTool.onPointerDown(ctx, pointerDown(10, 0), 10, 0);
        polygonSelectTool.onPointerDown(ctx, pointerDown(10, 10), 10, 10);
        polygonSelectTool.onKeyDown?.(keyEvent('Backspace'));
        polygonSelectTool.onKeyDown?.(keyEvent('Enter'));
        expect(handle.select_lasso).not.toHaveBeenCalled();  // only 2 verts left
    });

    it('Escape mid-draw cancels without committing or clearing the selection', () => {
        polygonSelectTool.onPointerDown(ctx, pointerDown(0, 0), 0, 0);
        polygonSelectTool.onPointerDown(ctx, pointerDown(10, 0), 10, 0);
        handle.clear_selection.mockClear();
        polygonSelectTool.onKeyDown?.(keyEvent('Escape'));
        expect(handle.select_lasso).not.toHaveBeenCalled();
        // No in-progress polygon was committed AND the existing doc selection
        // was not touched — Escape only cancels the WIP polygon here.
        expect(handle.clear_selection).not.toHaveBeenCalled();
    });

    it('Escape with no in-progress polygon clears the selection', () => {
        polygonSelectTool.onKeyDown?.(keyEvent('Escape'));
        expect(handle.clear_selection).toHaveBeenCalledTimes(1);
    });

    it('Shift held when closing yields add-to-selection mode', () => {
        polygonSelectTool.onPointerDown(ctx, pointerDown(0, 0), 0, 0);
        polygonSelectTool.onPointerDown(ctx, pointerDown(10, 0), 10, 0);
        polygonSelectTool.onPointerDown(ctx, pointerDown(10, 10), 10, 10);
        polygonSelectTool.onKeyDown?.(keyEvent('Enter', { shiftKey: true }));
        expect(handle.select_lasso.mock.calls[0][1]).toBe('add');
    });
});
