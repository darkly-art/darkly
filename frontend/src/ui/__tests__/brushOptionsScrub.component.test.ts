// @vitest-environment jsdom
//
// Regression: the brush bar's Size scrub moved 1% per pixel across its whole
// 0..400% range, so getting from 1% to 3% was a 2 px target. Size declares a
// travel curve that spends the drag on its low end; this drives a real drag
// through `BrushOptions` so the test fails if the bar ever stops handing that
// curve to its scrub, which the optional `travel` prop would not flag.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import type { ExposedPortInfo } from '../../state/brush_graph.svelte';

const { fakeApp, fakeBrushGraph } = vi.hoisted(() => ({
    fakeApp: {
        engine: null,
        engineState: null,
        beginInteraction: () => {},
        endInteraction: () => {},
    },
    fakeBrushGraph: {
        exposedPorts: [] as ExposedPortInfo[],
        activeBrush: 'Round',
        modified: false,
        supportsErase: false,
        isOpen: false,
        error: null,
        graph: null,
        setExposedPortValueLocal: vi.fn(),
        setExposedPortValue: vi.fn(),
    },
}));
vi.mock('../../state/app.svelte', () => ({ app: fakeApp, getActiveInstance: () => null }));
vi.mock('../../state/brush_graph.svelte', () => ({ brushGraph: fakeBrushGraph }));
vi.mock('../../state/brush_library.svelte', () => ({
    brushLibrary: { paletteForBrush: () => null },
}));
vi.mock('../../lib/packPalette', () => ({ packPalette: () => ({}) }));
vi.mock('../../config/store.svelte', () => ({
    config: { get: () => false, set: () => {} },
    tooltipForAction: (label: string) => label,
}));
vi.mock('../../tools/brush.svelte', () => ({
    brushSession: { eraseMode: false },
    focusedBrushTool: () => null,
    strokeEditOpen: () => false,
    toggleStrokeEdit: () => {},
}));
// Children that are not the subject and need an engine to render.
vi.mock('../brush_library/LiveBrushPreviewStrip.svelte', () => ({ default: function () {} }));
vi.mock('../brush_explorer/BrushExplorer.svelte', () => ({ default: function () {} }));
vi.mock('../color/FgBgSwatches.svelte', () => ({ default: function () {} }));

import BrushOptions from '../BrushOptions.svelte';

const SIZE: ExposedPortInfo = {
    key: 'brush_settings.size',
    nodeId: 'brush_settings',
    portName: 'size',
    label: 'Size',
    portLabel: 'Size',
    icon: '',
    description: '',
    nodeDisplayName: 'Brush Settings',
    data: {
        kind: 'scalar',
        value: 1,
        min: 0.01,
        max: 400,
        default: 10,
        unitType: 'Percent',
        invert: false,
        unitOverride: null,
        inheritedUnit: 'Percent',
        travel: { exponent: 3, reversed: false },
    },
};

let instance: Record<string, unknown> | undefined;

afterEach(() => {
    if (instance) unmount(instance);
    instance = undefined;
    document.body.innerHTML = '';
    fakeBrushGraph.setExposedPortValueLocal.mockClear();
});

describe('BrushOptions size scrub', () => {
    it('a 20 px drag from 1% stays below 3%', () => {
        fakeBrushGraph.exposedPorts = [SIZE];
        const target = document.createElement('div');
        document.body.append(target);
        instance = mount(BrushOptions, { target });
        flushSync();

        const scrub = [...target.querySelectorAll<HTMLElement>('.scrub')].find((el) =>
            el.textContent?.includes('Size'),
        )!;
        scrub.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerId: 1, clientX: 100 }));
        scrub.dispatchEvent(new PointerEvent('pointermove', { bubbles: true, pointerId: 1, clientX: 120 }));
        flushSync();

        const calls = fakeBrushGraph.setExposedPortValueLocal.mock.calls;
        expect(calls.length).toBeGreaterThan(0);
        const previewed = calls[calls.length - 1][2] as number;
        expect(previewed).toBeGreaterThan(1);
        expect(previewed).toBeLessThan(3);
    });
});
