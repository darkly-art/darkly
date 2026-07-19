import { describe, it, expect, vi, beforeEach } from 'vitest';
import { withApi } from '../../engine/testApi';

// Mock the app module before importing the tool, so the tool's
// `import { app } from '../state/app.svelte'` resolves to our fake.
// Avoids pulling in the Svelte runtime ($state runes) for unit tests.
//
// `vi.mock` is hoisted above any top-level `const` — `vi.hoisted` is the
// supported escape hatch to declare spies that the mock factory and the
// tests can both reference.
const { engine, fakeApp } = vi.hoisted(() => {
    const engine = {
        post: vi.fn(),
        send: vi.fn().mockResolvedValue({}),
    };
    // The tool reads its engine off `inst.session`; point it at the same fake
    // engine (its `api`, added by `withApi` below, forwards to these spies).
    return { engine, fakeApp: { engine, session: engine, zoom: 1.0 } };
});
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));

// Module under test — imported after the mock is registered.
import { polygonSelectTool } from '../polygon_select.svelte';

const tool = polygonSelectTool.create(fakeApp as never) as unknown as {
    onPointerDown(e: PointerEvent, cx: number, cy: number): void;
    onPointerMove(e: PointerEvent, cx: number, cy: number): void;
    onKeyDown(e: KeyboardEvent): boolean;
};

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

/** All `engine.post` calls whose first arg (the request kind) is `kind`. */
function postCalls(kind: string) {
    return engine.post.mock.calls.filter(([k]) => k === kind);
}

function reset() {
    engine.post.mockClear();
    engine.send.mockClear();
    // Escape clears any in-progress polygon without committing — guarantees
    // each test starts with an empty module-level vertex buffer.
    tool.onKeyDown(keyEvent('Escape'));
    // Then run an explicit Escape again to clear the now-empty buffer state
    // (the first Escape may have committed to clear_selection if the buffer
    // was already empty), and zero out the spies one more time.
    engine.post.mockClear();
    clock = 0;
}

// Give the fake engine a real typed `api` over its send/post spies.
withApi(engine);

describe('polygonSelectTool', () => {
    beforeEach(reset);

    it('single click adds a vertex and draws a preview overlay', () => {
        tool.onPointerDown(pointerDown(10, 20), 10, 20);
        expect(postCalls('select_lasso')).toHaveLength(0);
        expect(postCalls('set_overlay').length).toBeGreaterThan(0);
    });

    it('does not commit before three vertices are placed', () => {
        tool.onPointerDown(pointerDown(0, 0), 0, 0);
        tool.onPointerDown(pointerDown(10, 0), 10, 0);
        tool.onKeyDown(keyEvent('Enter'));
        expect(postCalls('select_lasso')).toHaveLength(0);
    });

    it('Enter closes the polygon and commits all placed vertices', () => {
        tool.onPointerDown(pointerDown(0, 0), 0, 0);
        tool.onPointerDown(pointerDown(10, 0), 10, 0);
        tool.onPointerDown(pointerDown(10, 10), 10, 10);
        tool.onKeyDown(keyEvent('Enter'));
        const calls = postCalls('select_lasso');
        expect(calls).toHaveLength(1);
        const payload = calls[0][1];
        expect(payload.verts).toEqual([[0, 0], [10, 0], [10, 10]]);
        expect(payload.mode).toBe('replace');
    });

    it('double-click closes without adding a duplicate vertex', () => {
        tool.onPointerDown(pointerDown(0, 0), 0, 0);
        tool.onPointerDown(pointerDown(10, 0), 10, 0);
        tool.onPointerDown(pointerDown(10, 10), 10, 10);
        // Second click of a double-click — small dt, same position.
        tool.onPointerDown(pointerDown(10, 10, 50), 10, 10);
        const calls = postCalls('select_lasso');
        expect(calls).toHaveLength(1);
        expect(calls[0][1].verts).toEqual([[0, 0], [10, 0], [10, 10]]);
    });

    it('clicking inside the first-vertex snap zone closes', () => {
        // zoom=1 → snap radius = 10 canvas-px; (3,4) is at distance 5 from origin.
        tool.onPointerDown(pointerDown(0, 0), 0, 0);
        tool.onPointerDown(pointerDown(100, 0), 100, 0);
        tool.onPointerDown(pointerDown(100, 100), 100, 100);
        // Move into snap zone first so the snap indicator becomes active.
        tool.onPointerMove(pointerDown(3, 4), 3, 4);
        tool.onPointerDown(pointerDown(3, 4), 3, 4);
        const calls = postCalls('select_lasso');
        expect(calls).toHaveLength(1);
        // Snap-click should NOT add the snap point as a new vertex.
        expect(calls[0][1].verts).toEqual([[0, 0], [100, 0], [100, 100]]);
    });

    it('Backspace removes the last placed vertex', () => {
        tool.onPointerDown(pointerDown(0, 0), 0, 0);
        tool.onPointerDown(pointerDown(10, 0), 10, 0);
        tool.onPointerDown(pointerDown(10, 10), 10, 10);
        tool.onKeyDown(keyEvent('Backspace'));
        tool.onKeyDown(keyEvent('Enter'));
        expect(postCalls('select_lasso')).toHaveLength(0);  // only 2 verts left
    });

    it('Escape mid-draw cancels without committing or clearing the selection', () => {
        tool.onPointerDown(pointerDown(0, 0), 0, 0);
        tool.onPointerDown(pointerDown(10, 0), 10, 0);
        engine.post.mockClear();
        tool.onKeyDown(keyEvent('Escape'));
        expect(postCalls('select_lasso')).toHaveLength(0);
        // No in-progress polygon was committed AND the existing doc selection
        // was not touched — Escape only cancels the WIP polygon here.
        expect(postCalls('clear_selection')).toHaveLength(0);
    });

    it('Escape with no in-progress polygon clears the selection', () => {
        tool.onKeyDown(keyEvent('Escape'));
        expect(postCalls('clear_selection')).toHaveLength(1);
    });

    it('Shift held when closing yields add-to-selection mode', () => {
        tool.onPointerDown(pointerDown(0, 0), 0, 0);
        tool.onPointerDown(pointerDown(10, 0), 10, 0);
        tool.onPointerDown(pointerDown(10, 10), 10, 10);
        tool.onKeyDown(keyEvent('Enter', { shiftKey: true }));
        expect(postCalls('select_lasso')[0][1].mode).toBe('add');
    });
});
