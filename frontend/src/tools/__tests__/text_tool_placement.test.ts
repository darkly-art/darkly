import { describe, it, expect, vi, beforeEach } from 'vitest';

// Mock `app` and `config` before importing the tool so we never pull in the
// real engine/wasm. The regression guard: a first click on empty canvas must
// set a pending placement WITHOUT deselecting the active layer — deselecting
// changed `activeLayerId`, which tripped CanvasView's dismiss effect and wiped
// the placement on the same click (the "first click does nothing" bug).
const { fakeApp } = vi.hoisted(() => ({
    fakeApp: {
        engine: null as unknown,
        selectLayer: vi.fn(),
        requestFrame: vi.fn(),
        activeLayerId: 5 as number | null,
        toolCursor: null as string | null,
    },
}));
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));
vi.mock('../../config/store.svelte', () => ({ config: { get: () => undefined } }));

import { textTool, textSession } from '../text.svelte';

const ctx = {} as never;
const ev = (button = 0) => ({ button }) as unknown as PointerEvent;

beforeEach(() => {
    fakeApp.selectLayer.mockClear();
    fakeApp.engine = null;
    fakeApp.activeLayerId = 5;
    textSession.placement = null;
    textSession.editing = null;
});

describe('text tool placement (first-click regression)', () => {
    it('a click on empty canvas sets a point placement and does not deselect', async () => {
        await textTool.onPointerDown(ctx, ev(), 100, 60);
        // No movement between down and up → a click, i.e. point text.
        textTool.onPointerUp(ctx, ev());
        expect(textSession.placement).toEqual({ x: 100, y: 60, anchorLayerId: 5, box: null });
        expect(fakeApp.selectLayer).not.toHaveBeenCalled();
    });

    it('a drag commits an area-text box placement', async () => {
        await textTool.onPointerDown(ctx, ev(), 20, 20);
        textTool.onPointerMove(ctx, ev(), 220, 140);
        textTool.onPointerUp(ctx, ev());
        expect(textSession.placement).toEqual({
            x: 20,
            y: 20,
            anchorLayerId: 5,
            box: [200, 120],
        });
        expect(fakeApp.selectLayer).not.toHaveBeenCalled();
    });
});
