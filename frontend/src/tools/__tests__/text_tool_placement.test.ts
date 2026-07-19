import { describe, it, expect, vi, beforeEach } from 'vitest';

// Mock `app` and `config` before importing the tool so we never pull in the
// real engine/wasm. The regression guard: a first click on empty canvas must
// set a pending placement WITHOUT deselecting the active layer — deselecting
// changed `activeLayerId`, which tripped CanvasView's dismiss effect and wiped
// the placement on the same click (the "first click does nothing" bug).
const { fakeApp } = vi.hoisted(() => ({
    fakeApp: {
        engine: null as unknown,
        canvasEl: {} as unknown,
        selectLayer: vi.fn(),
        requestFrame: vi.fn(),
        activeLayerId: 5 as number | null,
        toolCursor: null as string | null,
    },
}));
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));
vi.mock('../../config/store.svelte', () => ({ config: { get: () => undefined } }));

import { textTool, type TextPlacement } from '../text.svelte';

// The per-document edit state lives on the tool instance now; widen the created
// tool so the placement field is visible.
type TextToolLike = {
    onPointerDown(e: PointerEvent, cx: number, cy: number): Promise<void>;
    onPointerMove(e: PointerEvent, cx: number, cy: number): void;
    onPointerUp(): void;
    placement: TextPlacement | null;
    editing: unknown;
};
const tool = textTool.create(fakeApp as never) as unknown as TextToolLike;

const ev = (button = 0) => ({ button }) as unknown as PointerEvent;

beforeEach(() => {
    fakeApp.selectLayer.mockClear();
    fakeApp.engine = null;
    fakeApp.activeLayerId = 5;
    tool.placement = null;
    tool.editing = null;
});

describe('text tool placement (first-click regression)', () => {
    it('a click on empty canvas sets a point placement and does not deselect', async () => {
        await tool.onPointerDown(ev(), 100, 60);
        // No movement between down and up → a click, i.e. point text.
        tool.onPointerUp();
        expect(tool.placement).toEqual({ x: 100, y: 60, anchorLayerId: 5, box: null });
        expect(fakeApp.selectLayer).not.toHaveBeenCalled();
    });

    it('a drag commits an area-text box placement', async () => {
        await tool.onPointerDown(ev(), 20, 20);
        tool.onPointerMove(ev(), 220, 140);
        tool.onPointerUp();
        expect(tool.placement).toEqual({
            x: 20,
            y: 20,
            anchorLayerId: 5,
            box: [200, 120],
        });
        expect(fakeApp.selectLayer).not.toHaveBeenCalled();
    });
});
