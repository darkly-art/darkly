import { describe, it, expect, vi, beforeEach } from 'vitest';

// Mock the stores before importing the modules under test: the sections and
// the popup store resolve their data through these.
const { fakeApp, loadBrush } = vi.hoisted(() => {
    const loadBrush = vi.fn();
    const fakeApp = {
        pointerActive: false,
        foreground: { r: 1, g: 2, b: 3, a: 255 },
        background: { r: 9, g: 9, b: 9, a: 255 },
    };
    return { fakeApp, loadBrush };
});
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));
vi.mock('../../state/recents.svelte', () => ({
    recentColors: { items: ['#ff0000ff', '#00ff00ff'] },
    recentBrushes: { items: ['b1'] },
}));
vi.mock('../../state/brush_graph.svelte', () => ({ brushGraph: { loadBrush } }));
vi.mock('../../state/brush_library.svelte', () => ({
    brushLibrary: {
        brushes: [{ id: 'b1', name: 'Ink', icon: null }],
        packs: [],
    },
}));

import { actions } from '../registry';
import { registerPalettePopupAction } from '../palette_popup';
import { palettePopup } from '../../state/palettePopup.svelte';
import { buildChordIndex, resolveChord } from '../hotkey_resolve';
import { HUB_R, RING_T } from '../../ui/palette_popup/wheel_geometry';

const down = (x: number, y: number, pointerId = 1) =>
    ({ pointerId, clientX: x, clientY: y, type: 'pointerdown' }) as unknown as PointerEvent;
const move = (x: number, y: number, pointerId = 1) =>
    ({ pointerId, clientX: x, clientY: y, type: 'pointermove' }) as unknown as PointerEvent;
const up = (pointerId = 1) =>
    ({ pointerId, clientX: 0, clientY: 0, type: 'pointerup' }) as unknown as PointerEvent;
const cancelEv = (pointerId = 1) =>
    ({ pointerId, clientX: 0, clientY: 0, type: 'pointercancel' }) as unknown as PointerEvent;

registerPalettePopupAction();
const action = actions.get('palettePopup')!;

/** Ring-0 bottom-half midpoint of the first of two color sectors. */
const colorPoint = (cx: number, cy: number): [number, number] => {
    const r = HUB_R + RING_T / 2;
    return [cx + r * Math.cos(Math.PI / 4), cy + r * Math.sin(Math.PI / 4)];
};

beforeEach(() => {
    palettePopup.cancel();
    fakeApp.pointerActive = false;
    fakeApp.foreground = { r: 1, g: 2, b: 3, a: 255 };
    loadBrush.mockClear();
});

describe('palettePopup action', () => {
    it('registers as a hold action', () => {
        expect(action.type).toBe('hold');
    });

    it('handler opens at the pointerdown position', () => {
        action.handler({ event: down(200, 300) });
        expect(palettePopup.isOpen).toBe(true);
        const s = palettePopup.state;
        expect(s.kind === 'engaged' && s.center).toEqual({ x: 200, y: 300 });
    });

    it('handler is a no-op while already open', () => {
        action.handler({ event: down(200, 300) });
        action.handler({ event: down(50, 60, 2) });
        const s = palettePopup.state;
        expect(s.kind === 'engaged' && s.center).toEqual({ x: 200, y: 300 });
    });

    it('handler is a no-op while a stroke is in flight', () => {
        fakeApp.pointerActive = true;
        action.handler({ event: down(200, 300) });
        expect(palettePopup.isOpen).toBe(false);
    });

    it('thread to a color leaf and release commits it to the foreground', () => {
        action.handler({ event: down(400, 400) });
        const [x, y] = colorPoint(400, 400);
        action.onMove!({}, move(x, y), 0, 0);
        action.deactivate!({ upEvent: up() });
        expect(palettePopup.isOpen).toBe(false);
        expect(fakeApp.foreground).toEqual({ r: 255, g: 0, b: 0, a: 255 });
    });

    it('release over the hub cancels (zero-movement gesture)', () => {
        action.handler({ event: down(400, 400) });
        action.deactivate!({ upEvent: up() });
        expect(palettePopup.isOpen).toBe(false);
        expect(fakeApp.foreground).toEqual({ r: 1, g: 2, b: 3, a: 255 });
    });

    it('a pointercancel release cancels even with a leaf highlighted', () => {
        action.handler({ event: down(400, 400) });
        const [x, y] = colorPoint(400, 400);
        action.onMove!({}, move(x, y), 0, 0);
        action.deactivate!({ upEvent: cancelEv() });
        expect(palettePopup.isOpen).toBe(false);
        expect(fakeApp.foreground).toEqual({ r: 1, g: 2, b: 3, a: 255 });
    });

    it('ignores moves and releases from non-latched pointers', () => {
        action.handler({ event: down(400, 400) });
        const [x, y] = colorPoint(400, 400);
        action.onMove!({}, move(x, y, 9), 0, 0);
        const s = palettePopup.state;
        expect(s.kind === 'engaged' && s.highlight.kind).toBe('hub');
        action.deactivate!({ upEvent: up(9) });
        expect(palettePopup.isOpen).toBe(true);
    });
});

describe('palettePopup chord resolution (canvas:rightDrag)', () => {
    it('resolves the preset binding at the canvas site for any tool group', () => {
        const idx = buildChordIndex([
            { actionId: 'palettePopup', bindings: ['canvas:rightDrag'] },
        ]);
        const entries = idx.get('rightDrag');
        expect(entries).toBeDefined();
        for (const group of ['paint', 'select', null]) {
            const resolved = resolveChord(entries!, [{ name: 'canvas' }], group);
            expect(resolved?.entry.actionId).toBe('palettePopup');
        }
    });
});
