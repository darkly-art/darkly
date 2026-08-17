import { describe, it, expect, vi, beforeEach } from 'vitest';

// Regression for the clone-source-vs-color-picker modifier conflict: with the
// clone brush active, Ctrl used to engage BOTH the set-source gesture and the
// color picker, because each cursor derived its own arming modifier from
// binding data and ignored specificity. Engagement now routes through the one
// specificity-aware resolver (`dragModifierActions`) that the dispatcher uses,
// so the brush-scoped `setCloneSource` (`canvas@paint@clone`) out-ranks the
// group-scoped `sampleColor` (`canvas@paint`) and the picker yields.
//
// The seed mirrors `sample_color.test.ts` / `hotkey_resolve.test.ts`:
// `setCloneSource` bound on `canvas@paint@clone`, `sampleColor` on
// `canvas@paint`, both to `ctrl+drag`.

const { fakeApp, fakeConfig, fakeBrushGraph, fakeActions } = vi.hoisted(() => {
    const bindings: Record<string, string> = {
        'mouseclicks.setCloneSource': 'canvas@paint@clone:ctrl+drag',
        'mouseclicks.sampleColor': 'canvas@paint:ctrl+drag',
    };
    const fakeApp = {
        activeToolId: 'brush',
        engine: null,
        canvasEl: null as HTMLCanvasElement | null,
        foreground: { r: 0, g: 0, b: 0, a: 255 },
        background: { r: 255, g: 255, b: 255, a: 255 },
    };
    const fakeConfig = {
        get: vi.fn((key: string) => bindings[key]),
        onChange: vi.fn(() => () => undefined),
    };
    const fakeBrushGraph = { activeBrush: 'Clone' as string | null };
    const fakeActions = {
        all: () => [{ id: 'setCloneSource' }, { id: 'sampleColor' }],
        dispatch: vi.fn(),
        get: vi.fn(),
        release: vi.fn(),
    };
    return { fakeApp, fakeConfig, fakeBrushGraph, fakeActions };
});
vi.mock('../../state/app.svelte', () => ({ app: fakeApp }));
vi.mock('../../config/store.svelte', () => ({ config: fakeConfig }));
vi.mock('../../state/brush_graph.svelte', () => ({ brushGraph: fakeBrushGraph }));
// The active tool group is always `paint` for this test's scenarios.
vi.mock('../registry', () => ({ toolRegistry: { get: () => ({ group: 'paint' }) } }));
vi.mock('../../actions/registry', () => ({ actions: fakeActions }));
vi.mock('../tool_session', () => ({ toolEngine: () => null }));
vi.mock('../../canvas/coordinates', () => ({
    screenToCanvas: () => ({ x: 0, y: 0 }),
    canvasToScreen: () => ({ x: 0, y: 0 }),
}));
// SVG asset import isn't relevant here; stub it so Vite's `?raw` suffix
// doesn't blow up and `extractPathD` finds a path at module load.
vi.mock('../../assets/color-picker.svg?raw', () => ({
    default: '<svg><path d="M0,0 L1,1"/></svg>',
}));

import { rebuildClickIndex, dragModifierActions } from '../../actions/triggers';
import { pickerEngages } from '../colorpicker_cursor';

beforeEach(() => {
    rebuildClickIndex();
});

describe('clone set-source vs. color picker on the same modifier', () => {
    it('under the clone brush, ctrl resolves to setCloneSource — the picker yields', () => {
        fakeBrushGraph.activeBrush = 'Clone';
        const resolved = dragModifierActions('canvas', 'ctrl');
        expect(resolved.has('setCloneSource')).toBe(true);
        expect(resolved.has('sampleColor')).toBe(false);
        // Paint tool active, no pointer down — the picker still must NOT engage
        // because the winning action isn't sampleColor.
        expect(pickerEngages(resolved, true, false)).toBe(false);
    });

    it('under a normal brush, ctrl resolves to sampleColor — the picker engages', () => {
        fakeBrushGraph.activeBrush = 'Ink Pen';
        const resolved = dragModifierActions('canvas', 'ctrl');
        expect(resolved.has('sampleColor')).toBe(true);
        expect(resolved.has('setCloneSource')).toBe(false);
        expect(pickerEngages(resolved, true, false)).toBe(true);
    });

    it('a bare hover (no modifier) arms neither cursor', () => {
        fakeBrushGraph.activeBrush = 'Ink Pen';
        const resolved = dragModifierActions('canvas', '');
        expect(resolved.size).toBe(0);
        expect(pickerEngages(resolved, true, false)).toBe(false);
    });
});

describe('pickerEngages decision', () => {
    it('requires a paint tool, no pointer down, and sampleColor winning', () => {
        const yes = new Set(['sampleColor']);
        expect(pickerEngages(yes, true, false)).toBe(true);
        expect(pickerEngages(yes, false, false)).toBe(false); // non-paint tool
        expect(pickerEngages(yes, true, true)).toBe(false);   // pointer already down
        expect(pickerEngages(new Set(['setCloneSource']), true, false)).toBe(false);
        expect(pickerEngages(new Set(), true, false)).toBe(false);
    });
});
